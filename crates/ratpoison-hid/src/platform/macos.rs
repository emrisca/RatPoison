//! macOS HID backend using `CGEventTapCreate`, `CGEventPost`, `AXIsProcessTrusted`,
//! `CGPreflightListenEventAccess`, and `IsSecureEventInputEnabled`.
//!
//! Uses direct zero-dependency FFI bindings to macOS system frameworks (`CoreGraphics`,
//! `CoreFoundation`, `ApplicationServices`, `Carbon`) with RAII `CFRelease` guards.
//!
//! # Coordinate Model
//! CoreGraphics `CGPoint` operates directly in **logical points** (top-left origin on the
//! primary display, matching `SWARM_CONTRACT.md`'s `LogicalPoint`). `scale_factor` is
//! accepted by the trait interface and validated without double-scaling `CGPoint`.

use core::ffi::c_void;
use core::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crate::interceptor::InterceptDecision;
use crate::platform::{KeyEventCallback, KeyboardHookBackend, MouseSimulatorBackend};
use crate::types::{
    ClickKind, HidError, KeyCode, KeyPhase, LogicalPoint, Modifiers, MouseButton, PermissionReport,
    RawKeyEvent, ScrollDelta, RATPOISON_EVENT_MAGIC,
};

// CoreGraphics / CoreFoundation opaque types
type CFMachPortRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFStringRef = *const c_void;
type CGEventRef = *mut c_void;
type CGEventTapProxy = *mut c_void;

type CGEventTapCallBack = extern "C" fn(
    proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CGPoint {
    x: f64,
    y: f64,
}

// CGEventTap constants
const K_CG_HID_EVENT_TAP: u32 = 0;
const K_CG_SESSION_EVENT_TAP: u32 = 1;
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;

// CGEventType values
const K_CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const K_CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
const K_CG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const K_CG_EVENT_RIGHT_MOUSE_UP: u32 = 4;
const K_CG_EVENT_MOUSE_MOVED: u32 = 5;
const K_CG_EVENT_KEY_DOWN: u32 = 10;
const K_CG_EVENT_KEY_UP: u32 = 11;
const K_CG_EVENT_FLAGS_CHANGED: u32 = 12;
const K_CG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
const K_CG_EVENT_OTHER_MOUSE_UP: u32 = 26;
const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

// CGMouseButton values
const K_CG_MOUSE_BUTTON_LEFT: u32 = 0;
const K_CG_MOUSE_BUTTON_RIGHT: u32 = 1;
const K_CG_MOUSE_BUTTON_CENTER: u32 = 2;

// CGEventField values
const K_CG_MOUSE_EVENT_CLICK_STATE: u32 = 1;
const K_CG_KEYBOARD_EVENT_AUTOREPEAT: u32 = 8;
const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
const K_CG_EVENT_SOURCE_USER_DATA: u32 = 42;

// CGScrollEventUnit::Pixel
const K_CG_SCROLL_EVENT_UNIT_PIXEL: u32 = 0;

// CGEventFlags bitmasks
const K_CG_EVENT_FLAG_MASK_ALPHA_SHIFT: u64 = 0x0001_0000;
const K_CG_EVENT_FLAG_MASK_SHIFT: u64 = 0x0002_0000;
const K_CG_EVENT_FLAG_MASK_CONTROL: u64 = 0x0004_0000;
const K_CG_EVENT_FLAG_MASK_ALTERNATE: u64 = 0x0008_0000;
const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
const K_CG_EVENT_FLAG_MASK_SECONDARY_FN: u64 = 0x0080_0000;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    static kAXTrustedCheckOptionPrompt: CFStringRef;
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
}

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> u8;
    fn CGRequestListenEventAccess() -> u8;

    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;

    fn CGEventTapEnable(tap: CFMachPortRef, enable: u8);
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);

    fn CGEventCreate(source: *const c_void) -> CGEventRef;
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;

    fn CGEventCreateMouseEvent(
        source: *const c_void,
        mouse_type: u32,
        mouse_cursor_position: CGPoint,
        mouse_button: u32,
    ) -> CGEventRef;

    fn CGEventCreateScrollWheelEvent2(
        source: *const c_void,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> CGEventRef;

    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGWarpMouseCursorPosition(new_cursor_position: CGPoint) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopCommonModes: CFStringRef;
    static kCFBooleanTrue: *const c_void;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *mut c_void;

    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: CFMachPortRef,
        order: isize,
    ) -> CFRunLoopSourceRef;

    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRemoveSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRun();
    fn CFRunLoopStop(rl: CFRunLoopRef);
    fn CFRetain(cf: *const c_void) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

