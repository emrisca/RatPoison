//! Cross-platform low-level keyboard interception, secure mouse simulation, and zeroized memory hygiene.
//!
//! # Modules
//! - [`memory_hygiene`]: Compile-time bounded (`3 <= N <= 5`) [`SecureKeyBuffer`], OS page-locked
//!   [`LockedSecureKeyBuffer`], volatile zeroization, and anti-core-dump hardening.
//! - [`interceptor`]: Zero-leak Hold-For and Cycle [`CommandInterceptor`] state machine with
//!   balanced modifier tracking and automatic buffer purging.
//! - [`platform`]: Safe OS-specific keyboard hook and mouse simulation backends (`macos`,
//!   `windows`, `linux`) plus deterministic [`MockHidBackend`].
//! - [`types`]: Shared logical/physical coordinate types, normalized key/modifier primitives,
//!   and [`HidError`].

pub mod interceptor;
pub mod memory_hygiene;
pub mod platform;
pub mod types;

pub use interceptor::{
    ActivationChord, CommandInterceptor, CycleBinding, CycleTarget, HoldForBinding, HoldForTarget,
    InterceptDecision, InterceptorAction, InterceptorConfig, InterceptorMode, RedactedKey,
};
pub use memory_hygiene::{
    disable_core_dumps, harden_process_memory, LockedSecureKeyBuffer, MemoryLockStatus,
    ProcessHardeningReport, PushOutcome, SecureKeyBuffer, SecureKeyEntry,
    DEFAULT_BUFFER_CAPACITY, DEFAULT_IDLE_PURGE_MS, MAX_BUFFER_CAPACITY, MIN_BUFFER_CAPACITY,
};
pub use platform::{
    check_platform_permissions, create_platform_keyboard_hook, create_platform_mouse_simulator,
    KeyEventCallback, KeyboardHookBackend, MockHidBackend, MockMouseOp, MouseSimulatorBackend,
};
pub use types::{
    ClickKind, HidError, KeyCode, KeyPhase, LogicalPoint, LogicalRect, Modifiers, MouseButton,
    PermissionReport, PhysicalPoint, RawKeyEvent, ScrollDelta, SpeedTier, RATPOISON_EVENT_MAGIC,
};
