//! Linux HID backend supporting Wayland/X11-agnostic `evdev` + `uinput` with X11 `XTest` fallback.
//!
//! # Architecture
//! - **Primary (Wayland + X11): `evdev` (`EVIOCGRAB`) + `uinput` virtual devices**:
//!   1. Opens keyboard `/dev/input/event*` devices and acquires an exclusive kernel grab
//!      via `ioctl(fd, EVIOCGRAB, 1)`.
//!   2. Creates a companion `uinput` virtual keyboard device so `PassThrough` events are
//!      re-injected seamlessly into the compositor with sub-millisecond latency while
//!      `Swallow` / `SwallowAndEmit` events are dropped cleanly before reaching any app.
//!   3. Creates a `uinput` virtual mouse device (`/dev/uinput`) exposing `BTN_LEFT`,
//!      `BTN_RIGHT`, `BTN_MIDDLE`, `BTN_SIDE`, `BTN_EXTRA`, `REL_X`/`REL_Y`/`REL_WHEEL`/
//!      `REL_HWHEEL`, and `ABS_X`/`ABS_Y` for cursor warping, Free Mode relative movement,
//!      clicks, drags, and scrolling.
//! - **Fallback (X11 without `uinput` privileges): `XTest` (`libX11` + `libXtst`)**:
//!   When `/dev/uinput` is not writable and `DISPLAY` is set, mouse simulation can fall
//!   back to `XTestFakeMotionEvent`, `XTestFakeRelativeMotionEvent`, and `XTestFakeButtonEvent`
//!   loaded dynamically via `dlopen` so the binary has zero hard link dependency on X11 libs
//!   on pure Wayland systems.

use core::ffi::{c_char, c_int, c_ulong, c_void};
use core::mem::{size_of, zeroed};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crate::interceptor::InterceptDecision;
use crate::platform::{KeyEventCallback, KeyboardHookBackend, MouseSimulatorBackend};
use crate::types::{
    ClickKind, HidError, KeyCode, KeyPhase, LogicalPoint, Modifiers, MouseButton, PermissionReport,
    RawKeyEvent, ScrollDelta,
};

// Linux input event constants (from <linux/input-event-codes.h>)
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;

const SYN_REPORT: u16 = 0;

const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_HWHEEL: u16 = 0x06;
const REL_WHEEL: u16 = 0x08;

const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;

const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const BTN_SIDE: u16 = 0x113;
const BTN_EXTRA: u16 = 0x114;

// Linux evdev & uinput ioctl request codes
const EVIOCGRAB: c_ulong = 0x4004_4590;
const UI_DEV_CREATE: c_ulong = 0x5501;
const UI_DEV_DESTROY: c_ulong = 0x5502;
const UI_SET_EVBIT: c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: c_ulong = 0x4004_5565;
const UI_SET_RELBIT: c_ulong = 0x4004_5566;
const UI_SET_ABSBIT: c_ulong = 0x4004_5567;
const UI_DEV_SETUP: c_ulong = 0x405C_5503;

const BUS_VIRTUAL: u16 = 0x06;
const UINPUT_MAX_NAME_SIZE: usize = 80;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TimeVal {
    tv_sec: isize,
    tv_usec: isize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct InputEvent {
    time: TimeVal,
    type_: u16,
    code: u16,
    value: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [c_char; UINPUT_MAX_NAME_SIZE],
    ff_effects_max: u32,
}

extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

/// Active Linux HID transport mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxBackendKind {
    EvdevUinput,
    X11XTest,
}

