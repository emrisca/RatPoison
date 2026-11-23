//! Pure, deterministic mode state machine for RatPoison.
//!
//! Handles:
//! - `Normal -> Grid -> Subgrid -> Nudge` (prefix matching, backspace/undo, monitor switching,
//!   clicks, double-click, right/middle-click, drag-start/drag-end)
//! - `Normal <-> FreeMode` (overlay-free relative movement via `I,J,K,L`, scrolling, and
//!   speed modifiers `A,S,D,F` with velocity/acceleration curves)
//! - `Normal <-> LabelMode` (accessibility badge targets, prefix filtering, auto-click on unique match)
//! - Quick Select + Free Mode preset handoff (single jump or grid+subgrid jump -> immediate `FreeMode`)
//!
//! Zero OS side effects: every transition returns a [`StepOutput`] containing explicit
//! [`EngineEffect`] descriptors and a serializable [`OverlaySnapshot`] matching
//! `ratpoison://overlay-state`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::config::RatPoisonConfig;
use crate::geometry::{
    clamp_logical_to_monitor, logical_to_physical_clamped, Direction, LogicalPoint, LogicalRect,
    MonitorInfo, MonitorLayout, PhysicalPoint,
};
use crate::grid::{
    generate_grid, generate_subgrid, normalize_str, nudge_in_direction, Grid, GridCell, NudgeStep,
    PrefixMatch,
};
use crate::presets::{
    resolve_preset, FreeModeProfile, MouseButton, QuickSelectAction, SpeedModifier, WorkflowPreset,
};

/// Active operating mode of the RatPoison core engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineMode {
    #[default]
    Normal,
    Grid,
    Subgrid,
    Nudge,
    FreeMode,
    LabelMode,
}

/// Target accessibility element rendered as a clickable badge in `LabelMode`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelBadge {
    pub id: String,
    pub label: String,
    pub title: String,
    pub role: String,
    pub rect: LogicalRect,
    pub center: LogicalPoint,
    pub monitor_id: String,
}

/// Monitor switching instruction during Grid, Subgrid, or Nudge modes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MonitorSwitch {
    Next,
    Prev,
    ById { id: String },
    ByIndex { index: usize },
    Direction { direction: Direction },
}

/// Mouse action triggered from Grid, Subgrid, Nudge, or FreeMode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseActionKind {
    LeftClick,
    DoubleClick,
    RightClick,
    MiddleClick,
    DragStart,
    DragEnd,
    MoveOnly,
}

/// Logical key inputs for overlay-free `FreeMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreeKey {
    /// `I` — Move cursor up.
    MoveUp,
    /// `K` — Move cursor down.
    MoveDown,
    /// `J` — Move cursor left.
    MoveLeft,
    /// `L` — Move cursor right.
    MoveRight,
    /// Scroll wheel up.
    ScrollUp,
    /// Scroll wheel down.
    ScrollDown,
    /// Scroll wheel left.
    ScrollLeft,
    /// Scroll wheel right.
    ScrollRight,
    /// `A` — Precision / slow speed modifier.
    SpeedSlow,
    /// `S` — Normal speed modifier.
    SpeedNormal,
    /// `D` — Fast speed modifier.
    SpeedFast,
    /// `F` — Turbo speed modifier.
    SpeedTurbo,
}

impl FreeKey {
    /// Map default Mouseless-compatible QWERTY Free Mode keys (`I,J,K,L` + `A,S,D,F`) to `FreeKey`.
    pub fn from_qwerty_char(ch: char) -> Option<Self> {
        match ch.to_ascii_lowercase() {
            'i' => Some(Self::MoveUp),
            'k' => Some(Self::MoveDown),
            'j' => Some(Self::MoveLeft),
            'l' => Some(Self::MoveRight),
            'a' => Some(Self::SpeedSlow),
            's' => Some(Self::SpeedNormal),
            'd' => Some(Self::SpeedFast),
            'f' => Some(Self::SpeedTurbo),
            _ => None,
        }
    }

    pub const fn as_speed_modifier(self) -> Option<SpeedModifier> {
        match self {
            Self::SpeedSlow => Some(SpeedModifier::Slow),
            Self::SpeedNormal => Some(SpeedModifier::Normal),
            Self::SpeedFast => Some(SpeedModifier::Fast),
            Self::SpeedTurbo => Some(SpeedModifier::Turbo),
            _ => None,
        }
    }
}

/// Active keyboard/modifier indicators surfaced in [`OverlaySnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ActiveModifiers {
    pub speed: SpeedModifier,
    pub speed_keys_held: Vec<SpeedModifier>,
    pub nudge_step: NudgeStep,
    pub dragging: bool,
}

/// Serializable overlay state payload matching the `ratpoison://overlay-state` Tauri event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlaySnapshot {
    pub mode: EngineMode,
    pub visible: bool,
    pub active_monitor_id: Option<String>,
    pub active_monitor: Option<MonitorInfo>,
    pub grid_cells: Vec<GridCell>,
    pub subgrid_rect: Option<LogicalRect>,
    pub subgrid_cells: Vec<GridCell>,
    pub label_badges: Vec<LabelBadge>,
    pub typed_prefix: String,
    pub cursor_preview: Option<LogicalPoint>,
    pub active_modifiers: ActiveModifiers,
    pub active_preset_id: Option<String>,
}

impl OverlaySnapshot {
    /// Actual `(rows, cols)` represented by `grid_cells`, or `(0, 0)` when empty.
    pub fn grid_dimensions(&self) -> (u16, u16) {
        let rows = self
            .grid_cells
            .iter()
            .map(|c| c.row.saturating_add(1))
            .max()
            .unwrap_or(0);
        let cols = self
            .grid_cells
            .iter()
            .map(|c| c.col.saturating_add(1))
            .max()
            .unwrap_or(0);
        (rows, cols)
    }

    /// Actual `(rows, cols)` represented by `subgrid_cells`, or `(0, 0)` when empty.
    pub fn subgrid_dimensions(&self) -> (u16, u16) {
        let rows = self
            .subgrid_cells
            .iter()
            .map(|c| c.row.saturating_add(1))
            .max()
            .unwrap_or(0);
        let cols = self
            .subgrid_cells
            .iter()
            .map(|c| c.col.saturating_add(1))
            .max()
            .unwrap_or(0);
        (rows, cols)
    }
}

/// Explicit OS/UI side-effect descriptors emitted by [`CoreEngine::handle_event`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEffect {
    MoveCursor {
        logical: LogicalPoint,
        physical: PhysicalPoint,
        monitor_id: String,
    },
    Click {
        button: MouseButton,
        count: u8,
        logical: LogicalPoint,
        physical: PhysicalPoint,
        monitor_id: String,
    },
    MouseDown {
        button: MouseButton,
        logical: LogicalPoint,
        physical: PhysicalPoint,
        monitor_id: String,
    },
    MouseUp {
        button: MouseButton,
        logical: LogicalPoint,
        physical: PhysicalPoint,
        monitor_id: String,
    },
    Scroll {
        delta_x: f64,
        delta_y: f64,
    },
    ShowOverlay {
        monitor_id: String,
    },
    HideOverlay,
    QueryAccessibility {
        monitor_id: String,
    },
    ClearKeyBuffer,
    EmitOverlaySnapshot {
        snapshot: Box<OverlaySnapshot>,
    },
}

/// Input events processed deterministically by [`CoreEngine`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    /// Enter `Grid` mode (optionally targeting a specific monitor and/or explicit preset such as `"quick_select_free"`).
    ActivateGrid {
        monitor_id: Option<String>,
        preset_id: Option<String>,
    },
    /// Toggle `Normal <-> FreeMode` (or activate `FreeMode` with an explicit preset).
    ActivateFreeMode { preset_id: Option<String> },
    /// Toggle `Normal <-> LabelMode` and query OS accessibility badges.
    ActivateLabelMode,
    /// Cancel active mode (Escape), releasing any held drag and hiding the overlay.
    ExitToNormal,
    /// Typed character in `Grid`, `Subgrid`, or `LabelMode`.
    KeyChar { ch: char },
    /// Backspace / undo step: removes last typed char, or steps back `Nudge -> Subgrid -> Grid -> Normal`.
    Backspace,
    /// Switch active monitor in `Grid`, `Subgrid`, or `Nudge`.
    SwitchMonitor { switch: MonitorSwitch },
    /// Nudge cursor by coarse/normal/fine step in `Nudge` (or `Subgrid`/`Grid`).
    Nudge {
        direction: Direction,
        step: NudgeStep,
    },
    /// Perform click, double-click, right/middle-click, drag start/end, or move-only exit.
    MouseAction { action: MouseActionKind },
    /// Press a directional, scroll, or speed modifier key in `FreeMode`.
    FreeModeKeyDown { key: FreeKey },
    /// Release a directional, scroll, or speed modifier key in `FreeMode`.
    FreeModeKeyUp { key: FreeKey },
    /// Advance `FreeMode` cursor/scroll physics deterministically by `dt_ms` milliseconds.
    FreeModeTick { dt_ms: u32 },
    /// Deliver discovered accessibility targets from `ratpoison-a11y` into `LabelMode`.
    AccessibilityLabelsLoaded { badges: Vec<LabelBadge> },
    /// Update external cursor position observed by the OS/HID layer while in `Normal` mode.
    SyncCursor { logical: LogicalPoint },
    /// Hot-plug or DPI layout update.
    MonitorsUpdated { layout: MonitorLayout },
    /// Live configuration reload.
    ConfigUpdated { config: Box<RatPoisonConfig> },
    /// Notify engine of focused application or active monitor context for preset override matching.
    ActiveContextChanged {
        app_id: Option<String>,
        monitor_id: Option<String>,
    },
}

