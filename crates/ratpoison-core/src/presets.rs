//! Quick Select + Free Mode preset workflow, Free Mode velocity/acceleration curves,
//! and deterministic per-app / per-monitor preset override resolution.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::geometry::{MonitorInfo, Orientation};
use crate::grid::{GridConfig, LabelScheme, NudgeConfig};

/// Logical mouse button identifier emitted in `EngineEffect`s and preset actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

/// Speed modifier tiers for overlay-free Free Mode (`A`, `S`, `D`, `F`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedModifier {
    /// `A` — Precision / slow movement.
    Slow,
    /// `S` — Normal / default movement.
    #[default]
    Normal,
    /// `D` — Fast traversal.
    Fast,
    /// `F` — Turbo / cross-monitor flick.
    Turbo,
}

/// Velocity ramp parameters for one Free Mode speed tier.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpeedTier {
    /// Initial speed (logical px/s) immediately upon pressing a directional key.
    pub base_px_per_sec: f64,
    /// Maximum cruising speed (logical px/s) reached after `ramp_ms`.
    pub max_px_per_sec: f64,
}

impl SpeedTier {
    pub const fn new(base_px_per_sec: f64, max_px_per_sec: f64) -> Self {
        Self {
            base_px_per_sec,
            max_px_per_sec,
        }
    }

    /// Deterministic speed at `hold_ms` along a power ease-in curve:
    /// `v(t) = base + (max - base) * clamp(t / ramp_ms, 0, 1)^curve_exponent`.
    pub fn velocity_at(self, hold_ms: u32, ramp_ms: u32, curve_exponent: f64) -> f64 {
        let base = self.base_px_per_sec.max(0.0);
        let max = self.max_px_per_sec.max(base);
        if ramp_ms == 0 {
            return max;
        }
        let u = (hold_ms as f64 / ramp_ms as f64).clamp(0.0, 1.0);
        let p = if curve_exponent.is_finite() && curve_exponent > 0.0 {
            curve_exponent
        } else {
            1.0
        };
        base + (max - base) * u.powf(p)
    }
}

/// Complete Free Mode physics & scrolling profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FreeModeProfile {
    #[serde(default = "default_slow_tier")]
    pub slow: SpeedTier,
    #[serde(default = "default_normal_tier")]
    pub normal: SpeedTier,
    #[serde(default = "default_fast_tier")]
    pub fast: SpeedTier,
    #[serde(default = "default_turbo_tier")]
    pub turbo: SpeedTier,
    /// Acceleration duration in milliseconds from `base_px_per_sec` to `max_px_per_sec`.
    #[serde(default = "default_ramp_ms")]
    pub ramp_ms: u32,
    /// Power curve exponent (`> 1.0` eases in gently for micro-adjustments).
    #[serde(default = "default_curve_exponent")]
    pub curve_exponent: f64,
    /// Base scroll rate in wheel ticks/logical scroll units per second.
    #[serde(default = "default_scroll_speed")]
    pub scroll_speed: f64,
    /// Normalize diagonal velocity by `1 / sqrt(2)` so diagonal speed matches cardinal speed.
    #[serde(default = "default_true")]
    pub normalize_diagonal: bool,
    /// Allow cursor to cross monitor boundaries smoothly in Free Mode.
    #[serde(default = "default_true")]
    pub allow_cross_monitor: bool,
    /// Preserves unknown/future keys during config load/save round-trips.
    #[serde(flatten, default)]
    pub unknown_fields: BTreeMap<String, serde_json::Value>,
}

fn default_slow_tier() -> SpeedTier {
    SpeedTier::new(60.0, 140.0)
}
fn default_normal_tier() -> SpeedTier {
    SpeedTier::new(240.0, 650.0)
}
fn default_fast_tier() -> SpeedTier {
    SpeedTier::new(550.0, 1400.0)
}
fn default_turbo_tier() -> SpeedTier {
    SpeedTier::new(1100.0, 2600.0)
}
fn default_ramp_ms() -> u32 {
    450
}
fn default_curve_exponent() -> f64 {
    1.6
}
fn default_scroll_speed() -> f64 {
    18.0
}
fn default_true() -> bool {
    true
}

impl Default for FreeModeProfile {
    fn default() -> Self {
        Self {
            slow: default_slow_tier(),
            normal: default_normal_tier(),
            fast: default_fast_tier(),
            turbo: default_turbo_tier(),
            ramp_ms: default_ramp_ms(),
            curve_exponent: default_curve_exponent(),
            scroll_speed: default_scroll_speed(),
            normalize_diagonal: true,
            allow_cross_monitor: true,
            unknown_fields: BTreeMap::new(),
        }
    }
}

