//! Windows HID backend using `SetWindowsHookExW(WH_KEYBOARD_LL)` and `SendInput`.
//!
//! # Architecture & Security Caveats
//! - **Dedicated Win32 Message Loop Thread**: `WH_KEYBOARD_LL` hooks MUST execute on a
//!   thread running a live `GetMessageW` loop and return quickly (well under the OS
//!   `LowLevelHooksTimeout`, typically 300ms) or Windows silently removes the hook.
//! - **Loop Prevention**: Synthetic events emitted via `SendInput` set `dwExtraInfo =
//!   RATPOISON_EVENT_MAGIC as usize`, and incoming events with `LLKHF_INJECTED` or our
//!   magic tag are marked `is_injected = true` and passed through untouched.
//! - **UIPI (User Interface Privilege Isolation)**: When RatPoison runs at Medium integrity
//!   (standard user), Windows blocks `WH_KEYBOARD_LL` and `SendInput` against elevated
//!   (High-integrity) foreground windows such as Task Manager or Administrator terminals.
//!   Signing the release binary with a trusted certificate and `uiAccess="true"` in the
//!   application manifest resolves UIPI without running the entire app as Administrator.

use core::ffi::c_void;
use core::mem::{size_of, zeroed};
use core::ptr;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crate::interceptor::InterceptDecision;
use crate::platform::{
    normalize_virtual_desktop_coords, KeyEventCallback, KeyboardHookBackend, MouseSimulatorBackend,
};
use crate::types::{
    ClickKind, HidError, KeyCode, KeyPhase, LogicalPoint, Modifiers, MouseButton, PermissionReport,
    RawKeyEvent, ScrollDelta, RATPOISON_EVENT_MAGIC,
};

type Handle = *mut c_void;
type HHook = *mut c_void;
type HInstance = *mut c_void;
type HWnd = *mut c_void;
type WParam = usize;
type LParam = isize;
type LResult = isize;
type DWord = u32;
type Bool = i32;

type HookProc = unsafe extern "system" fn(code: i32, wparam: WParam, lparam: LParam) -> LResult;

const WH_KEYBOARD_LL: i32 = 13;
const HC_ACTION: i32 = 0;

const WM_QUIT: u32 = 0x0012;
const WM_KEYDOWN: usize = 0x0100;
const WM_KEYUP: usize = 0x0101;
const WM_SYSKEYDOWN: usize = 0x0104;
const WM_SYSKEYUP: usize = 0x0105;

const LLKHF_INJECTED: u32 = 0x0000_0010;

// SendInput constants
const INPUT_MOUSE: u32 = 0;
const MOUSEEVENTF_MOVE: u32 = 0x0001;
const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;
const MOUSEEVENTF_MIDDLEDOWN: u32 = 0x0020;
const MOUSEEVENTF_MIDDLEUP: u32 = 0x0040;
const MOUSEEVENTF_XDOWN: u32 = 0x0080;
const MOUSEEVENTF_XUP: u32 = 0x0100;
const MOUSEEVENTF_WHEEL: u32 = 0x0800;
const MOUSEEVENTF_HWHEEL: u32 = 0x1000;
const MOUSEEVENTF_VIRTUALDESK: u32 = 0x4000;
const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;

const XBUTTON1: u32 = 0x0001;
const XBUTTON2: u32 = 0x0002;
const WHEEL_DELTA: f64 = 120.0;

// GetSystemMetrics indices
const SM_XVIRTUALSCREEN: i32 = 76;
const SM_YVIRTUALSCREEN: i32 = 77;
const SM_CXVIRTUALSCREEN: i32 = 78;
const SM_CYVIRTUALSCREEN: i32 = 79;

// Virtual keycodes for modifier queries
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
const VK_LWIN: i32 = 0x5B;
const VK_RWIN: i32 = 0x5C;
const VK_CAPITAL: i32 = 0x14;

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Msg {
    hwnd: HWnd,
    message: u32,
    wparam: WParam,
    lparam: LParam,
    time: DWord,
    pt: Point,
}

