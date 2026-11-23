//! User configuration schema with lossless unknown-field preservation and single-key
//! `restore_default(key)` support for the Settings UI right-click menu.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::grid::{GridConfig, LabelScheme, NudgeConfig};
use crate::presets::{default_presets, FreeModeProfile, WorkflowPreset};

/// Configuration for accessibility-driven Label Mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelModeConfig {
    /// Automatically left-click when a unique accessibility badge label is matched.
    #[serde(default = "default_true")]
    pub auto_click: bool,
    /// Treat badge labels case-sensitively.
    #[serde(default)]
    pub case_sensitive: bool,
    /// Maximum number of accessibility badges rendered simultaneously.
    #[serde(default = "default_max_badges")]
    pub max_badges: usize,
    /// Return to `Normal` mode immediately after activating a matched badge.
    #[serde(default = "default_true")]
    pub exit_after_action: bool,
    /// Open Label Mode directly in Spotlight text search instead of Compact Keycaps (`/` still toggles).
    #[serde(default)]
    pub start_in_search_mode: bool,
    /// Preserves unknown/future keys during config load/save round-trips.
    #[serde(flatten, default)]
    pub unknown_fields: BTreeMap<String, serde_json::Value>,
}

fn default_true() -> bool {
    true
}
fn default_max_badges() -> usize {
    256
}

impl Default for LabelModeConfig {
    fn default() -> Self {
        Self {
            auto_click: true,
            case_sensitive: false,
            max_badges: default_max_badges(),
            exit_after_action: true,
            start_in_search_mode: false,
            unknown_fields: BTreeMap::new(),
        }
    }
}

/// Visual overlay appearance preferences shared with the Tauri frontend via `ratpoison://config-updated`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlayConfig {
    #[serde(default = "default_opacity")]
    pub opacity: f64,
    #[serde(default = "default_font_size")]
    pub font_size: f64,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_true")]
    pub show_subgrid_preview: bool,
    #[serde(default = "default_true")]
    pub show_cursor_preview: bool,
    /// Preserves unknown/future keys during config load/save round-trips.
    #[serde(flatten, default)]
    pub unknown_fields: BTreeMap<String, serde_json::Value>,
}

fn default_opacity() -> f64 {
    0.82
}
fn default_font_size() -> f64 {
    13.0
}
fn default_theme() -> String {
    "midnight".to_string()
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            opacity: default_opacity(),
            font_size: default_font_size(),
            theme: default_theme(),
            show_subgrid_preview: true,
            show_cursor_preview: true,
            unknown_fields: BTreeMap::new(),
        }
    }
}

/// Global activation hotkey bindings (stored as human-readable accelerator strings).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotkeyConfig {
    #[serde(default = "default_hk_grid")]
    pub activate_grid: String,
    #[serde(default = "default_hk_free")]
    pub activate_free_mode: String,
    #[serde(default = "default_hk_label")]
    pub activate_label_mode: String,
    #[serde(default = "default_hk_quick_select")]
    pub quick_select: String,
    /// Preserves unknown/future keys during config load/save round-trips.
    #[serde(flatten, default)]
    pub unknown_fields: BTreeMap<String, serde_json::Value>,
}

fn default_hk_grid() -> String {
    "CmdOrCtrl+Shift+Space".to_string()
}
fn default_hk_free() -> String {
    "CmdOrCtrl+Shift+F".to_string()
}
fn default_hk_label() -> String {
    "CmdOrCtrl+Shift+L".to_string()
}
fn default_hk_quick_select() -> String {
    "CmdOrCtrl+Shift+Q".to_string()
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            activate_grid: default_hk_grid(),
            activate_free_mode: default_hk_free(),
            activate_label_mode: default_hk_label(),
            quick_select: default_hk_quick_select(),
            unknown_fields: BTreeMap::new(),
        }
    }
}