/// Maps a Linux `KEY_*` evdev scan code to RatPoison's normalized [`KeyCode`].
pub fn linux_evdev_key_to_normalized(ev_code: u16) -> KeyCode {
    match ev_code {
        1 => KeyCode::ESCAPE,
        28 | 96 => KeyCode::RETURN,
        57 => KeyCode::SPACE,
        15 => KeyCode::TAB,
        14 => KeyCode::BACKSPACE,
        42 => KeyCode::SHIFT_L,
        54 => KeyCode::SHIFT_R,
        29 => KeyCode::CTRL_L,
        97 => KeyCode::CTRL_R,
        56 => KeyCode::ALT_L,
        100 => KeyCode::ALT_R,
        125 => KeyCode::META_L,
        126 => KeyCode::META_R,
        // US QWERTY evdev codes
        30 => KeyCode::from_ascii_upper(b'A'),
        48 => KeyCode::from_ascii_upper(b'B'),
        46 => KeyCode::from_ascii_upper(b'C'),
        32 => KeyCode::from_ascii_upper(b'D'),
        18 => KeyCode::from_ascii_upper(b'E'),
        33 => KeyCode::from_ascii_upper(b'F'),
        34 => KeyCode::from_ascii_upper(b'G'),
        35 => KeyCode::from_ascii_upper(b'H'),
        23 => KeyCode::from_ascii_upper(b'I'),
        36 => KeyCode::from_ascii_upper(b'J'),
        37 => KeyCode::from_ascii_upper(b'K'),
        38 => KeyCode::from_ascii_upper(b'L'),
        50 => KeyCode::from_ascii_upper(b'M'),
        49 => KeyCode::from_ascii_upper(b'N'),
        24 => KeyCode::from_ascii_upper(b'O'),
        25 => KeyCode::from_ascii_upper(b'P'),
        16 => KeyCode::from_ascii_upper(b'Q'),
        19 => KeyCode::from_ascii_upper(b'R'),
        31 => KeyCode::from_ascii_upper(b'S'),
        20 => KeyCode::from_ascii_upper(b'T'),
        22 => KeyCode::from_ascii_upper(b'U'),
        47 => KeyCode::from_ascii_upper(b'V'),
        17 => KeyCode::from_ascii_upper(b'W'),
        45 => KeyCode::from_ascii_upper(b'X'),
        21 => KeyCode::from_ascii_upper(b'Y'),
        44 => KeyCode::from_ascii_upper(b'Z'),
        other => KeyCode(0x8000 | other),
    }
}

fn can_open_uinput() -> bool {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/uinput")
        .is_ok()
}

fn has_readable_evdev_keyboard() -> Option<PathBuf> {
    let entries = std::fs::read_dir("/dev/input").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with("event") && File::open(&path).is_ok() {
                return Some(path);
            }
        }
    }
    None
}

fn has_x11_display() -> bool {
    std::env::var_os("DISPLAY").map(|v| !v.is_empty()).unwrap_or(false)
}

fn emit_uinput_event(file: &mut File, type_: u16, code: u16, value: i32) -> Result<(), HidError> {
    let ev = InputEvent {
        time: TimeVal::default(),
        type_,
        code,
        value,
    };
    let bytes = unsafe {
        core::slice::from_raw_parts((&ev as *const InputEvent).cast::<u8>(), size_of::<InputEvent>())
    };
    file.write_all(bytes)
        .map_err(|e| HidError::SimulationFailed(format!("uinput write failed: {e}")))
}

fn emit_uinput_sync(file: &mut File) -> Result<(), HidError> {
    emit_uinput_event(file, EV_SYN, SYN_REPORT, 0)
}

