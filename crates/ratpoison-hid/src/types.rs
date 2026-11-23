//! Core HID, coordinate, and error types for `ratpoison-hid`.
//!
//! Geometry types follow the shared RatPoison swarm contract:
//! - Screen coordinates use `f64` logical points (`LogicalPoint`) paired with
//!   `scale_factor: f64` to convert cleanly to `PhysicalPoint { x: i32, y: i32 }`.
//!
//! Security note: raw keypress types (`RawKeyEvent`) intentionally redact keycodes
//! in `Debug` and do **not** implement `Display` or `serde::Serialize` so keystrokes
//! can never be accidentally logged, serialized, or persisted.

use core::fmt;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroize;

/// Magic event source marker attached to synthetic mouse/keyboard events emitted by
/// RatPoison so our own OS hook never re-intercepts synthetic output (loop prevention).
pub const RATPOISON_EVENT_MAGIC: u64 = 0x5241_5450_4F49_534E; // "RATPOISN"

/// Logical screen coordinate in density-independent points (`f64`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct LogicalPoint {
    pub x: f64,
    pub y: f64,
}

impl LogicalPoint {
    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Converts this logical point into physical device pixels using `scale_factor`.
    /// Non-positive or non-finite scale factors safely fall back to `1.0`.
    #[inline]
    pub fn to_physical(self, scale_factor: f64) -> PhysicalPoint {
        let sf = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        PhysicalPoint {
            x: (self.x * sf).round() as i32,
            y: (self.y * sf).round() as i32,
        }
    }
}

/// Physical screen coordinate in device pixels (`i32`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct PhysicalPoint {
    pub x: i32,
    pub y: i32,
}

impl PhysicalPoint {
    #[inline]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Converts this physical pixel coordinate back into logical points.
    #[inline]
    pub fn to_logical(self, scale_factor: f64) -> LogicalPoint {
        let sf = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        LogicalPoint {
            x: f64::from(self.x) / sf,
            y: f64::from(self.y) / sf,
        }
    }
}

/// Logical screen rectangle (`f64`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// OS-normalized virtual keycode.
///
/// Stored as a compact `u16` (matching macOS `CGKeyCode`, Windows `VK_*`, and Linux
/// `KEY_*` evdev codes) and explicitly implements [`Zeroize`].
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Zeroize, Serialize, Deserialize)]
pub struct KeyCode(pub u16);

impl KeyCode {
    pub const NONE: Self = Self(0);
    pub const ESCAPE: Self = Self(0x001B);
    pub const RETURN: Self = Self(0x000D);
    pub const SPACE: Self = Self(0x0020);
    pub const TAB: Self = Self(0x0009);
    pub const BACKSPACE: Self = Self(0x0008);

    // Modifier virtual keycodes (platform-normalized)
    pub const SHIFT_L: Self = Self(0x0100);
    pub const SHIFT_R: Self = Self(0x0101);
    pub const CTRL_L: Self = Self(0x0102);
    pub const CTRL_R: Self = Self(0x0103);
    pub const ALT_L: Self = Self(0x0104);
    pub const ALT_R: Self = Self(0x0105);
    pub const META_L: Self = Self(0x0106);
    pub const META_R: Self = Self(0x0107);

    // Arrow keys (platform-normalized)
    pub const ARROW_LEFT: Self = Self(0x0200);
    pub const ARROW_RIGHT: Self = Self(0x0201);
    pub const ARROW_UP: Self = Self(0x0202);
    pub const ARROW_DOWN: Self = Self(0x0203);

    // Common punctuation / symbol keys (ASCII code points)
    pub const COMMA: Self = Self(b',' as u16);
    pub const PERIOD: Self = Self(b'.' as u16);
    pub const SLASH: Self = Self(b'/' as u16);
    pub const SEMICOLON: Self = Self(b';' as u16);
    pub const APOSTROPHE: Self = Self(b'\'' as u16);
    pub const BRACKET_LEFT: Self = Self(b'[' as u16);
    pub const BRACKET_RIGHT: Self = Self(b']' as u16);
    pub const MINUS: Self = Self(b'-' as u16);
    pub const EQUAL: Self = Self(b'=' as u16);
    pub const GRAVE: Self = Self(b'`' as u16);
    pub const BACKSLASH: Self = Self(b'\\' as u16);