/// Root RatPoison configuration persisted to JSON.
///
/// Uses `#[serde(flatten)]` on `unknown_fields` (here and on nested config sections) so
/// unknown or future keys written by newer versions, plugins, or external tools are NEVER
/// dropped across load -> modify -> save round-trips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RatPoisonConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub grid: GridConfig,
    #[serde(default)]
    pub nudge: NudgeConfig,
    #[serde(default)]
    pub free_mode: FreeModeProfile,
    #[serde(default)]
    pub label_mode: LabelModeConfig,
    #[serde(default)]
    pub overlay: OverlayConfig,
    #[serde(default)]
    pub hotkeys: HotkeyConfig,
    #[serde(default = "default_active_preset_id")]
    pub active_preset_id: String,
    #[serde(default = "default_presets")]
    pub presets: Vec<WorkflowPreset>,
    /// Preserves unknown/future top-level keys during load/save round-trips.
    #[serde(flatten, default)]
    pub unknown_fields: BTreeMap<String, serde_json::Value>,
}

fn default_schema_version() -> u32 {
    1
}
fn default_active_preset_id() -> String {
    "standard".to_string()
}

impl Default for RatPoisonConfig {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            grid: GridConfig::default(),
            nudge: NudgeConfig::default(),
            free_mode: FreeModeProfile::default(),
            label_mode: LabelModeConfig::default(),
            overlay: OverlayConfig::default(),
            hotkeys: HotkeyConfig::default(),
            active_preset_id: default_active_preset_id(),
            presets: default_presets(),
            unknown_fields: BTreeMap::new(),
        }
    }
}

/// Canonical list of dot-notation keys supported by [`RatPoisonConfig::restore_default`].
pub const KNOWN_SETTING_KEYS: &[&str] = &[
    "schema_version",
    "grid",
    "grid.cols",
    "grid.rows",
    "grid.label_length",
    "grid.label_scheme",
    "grid.label_scheme.alphabet",
    "grid.label_scheme.second_alphabet",
    "grid.label_scheme.case_sensitive",
    "grid.label_scheme.key_rows",
    "grid.label_scheme.ordering",
    "grid.subgrid_cols",
    "grid.subgrid_rows",
    "grid.subgrid_keys",
    "nudge",
    "nudge.coarse_px",
    "nudge.normal_px",
    "nudge.fine_px",
    "nudge.clamp_mode",
    "free_mode",
    "free_mode.slow",
    "free_mode.normal",
    "free_mode.fast",
    "free_mode.turbo",
    "free_mode.ramp_ms",
    "free_mode.curve_exponent",
    "free_mode.scroll_speed",
    "free_mode.normalize_diagonal",
    "free_mode.allow_cross_monitor",
    "label_mode",
    "label_mode.auto_click",
    "label_mode.case_sensitive",
    "label_mode.max_badges",
    "label_mode.exit_after_action",
    "overlay",
    "overlay.opacity",
    "overlay.font_size",
    "overlay.theme",
    "overlay.show_subgrid_preview",
    "overlay.show_cursor_preview",
    "hotkeys",
    "hotkeys.activate_grid",
    "hotkeys.activate_free_mode",
    "hotkeys.activate_label_mode",
    "hotkeys.quick_select",
    "active_preset_id",
    "presets",
];

impl RatPoisonConfig {
    /// Deserialize a `RatPoisonConfig` from JSON while preserving all unknown keys.
    pub fn from_json_str(json: &str) -> Result<Self, CoreError> {
        serde_json::from_str(json).map_err(|e| CoreError::Serialization(e.to_string()))
    }

    /// Serialize this `RatPoisonConfig` to pretty-printed JSON including preserved `unknown_fields`.
    pub fn to_json_string_pretty(&self) -> Result<String, CoreError> {
        serde_json::to_string_pretty(self).map_err(|e| CoreError::Serialization(e.to_string()))
    }