fn create_uinput_device(name: &str, configure: impl FnOnce(c_int) -> Result<(), HidError>) -> Result<File, HidError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/uinput")
        .map_err(|e| {
            HidError::PermissionDenied {
                permission: "/dev/uinput",
                detail: format!("cannot open /dev/uinput ({e}); add user to 'input' group and install udev rule for /dev/uinput"),
            }
        })?;

    let fd = file.as_raw_fd();
    configure(fd)?;

    let mut setup: UinputSetup = unsafe { zeroed() };
    setup.id = InputId {
        bustype: BUS_VIRTUAL,
        vendor: 0x1D6B,
        product: 0x5250, // "RP"
        version: 1,
    };
    for (dst, src) in setup.name.iter_mut().zip(name.as_bytes().iter()) {
        *dst = *src as c_char;
    }

    // SAFETY: `setup` matches `struct uinput_setup` for `UI_DEV_SETUP`.
    if unsafe { ioctl(fd, UI_DEV_SETUP, &setup) } < 0 {
        return Err(HidError::DeviceUnavailable(format!(
            "UI_DEV_SETUP failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    if unsafe { ioctl(fd, UI_DEV_CREATE) } < 0 {
        return Err(HidError::DeviceUnavailable(format!(
            "UI_DEV_CREATE failed: {}",
            std::io::Error::last_os_error()
        )));
    }

    Ok(file)
}

/// Safe Linux keyboard hook using `evdev` (`EVIOCGRAB`) + `uinput` virtual keyboard passthrough.
pub struct LinuxKeyboardHook {
    device_path: Option<PathBuf>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl LinuxKeyboardHook {
    pub fn new() -> Self {
        Self {
            device_path: None,
            running: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }

    /// Explicitly binds a specific `/dev/input/eventX` keyboard device path.
    pub fn with_device_path(path: impl AsRef<Path>) -> Self {
        Self {
            device_path: Some(path.as_ref().to_path_buf()),
            running: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }
}

impl Default for LinuxKeyboardHook {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyboardHookBackend for LinuxKeyboardHook {
    fn backend_name(&self) -> &'static str {
        "Linux evdev + uinput"
    }

    fn check_permissions(&self) -> PermissionReport {
        let uinput_available = can_open_uinput();
        let evdev_readable = has_readable_evdev_keyboard().is_some();
        let xtest_available = has_x11_display();

        let guidance = if !uinput_available || !evdev_readable {
            Some(
                "Grant evdev/uinput access via: sudo usermod -aG input $USER and install udev rule KERNEL==\"uinput\", GROUP=\"input\", MODE=\"0660\"."
                    .to_string(),
            )
        } else {
            None
        };

        PermissionReport {
            platform: "linux",
            accessibility_granted: evdev_readable && uinput_available,
            input_monitoring_granted: evdev_readable,
            secure_input_active: false,
            elevated_target_caveat: false,
            uinput_available,
            xtest_available,
            guidance,
        }
    }

    fn start(&mut self, mut callback: KeyEventCallback) -> Result<(), HidError> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        let dev_path = self
            .device_path
            .clone()
            .or_else(has_readable_evdev_keyboard)
            .ok_or_else(|| HidError::PermissionDenied {
                permission: "/dev/input/event*",
                detail: "no readable evdev keyboard device found; add user to 'input' group"
                    .to_string(),
            })?;

        let mut evdev_file = File::open(&dev_path).map_err(|e| HidError::PermissionDenied {
            permission: "/dev/input/event*",
            detail: format!("failed to open {}: {e}", dev_path.display()),
        })?;

        let mut virtual_kbd = create_uinput_device("RatPoison Virtual Keyboard", |fd| {
            unsafe {
                if ioctl(fd, UI_SET_EVBIT, EV_KEY as c_int) < 0 {
                    return Err(HidError::DeviceUnavailable("UI_SET_EVBIT(EV_KEY) failed".to_string()));
                }
                for key in 1..248i32 {
                    let _ = ioctl(fd, UI_SET_KEYBIT, key);
                }
            }
            Ok(())
        })?;

        // Acquire exclusive grab so swallowed keys never leak to the compositor.
        let evdev_fd = evdev_file.as_raw_fd();
        if unsafe { ioctl(evdev_fd, EVIOCGRAB, 1 as c_int) } < 0 {
            return Err(HidError::HookInstallFailed(format!(
                "EVIOCGRAB failed on {}: {}",
                dev_path.display(),
                std::io::Error::last_os_error()
            )));
        }

        let running = Arc::clone(&self.running);
        running.store(true, Ordering::SeqCst);

        let handle = thread::Builder::new()
            .name("ratpoison-linux-evdev".to_string())
            .spawn(move || {
                let epoch = Instant::now();
                let mut modifiers = Modifiers::EMPTY;
                let mut buf = [0u8; size_of::<InputEvent>()];

                while running.load(Ordering::SeqCst) {
                    if evdev_file.read_exact(&mut buf).is_err() {
                        break;
                    }
                    // SAFETY: `buf` has exact size of `#[repr(C)] InputEvent`.
                    let ev: InputEvent = unsafe { core::ptr::read_unaligned(buf.as_ptr().cast()) };

                    if ev.type_ != EV_KEY {
                        let _ = emit_uinput_event(&mut virtual_kbd, ev.type_, ev.code, ev.value);
                        continue;
                    }

                    let code = linux_evdev_key_to_normalized(ev.code);
                    let phase = match ev.value {
                        0 => KeyPhase::Up,
                        1 => KeyPhase::Down,
                        2 => KeyPhase::Repeat,
                        _ => continue,
                    };

                    let mod_bit = code.as_modifier();
                    if !mod_bit.is_empty() {
                        match phase {
                            KeyPhase::Down | KeyPhase::Repeat => modifiers.insert(mod_bit),
                            KeyPhase::Up => modifiers.remove(mod_bit),
                        }
                    }

                    let mut raw = RawKeyEvent {
                        code,
                        modifiers,
                        phase,
                        timestamp_ms: epoch.elapsed().as_millis() as u64,
                        is_injected: false,
                    };
                    let decision = callback(raw);
                    zeroize::Zeroize::zeroize(&mut raw);

                    if matches!(decision, InterceptDecision::PassThrough) {
                        let _ = emit_uinput_event(&mut virtual_kbd, EV_KEY, ev.code, ev.value);
                        let _ = emit_uinput_sync(&mut virtual_kbd);
                    }
                }

                // Release exclusive evdev grab and destroy uinput device cleanly.
                unsafe {
                    let _ = ioctl(evdev_file.as_raw_fd(), EVIOCGRAB, 0 as c_int);
                    let _ = ioctl(virtual_kbd.as_raw_fd(), UI_DEV_DESTROY);
                }
            })
            .map_err(|e| HidError::HookInstallFailed(format!("failed to spawn evdev thread: {e}")))?;

        self.worker = Some(handle);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), HidError> {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

impl Drop for LinuxKeyboardHook {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Safe Linux mouse simulator preferring `uinput` (Wayland + X11) with dynamic X11 `XTest` fallback.
pub struct LinuxMouseSimulator {
    backend_kind: LinuxBackendKind,
    uinput_mouse: Option<File>,
    x11_fallback: Option<XTestFallback>,
    last_cursor: LogicalPoint,
}

impl LinuxMouseSimulator {
    pub fn new() -> Self {
        if let Ok(file) = Self::open_uinput_mouse() {
            return Self {
                backend_kind: LinuxBackendKind::EvdevUinput,
                uinput_mouse: Some(file),
                x11_fallback: None,
                last_cursor: LogicalPoint::default(),
            };
        }

        let x11 = XTestFallback::try_open().ok();
        Self {
            backend_kind: LinuxBackendKind::X11XTest,
            uinput_mouse: None,
            x11_fallback: x11,
            last_cursor: LogicalPoint::default(),
        }
    }

    pub const fn backend_kind(&self) -> LinuxBackendKind {
        self.backend_kind
    }

    fn open_uinput_mouse() -> Result<File, HidError> {
        create_uinput_device("RatPoison Virtual Mouse", |fd| {
            unsafe {
                let _ = ioctl(fd, UI_SET_EVBIT, EV_KEY as c_int);
                let _ = ioctl(fd, UI_SET_EVBIT, EV_REL as c_int);
                let _ = ioctl(fd, UI_SET_EVBIT, EV_ABS as c_int);

                for btn in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, BTN_SIDE, BTN_EXTRA] {
                    let _ = ioctl(fd, UI_SET_KEYBIT, btn as c_int);
                }
                for rel in [REL_X, REL_Y, REL_WHEEL, REL_HWHEEL] {
                    let _ = ioctl(fd, UI_SET_RELBIT, rel as c_int);
                }
                for abs in [ABS_X, ABS_Y] {
                    let _ = ioctl(fd, UI_SET_ABSBIT, abs as c_int);
                }
            }
            Ok(())
        })
    }

    fn map_uinput_button(button: MouseButton) -> u16 {
        match button {
            MouseButton::Left => BTN_LEFT,
            MouseButton::Right => BTN_RIGHT,
            MouseButton::Middle => BTN_MIDDLE,
            MouseButton::Back => BTN_SIDE,
            MouseButton::Forward => BTN_EXTRA,
        }
    }

    fn map_x11_button(button: MouseButton) -> u32 {
        match button {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 3,
            MouseButton::Back => 8,
            MouseButton::Forward => 9,
        }
    }
}

impl Default for LinuxMouseSimulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LinuxMouseSimulator {
    fn drop(&mut self) {
        if let Some(ref file) = self.uinput_mouse {
            unsafe {
                let _ = ioctl(file.as_raw_fd(), UI_DEV_DESTROY);
            }
        }
    }
}

impl MouseSimulatorBackend for LinuxMouseSimulator {
    fn backend_name(&self) -> &'static str {
        match self.backend_kind {
            LinuxBackendKind::EvdevUinput => "Linux uinput Virtual Mouse",
            LinuxBackendKind::X11XTest => "Linux X11 XTest Fallback",
        }
    }

    fn move_to(&mut self, point: LogicalPoint, scale_factor: f64) -> Result<(), HidError> {
        let phys = point.to_physical(scale_factor);
        self.last_cursor = point;

        if let Some(ref mut file) = self.uinput_mouse {
            emit_uinput_event(file, EV_ABS, ABS_X, phys.x)?;
            emit_uinput_event(file, EV_ABS, ABS_Y, phys.y)?;
            return emit_uinput_sync(file);
        }

        if let Some(ref x11) = self.x11_fallback {
            return x11.motion(phys.x, phys.y);
        }

        Err(HidError::DeviceUnavailable(
            "neither /dev/uinput nor X11 XTest is available for mouse simulation".to_string(),
        ))
    }

    fn move_relative(&mut self, dx: f64, dy: f64) -> Result<(), HidError> {
        self.last_cursor = LogicalPoint::new(self.last_cursor.x + dx, self.last_cursor.y + dy);

        if let Some(ref mut file) = self.uinput_mouse {
            emit_uinput_event(file, EV_REL, REL_X, dx.round() as i32)?;
            emit_uinput_event(file, EV_REL, REL_Y, dy.round() as i32)?;
            return emit_uinput_sync(file);
        }

        if let Some(ref x11) = self.x11_fallback {
            return x11.relative_motion(dx.round() as i32, dy.round() as i32);
        }

        Err(HidError::DeviceUnavailable(
            "neither /dev/uinput nor X11 XTest is available".to_string(),
        ))
    }

    fn mouse_down(
        &mut self,
        button: MouseButton,
        point: LogicalPoint,
        scale_factor: f64,
    ) -> Result<(), HidError> {
        self.move_to(point, scale_factor)?;
        if let Some(ref mut file) = self.uinput_mouse {
            emit_uinput_event(file, EV_KEY, Self::map_uinput_button(button), 1)?;
            return emit_uinput_sync(file);
        }
        if let Some(ref x11) = self.x11_fallback {
            return x11.button(Self::map_x11_button(button), true);
        }
        Err(HidError::DeviceUnavailable(
            "no Linux mouse simulation backend available".to_string(),
        ))
    }

    fn mouse_up(
        &mut self,
        button: MouseButton,
        point: LogicalPoint,
        scale_factor: f64,
    ) -> Result<(), HidError> {
        self.move_to(point, scale_factor)?;
        if let Some(ref mut file) = self.uinput_mouse {
            emit_uinput_event(file, EV_KEY, Self::map_uinput_button(button), 0)?;
            return emit_uinput_sync(file);
        }
        if let Some(ref x11) = self.x11_fallback {
            return x11.button(Self::map_x11_button(button), false);
        }
        Err(HidError::DeviceUnavailable(
            "no Linux mouse simulation backend available".to_string(),
        ))
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
        if let Some(ref mut file) = self.uinput_mouse {
            if delta.dy.abs() > f64::EPSILON {
                emit_uinput_event(file, EV_REL, REL_WHEEL, delta.dy.round() as i32)?;
            }
            if delta.dx.abs() > f64::EPSILON {
                emit_uinput_event(file, EV_REL, REL_HWHEEL, delta.dx.round() as i32)?;
            }
            return emit_uinput_sync(file);
        }

        if let Some(ref x11) = self.x11_fallback {
            // X11 maps scroll ticks to buttons 4/5 (vertical) and 6/7 (horizontal).
            if delta.dy > 0.0 {
                x11.button(4, true)?;
                x11.button(4, false)?;
            } else if delta.dy < 0.0 {
                x11.button(5, true)?;
                x11.button(5, false)?;
            }
            if delta.dx > 0.0 {
                x11.button(7, true)?;
                x11.button(7, false)?;
            } else if delta.dx < 0.0 {
                x11.button(6, true)?;
                x11.button(6, false)?;
            }
            return Ok(());
        }

        Err(HidError::DeviceUnavailable(
            "no Linux scroll backend available".to_string(),
        ))
    }

    fn cursor_position(&self) -> Result<LogicalPoint, HidError> {
        Ok(self.last_cursor)
    }
}

/// Dynamic runtime `dlopen` binding to `libX11.so.6` and `libXtst.so.6` so pure Wayland
/// Linux builds do not fail to link if X11 development libraries are absent.
struct XTestFallback {
    lib_x11: *mut c_void,
    lib_xtst: *mut c_void,
    display: *mut c_void,
    x_close_display: unsafe extern "C" fn(*mut c_void) -> c_int,
    x_flush: unsafe extern "C" fn(*mut c_void) -> c_int,
    xtest_fake_motion: unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int, c_ulong) -> c_int,
    xtest_fake_rel_motion: unsafe extern "C" fn(*mut c_void, c_int, c_int, c_ulong) -> c_int,
    xtest_fake_button: unsafe extern "C" fn(*mut c_void, u32, c_int, c_ulong) -> c_int,
}

// SAFETY: `XTestFallback` owns its private X11 `Display*` connection and is only accessed via `&mut self`.
unsafe impl Send for XTestFallback {}

impl XTestFallback {
    fn try_open() -> Result<Self, HidError> {
        const RTLD_LAZY: c_int = 1;
        extern "C" {
            fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
            fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
            fn dlclose(handle: *mut c_void) -> c_int;
        }

        if !has_x11_display() {
            return Err(HidError::DeviceUnavailable("DISPLAY is not set".to_string()));
        }

        unsafe {
            let lib_x11 = dlopen(b"libX11.so.6\0".as_ptr().cast(), RTLD_LAZY);
            if lib_x11.is_null() {
                return Err(HidError::DeviceUnavailable("libX11.so.6 not found".to_string()));
            }
            let lib_xtst = dlopen(b"libXtst.so.6\0".as_ptr().cast(), RTLD_LAZY);
            if lib_xtst.is_null() {
                dlclose(lib_x11);
                return Err(HidError::DeviceUnavailable("libXtst.so.6 not found".to_string()));
            }

            let open_sym = dlsym(lib_x11, b"XOpenDisplay\0".as_ptr().cast());
            let close_sym = dlsym(lib_x11, b"XCloseDisplay\0".as_ptr().cast());
            let flush_sym = dlsym(lib_x11, b"XFlush\0".as_ptr().cast());
            let motion_sym = dlsym(lib_xtst, b"XTestFakeMotionEvent\0".as_ptr().cast());
            let rel_sym = dlsym(lib_xtst, b"XTestFakeRelativeMotionEvent\0".as_ptr().cast());
            let btn_sym = dlsym(lib_xtst, b"XTestFakeButtonEvent\0".as_ptr().cast());

            if open_sym.is_null()
                || close_sym.is_null()
                || flush_sym.is_null()
                || motion_sym.is_null()
                || rel_sym.is_null()
                || btn_sym.is_null()
            {
                dlclose(lib_xtst);
                dlclose(lib_x11);
                return Err(HidError::DeviceUnavailable(
                    "required X11/XTest symbols missing".to_string(),
                ));
            }

            let x_open_display: unsafe extern "C" fn(*const c_char) -> *mut c_void =
                core::mem::transmute(open_sym);
            let display = x_open_display(core::ptr::null());
            if display.is_null() {
                dlclose(lib_xtst);
                dlclose(lib_x11);
                return Err(HidError::DeviceUnavailable(
                    "XOpenDisplay(NULL) failed".to_string(),
                ));
            }

            Ok(Self {
                lib_x11,
                lib_xtst,
                display,
                x_close_display: core::mem::transmute(close_sym),
                x_flush: core::mem::transmute(flush_sym),
                xtest_fake_motion: core::mem::transmute(motion_sym),
                xtest_fake_rel_motion: core::mem::transmute(rel_sym),
                xtest_fake_button: core::mem::transmute(btn_sym),
            })
        }
    }

    fn motion(&self, x: i32, y: i32) -> Result<(), HidError> {
        unsafe {
            (self.xtest_fake_motion)(self.display, -1, x, y, 0);
            (self.x_flush)(self.display);
        }
        Ok(())
    }

    fn relative_motion(&self, dx: i32, dy: i32) -> Result<(), HidError> {
        unsafe {
            (self.xtest_fake_rel_motion)(self.display, dx, dy, 0);
            (self.x_flush)(self.display);
        }
        Ok(())
    }

    fn button(&self, btn: u32, pressed: bool) -> Result<(), HidError> {
        unsafe {
            (self.xtest_fake_button)(self.display, btn, i32::from(pressed), 0);
            (self.x_flush)(self.display);
        }
        Ok(())
    }
}

impl Drop for XTestFallback {
    fn drop(&mut self) {
        extern "C" {
            fn dlclose(handle: *mut c_void) -> c_int;
        }
        unsafe {
            if !self.display.is_null() {
                (self.x_close_display)(self.display);
            }
            if !self.lib_xtst.is_null() {
                dlclose(self.lib_xtst);
            }
            if !self.lib_x11.is_null() {
                dlclose(self.lib_x11);
            }
        }
    }
}