impl FreeModeProfile {
    /// Look up the `SpeedTier` corresponding to `modifier`.
    pub const fn tier(&self, modifier: SpeedModifier) -> SpeedTier {
        match modifier {
            SpeedModifier::Slow => self.slow,
            SpeedModifier::Normal => self.normal,
            SpeedModifier::Fast => self.fast,
            SpeedModifier::Turbo => self.turbo,
        }
    }

    /// Compute cursor speed (logical px/s) for a given modifier and hold duration.
    pub fn velocity_px_per_sec(&self, modifier: SpeedModifier, hold_ms: u32) -> f64 {
        self.tier(modifier)
            .velocity_at(hold_ms, self.ramp_ms, self.curve_exponent)
    }

    /// Compute scroll delta for `dt_ms`, scaled gently by the active speed modifier.
    pub fn scroll_delta_for_tick(&self, modifier: SpeedModifier, dt_ms: u32) -> f64 {
        let mult = match modifier {
            SpeedModifier::Slow => 0.35,
            SpeedModifier::Normal => 1.0,
            SpeedModifier::Fast => 2.25,
            SpeedModifier::Turbo => 4.0,
        };
        self.scroll_speed.max(0.0) * mult * (dt_ms as f64 / 1000.0)
    }
}

/// What happens immediately after selecting a primary grid cell under a preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuickSelectAction {
    /// Standard flow: subdivide the chosen cell into a nested `Subgrid` (then `Nudge`).
    #[default]
    EnterSubgrid,
    /// Quick Select + Free Mode: jump immediately to the coarse zone center, hide the overlay,
    /// and hand off straight into `FreeMode` for fluid micro-positioning.
    HandoffToFreeMode,
    /// Two-stage Quick Select: subdivide once into `Subgrid`, then upon subgrid pick immediately
    /// hide the overlay and hand off into `FreeMode`.
    SubgridThenFreeMode,
    /// Jump to cell center and enter `Nudge` mode directly (skipping subgrid).
    EnterNudge,
    /// Jump to cell center, perform a click, and return to `Normal`.
    ClickAndExit { button: MouseButton },
    /// Jump to cell center and return to `Normal`.
    MoveAndExit,
}

/// Optional app / monitor scope for automatic preset override matching.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PresetOverrideScope {
    /// Application identifiers (bundle IDs, executable names, or WM_CLASS; matched case-insensitively).
    #[serde(default)]
    pub app_ids: Vec<String>,
    /// Monitor identifiers (exact match against `MonitorInfo.id`).
    #[serde(default)]
    pub monitor_ids: Vec<String>,
    /// Optional monitor orientation constraint (e.g., portrait side monitor).
    #[serde(default)]
    pub orientation: Option<Orientation>,
    /// Preserves unknown/future keys during config load/save round-trips.
    #[serde(flatten, default)]
    pub unknown_fields: BTreeMap<String, serde_json::Value>,
}

impl PresetOverrideScope {
    pub fn is_unscoped(&self) -> bool {
        self.app_ids.is_empty() && self.monitor_ids.is_empty() && self.orientation.is_none()
    }

    /// Evaluate whether this scope matches the current `(app_id, monitor)` context.
    ///
    /// Returns `Some(specificity)` when all configured constraints match, or `None` otherwise:
    /// - `+4` if `app_ids` matched
    /// - `+2` if `monitor_ids` matched
    /// - `+1` if `orientation` matched
    ///
    /// An unscoped override returns `Some(0)`.
    pub fn match_specificity(
        &self,
        app_id: Option<&str>,
        monitor: Option<&MonitorInfo>,
    ) -> Option<u8> {
        let mut specificity = 0u8;

        if !self.app_ids.is_empty() {
            let active_app = app_id?;
            let matched = self
                .app_ids
                .iter()
                .any(|a| a.eq_ignore_ascii_case(active_app));
            if !matched {
                return None;
            }
            specificity += 4;
        }

        if !self.monitor_ids.is_empty() {
            let active_mon = monitor?;
            if !self.monitor_ids.iter().any(|m| m == &active_mon.id) {
                return None;
            }
            specificity += 2;
        }

        if let Some(req_orient) = self.orientation {
            let active_mon = monitor?;
            if active_mon.orientation() != req_orient {
                return None;
            }
            specificity += 1;
        }

        Some(specificity)
    }
}