#[repr(C)]
struct KbdLlHookStruct {
    vk_code: DWord,
    scan_code: DWord,
    flags: DWord,
    time: DWord,
    dw_extra_info: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MouseInput {
    dx: i32,
    dy: i32,
    mouse_data: DWord,
    dw_flags: DWord,
    time: DWord,
    dw_extra_info: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KeybdInput {
    w_vk: u16,
    w_scan: u16,
    dw_flags: DWord,
    time: DWord,
    dw_extra_info: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct HardwareInput {
    u_msg: DWord,
    w_param_l: u16,
    w_param_h: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
union InputUnion {
    mi: MouseInput,
    ki: KeybdInput,
    hi: HardwareInput,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Input {
    input_type: DWord,
    union: InputUnion,
}

#[link(name = "user32")]
extern "system" {
    fn SetWindowsHookExW(
        id_hook: i32,
        lpfn: HookProc,
        hmod: HInstance,
        dw_thread_id: DWord,
    ) -> HHook;
    fn UnhookWindowsHookEx(hhk: HHook) -> Bool;
    fn CallNextHookEx(hhk: HHook, n_code: i32, wparam: WParam, lparam: LParam) -> LResult;
    fn GetMessageW(lp_msg: *mut Msg, hwnd: HWnd, msg_filter_min: u32, msg_filter_max: u32) -> Bool;
    fn PostThreadMessageW(id_thread: DWord, msg: u32, wparam: WParam, lparam: LParam) -> Bool;
    fn SendInput(c_inputs: u32, p_inputs: *const Input, cb_size: i32) -> u32;
    fn GetCursorPos(lp_point: *mut Point) -> Bool;
    fn GetSystemMetrics(n_index: i32) -> i32;
    fn GetAsyncKeyState(v_key: i32) -> i16;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentThreadId() -> DWord;
    fn GetCurrentProcess() -> Handle;
    fn CloseHandle(h_object: Handle) -> Bool;
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(process_handle: Handle, desired_access: DWord, token_handle: *mut Handle) -> Bool;
    fn GetTokenInformation(
        token_handle: Handle,
        token_information_class: u32,
        token_information: *mut c_void,
        token_information_length: DWord,
        return_length: *mut DWord,
    ) -> Bool;
}

/// Checks whether the current Windows process token is elevated (Administrator).
pub fn is_process_elevated() -> bool {
    const TOKEN_QUERY: DWord = 0x0008;
    const TOKEN_ELEVATION_CLASS: u32 = 20;

    let mut token: Handle = ptr::null_mut();
    // SAFETY: Queries read-only elevation info on current process handle.
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if ok == 0 || token.is_null() {
        return false;
    }

    let mut elevation: DWord = 0;
    let mut ret_len: DWord = 0;
    // SAFETY: `elevation` matches `TOKEN_ELEVATION` (single `DWORD`).
    let info_ok = unsafe {
        GetTokenInformation(
            token,
            TOKEN_ELEVATION_CLASS,
            (&mut elevation as *mut DWord).cast(),
            size_of::<DWord>() as DWord,
            &mut ret_len,
        )
    };
    unsafe { CloseHandle(token) };
    info_ok != 0 && elevation != 0
}

fn current_modifiers() -> Modifiers {
    let mut mods = Modifiers::EMPTY;
    // SAFETY: `GetAsyncKeyState` is a pure Win32 state query.
    unsafe {
        if (GetAsyncKeyState(VK_SHIFT) as u16 & 0x8000) != 0 {
            mods.insert(Modifiers::SHIFT);
        }
        if (GetAsyncKeyState(VK_CONTROL) as u16 & 0x8000) != 0 {
            mods.insert(Modifiers::CONTROL);
        }
        if (GetAsyncKeyState(VK_MENU) as u16 & 0x8000) != 0 {
            mods.insert(Modifiers::ALT);
        }
        if ((GetAsyncKeyState(VK_LWIN) as u16 | GetAsyncKeyState(VK_RWIN) as u16) & 0x8000) != 0 {
            mods.insert(Modifiers::META);
        }
        if (GetAsyncKeyState(VK_CAPITAL) as u16 & 0x0001) != 0 {
            mods.insert(Modifiers::CAPS_LOCK);
        }
    }
    mods
}

/// Normalizes a Windows Virtual-Key code (`VK_*`) into RatPoison's [`KeyCode`].
pub fn windows_vk_to_normalized(vk: u32) -> KeyCode {
    match vk {
        0x1B => KeyCode::ESCAPE,
        0x0D => KeyCode::RETURN,
        0x20 => KeyCode::SPACE,
        0x09 => KeyCode::TAB,
        0x08 => KeyCode::BACKSPACE,
        0xA0 => KeyCode::SHIFT_L,
        0xA1 => KeyCode::SHIFT_R,
        0xA2 => KeyCode::CTRL_L,
        0xA3 => KeyCode::CTRL_R,
        0xA4 => KeyCode::ALT_L,
        0xA5 => KeyCode::ALT_R,
        0x5B => KeyCode::META_L,
        0x5C => KeyCode::META_R,
        0x41..=0x5A => KeyCode::from_ascii_upper(vk as u8),
        0x30..=0x39 => KeyCode::from_ascii_digit(vk as u8),
        0x25 => KeyCode::ARROW_LEFT,
        0x26 => KeyCode::ARROW_UP,
        0x27 => KeyCode::ARROW_RIGHT,
        0x28 => KeyCode::ARROW_DOWN,
        0xBC => KeyCode::COMMA,
        0xBE => KeyCode::PERIOD,
        0xBF => KeyCode::SLASH,
        0xBA => KeyCode::SEMICOLON,
        0xDE => KeyCode::APOSTROPHE,
        0xDB => KeyCode::BRACKET_LEFT,
        0xDD => KeyCode::BRACKET_RIGHT,
        0xBD => KeyCode::MINUS,
        0xBB => KeyCode::EQUAL,
        0xC0 => KeyCode::GRAVE,
        0xDC => KeyCode::BACKSLASH,
        other => KeyCode((other & 0xFFFF) as u16),
    }
}

struct HookThreadState {
    callback: KeyEventCallback,
    epoch: Instant,
    held_keys: [u32; 16],
    held_len: usize,
}

impl HookThreadState {
    fn is_held(&self, vk: u32) -> bool {
        self.held_keys[..self.held_len].contains(&vk)
    }

    fn mark_down(&mut self, vk: u32) -> bool {
        if self.is_held(vk) {
            return true; // Repeat
        }
        if self.held_len < self.held_keys.len() {
            self.held_keys[self.held_len] = vk;
            self.held_len += 1;
        }
        false
    }

    fn mark_up(&mut self, vk: u32) {
        if let Some(pos) = self.held_keys[..self.held_len].iter().position(|&k| k == vk) {
            for i in (pos + 1)..self.held_len {
                self.held_keys[i - 1] = self.held_keys[i];
            }
            self.held_len -= 1;
            self.held_keys[self.held_len] = 0;
        }
    }
}

thread_local! {
    static HOOK_STATE: RefCell<Option<HookThreadState>> = const { RefCell::new(None) };
}

unsafe extern "system" fn ll_keyboard_proc(code: i32, wparam: WParam, lparam: LParam) -> LResult {
    if code == HC_ACTION && lparam != 0 {
        let kb = &*(lparam as *const KbdLlHookStruct);
        let is_injected = (kb.flags & LLKHF_INJECTED) != 0
            || kb.dw_extra_info == (RATPOISON_EVENT_MAGIC as usize);

        let decision = HOOK_STATE.with(|cell| {
            let mut opt = cell.borrow_mut();
            let Some(state) = opt.as_mut() else {
                return InterceptDecision::PassThrough;
            };

            let phase = match wparam {
                WM_KEYDOWN | WM_SYSKEYDOWN => {
                    if state.mark_down(kb.vk_code) {
                        KeyPhase::Repeat
                    } else {
                        KeyPhase::Down
                    }
                }
                WM_KEYUP | WM_SYSKEYUP => {
                    state.mark_up(kb.vk_code);
                    KeyPhase::Up
                }
                _ => return InterceptDecision::PassThrough,
            };

            let mut raw = RawKeyEvent {
                code: windows_vk_to_normalized(kb.vk_code),
                modifiers: current_modifiers(),
                phase,
                timestamp_ms: state.epoch.elapsed().as_millis() as u64,
                is_injected,
            };
            let dec = (state.callback)(raw);
            zeroize::Zeroize::zeroize(&mut raw);
            dec
        });

        if matches!(
            decision,
            InterceptDecision::Swallow | InterceptDecision::SwallowAndEmit(_)
        ) {
            // Non-zero return value consumes the key event in WH_KEYBOARD_LL.
            return 1;
        }
    }

    CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
}

/// Safe Windows global keyboard hook backed by `SetWindowsHookExW(WH_KEYBOARD_LL)`.
pub struct WindowsKeyboardHook {
    running: Arc<AtomicBool>,
    thread_id: Arc<AtomicU32>,
    worker: Option<JoinHandle<()>>,
}

impl WindowsKeyboardHook {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            thread_id: Arc::new(AtomicU32::new(0)),
            worker: None,
        }
    }
}

impl Default for WindowsKeyboardHook {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyboardHookBackend for WindowsKeyboardHook {
    fn backend_name(&self) -> &'static str {
        "Windows WH_KEYBOARD_LL"
    }

    fn check_permissions(&self) -> PermissionReport {
        let elevated = is_process_elevated();
        let guidance = if elevated {
            None
        } else {
            Some(
                "Running at standard integrity: Windows UIPI prevents controlling elevated windows (e.g. Task Manager). Use a signed uiAccess=\"true\" build or run elevated when needed."
                    .to_string(),
            )
        };

        PermissionReport {
            platform: "windows",
            accessibility_granted: true,
            input_monitoring_granted: true,
            secure_input_active: false,
            elevated_target_caveat: !elevated,
            uinput_available: false,
            xtest_available: false,
            guidance,
        }
    }

    fn start(&mut self, callback: KeyEventCallback) -> Result<(), HidError> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        let (tx, rx) = mpsc::sync_channel::<Result<(), HidError>>(1);
        let running = Arc::clone(&self.running);
        let thread_id = Arc::clone(&self.thread_id);

        let handle = thread::Builder::new()
            .name("ratpoison-win32-llhook".to_string())
            .spawn(move || {
                HOOK_STATE.with(|cell| {
                    *cell.borrow_mut() = Some(HookThreadState {
                        callback,
                        epoch: Instant::now(),
                        held_keys: [0; 16],
                        held_len: 0,
                    });
                });

                // SAFETY: Dedicated hook thread installs WH_KEYBOARD_LL and runs GetMessageW loop.
                let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, ll_keyboard_proc, ptr::null_mut(), 0) };
                if hook.is_null() {
                    HOOK_STATE.with(|cell| *cell.borrow_mut() = None);
                    let err = std::io::Error::last_os_error();
                    let _ = tx.send(Err(HidError::HookInstallFailed(format!(
                        "SetWindowsHookExW(WH_KEYBOARD_LL) failed: {err}"
                    ))));
                    return;
                }

                let tid = unsafe { GetCurrentThreadId() };
                thread_id.store(tid, Ordering::SeqCst);
                running.store(true, Ordering::SeqCst);
                let _ = tx.send(Ok(()));

                let mut msg: Msg = unsafe { zeroed() };
                while unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) } > 0 {}

                unsafe { UnhookWindowsHookEx(hook) };
                HOOK_STATE.with(|cell| *cell.borrow_mut() = None);
                thread_id.store(0, Ordering::SeqCst);
                running.store(false, Ordering::SeqCst);
            })
            .map_err(|e| HidError::HookInstallFailed(format!("failed to spawn Win32 hook thread: {e}")))?;

        match rx.recv() {
            Ok(Ok(())) => {
                self.worker = Some(handle);
                Ok(())
            }
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                let _ = handle.join();
                Err(HidError::HookInstallFailed(
                    "Win32 hook thread terminated unexpectedly".to_string(),
                ))
            }
        }
    }

    fn stop(&mut self) -> Result<(), HidError> {
        let tid = self.thread_id.swap(0, Ordering::SeqCst);
        if tid != 0 {
            // SAFETY: Posting WM_QUIT exits `GetMessageW` cleanly on the hook thread.
            unsafe { PostThreadMessageW(tid, WM_QUIT, 0, 0) };
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

impl Drop for WindowsKeyboardHook {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Safe Windows mouse simulator backed by `SendInput`.
pub struct WindowsMouseSimulator;

impl WindowsMouseSimulator {
    pub fn new() -> Self {
        Self
    }

    fn send_mouse_input(dx: i32, dy: i32, mouse_data: u32, flags: u32) -> Result<(), HidError> {
        let input = Input {
            input_type: INPUT_MOUSE,
            union: InputUnion {
                mi: MouseInput {
                    dx,
                    dy,
                    mouse_data,
                    dw_flags: flags,
                    time: 0,
                    dw_extra_info: RATPOISON_EVENT_MAGIC as usize,
                },
            },
        };
        // SAFETY: Valid single `INPUT` struct passed to `SendInput`.
        let sent = unsafe { SendInput(1, &input, size_of::<Input>() as i32) };
        if sent == 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(5) {
                return Err(HidError::ElevatedWindowBlocked);
            }
            return Err(HidError::SimulationFailed(format!("SendInput failed: {err}")));
        }
        Ok(())
    }

    fn button_flags(button: MouseButton, down: bool) -> (u32, u32) {
        match (button, down) {
            (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
            (MouseButton::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1),
            (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
            (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2),
        }
    }
}

impl Default for WindowsMouseSimulator {
    fn default() -> Self {
        Self::new()
    }
}

impl MouseSimulatorBackend for WindowsMouseSimulator {
    fn backend_name(&self) -> &'static str {
        "Windows SendInput"
    }

    fn move_to(&mut self, point: LogicalPoint, scale_factor: f64) -> Result<(), HidError> {
        let phys = point.to_physical(scale_factor);
        // SAFETY: Pure Win32 virtual screen geometry queries.
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        let (norm_x, norm_y) = normalize_virtual_desktop_coords(phys, vx, vy, vw, vh);
        Self::send_mouse_input(
            norm_x,
            norm_y,
            0,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        )
    }

    fn move_relative(&mut self, dx: f64, dy: f64) -> Result<(), HidError> {
        Self::send_mouse_input(dx.round() as i32, dy.round() as i32, 0, MOUSEEVENTF_MOVE)
    }

    fn mouse_down(
        &mut self,
        button: MouseButton,
        point: LogicalPoint,
        scale_factor: f64,
    ) -> Result<(), HidError> {
        self.move_to(point, scale_factor)?;
        let (flags, data) = Self::button_flags(button, true);
        Self::send_mouse_input(0, 0, data, flags)
    }

    fn mouse_up(
        &mut self,
        button: MouseButton,
        point: LogicalPoint,
        scale_factor: f64,
    ) -> Result<(), HidError> {
        self.move_to(point, scale_factor)?;
        let (flags, data) = Self::button_flags(button, false);
        Self::send_mouse_input(0, 0, data, flags)
    }

    fn perform_click(
        &mut self,
        kind: ClickKind,
        point: LogicalPoint,
        scale_factor: f64,
    ) -> Result<(), HidError> {
        match kind {
            ClickKind::Left => {
                self.mouse_down(MouseButton::Left, point, scale_factor)?;
                self.mouse_up(MouseButton::Left, point, scale_factor)
            }
            ClickKind::Right => {
                self.mouse_down(MouseButton::Right, point, scale_factor)?;
                self.mouse_up(MouseButton::Right, point, scale_factor)
            }
            ClickKind::Middle => {
                self.mouse_down(MouseButton::Middle, point, scale_factor)?;
                self.mouse_up(MouseButton::Middle, point, scale_factor)
            }
            ClickKind::DoubleLeft => {
                for _ in 0..2 {
                    self.mouse_down(MouseButton::Left, point, scale_factor)?;
                    self.mouse_up(MouseButton::Left, point, scale_factor)?;
                }
                Ok(())
            }
            ClickKind::DragStart => self.mouse_down(MouseButton::Left, point, scale_factor),
            ClickKind::DragEnd => self.mouse_up(MouseButton::Left, point, scale_factor),
        }
    }

    fn scroll(&mut self, delta: ScrollDelta) -> Result<(), HidError> {
        if delta.dy.abs() > f64::EPSILON {
            let ticks = (delta.dy * WHEEL_DELTA).round() as i32 as u32;
            Self::send_mouse_input(0, 0, ticks, MOUSEEVENTF_WHEEL)?;
        }
        if delta.dx.abs() > f64::EPSILON {
            let ticks = (delta.dx * WHEEL_DELTA).round() as i32 as u32;
            Self::send_mouse_input(0, 0, ticks, MOUSEEVENTF_HWHEEL)?;
        }
        Ok(())
    }

    fn cursor_position(&self) -> Result<LogicalPoint, HidError> {
        let mut pt = Point { x: 0, y: 0 };
        // SAFETY: `pt` is a valid mutable `POINT`.
        if unsafe { GetCursorPos(&mut pt) } == 0 {
            return Err(HidError::SimulationFailed(format!(
                "GetCursorPos failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(LogicalPoint::new(f64::from(pt.x), f64::from(pt.y)))
    }
}