/// Output produced by one deterministic step of [`CoreEngine::handle_event`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepOutput {
    pub effects: Vec<EngineEffect>,
    pub snapshot: OverlaySnapshot,
}

#[derive(Debug, Clone, Default)]
struct FreeModeRuntime {
    held_keys: BTreeSet<FreeKey>,
    speed_stack: Vec<SpeedModifier>,
    move_hold_ms: u32,
}

impl FreeModeRuntime {
    fn reset(&mut self) {
        self.held_keys.clear();
        self.speed_stack.clear();
        self.move_hold_ms = 0;
    }

    fn key_down(&mut self, key: FreeKey) {
        self.held_keys.insert(key);
        if let Some(speed) = key.as_speed_modifier() {
            self.speed_stack.retain(|&s| s != speed);
            self.speed_stack.push(speed);
        }
    }

    fn key_up(&mut self, key: FreeKey) {
        self.held_keys.remove(&key);
        if let Some(speed) = key.as_speed_modifier() {
            self.speed_stack.retain(|&s| s != speed);
        }
    }

    fn active_speed(&self) -> SpeedModifier {
        self.speed_stack
            .last()
            .copied()
            .unwrap_or(SpeedModifier::Normal)
    }

    fn move_vector(&self) -> (f64, f64) {
        let mut dx = 0.0;
        let mut dy = 0.0;
        if self.held_keys.contains(&FreeKey::MoveLeft) {
            dx -= 1.0;
        }
        if self.held_keys.contains(&FreeKey::MoveRight) {
            dx += 1.0;
        }
        if self.held_keys.contains(&FreeKey::MoveUp) {
            dy -= 1.0;
        }
        if self.held_keys.contains(&FreeKey::MoveDown) {
            dy += 1.0;
        }
        (dx, dy)
    }

    fn scroll_vector(&self) -> (f64, f64) {
        let mut sx = 0.0;
        let mut sy = 0.0;
        if self.held_keys.contains(&FreeKey::ScrollLeft) {
            sx -= 1.0;
        }
        if self.held_keys.contains(&FreeKey::ScrollRight) {
            sx += 1.0;
        }
        if self.held_keys.contains(&FreeKey::ScrollUp) {
            sy += 1.0;
        }
        if self.held_keys.contains(&FreeKey::ScrollDown) {
            sy -= 1.0;
        }
        (sx, sy)
    }
}

/// Pure, deterministic RatPoison core state machine.
#[derive(Debug, Clone)]
pub struct CoreEngine {
    config: RatPoisonConfig,
    layout: MonitorLayout,
    mode: EngineMode,
    active_monitor_id: Option<String>,
    active_app_id: Option<String>,
    active_preset: Option<WorkflowPreset>,
    explicit_preset_id: Option<String>,
    primary_grid: Option<Grid>,
    subgrid: Option<Grid>,
    label_badges: Vec<LabelBadge>,
    typed_prefix: String,
    cursor_logical: LogicalPoint,
    last_nudge_step: NudgeStep,
    dragging: bool,
    free_runtime: FreeModeRuntime,
}

impl CoreEngine {
    /// Create a new core engine initialized to `Normal` mode centered on the primary monitor.
    pub fn new(config: RatPoisonConfig, layout: MonitorLayout) -> Self {
        let primary = layout.primary();
        let active_monitor_id = primary.map(|m| m.id.clone());
        let cursor_logical = primary
            .map(MonitorInfo::logical_center)
            .unwrap_or(LogicalPoint::new(0.0, 0.0));

        Self {
            config,
            layout,
            mode: EngineMode::Normal,
            active_monitor_id,
            active_app_id: None,
            active_preset: None,
            explicit_preset_id: None,
            primary_grid: None,
            subgrid: None,
            label_badges: Vec::new(),
            typed_prefix: String::new(),
            cursor_logical,
            last_nudge_step: NudgeStep::Normal,
            dragging: false,
            free_runtime: FreeModeRuntime::default(),
        }
    }

    pub fn mode(&self) -> EngineMode {
        self.mode
    }

    pub fn config(&self) -> &RatPoisonConfig {
        &self.config
    }

    pub fn layout(&self) -> &MonitorLayout {
        &self.layout
    }

    pub fn cursor_logical(&self) -> LogicalPoint {
        self.cursor_logical
    }