/// A named workflow preset unifying Grid geometry, Quick Select handoff behavior,
/// Free Mode velocity curves, Nudge distances, and optional app/monitor overrides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowPreset {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub grid: GridConfig,
    #[serde(default)]
    pub quick_select_action: QuickSelectAction,
    #[serde(default)]
    pub free_mode: FreeModeProfile,
    #[serde(default)]
    pub nudge: NudgeConfig,
    /// Higher priority breaks ties among equally-specific overrides.
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub scope: PresetOverrideScope,
    /// Preserves unknown/future keys during config load/save round-trips.
    #[serde(flatten, default)]
    pub unknown_fields: BTreeMap<String, serde_json::Value>,
}

impl WorkflowPreset {
    /// Default full-precision Grid -> Subgrid -> Nudge preset.
    pub fn standard_grid() -> Self {
        Self {
            id: "standard".to_string(),
            name: "Standard Grid".to_string(),
            description: "16x12 two-key grid with 3x3 subgrid and precision nudge".to_string(),
            grid: GridConfig::default(),
            quick_select_action: QuickSelectAction::EnterSubgrid,
            free_mode: FreeModeProfile::default(),
            nudge: NudgeConfig::default(),
            priority: 0,
            scope: PresetOverrideScope::default(),
            unknown_fields: BTreeMap::new(),
        }
    }

    /// Quick Select + Free Mode preset: single-key 6x4 coarse zone jump (24 keys) followed
    /// immediately by overlay-free Free Mode for fluid micro-positioning.
    pub fn quick_select_free_mode() -> Self {
        let scheme = LabelScheme::from_key_rows(&["qwertya", "sdfghjk", "zxcvbnm", "uiop"], false)
            .unwrap_or_else(|_| LabelScheme::lowercase_az());
        Self {
            id: "quick_select_free".to_string(),
            name: "Quick Select + Free Mode".to_string(),
            description: "Rapid 6x4 single-key zone jump with immediate handoff into Free Mode"
                .to_string(),
            grid: GridConfig {
                cols: 6,
                rows: 4,
                label_length: 1,
                label_scheme: scheme,
                subgrid_cols: 3,
                subgrid_rows: 3,
                ..GridConfig::default()
            },
            quick_select_action: QuickSelectAction::HandoffToFreeMode,
            free_mode: FreeModeProfile {
                slow: SpeedTier::new(45.0, 110.0),
                normal: SpeedTier::new(210.0, 560.0),
                fast: SpeedTier::new(500.0, 1300.0),
                turbo: SpeedTier::new(1050.0, 2500.0),
                ramp_ms: 400,
                curve_exponent: 1.5,
                ..FreeModeProfile::default()
            },
            nudge: NudgeConfig::default(),
            priority: 0,
            scope: PresetOverrideScope::default(),
            unknown_fields: BTreeMap::new(),
        }
    }

    /// Precision design preset: Grid -> Subgrid -> immediate Free Mode with fine low-speed curves.
    pub fn precision_design() -> Self {
        Self {
            id: "precision_design".to_string(),
            name: "Precision Design".to_string(),
            description: "Grid + subgrid jump with fine-tuned low-speed Free Mode handoff"
                .to_string(),
            grid: GridConfig {
                cols: 16,
                rows: 10,
                ..GridConfig::default()
            },
            quick_select_action: QuickSelectAction::SubgridThenFreeMode,
            free_mode: FreeModeProfile {
                slow: SpeedTier::new(25.0, 70.0),
                normal: SpeedTier::new(140.0, 380.0),
                fast: SpeedTier::new(400.0, 1000.0),
                turbo: SpeedTier::new(900.0, 2000.0),
                ramp_ms: 550,
                curve_exponent: 1.8,
                ..FreeModeProfile::default()
            },
            nudge: NudgeConfig {
                coarse_px: 12.0,
                normal_px: 4.0,
                fine_px: 0.5,
                ..NudgeConfig::default()
            },
            priority: 0,
            scope: PresetOverrideScope::default(),
            unknown_fields: BTreeMap::new(),
        }
    }
}

/// Default preset list shipped in `RatPoisonConfig`.
pub fn default_presets() -> Vec<WorkflowPreset> {
    vec![
        WorkflowPreset::standard_grid(),
        WorkflowPreset::quick_select_free_mode(),
        WorkflowPreset::precision_design(),
    ]
}