    /// Return all dot-notation setting keys accepted by [`Self::restore_default`].
    pub const fn known_setting_keys() -> &'static [&'static str] {
        KNOWN_SETTING_KEYS
    }

    /// Restore a single setting (or entire section) to its built-in default value without
    /// altering any sibling setting or dropping any preserved `unknown_fields`.
    pub fn restore_default(&mut self, key: &str) -> Result<(), CoreError> {
        let defaults = Self::default();
        match key {
            "schema_version" => self.schema_version = defaults.schema_version,

            "grid" => {
                let preserved = std::mem::take(&mut self.grid.unknown_fields);
                let preserved_scheme = std::mem::take(&mut self.grid.label_scheme.unknown_fields);
                self.grid = defaults.grid;
                self.grid.unknown_fields = preserved;
                self.grid.label_scheme.unknown_fields = preserved_scheme;
            }
            "grid.cols" => self.grid.cols = defaults.grid.cols,
            "grid.rows" => self.grid.rows = defaults.grid.rows,
            "grid.label_length" => self.grid.label_length = defaults.grid.label_length,
            "grid.label_scheme" => {
                let preserved = std::mem::take(&mut self.grid.label_scheme.unknown_fields);
                self.grid.label_scheme = LabelScheme::default();
                self.grid.label_scheme.unknown_fields = preserved;
            }
            "grid.label_scheme.alphabet" => {
                self.grid.label_scheme.alphabet = defaults.grid.label_scheme.alphabet;
            }
            "grid.label_scheme.second_alphabet" => {
                self.grid.label_scheme.second_alphabet = defaults.grid.label_scheme.second_alphabet;
            }
            "grid.label_scheme.case_sensitive" => {
                self.grid.label_scheme.case_sensitive = defaults.grid.label_scheme.case_sensitive;
            }
            "grid.label_scheme.key_rows" => {
                self.grid.label_scheme.key_rows = defaults.grid.label_scheme.key_rows;
            }
            "grid.label_scheme.ordering" => {
                self.grid.label_scheme.ordering = defaults.grid.label_scheme.ordering;
            }
            "grid.subgrid_cols" => self.grid.subgrid_cols = defaults.grid.subgrid_cols,
            "grid.subgrid_rows" => self.grid.subgrid_rows = defaults.grid.subgrid_rows,
            "grid.subgrid_keys" => self.grid.subgrid_keys = defaults.grid.subgrid_keys,

            "nudge" => {
                let preserved = std::mem::take(&mut self.nudge.unknown_fields);
                self.nudge = defaults.nudge;
                self.nudge.unknown_fields = preserved;
            }
            "nudge.coarse_px" => self.nudge.coarse_px = defaults.nudge.coarse_px,
            "nudge.normal_px" => self.nudge.normal_px = defaults.nudge.normal_px,
            "nudge.fine_px" => self.nudge.fine_px = defaults.nudge.fine_px,
            "nudge.clamp_mode" => self.nudge.clamp_mode = defaults.nudge.clamp_mode,

            "free_mode" => {
                let preserved = std::mem::take(&mut self.free_mode.unknown_fields);
                self.free_mode = defaults.free_mode;
                self.free_mode.unknown_fields = preserved;
            }
            "free_mode.slow" => self.free_mode.slow = defaults.free_mode.slow,
            "free_mode.normal" => self.free_mode.normal = defaults.free_mode.normal,
            "free_mode.fast" => self.free_mode.fast = defaults.free_mode.fast,
            "free_mode.turbo" => self.free_mode.turbo = defaults.free_mode.turbo,
            "free_mode.ramp_ms" => self.free_mode.ramp_ms = defaults.free_mode.ramp_ms,
            "free_mode.curve_exponent" => {
                self.free_mode.curve_exponent = defaults.free_mode.curve_exponent;
            }
            "free_mode.scroll_speed" => {
                self.free_mode.scroll_speed = defaults.free_mode.scroll_speed;
            }
            "free_mode.normalize_diagonal" => {
                self.free_mode.normalize_diagonal = defaults.free_mode.normalize_diagonal;
            }
            "free_mode.allow_cross_monitor" => {
                self.free_mode.allow_cross_monitor = defaults.free_mode.allow_cross_monitor;
            }

            "label_mode" => {
                let preserved = std::mem::take(&mut self.label_mode.unknown_fields);
                self.label_mode = defaults.label_mode;
                self.label_mode.unknown_fields = preserved;
            }
            "label_mode.auto_click" => self.label_mode.auto_click = defaults.label_mode.auto_click,
            "label_mode.case_sensitive" => {
                self.label_mode.case_sensitive = defaults.label_mode.case_sensitive;
            }
            "label_mode.max_badges" => self.label_mode.max_badges = defaults.label_mode.max_badges,
            "label_mode.exit_after_action" => {
                self.label_mode.exit_after_action = defaults.label_mode.exit_after_action;
            }

            "overlay" => {
                let preserved = std::mem::take(&mut self.overlay.unknown_fields);
                self.overlay = defaults.overlay;
                self.overlay.unknown_fields = preserved;
            }
            "overlay.opacity" => self.overlay.opacity = defaults.overlay.opacity,
            "overlay.font_size" => self.overlay.font_size = defaults.overlay.font_size,
            "overlay.theme" => self.overlay.theme = defaults.overlay.theme,
            "overlay.show_subgrid_preview" => {
                self.overlay.show_subgrid_preview = defaults.overlay.show_subgrid_preview;
            }
            "overlay.show_cursor_preview" => {
                self.overlay.show_cursor_preview = defaults.overlay.show_cursor_preview;
            }

            "hotkeys" => {
                let preserved = std::mem::take(&mut self.hotkeys.unknown_fields);
                self.hotkeys = defaults.hotkeys;
                self.hotkeys.unknown_fields = preserved;
            }
            "hotkeys.activate_grid" => {
                self.hotkeys.activate_grid = defaults.hotkeys.activate_grid;
            }
            "hotkeys.activate_free_mode" => {
                self.hotkeys.activate_free_mode = defaults.hotkeys.activate_free_mode;
            }
            "hotkeys.activate_label_mode" => {
                self.hotkeys.activate_label_mode = defaults.hotkeys.activate_label_mode;
            }
            "hotkeys.quick_select" => {
                self.hotkeys.quick_select = defaults.hotkeys.quick_select;
            }

            "active_preset_id" => self.active_preset_id = defaults.active_preset_id,
            "presets" => self.presets = defaults.presets,

            _ => return Err(CoreError::UnknownSettingKey(key.to_string())),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_unknown_top_level_and_nested_fields_across_roundtrip() {
        let raw = r#"{
            "schema_version": 1,
            "future_flag": true,
            "experimental_plugin": { "enabled": 42, "tags": ["a", "b"] },
            "grid": {
                "cols": 20,
                "future_grid_shader": "crt-scanline"
            },
            "overlay": {
                "opacity": 0.5,
                "custom_blur_radius": 12
            }
        }"#;

        let mut cfg = RatPoisonConfig::from_json_str(raw).unwrap();
        assert_eq!(cfg.grid.cols, 20);
        assert_eq!(cfg.grid.rows, 12); // default filled in
        assert_eq!(
            cfg.unknown_fields.get("future_flag"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            cfg.grid.unknown_fields.get("future_grid_shader"),
            Some(&serde_json::json!("crt-scanline"))
        );

        // Mutate a known field and restore another
        cfg.restore_default("grid.cols").unwrap();
        assert_eq!(cfg.grid.cols, 16);

        // Round-trip serialize and inspect JSON
        let saved = cfg.to_json_string_pretty().unwrap();
        let val: serde_json::Value = serde_json::from_str(&saved).unwrap();
        assert_eq!(val["future_flag"], serde_json::json!(true));
        assert_eq!(val["experimental_plugin"]["enabled"], serde_json::json!(42));
        assert_eq!(
            val["grid"]["future_grid_shader"],
            serde_json::json!("crt-scanline")
        );
        assert_eq!(val["overlay"]["custom_blur_radius"], serde_json::json!(12));
        assert_eq!(val["grid"]["cols"], serde_json::json!(16));
        assert_eq!(val["overlay"]["opacity"], serde_json::json!(0.5));
    }

    #[test]
    fn restore_default_supports_every_listed_key_and_rejects_unknown() {
        let mut cfg = RatPoisonConfig::default();
        cfg.grid.cols = 30;
        cfg.grid.rows = 20;
        cfg.nudge.fine_px = 9.0;
        cfg.overlay.opacity = 0.2;
        cfg.unknown_fields
            .insert("keep_me".to_string(), serde_json::json!("safe"));

        // Restoring grid.cols leaves grid.rows untouched
        cfg.restore_default("grid.cols").unwrap();
        assert_eq!(cfg.grid.cols, 16);
        assert_eq!(cfg.grid.rows, 20);

        // Every documented key in KNOWN_SETTING_KEYS succeeds
        for &key in RatPoisonConfig::known_setting_keys() {
            cfg.restore_default(key)
                .unwrap_or_else(|e| panic!("key '{key}' failed restore_default: {e}"));
        }

        // Unknown top-level field survived all restores
        assert_eq!(
            cfg.unknown_fields.get("keep_me"),
            Some(&serde_json::json!("safe"))
        );

        // Unrecognized key returns explicit error
        assert_eq!(
            cfg.restore_default("nonexistent.key"),
            Err(CoreError::UnknownSettingKey("nonexistent.key".to_string()))
        );
    }
}