    // Common ASCII letter/digit helpers (uppercase code points for layout-normalized bindings)
    pub const fn from_ascii_upper(ch: u8) -> Self {
        Self(ch as u16)
    }

    pub const fn from_ascii_digit(ch: u8) -> Self {
        Self(ch as u16)
    }

    /// Converts an ASCII letter/digit/punctuation keycode into its printable character.
    pub const fn to_ascii_char(self, shift: bool) -> Option<char> {
        match self.0 {
            0x41..=0x5A => {
                let b = self.0 as u8;
                if shift {
                    Some(b as char)
                } else {
                    Some((b + 32) as char)
                }
            }
            0x30..=0x39 => Some((self.0 as u8) as char),
            _ => None,
        }
    }

    /// Returns the raw numeric keycode value (for in-memory matching only; never log).
    #[inline]
    pub const fn raw(self) -> u16 {
        self.0
    }

    /// Returns the corresponding [`Modifiers`] bit if this keycode is a modifier key.
    pub const fn as_modifier(self) -> Modifiers {
        match self.0 {
            0x0100 | 0x0101 => Modifiers::SHIFT,
            0x0102 | 0x0103 => Modifiers::CONTROL,
            0x0104 | 0x0105 => Modifiers::ALT,
            0x0106 | 0x0107 => Modifiers::META,
            _ => Modifiers::EMPTY,
        }
    }
}

impl fmt::Debug for KeyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KeyCode([REDACTED])")
    }
}

/// Bitmask of active keyboard modifier keys.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Zeroize, Serialize, Deserialize)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const EMPTY: Self = Self(0);
    pub const SHIFT: Self = Self(1 << 0);
    pub const CONTROL: Self = Self(1 << 1);
    pub const ALT: Self = Self(1 << 2);
    pub const META: Self = Self(1 << 3);
    pub const CAPS_LOCK: Self = Self(1 << 4);
    pub const FN: Self = Self(1 << 5);

    #[inline]
    pub const fn from_bits_truncate(bits: u8) -> Self {
        Self(bits & 0b0011_1111)
    }

    #[inline]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[inline]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    #[inline]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    #[inline]
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    #[inline]
    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

impl fmt::Debug for Modifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Modifiers([REDACTED])")
    }
}

/// Lifecycle phase of a keyboard event.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum KeyPhase {
    #[default]
    Down = 1,
    Repeat = 2,
    Up = 3,
}

impl Zeroize for KeyPhase {
    fn zeroize(&mut self) {
        let ptr = self as *mut KeyPhase as *mut u8;
        // SAFETY: `KeyPhase` is `#[repr(u8)]` and `KeyPhase::Down = 1` is a valid discriminant.
        unsafe {
            core::ptr::write_volatile(ptr, KeyPhase::Down as u8);
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}

/// Raw keyboard event delivered from an OS hook backend.
///
/// Security guarantees:
/// - Does **not** implement `Display`, `Serialize`, or `Deserialize`.
/// - `Debug` redacts `code` and `modifiers` as `[REDACTED]`.
/// - Implements [`Zeroize`] so transient events can be wiped immediately after processing.
#[derive(Clone, Copy, PartialEq, Eq, Default, Zeroize)]
pub struct RawKeyEvent {
    pub code: KeyCode,
    pub modifiers: Modifiers,
    pub phase: KeyPhase,
    /// Monotonic timestamp in milliseconds (relative to an arbitrary monotonic epoch).
    pub timestamp_ms: u64,
    /// True if the event was injected programmatically (e.g. by RatPoison or another tool).
    pub is_injected: bool,
}

impl RawKeyEvent {
    #[inline]
    pub const fn new(
        code: KeyCode,
        modifiers: Modifiers,
        phase: KeyPhase,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            code,
            modifiers,
            phase,
            timestamp_ms,
            is_injected: false,
        }
    }

    #[inline]
    pub const fn injected(
        code: KeyCode,
        modifiers: Modifiers,
        phase: KeyPhase,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            code,
            modifiers,
            phase,
            timestamp_ms,
            is_injected: true,
        }
    }
}

