//! OS-specific keyboard interception and mouse simulation backends.
//!
//! Each supported OS exposes safe Rust wrappers implementing [`KeyboardHookBackend`]
//! and [`MouseSimulatorBackend`], alongside a deterministic [`MockHidBackend`] for
//! headless unit and integration testing.

pub mod mock;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "linux")]
pub mod linux;

pub use mock::{MockHidBackend, MockMouseOp};

use crate::interceptor::InterceptDecision;
use crate::types::{
    ClickKind, HidError, LogicalPoint, MouseButton, PermissionReport, PhysicalPoint, RawKeyEvent,
    ScrollDelta,
};

/// Callback invoked synchronously on the OS hook thread for every raw keyboard event.
///
/// Implementations MUST execute in microseconds (well below Windows `LowLevelHooksTimeout`
/// and macOS `CGEventTap` watchdog timeouts) and must never perform blocking I/O or logging.
pub type KeyEventCallback = Box<dyn FnMut(RawKeyEvent) -> InterceptDecision + Send + 'static>;

/// Platform-agnostic global keyboard interception backend.
pub trait KeyboardHookBackend: Send {
    /// Human-readable backend identifier (e.g. `"macOS CGEventTap"`).
    fn backend_name(&self) -> &'static str;

    /// Inspects OS-level permissions and runtime security caveats without prompting.
    fn check_permissions(&self) -> PermissionReport;

    /// Starts intercepting global keyboard events, invoking `callback` for each event.
    fn start(&mut self, callback: KeyEventCallback) -> Result<(), HidError>;

    /// Stops intercepting global keyboard events and releases OS hook handles cleanly.
    fn stop(&mut self) -> Result<(), HidError>;

    /// Returns `true` if the keyboard hook is currently active.
    fn is_running(&self) -> bool;

    /// Returns `true` if the OS reports Secure Keyboard Input is currently active
    /// (e.g. macOS password fields / Terminal Secure Keyboard Entry).
    fn is_secure_input_active(&self) -> bool {
        false
    }
}

/// Platform-agnostic synthetic mouse movement, clicking, dragging, and scrolling backend.
pub trait MouseSimulatorBackend: Send {
    /// Human-readable backend identifier (e.g. `"macOS CGEventPost"`).
    fn backend_name(&self) -> &'static str;

    /// Warps/moves the cursor to an absolute logical coordinate.
    fn move_to(&mut self, point: LogicalPoint, scale_factor: f64) -> Result<(), HidError>;

    /// Moves the cursor by a relative logical delta (used in Free Mode).
    fn move_relative(&mut self, dx: f64, dy: f64) -> Result<(), HidError>;

    /// Presses `button` down at `point`.
    fn mouse_down(
        &mut self,
        button: MouseButton,
        point: LogicalPoint,
        scale_factor: f64,
    ) -> Result<(), HidError>;

    /// Releases `button` at `point`.
    fn mouse_up(
        &mut self,
        button: MouseButton,
        point: LogicalPoint,
        scale_factor: f64,
    ) -> Result<(), HidError>;

    /// Executes a high-level [`ClickKind`] (single, right, double, middle, drag start/end).
    fn perform_click(
        &mut self,
        kind: ClickKind,
        point: LogicalPoint,
        scale_factor: f64,
    ) -> Result<(), HidError>;

    /// Emits smooth or notched scroll wheel deltas.
    fn scroll(&mut self, delta: ScrollDelta) -> Result<(), HidError>;

    /// Queries the current cursor position in logical coordinates.
    fn cursor_position(&self) -> Result<LogicalPoint, HidError>;
}

/// Converts a physical pixel coordinate into normalized `0..=65535` virtual-desktop space
/// required by Windows `SendInput(MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK)`.
///
/// Kept platform-independent so multi-monitor negative-offset normalization can be
/// unit-tested on every OS.
pub fn normalize_virtual_desktop_coords(
    point: PhysicalPoint,
    virtual_origin_x: i32,
    virtual_origin_y: i32,
    virtual_width: i32,
    virtual_height: i32,
) -> (i32, i32) {
    let w = (virtual_width - 1).max(1) as f64;
    let h = (virtual_height - 1).max(1) as f64;
    let rel_x = (point.x - virtual_origin_x) as f64;
    let rel_y = (point.y - virtual_origin_y) as f64;

    let norm_x = ((rel_x * 65535.0) / w).round().clamp(0.0, 65535.0) as i32;
    let norm_y = ((rel_y * 65535.0) / h).round().clamp(0.0, 65535.0) as i32;
    (norm_x, norm_y)
}

/// Creates the native [`KeyboardHookBackend`] for the current operating system.
pub fn create_platform_keyboard_hook() -> Box<dyn KeyboardHookBackend> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacOsKeyboardHook::new())
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsKeyboardHook::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxKeyboardHook::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Box::new(MockHidBackend::new())
    }
}

/// Creates the native [`MouseSimulatorBackend`] for the current operating system.
pub fn create_platform_mouse_simulator() -> Box<dyn MouseSimulatorBackend> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacOsMouseSimulator::new())
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsMouseSimulator::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxMouseSimulator::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Box::new(MockHidBackend::new())
    }
}

/// Queries the native OS permission and security report for the current platform.
pub fn check_platform_permissions() -> PermissionReport {
    create_platform_keyboard_hook().check_permissions()
}

/// Prompts the OS for required global input/accessibility permissions (when supported)
/// and returns the refreshed [`PermissionReport`].
pub fn request_platform_permissions() -> PermissionReport {
    #[cfg(target_os = "macos")]
    {
        let _ = macos::request_accessibility_and_input_monitoring();
    }
    check_platform_permissions()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_multi_monitor_virtual_desktop_with_negative_origin() {
        // Dual 1920x1080 monitors where left monitor starts at x = -1920, total width = 3840
        let (x0, y0) = normalize_virtual_desktop_coords(PhysicalPoint::new(-1920, 0), -1920, 0, 3840, 1080);
        assert_eq!((x0, y0), (0, 0));

        let (x_max, y_max) =
            normalize_virtual_desktop_coords(PhysicalPoint::new(1919, 1079), -1920, 0, 3840, 1080);
        assert_eq!((x_max, y_max), (65535, 65535));

        // Center seam (x = 0, y = 540)
        let (xm, ym) = normalize_virtual_desktop_coords(PhysicalPoint::new(0, 540), -1920, 0, 3840, 1080);
        assert!((32760..=32785).contains(&xm));
        assert!((32760..=32820).contains(&ym));
    }

    #[test]
    fn platform_factory_returns_permission_report_cleanly() {
        let report = check_platform_permissions();
        assert!(!report.platform.is_empty());
    }
}