    pub fn active_monitor_id(&self) -> Option<&str> {
        self.active_monitor_id.as_deref()
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Build the current serializable [`OverlaySnapshot`].
    pub fn snapshot(&self) -> OverlaySnapshot {
        let active_monitor = self.current_monitor().cloned();
        let visible = matches!(
            self.mode,
            EngineMode::Grid | EngineMode::Subgrid | EngineMode::Nudge | EngineMode::LabelMode
        );
        let grid_cells = match self.mode {
            EngineMode::Grid | EngineMode::Subgrid => self
                .primary_grid
                .as_ref()
                .map(|g| g.cells.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let (subgrid_rect, subgrid_cells) = match self.mode {
            EngineMode::Subgrid => self
                .subgrid
                .as_ref()
                .map(|g| (Some(g.bounds), g.cells.clone()))
                .unwrap_or((None, Vec::new())),
            _ => (None, Vec::new()),
        };
        let label_badges = if self.mode == EngineMode::LabelMode {
            self.filtered_label_badges()
        } else {
            Vec::new()
        };

        OverlaySnapshot {
            mode: self.mode,
            visible,
            active_monitor_id: active_monitor.as_ref().map(|m| m.id.clone()),
            active_monitor,
            grid_cells,
            subgrid_rect,
            subgrid_cells,
            label_badges,
            typed_prefix: self.typed_prefix.clone(),
            cursor_preview: if self.mode == EngineMode::Normal {
                None
            } else {
                Some(self.cursor_logical)
            },
            active_modifiers: ActiveModifiers {
                speed: self.free_runtime.active_speed(),
                speed_keys_held: self.free_runtime.speed_stack.clone(),
                nudge_step: self.last_nudge_step,
                dragging: self.dragging,
            },
            active_preset_id: self.active_preset.as_ref().map(|p| p.id.clone()),
        }
    }

    /// Process a single [`EngineEvent`] deterministically and return emitted [`EngineEffect`]s
    /// alongside the updated [`OverlaySnapshot`].
    pub fn handle_event(&mut self, event: EngineEvent) -> StepOutput {
        let mut effects = Vec::new();

        match event {
            EngineEvent::ActivateGrid {
                monitor_id,
                preset_id,
            } => {
                self.activate_grid(monitor_id.as_deref(), preset_id.as_deref(), &mut effects);
            }
            EngineEvent::ActivateFreeMode { preset_id } => {
                self.activate_free_mode(preset_id.as_deref(), &mut effects);
            }
            EngineEvent::ActivateLabelMode => {
                self.activate_label_mode(&mut effects);
            }
            EngineEvent::ExitToNormal => {
                self.exit_to_normal(true, &mut effects);
            }
            EngineEvent::KeyChar { ch } => {
                self.handle_key_char(ch, &mut effects);
            }
            EngineEvent::Backspace => {
                self.handle_backspace(&mut effects);
            }
            EngineEvent::SwitchMonitor { switch } => {
                self.handle_switch_monitor(switch, &mut effects);
            }
            EngineEvent::Nudge { direction, step } => {
                self.handle_nudge(direction, step, &mut effects);
            }
            EngineEvent::MouseAction { action } => {
                self.handle_mouse_action(action, &mut effects);
            }
            EngineEvent::FreeModeKeyDown { key } => {
                if self.mode == EngineMode::FreeMode {
                    self.free_runtime.key_down(key);
                }
            }
            EngineEvent::FreeModeKeyUp { key } => {
                if self.mode == EngineMode::FreeMode {
                    self.free_runtime.key_up(key);
                    let (dx, dy) = self.free_runtime.move_vector();
                    if dx == 0.0 && dy == 0.0 {
                        self.free_runtime.move_hold_ms = 0;
                    }
                }
            }
            EngineEvent::FreeModeTick { dt_ms } => {
                if self.mode == EngineMode::FreeMode && dt_ms > 0 {
                    self.step_free_mode(dt_ms, &mut effects);
                }
            }
            EngineEvent::AccessibilityLabelsLoaded { badges } => {
                if self.mode == EngineMode::LabelMode {
                    self.load_accessibility_badges(badges);
                }
            }
            EngineEvent::SyncCursor { logical } => {
                self.cursor_logical = logical;
                if let Some(m) = self.layout.monitor_at_logical(logical) {
                    self.active_monitor_id = Some(m.id.clone());
                }
            }
            EngineEvent::MonitorsUpdated { layout } => {
                self.layout = layout;
                if self
                    .active_monitor_id
                    .as_deref()
                    .and_then(|id| self.layout.by_id(id))
                    .is_none()
                {
                    self.active_monitor_id = self.layout.primary().map(|m| m.id.clone());
                }
                if let Some((mid, clamped)) = self
                    .layout
                    .clamp_logical_to_virtual_desktop(self.cursor_logical)
                {
                    self.cursor_logical = clamped;
                    if self.active_monitor_id.is_none() {
                        self.active_monitor_id = Some(mid);
                    }
                }
                if matches!(self.mode, EngineMode::Grid | EngineMode::Subgrid) {
                    let preset_id = self.explicit_preset_id.clone();
                    let target_mon = self.active_monitor_id.clone();
                    self.activate_grid(target_mon.as_deref(), preset_id.as_deref(), &mut effects);
                }
            }
            EngineEvent::ConfigUpdated { config } => {
                self.config = *config;
                if self.mode != EngineMode::Normal {
                    let monitor = self.current_monitor().cloned();
                    self.active_preset = resolve_preset(
                        &self.config.presets,
                        self.explicit_preset_id.as_deref(),
                        &self.config.active_preset_id,
                        self.active_app_id.as_deref(),
                        monitor.as_ref(),
                    )
                    .cloned();
                    if self.mode == EngineMode::Grid {
                        if let Some(mon) = monitor {
                            let grid_cfg = self.effective_grid_config().clone();
                            if let Ok(grid) = generate_grid(&mon, &grid_cfg)
                                .or_else(|_| generate_grid(&mon, &self.config.grid))
                                .or_else(|_| generate_grid(&mon, &crate::grid::GridConfig::default()))
                            {
                                let changed = self.primary_grid.as_ref() != Some(&grid);
                                self.primary_grid = Some(grid);
                                if changed {
                                    self.typed_prefix.clear();
                                }
                            }
                        }
                    }
                }
            }
            EngineEvent::ActiveContextChanged { app_id, monitor_id } => {
                self.active_app_id = app_id;
                if let Some(mid) = monitor_id {
                    if self.layout.by_id(&mid).is_some() {
                        self.active_monitor_id = Some(mid);
                    }
                }
            }
        }

        let snapshot = self.snapshot();
        effects.push(EngineEffect::EmitOverlaySnapshot {
            snapshot: Box::new(snapshot.clone()),
        });
        StepOutput { effects, snapshot }
    }

    fn current_monitor(&self) -> Option<&MonitorInfo> {
        self.active_monitor_id
            .as_deref()
            .and_then(|id| self.layout.by_id(id))
            .or_else(|| self.layout.monitor_at_logical(self.cursor_logical))
            .or_else(|| self.layout.primary())
    }

    fn active_override_preset(&self) -> Option<&WorkflowPreset> {
        self.active_preset
            .as_ref()
            .filter(|p| !p.is_unscoped_standard())
    }

    fn effective_free_profile(&self) -> &FreeModeProfile {
        self.active_override_preset()
            .map(|p| &p.free_mode)
            .unwrap_or(&self.config.free_mode)
    }

    fn effective_grid_config(&self) -> &crate::grid::GridConfig {
        self.active_override_preset()
            .map(|p| &p.grid)
            .unwrap_or(&self.config.grid)
    }

    fn effective_nudge_config(&self) -> &crate::grid::NudgeConfig {
        self.active_override_preset()
            .map(|p| &p.nudge)
            .unwrap_or(&self.config.nudge)
    }

    fn activate_grid(
        &mut self,
        requested_monitor_id: Option<&str>,
        explicit_preset_id: Option<&str>,
        effects: &mut Vec<EngineEffect>,
    ) {
        let monitor = requested_monitor_id
            .and_then(|id| self.layout.by_id(id))
            .or_else(|| self.current_monitor())
            .cloned();

        let Some(monitor) = monitor else {
            return;
        };

        let resolved = resolve_preset(
            &self.config.presets,
            explicit_preset_id,
            &self.config.active_preset_id,
            self.active_app_id.as_deref(),
            Some(&monitor),
        )
        .cloned();

        let grid_cfg = resolved
            .as_ref()
            .filter(|p| !p.is_unscoped_standard())
            .map(|p| &p.grid)
            .unwrap_or(&self.config.grid);

        let grid = generate_grid(&monitor, grid_cfg)
            .or_else(|_| generate_grid(&monitor, &self.config.grid))
            .or_else(|_| generate_grid(&monitor, &crate::grid::GridConfig::default()));

        let Ok(grid) = grid else {
            return;
        };

        self.active_monitor_id = Some(monitor.id.clone());
        self.active_preset = resolved;
        self.explicit_preset_id = explicit_preset_id.map(str::to_string);
        self.primary_grid = Some(grid);
        self.subgrid = None;
        self.label_badges.clear();
        self.typed_prefix.clear();
        self.free_runtime.reset();
        self.mode = EngineMode::Grid;

        effects.push(EngineEffect::ClearKeyBuffer);
        effects.push(EngineEffect::ShowOverlay {
            monitor_id: monitor.id,
        });
    }

    fn activate_free_mode(
        &mut self,
        explicit_preset_id: Option<&str>,
        effects: &mut Vec<EngineEffect>,
    ) {
        // Pressing the Free Mode toggle while already in Free Mode (without explicit preset switch) exits to Normal.
        if self.mode == EngineMode::FreeMode && explicit_preset_id.is_none() {
            self.exit_to_normal(false, effects);
            return;
        }

        let monitor = self.current_monitor().cloned();
        let resolved = resolve_preset(
            &self.config.presets,
            explicit_preset_id,
            &self.config.active_preset_id,
            self.active_app_id.as_deref(),
            monitor.as_ref(),
        )
        .cloned();

        self.active_preset = resolved;
        self.explicit_preset_id = explicit_preset_id.map(str::to_string);
        self.primary_grid = None;
        self.subgrid = None;
        self.label_badges.clear();
        self.typed_prefix.clear();
        self.free_runtime.reset();
        self.mode = EngineMode::FreeMode;

        effects.push(EngineEffect::ClearKeyBuffer);
        effects.push(EngineEffect::HideOverlay);
    }

    fn activate_label_mode(&mut self, effects: &mut Vec<EngineEffect>) {
        if self.mode == EngineMode::LabelMode {
            self.exit_to_normal(false, effects);
            return;
        }
        let Some(monitor) = self.current_monitor().cloned() else {
            return;
        };

        self.active_monitor_id = Some(monitor.id.clone());
        self.primary_grid = None;
        self.subgrid = None;
        self.label_badges.clear();
        self.typed_prefix.clear();
        self.free_runtime.reset();
        self.mode = EngineMode::LabelMode;

        effects.push(EngineEffect::ClearKeyBuffer);
        effects.push(EngineEffect::ShowOverlay {
            monitor_id: monitor.id.clone(),
        });
        effects.push(EngineEffect::QueryAccessibility {
            monitor_id: monitor.id,
        });
    }

    fn exit_to_normal(&mut self, release_drag: bool, effects: &mut Vec<EngineEffect>) {
        if release_drag && self.dragging {
            if let Some((mid, phys)) = self.current_cursor_physical() {
                effects.push(EngineEffect::MouseUp {
                    button: MouseButton::Left,
                    logical: self.cursor_logical,
                    physical: phys,
                    monitor_id: mid,
                });
            }
            self.dragging = false;
        }

        self.mode = EngineMode::Normal;
        self.explicit_preset_id = None;
        self.primary_grid = None;
        self.subgrid = None;
        self.label_badges.clear();
        self.typed_prefix.clear();
        self.free_runtime.reset();

        effects.push(EngineEffect::ClearKeyBuffer);
        effects.push(EngineEffect::HideOverlay);
    }

    fn handle_key_char(&mut self, ch: char, effects: &mut Vec<EngineEffect>) {
        match self.mode {
            EngineMode::Grid => self.handle_grid_char(ch, effects),
            EngineMode::Subgrid => self.handle_subgrid_char(ch, effects),
            EngineMode::LabelMode => self.handle_label_char(ch, effects),
            EngineMode::Normal | EngineMode::Nudge | EngineMode::FreeMode => {}
        }
    }

    fn handle_grid_char(&mut self, ch: char, effects: &mut Vec<EngineEffect>) {
        let Some(grid) = self.primary_grid.as_ref() else {
            return;
        };
        let mut candidate = self.typed_prefix.clone();
        candidate.push(ch);

        match grid.match_prefix(&candidate) {
            PrefixMatch::NoMatch => {
                // Ignore invalid keystroke so partial progress is preserved.
            }
            PrefixMatch::Candidates(_) => {
                self.typed_prefix = candidate;
            }
            PrefixMatch::Exact(cell) => {
                let chosen = cell.clone();
                let monitor_id = grid.monitor_id.clone();
                self.typed_prefix.clear();
                effects.push(EngineEffect::ClearKeyBuffer);
                self.move_cursor_to(chosen.center, Some(&monitor_id), effects);

                let action = self
                    .active_preset
                    .as_ref()
                    .map(|p| p.quick_select_action)
                    .unwrap_or(QuickSelectAction::EnterSubgrid);

                match action {
                    QuickSelectAction::EnterSubgrid | QuickSelectAction::SubgridThenFreeMode => {
                        let grid_cfg = self.effective_grid_config().clone();
                        if let Ok(sub) = generate_subgrid(&chosen, &monitor_id, &grid_cfg) {
                            self.subgrid = Some(sub);
                            self.mode = EngineMode::Subgrid;
                        } else {
                            self.mode = EngineMode::Nudge;
                        }
                    }
                    QuickSelectAction::HandoffToFreeMode => {
                        self.primary_grid = None;
                        self.subgrid = None;
                        self.free_runtime.reset();
                        self.mode = EngineMode::FreeMode;
                        effects.push(EngineEffect::HideOverlay);
                    }
                    QuickSelectAction::EnterNudge => {
                        self.subgrid = None;
                        self.mode = EngineMode::Nudge;
                    }
                    QuickSelectAction::ClickAndExit { button } => {
                        if let Some((mid, phys)) = self.current_cursor_physical() {
                            effects.push(EngineEffect::Click {
                                button,
                                count: 1,
                                logical: self.cursor_logical,
                                physical: phys,
                                monitor_id: mid,
                            });
                        }
                        self.exit_to_normal(false, effects);
                    }
                    QuickSelectAction::MoveAndExit => {
                        self.exit_to_normal(false, effects);
                    }
                }
            }
        }
    }

    fn handle_subgrid_char(&mut self, ch: char, effects: &mut Vec<EngineEffect>) {
        let Some(subgrid) = self.subgrid.as_ref() else {
            return;
        };
        let mut candidate = self.typed_prefix.clone();
        candidate.push(ch);

        match subgrid.match_prefix(&candidate) {
            PrefixMatch::NoMatch => {}
            PrefixMatch::Candidates(_) => {
                self.typed_prefix = candidate;
            }
            PrefixMatch::Exact(cell) => {
                let chosen = cell.clone();
                let monitor_id = subgrid.monitor_id.clone();
                self.typed_prefix.clear();
                effects.push(EngineEffect::ClearKeyBuffer);
                self.move_cursor_to(chosen.center, Some(&monitor_id), effects);

                let handoff = matches!(
                    self.active_preset.as_ref().map(|p| p.quick_select_action),
                    Some(QuickSelectAction::SubgridThenFreeMode)
                );
                if handoff {
                    self.primary_grid = None;
                    self.subgrid = None;
                    self.free_runtime.reset();
                    self.mode = EngineMode::FreeMode;
                    effects.push(EngineEffect::HideOverlay);
                } else {
                    self.mode = EngineMode::Nudge;
                }
            }
        }
    }

    fn handle_label_char(&mut self, ch: char, effects: &mut Vec<EngineEffect>) {
        let case_sensitive = self.config.label_mode.case_sensitive;
        let mut candidate = self.typed_prefix.clone();
        candidate.push(ch);
        let norm_candidate = normalize_str(&candidate, case_sensitive);

        let matches: Vec<LabelBadge> = self
            .label_badges
            .iter()
            .filter(|b| normalize_str(&b.label, case_sensitive).starts_with(&norm_candidate))
            .cloned()
            .collect();

        if matches.is_empty() {
            return;
        }

        // Exact unique match OR single remaining prefix candidate triggers immediately.
        let exact_match = matches
            .iter()
            .find(|b| normalize_str(&b.label, case_sensitive) == norm_candidate);

        let trigger_badge = if matches.len() == 1 {
            Some(matches[0].clone())
        } else if exact_match.is_some() && matches.len() == 1 {
            exact_match.cloned()
        } else {
            None
        };

        if let Some(badge) = trigger_badge {
            self.typed_prefix.clear();
            effects.push(EngineEffect::ClearKeyBuffer);
            self.move_cursor_to(badge.center, Some(&badge.monitor_id), effects);

            if self.config.label_mode.auto_click {
                if let Some((mid, phys)) = self.current_cursor_physical() {
                    effects.push(EngineEffect::Click {
                        button: MouseButton::Left,
                        count: 1,
                        logical: self.cursor_logical,
                        physical: phys,
                        monitor_id: mid,
                    });
                }
            }

            if self.config.label_mode.exit_after_action {
                self.exit_to_normal(false, effects);
            }
        } else {
            self.typed_prefix = candidate;
        }
    }

    fn load_accessibility_badges(&mut self, mut badges: Vec<LabelBadge>) {
        badges.truncate(self.config.label_mode.max_badges);

        // Assign deterministic labels to any badge missing one.
        let needs_labels = badges.iter().any(|b| b.label.is_empty());
        if needs_labels && !badges.is_empty() {
            let mut scheme = self.config.grid.label_scheme.clone();
            scheme.case_sensitive = self.config.label_mode.case_sensitive;
            let count = badges.len();
            let label_len = if scheme.capacity(1) >= count { 1 } else { 2 };
            let cols = count.min(u16::MAX as usize) as u16;
            if let Ok(generated) = scheme.generate_labels(cols, 1, label_len) {
                for (badge, lbl) in badges.iter_mut().zip(generated) {
                    if badge.label.is_empty() {
                        badge.label = lbl;
                    }
                }
            }
        }

        self.label_badges = badges;
        self.typed_prefix.clear();
    }

    fn filtered_label_badges(&self) -> Vec<LabelBadge> {
        if self.typed_prefix.is_empty() {
            return self.label_badges.clone();
        }
        let case_sensitive = self.config.label_mode.case_sensitive;
        let norm_prefix = normalize_str(&self.typed_prefix, case_sensitive);
        self.label_badges
            .iter()
            .filter(|b| normalize_str(&b.label, case_sensitive).starts_with(&norm_prefix))
            .cloned()
            .collect()
    }

    fn handle_backspace(&mut self, effects: &mut Vec<EngineEffect>) {
        if !self.typed_prefix.is_empty() {
            self.typed_prefix.pop();
            return;
        }

        match self.mode {
            EngineMode::Nudge => {
                if self.subgrid.is_some() {
                    self.mode = EngineMode::Subgrid;
                } else if self.primary_grid.is_some() {
                    self.mode = EngineMode::Grid;
                } else {
                    self.exit_to_normal(false, effects);
                }
                effects.push(EngineEffect::ClearKeyBuffer);
            }
            EngineMode::Subgrid => {
                self.subgrid = None;
                if self.primary_grid.is_some() {
                    self.mode = EngineMode::Grid;
                    effects.push(EngineEffect::ClearKeyBuffer);
                } else {
                    self.exit_to_normal(false, effects);
                }
            }
            EngineMode::Grid | EngineMode::LabelMode => {
                self.exit_to_normal(false, effects);
            }
            EngineMode::Normal | EngineMode::FreeMode => {}
        }
    }

    fn handle_switch_monitor(&mut self, switch: MonitorSwitch, effects: &mut Vec<EngineEffect>) {
        if !matches!(
            self.mode,
            EngineMode::Grid | EngineMode::Subgrid | EngineMode::Nudge | EngineMode::LabelMode
        ) {
            return;
        }
        let Some(current_id) = self.current_monitor().map(|m| m.id.clone()) else {
            return;
        };

        let target_id: Option<String> = match switch {
            MonitorSwitch::Next => self.layout.next_monitor_id(&current_id).map(str::to_string),
            MonitorSwitch::Prev => self.layout.prev_monitor_id(&current_id).map(str::to_string),
            MonitorSwitch::ById { id } => self.layout.by_id(&id).map(|m| m.id.clone()),
            MonitorSwitch::ByIndex { index } => self.layout.by_index(index).map(|m| m.id.clone()),
            MonitorSwitch::Direction { direction } => self
                .layout
                .neighbor_in_direction(&current_id, direction)
                .map(str::to_string),
        };

        let Some(target_id) = target_id else {
            return;
        };
        if target_id == current_id && self.mode == EngineMode::Grid {
            return;
        }

        if let Some(target_mon) = self.layout.by_id(&target_id).cloned() {
            self.move_cursor_to(target_mon.logical_center(), Some(&target_mon.id), effects);
        }

        if self.mode == EngineMode::LabelMode {
            self.active_monitor_id = Some(target_id.clone());
            self.label_badges.clear();
            self.typed_prefix.clear();
            effects.push(EngineEffect::ClearKeyBuffer);
            effects.push(EngineEffect::ShowOverlay {
                monitor_id: target_id.clone(),
            });
            effects.push(EngineEffect::QueryAccessibility {
                monitor_id: target_id,
            });
        } else {
            let preset_id = self.explicit_preset_id.clone();
            self.activate_grid(Some(&target_id), preset_id.as_deref(), effects);
        }
    }

    fn handle_nudge(&mut self, direction: Direction, step: NudgeStep, effects: &mut Vec<EngineEffect>) {
        if !matches!(
            self.mode,
            EngineMode::Grid | EngineMode::Subgrid | EngineMode::Nudge
        ) {
            return;
        }
        let Some(active_mon) = self.current_monitor().cloned() else {
            return;
        };
        let nudge_cfg = self.effective_nudge_config().clone();
        let (new_mon_id, new_pt) = nudge_in_direction(
            self.cursor_logical,
            direction,
            step,
            &nudge_cfg,
            &active_mon,
            &self.layout,
        );
        if !self.typed_prefix.is_empty() {
            self.typed_prefix.clear();
            effects.push(EngineEffect::ClearKeyBuffer);
        }
        self.last_nudge_step = step;
        self.mode = EngineMode::Nudge;
        self.move_cursor_to(new_pt, Some(&new_mon_id), effects);
    }

    fn handle_mouse_action(&mut self, action: MouseActionKind, effects: &mut Vec<EngineEffect>) {
        if self.mode == EngineMode::Normal {
            return;
        }
        let Some((mid, phys)) = self.current_cursor_physical() else {
            return;
        };
        let logical = self.cursor_logical;
        let stay_in_free_mode = self.mode == EngineMode::FreeMode;

        match action {
            MouseActionKind::LeftClick => {
                effects.push(EngineEffect::Click {
                    button: MouseButton::Left,
                    count: 1,
                    logical,
                    physical: phys,
                    monitor_id: mid,
                });
                if !stay_in_free_mode {
                    self.exit_to_normal(false, effects);
                }
            }
            MouseActionKind::DoubleClick => {
                effects.push(EngineEffect::Click {
                    button: MouseButton::Left,
                    count: 2,
                    logical,
                    physical: phys,
                    monitor_id: mid,
                });
                if !stay_in_free_mode {
                    self.exit_to_normal(false, effects);
                }
            }
            MouseActionKind::RightClick => {
                effects.push(EngineEffect::Click {
                    button: MouseButton::Right,
                    count: 1,
                    logical,
                    physical: phys,
                    monitor_id: mid,
                });
                if !stay_in_free_mode {
                    self.exit_to_normal(false, effects);
                }
            }
            MouseActionKind::MiddleClick => {
                effects.push(EngineEffect::Click {
                    button: MouseButton::Middle,
                    count: 1,
                    logical,
                    physical: phys,
                    monitor_id: mid,
                });
                if !stay_in_free_mode {
                    self.exit_to_normal(false, effects);
                }
            }
            MouseActionKind::DragStart => {
                if !self.dragging {
                    self.dragging = true;
                    effects.push(EngineEffect::MouseDown {
                        button: MouseButton::Left,
                        logical,
                        physical: phys,
                        monitor_id: mid,
                    });
                }
            }
            MouseActionKind::DragEnd => {
                if self.dragging {
                    self.dragging = false;
                    effects.push(EngineEffect::MouseUp {
                        button: MouseButton::Left,
                        logical,
                        physical: phys,
                        monitor_id: mid,
                    });
                }
                if !stay_in_free_mode {
                    self.exit_to_normal(false, effects);
                }
            }
            MouseActionKind::MoveOnly => {
                self.exit_to_normal(false, effects);
            }
        }
    }

    fn step_free_mode(&mut self, dt_ms: u32, effects: &mut Vec<EngineEffect>) {
        let profile = self.effective_free_profile().clone();
        let active_speed = self.free_runtime.active_speed();
        let (mut dx, mut dy) = self.free_runtime.move_vector();

        if dx != 0.0 || dy != 0.0 {
            self.free_runtime.move_hold_ms = self.free_runtime.move_hold_ms.saturating_add(dt_ms);
            let speed = profile.velocity_px_per_sec(active_speed, self.free_runtime.move_hold_ms);

            if dx != 0.0 && dy != 0.0 && profile.normalize_diagonal {
                let inv_sqrt2 = std::f64::consts::FRAC_1_SQRT_2;
                dx *= inv_sqrt2;
                dy *= inv_sqrt2;
            }

            let dt_sec = dt_ms as f64 / 1000.0;
            let candidate = LogicalPoint::new(
                self.cursor_logical.x + dx * speed * dt_sec,
                self.cursor_logical.y + dy * speed * dt_sec,
            );

            let (new_mon_id, clamped) = if profile.allow_cross_monitor {
                self.layout
                    .clamp_logical_to_virtual_desktop(candidate)
                    .unwrap_or_else(|| {
                        let fallback_id = self
                            .active_monitor_id
                            .clone()
                            .unwrap_or_else(|| "default".to_string());
                        (fallback_id, candidate)
                    })
            } else if let Some(mon) = self.current_monitor() {
                (mon.id.clone(), clamp_logical_to_monitor(candidate, mon))
            } else {
                (
                    self.active_monitor_id
                        .clone()
                        .unwrap_or_else(|| "default".to_string()),
                    candidate,
                )
            };

            self.move_cursor_to(clamped, Some(&new_mon_id), effects);
        } else {
            self.free_runtime.move_hold_ms = 0;
        }

        let (sx, sy) = self.free_runtime.scroll_vector();
        if sx != 0.0 || sy != 0.0 {
            let delta = profile.scroll_delta_for_tick(active_speed, dt_ms);
            effects.push(EngineEffect::Scroll {
                delta_x: sx * delta,
                delta_y: sy * delta,
            });
        }
    }

    fn move_cursor_to(
        &mut self,
        logical: LogicalPoint,
        preferred_monitor_id: Option<&str>,
        effects: &mut Vec<EngineEffect>,
    ) {
        let monitor = preferred_monitor_id
            .and_then(|id| self.layout.by_id(id))
            .or_else(|| self.layout.nearest_monitor_logical(logical))
            .cloned();

        if let Some(mon) = monitor {
            let clamped = clamp_logical_to_monitor(logical, &mon);
            let physical = logical_to_physical_clamped(clamped, &mon);
            self.cursor_logical = clamped;
            self.active_monitor_id = Some(mon.id.clone());
            effects.push(EngineEffect::MoveCursor {
                logical: clamped,
                physical,
                monitor_id: mon.id,
            });
        } else {
            self.cursor_logical = logical;
        }
    }

    fn current_cursor_physical(&self) -> Option<(String, PhysicalPoint)> {
        let mon = self
            .layout
            .monitor_at_logical(self.cursor_logical)
            .or_else(|| self.current_monitor())?;
        let phys = logical_to_physical_clamped(self.cursor_logical, mon);
        Some((mon.id.clone(), phys))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::LogicalRect;

    fn dual_mixed_dpi_layout() -> MonitorLayout {
        let left = MonitorInfo::from_logical_and_scale(
            "LEFT",
            "Left 125%",
            LogicalRect::new(-1536.0, 0.0, 1536.0, 864.0),
            PhysicalPoint::new(-1920, 0),
            1.25,
            false,
        )
        .unwrap();
        let primary = MonitorInfo::from_logical_and_scale(
            "PRIMARY",
            "Primary 200%",
            LogicalRect::new(0.0, 0.0, 1440.0, 900.0),
            PhysicalPoint::new(0, 0),
            2.0,
            true,
        )
        .unwrap();
        MonitorLayout::new(vec![left, primary]).unwrap()
    }

    #[test]
    fn normal_grid_subgrid_nudge_click_full_lifecycle() {
        let mut engine = CoreEngine::new(RatPoisonConfig::default(), dual_mixed_dpi_layout());
        assert_eq!(engine.mode(), EngineMode::Normal);

        // 1. Activate Grid on PRIMARY
        let out = engine.handle_event(EngineEvent::ActivateGrid {
            monitor_id: None,
            preset_id: Some("standard".to_string()),
        });
        assert_eq!(out.snapshot.mode, EngineMode::Grid);
        assert!(out.snapshot.visible);
        assert_eq!(out.snapshot.active_monitor_id.as_deref(), Some("PRIMARY"));
        assert_eq!(out.snapshot.grid_cells.len(), 16 * 12);

        // 2. Type first char 'a' (partial prefix)
        let out = engine.handle_event(EngineEvent::KeyChar { ch: 'a' });
        assert_eq!(out.snapshot.mode, EngineMode::Grid);
        assert_eq!(out.snapshot.typed_prefix, "a");

        // 3. Type second char 'a' -> exact match -> enters Subgrid and moves cursor
        let out = engine.handle_event(EngineEvent::KeyChar { ch: 'a' });
        assert_eq!(out.snapshot.mode, EngineMode::Subgrid);
        assert!(out.snapshot.subgrid_rect.is_some());
        assert_eq!(out.snapshot.subgrid_cells.len(), 9);
        assert!(out
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::MoveCursor { monitor_id, .. } if monitor_id == "PRIMARY")));

        // 4. Pick center subgrid cell 's' -> enters Nudge
        let out = engine.handle_event(EngineEvent::KeyChar { ch: 's' });
        assert_eq!(out.snapshot.mode, EngineMode::Nudge);
        let before_nudge = engine.cursor_logical();

        // 5. Fine nudge right
        let out = engine.handle_event(EngineEvent::Nudge {
            direction: Direction::Right,
            step: NudgeStep::Fine,
        });
        assert_eq!(out.snapshot.mode, EngineMode::Nudge);
        assert!((engine.cursor_logical().x - (before_nudge.x + 1.5)).abs() < 1e-9);

        // 6. Double-click -> emits Click(count=2) + HideOverlay and returns to Normal
        let out = engine.handle_event(EngineEvent::MouseAction {
            action: MouseActionKind::DoubleClick,
        });
        assert_eq!(out.snapshot.mode, EngineMode::Normal);
        assert!(!out.snapshot.visible);
        assert!(out.effects.iter().any(|e| matches!(
            e,
            EngineEffect::Click {
                button: MouseButton::Left,
                count: 2,
                ..
            }
        )));
        assert!(out.effects.iter().any(|e| matches!(e, EngineEffect::HideOverlay)));
    }

    #[test]
    fn backspace_unwinds_nudge_to_subgrid_to_grid_to_normal() {
        let mut engine = CoreEngine::new(RatPoisonConfig::default(), dual_mixed_dpi_layout());
        engine.handle_event(EngineEvent::ActivateGrid {
            monitor_id: None,
            preset_id: Some("standard".to_string()),
        });
        engine.handle_event(EngineEvent::KeyChar { ch: 'a' });
        engine.handle_event(EngineEvent::KeyChar { ch: 'b' });
        assert_eq!(engine.mode(), EngineMode::Subgrid);

        engine.handle_event(EngineEvent::KeyChar { ch: 'q' });
        assert_eq!(engine.mode(), EngineMode::Nudge);

        // Backspace: Nudge -> Subgrid
        engine.handle_event(EngineEvent::Backspace);
        assert_eq!(engine.mode(), EngineMode::Subgrid);

        // Backspace: Subgrid -> Grid
        engine.handle_event(EngineEvent::Backspace);
        assert_eq!(engine.mode(), EngineMode::Grid);

        // Partial char then Backspace pops char without leaving Grid
        engine.handle_event(EngineEvent::KeyChar { ch: 'a' });
        let out = engine.handle_event(EngineEvent::Backspace);
        assert_eq!(out.snapshot.mode, EngineMode::Grid);
        assert_eq!(out.snapshot.typed_prefix, "");

        // Backspace on empty Grid exits to Normal
        let out = engine.handle_event(EngineEvent::Backspace);
        assert_eq!(out.snapshot.mode, EngineMode::Normal);
    }

    #[test]
    fn quick_select_preset_hands_off_immediately_into_free_mode() {
        let mut engine = CoreEngine::new(RatPoisonConfig::default(), dual_mixed_dpi_layout());

        // Activate Quick Select + Free Mode preset (6x4 single-char grid)
        let out = engine.handle_event(EngineEvent::ActivateGrid {
            monitor_id: Some("PRIMARY".to_string()),
            preset_id: Some("quick_select_free".to_string()),
        });
        assert_eq!(out.snapshot.mode, EngineMode::Grid);
        assert_eq!(out.snapshot.grid_cells.len(), 24);
        assert_eq!(
            out.snapshot.active_preset_id.as_deref(),
            Some("quick_select_free")
        );

        // Single keypress 'q' jumps to top-left zone and IMMEDIATELY hands off to FreeMode!
        let out = engine.handle_event(EngineEvent::KeyChar { ch: 'q' });
        assert_eq!(out.snapshot.mode, EngineMode::FreeMode);
        assert!(!out.snapshot.visible);
        assert!(out
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::MoveCursor { .. })));
        assert!(out
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::HideOverlay)));

        // Immediately nudge smoothly with FreeMode keys (L = right, A = precision slow)
        let start = engine.cursor_logical();
        engine.handle_event(EngineEvent::FreeModeKeyDown {
            key: FreeKey::SpeedSlow,
        });
        engine.handle_event(EngineEvent::FreeModeKeyDown {
            key: FreeKey::MoveRight,
        });
        let tick = engine.handle_event(EngineEvent::FreeModeTick { dt_ms: 100 });
        assert_eq!(tick.snapshot.active_modifiers.speed, SpeedModifier::Slow);
        assert!(engine.cursor_logical().x > start.x);
    }

    #[test]
    fn free_mode_speed_modifiers_acceleration_diagonal_and_cross_monitor() {
        let mut engine = CoreEngine::new(RatPoisonConfig::default(), dual_mixed_dpi_layout());
        engine.handle_event(EngineEvent::SyncCursor {
            logical: LogicalPoint::new(10.0, 200.0),
        });
        engine.handle_event(EngineEvent::ActivateFreeMode { preset_id: None });
        assert_eq!(engine.mode(), EngineMode::FreeMode);

        // Press Turbo (F) and MoveLeft (J) to cross from PRIMARY onto LEFT (negative X, 1.25x DPI)
        engine.handle_event(EngineEvent::FreeModeKeyDown {
            key: FreeKey::SpeedTurbo,
        });
        engine.handle_event(EngineEvent::FreeModeKeyDown {
            key: FreeKey::MoveLeft,
        });
        let tick1 = engine.handle_event(EngineEvent::FreeModeTick { dt_ms: 50 });
        assert!(engine.cursor_logical().x < 0.0);
        assert_eq!(engine.active_monitor_id(), Some("LEFT"));
        // Verify emitted MoveCursor carries LEFT monitor's negative physical X!
        let moved_phys = tick1.effects.iter().find_map(|e| match e {
            EngineEffect::MoveCursor {
                physical,
                monitor_id,
                ..
            } => Some((monitor_id.clone(), *physical)),
            _ => None,
        });
        let (mid, phys) = moved_phys.unwrap();
        assert_eq!(mid, "LEFT");
        assert!(phys.x < 0);

        // Press Slow (A) on top of Turbo (F): last-pressed Slow wins; releasing A restores Turbo
        let out_slow = engine.handle_event(EngineEvent::FreeModeKeyDown {
            key: FreeKey::SpeedSlow,
        });
        assert_eq!(out_slow.snapshot.active_modifiers.speed, SpeedModifier::Slow);
        let out_turbo = engine.handle_event(EngineEvent::FreeModeKeyUp {
            key: FreeKey::SpeedSlow,
        });
        assert_eq!(
            out_turbo.snapshot.active_modifiers.speed,
            SpeedModifier::Turbo
        );

        // Verify scrolling emits Scroll effect
        engine.handle_event(EngineEvent::FreeModeKeyDown {
            key: FreeKey::ScrollDown,
        });
        let tick_scroll = engine.handle_event(EngineEvent::FreeModeTick { dt_ms: 100 });
        assert!(tick_scroll
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::Scroll { delta_y, .. } if *delta_y < 0.0)));
    }

    #[test]
    fn label_mode_prefix_filters_and_auto_clicks_on_unique_match() {
        let mut engine = CoreEngine::new(RatPoisonConfig::default(), dual_mixed_dpi_layout());

        let out = engine.handle_event(EngineEvent::ActivateLabelMode);
        assert_eq!(out.snapshot.mode, EngineMode::LabelMode);
        assert!(out.effects.iter().any(|e| matches!(
            e,
            EngineEffect::QueryAccessibility { monitor_id } if monitor_id == "PRIMARY"
        )));

        let badges = vec![
            LabelBadge {
                id: "btn-save".to_string(),
                label: "sa".to_string(),
                title: "Save".to_string(),
                role: "AXButton".to_string(),
                rect: LogicalRect::new(100.0, 100.0, 80.0, 30.0),
                center: LogicalPoint::new(140.0, 115.0),
                monitor_id: "PRIMARY".to_string(),
            },
            LabelBadge {
                id: "btn-settings".to_string(),
                label: "se".to_string(),
                title: "Settings".to_string(),
                role: "AXButton".to_string(),
                rect: LogicalRect::new(200.0, 100.0, 80.0, 30.0),
                center: LogicalPoint::new(240.0, 115.0),
                monitor_id: "PRIMARY".to_string(),
            },
            LabelBadge {
                id: "btn-cancel".to_string(),
                label: "ca".to_string(),
                title: "Cancel".to_string(),
                role: "AXButton".to_string(),
                rect: LogicalRect::new(300.0, 100.0, 80.0, 30.0),
                center: LogicalPoint::new(340.0, 115.0),
                monitor_id: "PRIMARY".to_string(),
            },
        ];

        let out = engine.handle_event(EngineEvent::AccessibilityLabelsLoaded { badges });
        assert_eq!(out.snapshot.label_badges.len(), 3);

        // Typing 's' narrows to 2 badges ("sa", "se")
        let out = engine.handle_event(EngineEvent::KeyChar { ch: 's' });
        assert_eq!(out.snapshot.mode, EngineMode::LabelMode);
        assert_eq!(out.snapshot.label_badges.len(), 2);

        // Typing 'e' uniquely matches "se" -> moves to (240, 115), auto-clicks, exits to Normal!
        let out = engine.handle_event(EngineEvent::KeyChar { ch: 'e' });
        assert_eq!(out.snapshot.mode, EngineMode::Normal);
        assert_eq!(engine.cursor_logical(), LogicalPoint::new(240.0, 115.0));
        assert!(out.effects.iter().any(|e| matches!(
            e,
            EngineEffect::Click {
                button: MouseButton::Left,
                count: 1,
                physical: PhysicalPoint { x: 480, y: 230 },
                ..
            }
        )));
    }

    #[test]
    fn drag_start_persists_across_grid_and_releases_on_drag_end_or_escape() {
        let mut engine = CoreEngine::new(RatPoisonConfig::default(), dual_mixed_dpi_layout());
        engine.handle_event(EngineEvent::ActivateGrid {
            monitor_id: Some("PRIMARY".to_string()),
            preset_id: Some("standard".to_string()),
        });

        let out = engine.handle_event(EngineEvent::MouseAction {
            action: MouseActionKind::DragStart,
        });
        assert!(engine.is_dragging());
        assert!(out
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::MouseDown { .. })));

        // Switch monitor while dragging and finish drag
        engine.handle_event(EngineEvent::SwitchMonitor {
            switch: MonitorSwitch::Direction {
                direction: Direction::Left,
            },
        });
        assert_eq!(engine.active_monitor_id(), Some("LEFT"));
        assert!(engine.is_dragging());

        let out = engine.handle_event(EngineEvent::MouseAction {
            action: MouseActionKind::DragEnd,
        });
        assert!(!engine.is_dragging());
        assert_eq!(out.snapshot.mode, EngineMode::Normal);
        assert!(out.effects.iter().any(|e| matches!(
            e,
            EngineEffect::MouseUp { monitor_id, .. } if monitor_id == "LEFT"
        )));

        // Verify ExitToNormal also releases an active drag cleanly
        engine.handle_event(EngineEvent::ActivateFreeMode { preset_id: None });
        engine.handle_event(EngineEvent::MouseAction {
            action: MouseActionKind::DragStart,
        });
        let esc = engine.handle_event(EngineEvent::ExitToNormal);
        assert!(!engine.is_dragging());
        assert!(esc
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::MouseUp { .. })));
    }

    #[test]
    fn overlay_snapshot_serializes_cleanly_to_json() {
        let mut engine = CoreEngine::new(RatPoisonConfig::default(), dual_mixed_dpi_layout());
        let out = engine.handle_event(EngineEvent::ActivateGrid {
            monitor_id: None,
            preset_id: None,
        });
        let json = serde_json::to_string(&out.snapshot).unwrap();
        let parsed: OverlaySnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, out.snapshot);
    }

    fn config_18x26_case_sensitive_with_3x3_subgrid() -> RatPoisonConfig {
        let mut cfg = RatPoisonConfig::default();
        cfg.grid.rows = 18;
        cfg.grid.cols = 26;
        cfg.grid.subgrid_rows = 3;
        cfg.grid.subgrid_cols = 3;
        cfg.grid.label_scheme.case_sensitive = true;
        cfg.grid.label_scheme.alphabet =
            "asdfghjklqwertyuiopzxcvbnmASDFGHJKLQWERTYUIOPZXCVBNM"
                .chars()
                .collect();
        cfg.grid.label_scheme.second_alphabet = None;
        cfg.nudge.coarse_px = 30.0;
        cfg.nudge.normal_px = 10.0;
        cfg.nudge.fine_px = 2.0;
        cfg
    }

    #[test]
    fn live_config_grid_18x26_and_nudge_honored_over_default_standard_preset() {
        let cfg = config_18x26_case_sensitive_with_3x3_subgrid();
        // Built-in default "standard" preset is still 16x12 in cfg.presets!
        assert_eq!(cfg.presets[0].id, "standard");
        assert_eq!(cfg.presets[0].grid.cols, 16);
        assert_eq!(cfg.presets[0].grid.rows, 12);

        // 1. CoreEngine::new with 18x26 live config
        let mut engine = CoreEngine::new(cfg.clone(), dual_mixed_dpi_layout());
        let out = engine.handle_event(EngineEvent::ActivateGrid {
            monitor_id: None,
            preset_id: None,
        });
        assert_eq!(out.snapshot.mode, EngineMode::Grid);
        assert_eq!(out.snapshot.active_preset_id.as_deref(), Some("standard"));
        assert_eq!(out.snapshot.grid_cells.len(), 18 * 26);
        assert_eq!(out.snapshot.grid_dimensions(), (18, 26));

        // 2. Starting from default 16x12 and updating via ConfigUpdated in Normal mode
        let mut engine2 = CoreEngine::new(RatPoisonConfig::default(), dual_mixed_dpi_layout());
        engine2.handle_event(EngineEvent::ConfigUpdated {
            config: Box::new(cfg.clone()),
        });
        let out2 = engine2.handle_event(EngineEvent::ActivateGrid {
            monitor_id: None,
            preset_id: None,
        });
        assert_eq!(out2.snapshot.grid_cells.len(), 18 * 26);
        assert_eq!(out2.snapshot.grid_dimensions(), (18, 26));

        // 3. Updating via ConfigUpdated while Grid is already open refreshes grid live
        let mut smaller = cfg.clone();
        smaller.grid.rows = 10;
        smaller.grid.cols = 14;
        let out3 = engine2.handle_event(EngineEvent::ConfigUpdated {
            config: Box::new(smaller),
        });
        assert_eq!(out3.snapshot.mode, EngineMode::Grid);
        assert_eq!(out3.snapshot.grid_cells.len(), 10 * 14);
        assert_eq!(out3.snapshot.grid_dimensions(), (10, 14));

        // 4. Live free_mode config is also honored when using default standard preset
        let mut custom_free = RatPoisonConfig::default();
        custom_free.free_mode.scroll_speed = 50.0;
        let mut engine3 = CoreEngine::new(custom_free, dual_mixed_dpi_layout());
        engine3.handle_event(EngineEvent::ActivateFreeMode { preset_id: None });
        engine3.handle_event(EngineEvent::FreeModeKeyDown {
            key: FreeKey::ScrollUp,
        });
        let tick = engine3.handle_event(EngineEvent::FreeModeTick { dt_ms: 100 });
        let scroll_y = tick.effects.iter().find_map(|e| match e {
            EngineEffect::Scroll { delta_y, .. } => Some(*delta_y),
            _ => None,
        });
        assert!((scroll_y.unwrap() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn subgrid_18x26_exact_jump_coordinates_and_case_insensitive_keys_on_retina_and_offset_display()
    {
        let layout = dual_mixed_dpi_layout();
        let cfg = config_18x26_case_sensitive_with_3x3_subgrid();
        let sub_keys = ['q', 'w', 'e', 'a', 's', 'd', 'z', 'x', 'c'];

        for monitor_id in ["PRIMARY", "LEFT"] {
            let mon = layout.by_id(monitor_id).unwrap().clone();
            let mut engine = CoreEngine::new(cfg.clone(), layout.clone());

            let out = engine.handle_event(EngineEvent::ActivateGrid {
                monitor_id: Some(monitor_id.to_string()),
                preset_id: None,
            });
            assert_eq!(out.snapshot.grid_dimensions(), (18, 26));

            // Check top-left (0, 0), interior (9, 13), and bottom-right (17, 25) primary cells
            for (row, col) in [(0usize, 0usize), (9, 13), (17, 25)] {
                let target_cell = out.snapshot.grid_cells[row * 26 + col].clone();
                let mut chars = target_cell.label.chars();
                let c1 = chars.next().unwrap();
                let c2 = chars.next().unwrap();

                let p1 = engine.handle_event(EngineEvent::KeyChar { ch: c1 });
                assert_eq!(p1.snapshot.mode, EngineMode::Grid);

                let p2 = engine.handle_event(EngineEvent::KeyChar { ch: c2 });
                assert_eq!(p2.snapshot.mode, EngineMode::Subgrid);
                assert_eq!(p2.snapshot.subgrid_dimensions(), (3, 3));
                assert_eq!(p2.snapshot.subgrid_rect, Some(target_cell.rect));

                // Cursor jumped to exact primary cell center
                let expected_parent_phys = logical_to_physical_clamped(target_cell.center, &mon);
                assert!((engine.cursor_logical().x - target_cell.center.x).abs() < 1e-9);
                assert!((engine.cursor_logical().y - target_cell.center.y).abs() < 1e-9);
                assert!(p2.effects.iter().any(|e| matches!(
                    e,
                    EngineEffect::MoveCursor {
                        physical,
                        monitor_id: mid,
                        ..
                    } if *physical == expected_parent_phys && mid == monitor_id
                )));

                // Now test EVERY ONE of the 9 subgrid cells (mixing lower & UPPER case keys!)
                for (idx, &key) in sub_keys.iter().enumerate() {
                    let sr = idx / 3;
                    let sc = idx % 3;
                    let sx0 = target_cell.rect.x + (sc as f64) * target_cell.rect.width / 3.0;
                    let sx1 = if sc == 2 {
                        target_cell.rect.right()
                    } else {
                        target_cell.rect.x + ((sc + 1) as f64) * target_cell.rect.width / 3.0
                    };
                    let sy0 = target_cell.rect.y + (sr as f64) * target_cell.rect.height / 3.0;
                    let sy1 = if sr == 2 {
                        target_cell.rect.bottom()
                    } else {
                        target_cell.rect.y + ((sr + 1) as f64) * target_cell.rect.height / 3.0
                    };
                    let expected_logical =
                        LogicalPoint::new((sx0 + sx1) * 0.5, (sy0 + sy1) * 0.5);
                    let expected_phys = logical_to_physical_clamped(expected_logical, &mon);

                    // Alternate lowercase & uppercase to prove case-insensitive subgrid matching
                    let pressed = if idx % 2 == 1 {
                        key.to_ascii_uppercase()
                    } else {
                        key
                    };
                    let sub_step = engine.handle_event(EngineEvent::KeyChar { ch: pressed });
                    assert_eq!(
                        sub_step.snapshot.mode,
                        EngineMode::Nudge,
                        "monitor={monitor_id} cell=({row},{col}) key='{pressed}'"
                    );
                    assert!(target_cell.rect.contains_point_inclusive(expected_logical));
                    assert!(
                        (engine.cursor_logical().x - expected_logical.x).abs() < 1e-9,
                        "logical x mismatch on {monitor_id} cell ({row},{col}) sub '{pressed}'"
                    );
                    assert!(
                        (engine.cursor_logical().y - expected_logical.y).abs() < 1e-9,
                        "logical y mismatch on {monitor_id} cell ({row},{col}) sub '{pressed}'"
                    );
                    assert_eq!(sub_step.snapshot.cursor_preview, Some(engine.cursor_logical()));
                    assert!(sub_step.effects.iter().any(|e| matches!(
                        e,
                        EngineEffect::MoveCursor {
                            physical,
                            monitor_id: mid,
                            ..
                        } if *physical == expected_phys && mid == monitor_id
                    )));

                    // Step back Nudge -> Subgrid to test the remaining subgrid keys
                    let back = engine.handle_event(EngineEvent::Backspace);
                    assert_eq!(back.snapshot.mode, EngineMode::Subgrid);
                }

                // Step back Subgrid -> Grid for next primary cell
                let back_grid = engine.handle_event(EngineEvent::Backspace);
                assert_eq!(back_grid.snapshot.mode, EngineMode::Grid);
            }
        }
    }

    #[test]
    fn nudge_works_in_grid_subgrid_and_nudge_modes_with_live_config_and_clamping() {
        let mut cfg = config_18x26_case_sensitive_with_3x3_subgrid();
        let mut engine = CoreEngine::new(cfg.clone(), dual_mixed_dpi_layout());

        // 1. Nudge directly from Grid mode (even after a partial typed prefix)
        engine.handle_event(EngineEvent::SyncCursor {
            logical: LogicalPoint::new(500.0, 400.0),
        });
        engine.handle_event(EngineEvent::ActivateGrid {
            monitor_id: None,
            preset_id: None,
        });
        engine.handle_event(EngineEvent::KeyChar { ch: 'a' });
        let out = engine.handle_event(EngineEvent::Nudge {
            direction: Direction::Right,
            step: NudgeStep::Normal,
        });
        assert_eq!(out.snapshot.mode, EngineMode::Nudge);
        assert_eq!(out.snapshot.typed_prefix, "");
        assert!((engine.cursor_logical().x - 510.0).abs() < 1e-9);
        assert!((engine.cursor_logical().y - 400.0).abs() < 1e-9);
        assert_eq!(out.snapshot.cursor_preview, Some(engine.cursor_logical()));
        assert!(out
            .effects
            .iter()
            .any(|e| matches!(e, EngineEffect::ClearKeyBuffer)));

        // 2. Nudge directly from Subgrid mode
        engine.handle_event(EngineEvent::ExitToNormal);
        let grid_out = engine.handle_event(EngineEvent::ActivateGrid {
            monitor_id: None,
            preset_id: None,
        });
        let mid_cell = grid_out.snapshot.grid_cells[5 * 26 + 10].clone();
        for ch in mid_cell.label.chars() {
            engine.handle_event(EngineEvent::KeyChar { ch });
        }
        assert_eq!(engine.mode(), EngineMode::Subgrid);
        let sub_nudge = engine.handle_event(EngineEvent::Nudge {
            direction: Direction::Up,
            step: NudgeStep::Fine,
        });
        assert_eq!(sub_nudge.snapshot.mode, EngineMode::Nudge);
        assert!((engine.cursor_logical().x - mid_cell.center.x).abs() < 1e-9);
        assert!((engine.cursor_logical().y - (mid_cell.center.y - 2.0)).abs() < 1e-9);
        assert_eq!(
            sub_nudge.snapshot.cursor_preview,
            Some(engine.cursor_logical())
        );

        // 3. Continued Fine / Normal / Coarse nudges in Nudge mode
        let before = engine.cursor_logical();
        let coarse_down = engine.handle_event(EngineEvent::Nudge {
            direction: Direction::Down,
            step: NudgeStep::Coarse,
        });
        assert_eq!(coarse_down.snapshot.mode, EngineMode::Nudge);
        assert!((engine.cursor_logical().y - (before.y + 30.0)).abs() < 1e-9);

        // 4. Clamping to ActiveMonitor vs VirtualDesktop
        engine.handle_event(EngineEvent::ExitToNormal);
        engine.handle_event(EngineEvent::SyncCursor {
            logical: LogicalPoint::new(5.0, 200.0),
        });
        engine.handle_event(EngineEvent::ActivateGrid {
            monitor_id: None,
            preset_id: None,
        });
        let clamped = engine.handle_event(EngineEvent::Nudge {
            direction: Direction::Left,
            step: NudgeStep::Coarse,
        });
        assert_eq!(clamped.snapshot.active_monitor_id.as_deref(), Some("PRIMARY"));
        assert!((engine.cursor_logical().x - 0.0).abs() < 1e-9);

        // Switch clamp mode to VirtualDesktop via ConfigUpdated -> nudge crosses onto LEFT
        cfg.nudge.clamp_mode = crate::grid::NudgeClampMode::VirtualDesktop;
        engine.handle_event(EngineEvent::ConfigUpdated {
            config: Box::new(cfg),
        });
        let crossed = engine.handle_event(EngineEvent::Nudge {
            direction: Direction::Left,
            step: NudgeStep::Coarse,
        });
        assert_eq!(crossed.snapshot.active_monitor_id.as_deref(), Some("LEFT"));
        assert!((engine.cursor_logical().x - (-30.0)).abs() < 1e-9);
    }

    #[test]
    fn monitor_scoped_preset_override_still_applies_across_monitor_switch() {
        let mut cfg = config_18x26_case_sensitive_with_3x3_subgrid();
        let mut left_override = WorkflowPreset::quick_select_free_mode();
        left_override.id = "left_quick".to_string();
        left_override.scope.monitor_ids = vec!["LEFT".to_string()];
        cfg.presets.push(left_override);

        let mut engine = CoreEngine::new(cfg, dual_mixed_dpi_layout());

        // PRIMARY has no override -> uses live 18x26 config.grid
        let primary_out = engine.handle_event(EngineEvent::ActivateGrid {
            monitor_id: Some("PRIMARY".to_string()),
            preset_id: None,
        });
        assert_eq!(primary_out.snapshot.grid_dimensions(), (18, 26));
        assert_eq!(
            primary_out.snapshot.active_preset_id.as_deref(),
            Some("standard")
        );

        // Switching to LEFT activates the monitor-scoped 6x4 override
        let left_out = engine.handle_event(EngineEvent::SwitchMonitor {
            switch: MonitorSwitch::Direction {
                direction: Direction::Left,
            },
        });
        assert_eq!(left_out.snapshot.active_monitor_id.as_deref(), Some("LEFT"));
        assert_eq!(
            left_out.snapshot.active_preset_id.as_deref(),
            Some("left_quick")
        );
        assert_eq!(left_out.snapshot.grid_dimensions(), (4, 6));

        // Switching back to PRIMARY returns to live 18x26 config.grid
        let back_out = engine.handle_event(EngineEvent::SwitchMonitor {
            switch: MonitorSwitch::Direction {
                direction: Direction::Right,
            },
        });
        assert_eq!(
            back_out.snapshot.active_monitor_id.as_deref(),
            Some("PRIMARY")
        );
        assert_eq!(back_out.snapshot.grid_dimensions(), (18, 26));
    }
}