impl fmt::Debug for RawKeyEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawKeyEvent")
            .field("code", &"[REDACTED]")
            .field("modifiers", &"[REDACTED]")
            .field("phase", &self.phase)
            .field("timestamp_ms", &self.timestamp_ms)
            .field("is_injected", &self.is_injected)
            .finish()
    }
}

/// Physical mouse buttons supported across platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

/// Semantic click action performed by RatPoison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ClickKind {
    #[default]
    Left,
    Right,
    DoubleLeft,
    Middle,
    DragStart,
    DragEnd,
}

impl ClickKind {
    /// Default cycle order matching Mouseless ergonomics: Left → Right → Double → Middle.
    pub const DEFAULT_CYCLE: [Self; 4] = [
        Self::Left,
        Self::Right,
        Self::DoubleLeft,
        Self::Middle,
    ];
}

/// Cursor movement speed tier for Free Mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SpeedTier {
    Slow,
    #[default]
    Normal,
    Fast,
    Turbo,
}

impl SpeedTier {
    /// Default cycle order for Free Mode speed cycling.
    pub const DEFAULT_CYCLE: [Self; 4] = [
        Self::Normal,
        Self::Fast,
        Self::Turbo,
        Self::Slow,
    ];

    /// Suggested velocity multiplier relative to base Free Mode speed.
    #[inline]
    pub const fn multiplier(self) -> f64 {
        match self {
            Self::Slow => 0.35,
            Self::Normal => 1.0,
            Self::Fast => 2.25,
            Self::Turbo => 4.0,
        }
    }
}

/// Scroll delta in logical scroll units (positive `dy` scrolls up, positive `dx` scrolls right).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct ScrollDelta {
    pub dx: f64,
    pub dy: f64,
}

impl ScrollDelta {
    #[inline]
    pub const fn new(dx: f64, dy: f64) -> Self {
        Self { dx, dy }
    }
}

/// Diagnostic report describing OS HID permissions and runtime security constraints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionReport {
    pub platform: &'static str,
    pub accessibility_granted: bool,
    pub input_monitoring_granted: bool,
    pub secure_input_active: bool,
    pub elevated_target_caveat: bool,
    pub uinput_available: bool,
    pub xtest_available: bool,
    pub guidance: Option<String>,
}

/// Errors produced by HID hooks, mouse simulation, and memory-hygiene hardening.
#[derive(Debug, Error)]
pub enum HidError {
    #[error("missing OS permission '{permission}': {detail}")]
    PermissionDenied {
        permission: &'static str,
        detail: String,
    },

    #[error("OS secure keyboard input is currently active; global hook paused for privacy")]
    SecureInputActive,

    #[error("foreground window runs at a higher privilege level (UIPI blocked synthetic input)")]
    ElevatedWindowBlocked,

    #[error("failed to install keyboard hook: {0}")]
    HookInstallFailed(String),

    #[error("keyboard hook is not currently running")]
    HookNotRunning,

    #[error("failed to simulate mouse action: {0}")]
    SimulationFailed(String),

    #[error("required input/output device unavailable: {0}")]
    DeviceUnavailable(String),

    #[error("OS memory lock failed with errno {errno}")]
    MemoryLockFailed { errno: i32 },

    #[error("operation unsupported on platform: {0}")]
    UnsupportedPlatform(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_and_physical_point_conversions_handle_scale_factors() {
        let lp = LogicalPoint::new(100.5, 200.25);
        assert_eq!(lp.to_physical(2.0), PhysicalPoint::new(201, 401));
        assert_eq!(PhysicalPoint::new(200, 400).to_logical(2.0), LogicalPoint::new(100.0, 200.0));

        // Guard against invalid scale factors (0, negative, NaN)
        assert_eq!(lp.to_physical(0.0), PhysicalPoint::new(101, 200));
        assert_eq!(lp.to_physical(-2.0), PhysicalPoint::new(101, 200));
        assert_eq!(lp.to_physical(f64::NAN), PhysicalPoint::new(101, 200));
    }

    #[test]
    fn raw_key_event_debug_redacts_keycode_and_modifiers() {
        let ev = RawKeyEvent::new(KeyCode(4321), Modifiers::CONTROL, KeyPhase::Down, 999);
        let dbg = format!("{ev:?}");
        assert!(dbg.contains("[REDACTED]"));
        assert!(!dbg.contains("4321"));
    }
}
