//! Pure, deterministic, zero-unsafe core engine for RatPoison.
//!
//! Provides:
//! - [`geometry`]: Multi-monitor mixed-DPI coordinate mapping (`logical_to_physical`, `physical_to_logical`,
//!   virtual desktop clamping, and directional monitor switching).
//! - [`grid`]: Configurable NxM grid & nested subgrid subdivision, custom/case-sensitive label alphabets
//!   (> 676 cells), prefix matching, and coarse/normal/fine nudge math.
//! - [`presets`]: Quick Select + Free Mode preset workflow, velocity/acceleration profiles, and
//!   per-app / per-monitor preset overrides.
//! - [`config`]: `RatPoisonConfig` with lossless unknown-field preservation (`#[serde(flatten)]`) and
//!   single-key `restore_default(key)` support.
//! - [`state_machine`]: Pure deterministic mode state machine (`Normal -> Grid -> Subgrid -> Nudge`,
//!   `Normal <-> FreeMode`, `Normal <-> LabelMode`) emitting explicit [`EngineEffect`]s and
//!   serializable [`OverlaySnapshot`] payloads for `ratpoison://overlay-state`.

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod geometry;
pub mod grid;
pub mod presets;
pub mod state_machine;

pub use config::{
    HotkeyConfig, LabelModeConfig, OverlayConfig, RatPoisonConfig, KNOWN_SETTING_KEYS,
};
pub use error::CoreError;
pub use geometry::{
    clamp_logical_to_monitor, logical_to_physical, logical_to_physical_clamped,
    physical_to_logical, Direction, LogicalPoint, LogicalRect, MonitorInfo, MonitorLayout,
    Orientation, PhysicalPoint, PhysicalRect,
};
pub use grid::{
    default_subgrid_keys, generate_grid, generate_grid_in_rect, generate_subgrid,
    generate_subgrid_in_rect, nudge_by_vector, nudge_in_direction, Grid, GridCell, GridConfig,
    LabelOrdering, LabelScheme, NudgeClampMode, NudgeConfig, NudgeStep, PrefixMatch,
};
pub use presets::{
    default_presets, resolve_preset, FreeModeProfile, MouseButton, PresetOverrideScope,
    QuickSelectAction, SpeedModifier, SpeedTier, WorkflowPreset,
};
pub use state_machine::{
    ActiveModifiers, CoreEngine, EngineEffect, EngineEvent, EngineMode, FreeKey, LabelBadge,
    MonitorSwitch, MouseActionKind, OverlaySnapshot, StepOutput,
};