/// RAII wrapper guaranteeing `CFRelease` on any non-null CoreFoundation object.
struct CfOwned(*mut c_void);

impl CfOwned {
    fn new(ptr: *mut c_void) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    #[inline]
    fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}

impl Drop for CfOwned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` was obtained via a Create/Copy/Retain CF/CG API.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// Queries macOS Accessibility trust without triggering a system dialog.
pub fn is_accessibility_trusted() -> bool {
    // SAFETY: Pure C status query with no pointer arguments.
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Queries macOS Input Monitoring preflight status (macOS 10.15+) without prompting.
pub fn is_input_monitoring_granted() -> bool {
    // SAFETY: Pure C status query with no pointer arguments.
    unsafe { CGPreflightListenEventAccess() != 0 }
}

/// Triggers the native macOS Accessibility & Input Monitoring trust prompts if not yet granted.
pub fn request_accessibility_and_input_monitoring() -> bool {
    // SAFETY: Builds a single-entry CoreFoundation dictionary `{ kAXTrustedCheckOptionPrompt: true }`
    // and releases it via `CfOwned`.
    let trusted = unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let vals = [kCFBooleanTrue];
        let dict_raw = CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            vals.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const c_void,
        );
        let _dict_guard = CfOwned::new(dict_raw);
        if dict_raw.is_null() {
            AXIsProcessTrusted() != 0
        } else {
            AXIsProcessTrustedWithOptions(dict_raw) != 0
        }
    };
    if !is_input_monitoring_granted() {
        // SAFETY: Standard CoreGraphics permission request API.
        unsafe {
            let _ = CGRequestListenEventAccess();
        }
    }
    trusted
}

/// Returns `true` when macOS Secure Event Input is active (e.g., password fields,
/// 1Password, or Terminal Secure Keyboard Entry).
pub fn is_secure_event_input_enabled() -> bool {
    // SAFETY: Pure HIToolbox status query with no side effects.
    unsafe { IsSecureEventInputEnabled() != 0 }
}

/// Maps `CGEventFlags` bitmask to platform-normalized [`Modifiers`].
pub fn cg_flags_to_modifiers(flags: u64) -> Modifiers {
    let mut mods = Modifiers::EMPTY;
    if (flags & K_CG_EVENT_FLAG_MASK_SHIFT) != 0 {
        mods.insert(Modifiers::SHIFT);
    }
    if (flags & K_CG_EVENT_FLAG_MASK_CONTROL) != 0 {
        mods.insert(Modifiers::CONTROL);
    }
    if (flags & K_CG_EVENT_FLAG_MASK_ALTERNATE) != 0 {
        mods.insert(Modifiers::ALT);
    }
    if (flags & K_CG_EVENT_FLAG_MASK_COMMAND) != 0 {
        mods.insert(Modifiers::META);
    }
    if (flags & K_CG_EVENT_FLAG_MASK_ALPHA_SHIFT) != 0 {
        mods.insert(Modifiers::CAPS_LOCK);
    }
    if (flags & K_CG_EVENT_FLAG_MASK_SECONDARY_FN) != 0 {
        mods.insert(Modifiers::FN);
    }
    mods
}

/// Normalizes a macOS virtual `CGKeyCode` into RatPoison's cross-platform [`KeyCode`].
pub fn macos_keycode_to_normalized(cg_code: u16) -> KeyCode {
    match cg_code {
        0x35 => KeyCode::ESCAPE,
        0x24 | 0x4C => KeyCode::RETURN,
        0x31 => KeyCode::SPACE,
        0x30 => KeyCode::TAB,
        0x33 => KeyCode::BACKSPACE,
        0x38 => KeyCode::SHIFT_L,
        0x3C => KeyCode::SHIFT_R,
        0x3B => KeyCode::CTRL_L,
        0x3E => KeyCode::CTRL_R,
        0x3A => KeyCode::ALT_L,
        0x3D => KeyCode::ALT_R,
        0x37 => KeyCode::META_L,
        0x36 => KeyCode::META_R,
        // ANSI US QWERTY letters
        0x00 => KeyCode::from_ascii_upper(b'A'),
        0x0B => KeyCode::from_ascii_upper(b'B'),
        0x08 => KeyCode::from_ascii_upper(b'C'),
        0x02 => KeyCode::from_ascii_upper(b'D'),
        0x0E => KeyCode::from_ascii_upper(b'E'),
        0x03 => KeyCode::from_ascii_upper(b'F'),
        0x05 => KeyCode::from_ascii_upper(b'G'),
        0x04 => KeyCode::from_ascii_upper(b'H'),
        0x22 => KeyCode::from_ascii_upper(b'I'),
        0x26 => KeyCode::from_ascii_upper(b'J'),
        0x28 => KeyCode::from_ascii_upper(b'K'),
        0x25 => KeyCode::from_ascii_upper(b'L'),
        0x2E => KeyCode::from_ascii_upper(b'M'),
        0x2D => KeyCode::from_ascii_upper(b'N'),
        0x1F => KeyCode::from_ascii_upper(b'O'),
        0x23 => KeyCode::from_ascii_upper(b'P'),
        0x0C => KeyCode::from_ascii_upper(b'Q'),
        0x0F => KeyCode::from_ascii_upper(b'R'),
        0x01 => KeyCode::from_ascii_upper(b'S'),
        0x11 => KeyCode::from_ascii_upper(b'T'),
        0x20 => KeyCode::from_ascii_upper(b'U'),
        0x09 => KeyCode::from_ascii_upper(b'V'),
        0x0D => KeyCode::from_ascii_upper(b'W'),
        0x07 => KeyCode::from_ascii_upper(b'X'),
        0x10 => KeyCode::from_ascii_upper(b'Y'),
        0x06 => KeyCode::from_ascii_upper(b'Z'),
        // ANSI US digits
        0x1D => KeyCode::from_ascii_digit(b'0'),
        0x12 => KeyCode::from_ascii_digit(b'1'),
        0x13 => KeyCode::from_ascii_digit(b'2'),
        0x14 => KeyCode::from_ascii_digit(b'3'),
        0x15 => KeyCode::from_ascii_digit(b'4'),
        0x17 => KeyCode::from_ascii_digit(b'5'),
        0x16 => KeyCode::from_ascii_digit(b'6'),
        0x1A => KeyCode::from_ascii_digit(b'7'),
        0x1C => KeyCode::from_ascii_digit(b'8'),
        0x19 => KeyCode::from_ascii_digit(b'9'),
        // Arrow keys
        0x7B => KeyCode::ARROW_LEFT,
        0x7C => KeyCode::ARROW_RIGHT,
        0x7D => KeyCode::ARROW_DOWN,
        0x7E => KeyCode::ARROW_UP,
        // Common punctuation
        0x2B => KeyCode::COMMA,
        0x2F => KeyCode::PERIOD,
        0x2C => KeyCode::SLASH,
        0x29 => KeyCode::SEMICOLON,
        0x27 => KeyCode::APOSTROPHE,
        0x21 => KeyCode::BRACKET_LEFT,
        0x1E => KeyCode::BRACKET_RIGHT,
        0x1B => KeyCode::MINUS,
        0x18 => KeyCode::EQUAL,
        0x32 => KeyCode::GRAVE,
        0x2A => KeyCode::BACKSLASH,
        other => KeyCode(0x8000 | other),
    }
}

struct TapContext {
    callback: KeyEventCallback,
    tap_port: CFMachPortRef,
    epoch: Instant,
    prev_modifiers: Modifiers,
}

extern "C" fn cg_event_tap_trampoline(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    if user_info.is_null() {
        return event;
    }

    // SAFETY: `user_info` points to the heap-allocated `TapContext` that lives for the
    // entire duration of the `CFRunLoopRun` call on this thread.
    let ctx = unsafe { &mut *(user_info as *mut TapContext) };

    // Auto-recover if macOS temporarily disabled the tap due to watchdog timeout.
    if event_type == K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT
        || event_type == K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT
    {
        if !ctx.tap_port.is_null() {
            // SAFETY: `ctx.tap_port` is a valid `CFMachPortRef` owned by this thread.
            unsafe { CGEventTapEnable(ctx.tap_port, 1) };
        }
        return event;
    }

    // Privacy invariant: never intercept while macOS Secure Event Input is active.
    if is_secure_event_input_enabled() {
        return event;
    }

    // SAFETY: `event` is a valid non-null `CGEventRef` provided by CoreGraphics.
    let (user_data, raw_cg_code, is_autorepeat, flags) = unsafe {
        (
            CGEventGetIntegerValueField(event, K_CG_EVENT_SOURCE_USER_DATA) as u64,
            CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) as u16,
            CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0,
            CGEventGetFlags(event),
        )
    };

    let code = macos_keycode_to_normalized(raw_cg_code);
    let modifiers = cg_flags_to_modifiers(flags);
    let is_injected = user_data == RATPOISON_EVENT_MAGIC;
    let timestamp_ms = ctx.epoch.elapsed().as_millis() as u64;

    let phase = match event_type {
        K_CG_EVENT_KEY_DOWN => {
            if is_autorepeat {
                KeyPhase::Repeat
            } else {
                KeyPhase::Down
            }
        }
        K_CG_EVENT_KEY_UP => KeyPhase::Up,
        K_CG_EVENT_FLAGS_CHANGED => {
            let mod_bit = code.as_modifier();
            let now_down = !mod_bit.is_empty() && modifiers.contains(mod_bit);
            let was_down = !mod_bit.is_empty() && ctx.prev_modifiers.contains(mod_bit);
            ctx.prev_modifiers = modifiers;
            if now_down && !was_down {
                KeyPhase::Down
            } else if !now_down && was_down {
                KeyPhase::Up
            } else {
                return event;
            }
        }
        _ => return event,
    };

    ctx.prev_modifiers = modifiers;

    let mut raw_event = RawKeyEvent {
        code,
        modifiers,
        phase,
        timestamp_ms,
        is_injected,
    };

    let decision = (ctx.callback)(raw_event);
    // Explicitly wipe transient stack copy of the keystroke immediately after callback returns.
    zeroize::Zeroize::zeroize(&mut raw_event);

    match decision {
        InterceptDecision::PassThrough => event,
        InterceptDecision::Swallow | InterceptDecision::SwallowAndEmit(_) => ptr::null_mut(),
    }
}

/// Safe macOS global keyboard hook backed by `CGEventTapCreate` on a dedicated `CFRunLoop` thread.
pub struct MacOsKeyboardHook {
    running: Arc<AtomicBool>,
    run_loop_addr: Arc<AtomicUsize>,
    worker: Option<JoinHandle<()>>,
}

impl MacOsKeyboardHook {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            run_loop_addr: Arc::new(AtomicUsize::new(0)),
            worker: None,
        }
    }
}

impl Default for MacOsKeyboardHook {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyboardHookBackend for MacOsKeyboardHook {
    fn backend_name(&self) -> &'static str {
        "macOS CGEventTap"
    }

    fn check_permissions(&self) -> PermissionReport {
        let accessibility_granted = is_accessibility_trusted();
        let input_monitoring_granted = is_input_monitoring_granted();
        let secure_input_active = is_secure_event_input_enabled();

        let guidance = if !accessibility_granted {
            Some(
                "Grant Accessibility to RatPoison in System Settings -> Privacy & Security -> Accessibility. If RatPoison was already enabled from an older version, click Grant Permission to reset the stale macOS entry and toggle it back on."
                    .to_string(),
            )
        } else if secure_input_active {
            Some(
                "macOS Secure Keyboard Input is currently active (password field or Terminal Secure Keyboard Entry); global hook is paused."
                    .to_string(),
            )
        } else {
            None
        };

        PermissionReport {
            platform: "macos",
            accessibility_granted,
            input_monitoring_granted,
            secure_input_active,
            elevated_target_caveat: false,
            uinput_available: false,
            xtest_available: false,
            guidance,
        }
    }

    fn start(&mut self, callback: KeyEventCallback) -> Result<(), HidError> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }
        if !is_accessibility_trusted() {
            return Err(HidError::PermissionDenied {
                permission: "Accessibility",
                detail: "AXIsProcessTrusted() returned false; enable RatPoison in System Settings -> Privacy & Security -> Accessibility".to_string(),
            });
        }

        let (tx, rx) = mpsc::sync_channel::<Result<(), HidError>>(1);
        let running = Arc::clone(&self.running);
        let run_loop_addr = Arc::clone(&self.run_loop_addr);

        let handle = thread::Builder::new()
            .name("ratpoison-macos-cgeventtap".to_string())
            .spawn(move || {
                let event_mask: u64 = (1u64 << K_CG_EVENT_KEY_DOWN)
                    | (1u64 << K_CG_EVENT_KEY_UP)
                    | (1u64 << K_CG_EVENT_FLAGS_CHANGED);

                let mut ctx = Box::new(TapContext {
                    callback,
                    tap_port: ptr::null_mut(),
                    epoch: Instant::now(),
                    prev_modifiers: Modifiers::EMPTY,
                });
                let ctx_ptr = (&mut *ctx) as *mut TapContext as *mut c_void;

                // SAFETY: `cg_event_tap_trampoline` matches `CGEventTapCallBack`, and `ctx`
                // outlives `CFRunLoopRun` on this thread.
                let tap_raw = unsafe {
                    let hid = CGEventTapCreate(
                        K_CG_HID_EVENT_TAP,
                        K_CG_HEAD_INSERT_EVENT_TAP,
                        K_CG_EVENT_TAP_OPTION_DEFAULT,
                        event_mask,
                        cg_event_tap_trampoline,
                        ctx_ptr,
                    );
                    if hid.is_null() {
                        CGEventTapCreate(
                            K_CG_SESSION_EVENT_TAP,
                            K_CG_HEAD_INSERT_EVENT_TAP,
                            K_CG_EVENT_TAP_OPTION_DEFAULT,
                            event_mask,
                            cg_event_tap_trampoline,
                            ctx_ptr,
                        )
                    } else {
                        hid
                    }
                };

                let Some(tap_guard) = CfOwned::new(tap_raw) else {
                    let _ = tx.send(Err(HidError::HookInstallFailed(
                        "CGEventTapCreate returned NULL (verify Accessibility & Input Monitoring permissions)".to_string(),
                    )));
                    return;
                };

                ctx.tap_port = tap_guard.as_ptr();

                // SAFETY: `tap_guard` is a valid non-null `CFMachPortRef`.
                let source_raw = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), tap_guard.as_ptr(), 0) };
                let Some(source_guard) = CfOwned::new(source_raw) else {
                    let _ = tx.send(Err(HidError::HookInstallFailed(
                        "CFMachPortCreateRunLoopSource returned NULL".to_string(),
                    )));
                    return;
                };

                // SAFETY: Running on our dedicated thread's CFRunLoop.
                unsafe {
                    let rl = CFRunLoopGetCurrent();
                    CFRetain(rl);
                    run_loop_addr.store(rl as usize, Ordering::SeqCst);
                    CFRunLoopAddSource(rl, source_guard.as_ptr(), kCFRunLoopCommonModes);
                    CGEventTapEnable(tap_guard.as_ptr(), 1);

                    running.store(true, Ordering::SeqCst);
                    let _ = tx.send(Ok(()));

                    CFRunLoopRun();

                    CGEventTapEnable(tap_guard.as_ptr(), 0);
                    CFRunLoopRemoveSource(rl, source_guard.as_ptr(), kCFRunLoopCommonModes);
                    run_loop_addr.store(0, Ordering::SeqCst);
                    CFRelease(rl);
                }

                running.store(false, Ordering::SeqCst);
            })
            .map_err(|e| HidError::HookInstallFailed(format!("failed to spawn runloop thread: {e}")))?;

        match rx.recv() {
            Ok(Ok(())) => {
                self.worker = Some(handle);
                Ok(())
            }
            Ok(Err(err)) => {
                let _ = handle.join();
                Err(err)
            }
            Err(_) => {
                let _ = handle.join();
                Err(HidError::HookInstallFailed(
                    "macOS CGEventTap thread exited unexpectedly during startup".to_string(),
                ))
            }
        }
    }

    fn stop(&mut self) -> Result<(), HidError> {
        let rl_ptr = self.run_loop_addr.swap(0, Ordering::SeqCst) as CFRunLoopRef;
        if !rl_ptr.is_null() {
            // SAFETY: `CFRunLoopStop` is explicitly documented as thread-safe by Apple.
            unsafe { CFRunLoopStop(rl_ptr) };
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

    fn is_secure_input_active(&self) -> bool {
        is_secure_event_input_enabled()
    }
}

impl Drop for MacOsKeyboardHook {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Safe macOS synthetic mouse simulator backed by `CGEventPost(kCGHIDEventTap, ...)`.
pub struct MacOsMouseSimulator;

impl MacOsMouseSimulator {
    pub fn new() -> Self {
        Self
    }

    fn post_mouse_event(
        event_type: u32,
        point: LogicalPoint,
        button: u32,
        click_state: Option<i64>,
    ) -> Result<(), HidError> {
        let cg_pt = CGPoint {
            x: point.x,
            y: point.y,
        };
        // SAFETY: Valid CoreGraphics call; result is wrapped in `CfOwned` for guaranteed `CFRelease`.
        let ev_raw = unsafe { CGEventCreateMouseEvent(ptr::null(), event_type, cg_pt, button) };
        let ev = CfOwned::new(ev_raw).ok_or_else(|| {
            HidError::SimulationFailed("CGEventCreateMouseEvent returned NULL".to_string())
        })?;

        // SAFETY: `ev` is non-null; tag synthetic event with `RATPOISON_EVENT_MAGIC`.
        unsafe {
            CGEventSetIntegerValueField(
                ev.as_ptr(),
                K_CG_EVENT_SOURCE_USER_DATA,
                RATPOISON_EVENT_MAGIC as i64,
            );
            if let Some(clicks) = click_state {
                CGEventSetIntegerValueField(ev.as_ptr(), K_CG_MOUSE_EVENT_CLICK_STATE, clicks);
            }
            CGEventPost(K_CG_HID_EVENT_TAP, ev.as_ptr());
        }
        Ok(())
    }

    fn map_button(button: MouseButton) -> (u32, u32, u32) {
        match button {
            MouseButton::Left => (
                K_CG_MOUSE_BUTTON_LEFT,
                K_CG_EVENT_LEFT_MOUSE_DOWN,
                K_CG_EVENT_LEFT_MOUSE_UP,
            ),
            MouseButton::Right => (
                K_CG_MOUSE_BUTTON_RIGHT,
                K_CG_EVENT_RIGHT_MOUSE_DOWN,
                K_CG_EVENT_RIGHT_MOUSE_UP,
            ),
            MouseButton::Middle => (
                K_CG_MOUSE_BUTTON_CENTER,
                K_CG_EVENT_OTHER_MOUSE_DOWN,
                K_CG_EVENT_OTHER_MOUSE_UP,
            ),
            MouseButton::Back => (3, K_CG_EVENT_OTHER_MOUSE_DOWN, K_CG_EVENT_OTHER_MOUSE_UP),
            MouseButton::Forward => (4, K_CG_EVENT_OTHER_MOUSE_DOWN, K_CG_EVENT_OTHER_MOUSE_UP),
        }
    }
}

impl Default for MacOsMouseSimulator {
    fn default() -> Self {
        Self::new()
    }
}

impl MouseSimulatorBackend for MacOsMouseSimulator {
    fn backend_name(&self) -> &'static str {
        "macOS CGEventPost"
    }

    fn move_to(&mut self, point: LogicalPoint, _scale_factor: f64) -> Result<(), HidError> {
        let cg_pt = CGPoint {
            x: point.x,
            y: point.y,
        };
        // Warp cursor immediately and post a mouse-moved event so hover states update reliably.
        // SAFETY: `CGWarpMouseCursorPosition` takes logical points directly.
        let rc = unsafe { CGWarpMouseCursorPosition(cg_pt) };
        if rc != 0 {
            return Err(HidError::SimulationFailed(format!(
                "CGWarpMouseCursorPosition failed with CGError {rc}"
            )));
        }
        Self::post_mouse_event(K_CG_EVENT_MOUSE_MOVED, point, K_CG_MOUSE_BUTTON_LEFT, None)
    }

    fn move_relative(&mut self, dx: f64, dy: f64) -> Result<(), HidError> {
        let current = self.cursor_position()?;
        self.move_to(LogicalPoint::new(current.x + dx, current.y + dy), 1.0)
    }

    fn mouse_down(
        &mut self,
        button: MouseButton,
        point: LogicalPoint,
        _scale_factor: f64,
    ) -> Result<(), HidError> {
        let (cg_btn, down_type, _) = Self::map_button(button);
        Self::post_mouse_event(down_type, point, cg_btn, Some(1))
    }

    fn mouse_up(
        &mut self,
        button: MouseButton,
        point: LogicalPoint,
        _scale_factor: f64,
    ) -> Result<(), HidError> {
        let (cg_btn, _, up_type) = Self::map_button(button);
        Self::post_mouse_event(up_type, point, cg_btn, Some(1))
    }

    fn perform_click(
        &mut self,
        kind: ClickKind,
        point: LogicalPoint,
        scale_factor: f64,
    ) -> Result<(), HidError> {
        match kind {
            ClickKind::Left => {
                self.move_to(point, scale_factor)?;
                self.mouse_down(MouseButton::Left, point, scale_factor)?;
                self.mouse_up(MouseButton::Left, point, scale_factor)
            }
            ClickKind::Right => {
                self.move_to(point, scale_factor)?;
                self.mouse_down(MouseButton::Right, point, scale_factor)?;
                self.mouse_up(MouseButton::Right, point, scale_factor)
            }
            ClickKind::Middle => {
                self.move_to(point, scale_factor)?;
                self.mouse_down(MouseButton::Middle, point, scale_factor)?;
                self.mouse_up(MouseButton::Middle, point, scale_factor)
            }
            ClickKind::DoubleLeft => {
                self.move_to(point, scale_factor)?;
                Self::post_mouse_event(
                    K_CG_EVENT_LEFT_MOUSE_DOWN,
                    point,
                    K_CG_MOUSE_BUTTON_LEFT,
                    Some(1),
                )?;
                Self::post_mouse_event(
                    K_CG_EVENT_LEFT_MOUSE_UP,
                    point,
                    K_CG_MOUSE_BUTTON_LEFT,
                    Some(1),
                )?;
                Self::post_mouse_event(
                    K_CG_EVENT_LEFT_MOUSE_DOWN,
                    point,
                    K_CG_MOUSE_BUTTON_LEFT,
                    Some(2),
                )?;
                Self::post_mouse_event(
                    K_CG_EVENT_LEFT_MOUSE_UP,
                    point,
                    K_CG_MOUSE_BUTTON_LEFT,
                    Some(2),
                )
            }
            ClickKind::DragStart => {
                self.move_to(point, scale_factor)?;
                self.mouse_down(MouseButton::Left, point, scale_factor)
            }
            ClickKind::DragEnd => {
                self.move_to(point, scale_factor)?;
                self.mouse_up(MouseButton::Left, point, scale_factor)
            }
        }
    }

    fn scroll(&mut self, delta: ScrollDelta) -> Result<(), HidError> {
        let wheel_y = delta.dy.round() as i32;
        let wheel_x = delta.dx.round() as i32;
        // SAFETY: Non-variadic `CGEventCreateScrollWheelEvent2` avoids arm64 variadic ABI issues.
        let ev_raw = unsafe {
            CGEventCreateScrollWheelEvent2(
                ptr::null(),
                K_CG_SCROLL_EVENT_UNIT_PIXEL,
                2,
                wheel_y,
                wheel_x,
                0,
            )
        };
        let ev = CfOwned::new(ev_raw).ok_or_else(|| {
            HidError::SimulationFailed("CGEventCreateScrollWheelEvent2 returned NULL".to_string())
        })?;
        // SAFETY: Valid non-null `CGEventRef`.
        unsafe {
            CGEventSetIntegerValueField(
                ev.as_ptr(),
                K_CG_EVENT_SOURCE_USER_DATA,
                RATPOISON_EVENT_MAGIC as i64,
            );
            CGEventPost(K_CG_HID_EVENT_TAP, ev.as_ptr());
        }
        Ok(())
    }

    fn cursor_position(&self) -> Result<LogicalPoint, HidError> {
        // SAFETY: `CGEventCreate(NULL)` creates a transient empty event carrying current cursor location.
        let ev_raw = unsafe { CGEventCreate(ptr::null()) };
        let ev = CfOwned::new(ev_raw).ok_or_else(|| {
            HidError::SimulationFailed("CGEventCreate returned NULL while reading cursor".to_string())
        })?;
        // SAFETY: `ev` is valid and released when `CfOwned` drops.
        let pt = unsafe { CGEventGetLocation(ev.as_ptr()) };
        Ok(LogicalPoint::new(pt.x, pt.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_permission_and_secure_input_queries_do_not_crash() {
        let hook = MacOsKeyboardHook::new();
        let report = hook.check_permissions();
        assert_eq!(report.platform, "macos");
        assert!(!hook.is_running());
    }

    #[test]
    fn macos_flag_and_keycode_mapping() {
        let mods = cg_flags_to_modifiers(
            K_CG_EVENT_FLAG_MASK_SHIFT | K_CG_EVENT_FLAG_MASK_COMMAND | K_CG_EVENT_FLAG_MASK_ALTERNATE,
        );
        assert!(mods.contains(Modifiers::SHIFT));
        assert!(mods.contains(Modifiers::META));
        assert!(mods.contains(Modifiers::ALT));
        assert!(!mods.contains(Modifiers::CONTROL));

        assert_eq!(macos_keycode_to_normalized(0x35), KeyCode::ESCAPE);
        assert_eq!(macos_keycode_to_normalized(0x31), KeyCode::SPACE);
        assert_eq!(macos_keycode_to_normalized(0x00), KeyCode::from_ascii_upper(b'A'));
        assert_eq!(macos_keycode_to_normalized(0x0F), KeyCode::from_ascii_upper(b'R'));
    }
}