/// Deterministically resolve the effective `WorkflowPreset` for the current activation context.
///
/// Precedence order:
/// 1. Explicit `explicit_preset_id` (e.g., user pressed a dedicated Quick Select hotkey).
/// 2. Most specific matching scoped preset (`app + monitor + orientation > app + monitor > app > monitor > orientation`),
///    tie-broken by `priority` descending, then `id` ascending.
/// 3. User's `active_preset_id`.
/// 4. First preset in `presets`.
pub fn resolve_preset<'a>(
    presets: &'a [WorkflowPreset],
    explicit_preset_id: Option<&str>,
    active_preset_id: &str,
    app_id: Option<&str>,
    monitor: Option<&MonitorInfo>,
) -> Option<&'a WorkflowPreset> {
    if let Some(req_id) = explicit_preset_id {
        if let Some(found) = presets.iter().find(|p| p.id == req_id) {
            return Some(found);
        }
    }

    let mut best_scoped: Option<(&WorkflowPreset, u8)> = None;
    for preset in presets {
        if preset.scope.is_unscoped() {
            continue;
        }
        if let Some(spec) = preset.scope.match_specificity(app_id, monitor) {
            if spec == 0 {
                continue;
            }
            let better = match best_scoped {
                None => true,
                Some((best, best_spec)) => {
                    (spec, preset.priority, std::cmp::Reverse(preset.id.as_str()))
                        > (best_spec, best.priority, std::cmp::Reverse(best.id.as_str()))
                }
            };
            if better {
                best_scoped = Some((preset, spec));
            }
        }
    }

    if let Some((preset, _)) = best_scoped {
        return Some(preset);
    }

    presets
        .iter()
        .find(|p| p.id == active_preset_id)
        .or_else(|| presets.first())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{LogicalRect, PhysicalPoint};

    #[test]
    fn speed_tier_acceleration_curve_is_monotonic_and_bounded() {
        let tier = SpeedTier::new(100.0, 500.0);
        let v0 = tier.velocity_at(0, 400, 1.6);
        let v200 = tier.velocity_at(200, 400, 1.6);
        let v400 = tier.velocity_at(400, 400, 1.6);
        let v1000 = tier.velocity_at(1000, 400, 1.6);

        assert!((v0 - 100.0).abs() < 1e-9);
        assert!(v200 > v0 && v200 < v400);
        // Ease-in (p = 1.6 > 1.0) means midpoint speed is strictly below linear midpoint 300.0
        assert!(v200 < 300.0);
        assert!((v400 - 500.0).abs() < 1e-9);
        assert!((v1000 - 500.0).abs() < 1e-9);
    }

    #[test]
    fn preset_resolution_prefers_specific_app_monitor_override() {
        let mon_portrait = MonitorInfo::from_logical_and_scale(
            "DP-PORTRAIT",
            "Portrait",
            LogicalRect::new(1440.0, 0.0, 1080.0, 1920.0),
            PhysicalPoint::new(2880, 0),
            1.0,
            false,
        )
        .unwrap();

        let mut app_override = WorkflowPreset::precision_design();
        app_override.id = "figma_override".to_string();
        app_override.scope.app_ids = vec!["com.figma.Desktop".to_string()];

        let mut app_mon_override = WorkflowPreset::quick_select_free_mode();
        app_mon_override.id = "figma_portrait".to_string();
        app_mon_override.scope.app_ids = vec!["com.figma.Desktop".to_string()];
        app_mon_override.scope.monitor_ids = vec!["DP-PORTRAIT".to_string()];

        let mut presets = default_presets();
        presets.push(app_override);
        presets.push(app_mon_override);

        // App + monitor matches figma_portrait (specificity 6 > 4)
        let resolved = resolve_preset(
            &presets,
            None,
            "standard",
            Some("COM.FIGMA.DESKTOP"),
            Some(&mon_portrait),
        )
        .unwrap();
        assert_eq!(resolved.id, "figma_portrait");

        // Explicit preset id overrides scoped lookup
        let explicit = resolve_preset(
            &presets,
            Some("standard"),
            "quick_select_free",
            Some("com.figma.Desktop"),
            Some(&mon_portrait),
        )
        .unwrap();
        assert_eq!(explicit.id, "standard");

        // Unknown app falls back to active_preset_id
        let fallback = resolve_preset(
            &presets,
            None,
            "quick_select_free",
            Some("com.apple.Terminal"),
            None,
        )
        .unwrap();
        assert_eq!(fallback.id, "quick_select_free");
    }
}
