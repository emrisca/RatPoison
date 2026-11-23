//! Tauri v2 IPC DTO layer (strictly `camelCase`, matching `frontend/src/types/ipc.ts`),
//! window-relative overlay coordinate projection, and lossless config storage.

use ratpoison_core::{
    EngineMode, LogicalPoint, LogicalRect, MonitorInfo, OverlaySnapshot, PhysicalPoint, RatPoisonConfig,
    SpeedModifier, SpeedTier,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const EVENT_OVERLAY_STATE: &str = "ratpoison://overlay-state";
pub const EVENT_CONFIG_UPDATED: &str = "ratpoison://config-updated";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogicalPointDto {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogicalRectDto {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorDto {
    pub id: String,
    pub name: String,
    pub is_primary: bool,
    pub bounds: LogicalRectDto,
    pub scale_factor: f64,
}

impl From<&MonitorInfo> for MonitorDto {
    fn from(m: &MonitorInfo) -> Self {
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            is_primary: m.is_primary,
            bounds: LogicalRectDto {
                x: m.logical_rect.x,
                y: m.logical_rect.y,
                width: m.logical_rect.width,
                height: m.logical_rect.height,
            },
            scale_factor: m.scale_factor,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridCellDto {
    pub code: String,
    pub row: u16,
    pub col: u16,
    pub rect: LogicalRectDto,
    pub matched_prefix: bool,
    pub exact_match: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubgridStateDto {
    pub parent_code: String,
    pub parent_rect: LogicalRectDto,
    pub rows: u16,
    pub cols: u16,
    pub cells: Vec<GridCellDto>,
    pub nudge_step_px: f64,
    pub crosshair: LogicalPointDto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelBadgeDto {
    pub id: String,
    pub code: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub rect: LogicalRectDto,
    pub anchor: LogicalPointDto,
    pub matched_prefix: bool,
    pub exact_match: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VelocityDto {
    pub vx: f64,
    pub vy: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FreeModeStateDto {
    pub cursor: LogicalPointDto,
    pub velocity: VelocityDto,
    pub speed_profile: String,
    pub scroll_mode: bool,
    pub drag_active: bool,
    pub active_directions: Vec<String>,
    pub held_modifiers: Vec<String>,
}

/// Exact `camelCase` wire schema for `ratpoison://overlay-state`.
/// All geometry inside `gridCells`, `subgrid`, `labelBadges`, `freeMode`, and `cursorPreview`
/// is projected into logical coordinates relative to `activeMonitor.bounds.(0, 0)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayStatePayloadDto {
    pub mode: String,
    pub visible: bool,
    pub active_monitor: MonitorDto,
    pub typed_prefix: String,
    pub grid_rows: u16,
    pub grid_cols: u16,
    pub grid_cells: Vec<GridCellDto>,
    pub subgrid: Option<SubgridStateDto>,
    pub label_badges: Vec<LabelBadgeDto>,
    pub free_mode: Option<FreeModeStateDto>,
    pub cursor_preview: Option<LogicalPointDto>,
    pub active_modifiers: Vec<String>,
    pub frame_timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeybindingsDto {
    pub toggle_grid: String,
    pub toggle_label_mode: String,
    pub toggle_free_mode: String,
    pub left_click: String,
    pub right_click: String,
    pub middle_click: String,
    pub double_click: String,
    pub hold_drag: String,
    pub cycle_monitor: String,
    pub open_settings: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfoDto {
    /// Canonical OS identifier: `"macos"`, `"windows"`, or `"linux"`.
    pub os: String,
    /// CPU architecture from `std::env::consts::ARCH` (e.g. `"aarch64"`, `"x86_64"`).
    pub arch: String,
    /// Human-readable machine label (e.g. `"macOS (Apple Silicon)"`).
    pub display_name: String,
    /// Native OS-tailored default keybindings automatically chosen for this machine.
    pub default_keybindings: KeybindingsDto,
}

/// Returns the canonical OS string for the running machine.
pub fn detect_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

/// Builds conflict-free, ergonomic default keybindings tailored for the target OS.
pub fn default_keybindings_for_os(os: &str) -> KeybindingsDto {
    match os {
        "macos" => KeybindingsDto {
            toggle_grid: "Option+Space".to_string(),
            toggle_label_mode: "Option+Shift+Space".to_string(),
            toggle_free_mode: "Option+F".to_string(),
            left_click: "Space".to_string(),
            right_click: "Shift+Space".to_string(),
            middle_click: "Option+M".to_string(),
            double_click: "Enter".to_string(),
            hold_drag: "Shift".to_string(),
            cycle_monitor: "Tab".to_string(),
            open_settings: "Cmd+,".to_string(),
        },
        "windows" => KeybindingsDto {
            // Avoids Windows native `Alt+Space` system window menu & `Win+Space` layout switcher.
            toggle_grid: "Ctrl+Alt+Space".to_string(),
            toggle_label_mode: "Ctrl+Alt+Shift+Space".to_string(),
            toggle_free_mode: "Ctrl+Alt+F".to_string(),
            left_click: "Space".to_string(),
            right_click: "Shift+Space".to_string(),
            middle_click: "Alt+M".to_string(),
            double_click: "Enter".to_string(),
            hold_drag: "Shift".to_string(),
            cycle_monitor: "Tab".to_string(),
            open_settings: "Ctrl+,".to_string(),
        },
        _ => KeybindingsDto {
            // Linux: avoids GNOME/KDE `Super+Space` input switcher & `Alt+Space` window menu.
            toggle_grid: "Super+Alt+Space".to_string(),
            toggle_label_mode: "Super+Alt+Shift+Space".to_string(),
            toggle_free_mode: "Super+Alt+F".to_string(),
            left_click: "Space".to_string(),
            right_click: "Shift+Space".to_string(),
            middle_click: "Alt+M".to_string(),
            double_click: "Enter".to_string(),
            hold_drag: "Shift".to_string(),
            cycle_monitor: "Tab".to_string(),
            open_settings: "Ctrl+,".to_string(),
        },
    }
}

/// Detects the current host OS and architecture alongside its native default keybinding preset.
pub fn detect_platform_info() -> PlatformInfoDto {
    let os = detect_os();
    let arch = std::env::consts::ARCH;
    let display_name = match (os, arch) {
        ("macos", "aarch64") => "macOS (Apple Silicon)".to_string(),
        ("macos", "x86_64") => "macOS (Intel)".to_string(),
        ("macos", _) => format!("macOS ({arch})"),
        ("windows", "x86_64") => "Windows (x64)".to_string(),
        ("windows", "aarch64") => "Windows (ARM64)".to_string(),
        ("windows", _) => format!("Windows ({arch})"),
        (_, "x86_64") => "Linux (x64)".to_string(),
        (_, "aarch64") => "Linux (ARM64)".to_string(),
        (_, _) => format!("Linux ({arch})"),
    };
    PlatformInfoDto {
        os: os.to_string(),
        arch: arch.to_string(),
        display_name,
        default_keybindings: default_keybindings_for_os(os),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResultDto {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_notes: String,
    pub published_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_name: Option<String>,
}

/// Returns current UNIX epoch timestamp in seconds.
pub fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Returns current UNIX epoch timestamp in milliseconds.
pub fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Formats a UNIX epoch second timestamp as a UTC ISO-8601 / RFC-3339 string without external crates.
pub fn format_unix_iso8601(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn to_window_relative_point(pt: LogicalPoint, origin: LogicalRect) -> LogicalPointDto {
    LogicalPointDto {
        x: pt.x - origin.x,
        y: pt.y - origin.y,
    }
}

fn to_window_relative_rect(rect: LogicalRect, origin: LogicalRect) -> LogicalRectDto {
    LogicalRectDto {
        x: rect.x - origin.x,
        y: rect.y - origin.y,
        width: rect.width,
        height: rect.height,
    }
}

fn matches_prefix(code: &str, prefix: &str, case_sensitive: bool) -> (bool, bool) {
    if prefix.is_empty() {
        return (true, false);
    }
    if case_sensitive {
        (code.starts_with(prefix), code == prefix)
    } else {
        let cl = code.to_lowercase();
        let pl = prefix.to_lowercase();
        (cl.starts_with(&pl), cl == pl)
    }
}

/// Projects a `ratpoison_core::OverlaySnapshot` (global virtual-desktop coordinates, `snake_case`)
/// into the `OverlayStatePayloadDto` expected by `frontend/src/types/ipc.ts` (overlay window-relative
/// `(0, 0)` logical coordinates, `camelCase`).
pub fn project_snapshot_to_overlay_payload(
    snapshot: &OverlaySnapshot,
    config: &RatPoisonConfig,
) -> OverlayStatePayloadDto {
    let fallback_monitor = MonitorInfo::from_logical_and_scale(
        "display-1",
        "Primary Display",
        LogicalRect::new(0.0, 0.0, 1920.0, 1080.0),
        PhysicalPoint::new(0, 0),
        1.0,
        true,
    )
    .expect("fallback monitor dimensions are valid");
    let active = snapshot.active_monitor.as_ref().unwrap_or(&fallback_monitor);
    let origin = active.logical_rect;
    let case_sensitive = config.grid.label_scheme.case_sensitive;
    let prefix = snapshot.typed_prefix.as_str();

    let mode_str = match snapshot.mode {
        EngineMode::Normal => "idle",
        EngineMode::Grid => "grid",
        EngineMode::Subgrid | EngineMode::Nudge => "subgrid",
        EngineMode::LabelMode => "label",
        EngineMode::FreeMode => "free",
    };

    let grid_cells: Vec<GridCellDto> = snapshot
        .grid_cells
        .iter()
        .map(|cell| {
            let (matched_prefix, exact_match) = matches_prefix(&cell.label, prefix, case_sensitive);
            GridCellDto {
                code: cell.label.clone(),
                row: cell.row,
                col: cell.col,
                rect: to_window_relative_rect(cell.rect, origin),
                matched_prefix,
                exact_match,
            }
        })
        .collect();

    let subgrid = snapshot.subgrid_rect.map(|parent_rect| {
        let cells: Vec<GridCellDto> = snapshot
            .subgrid_cells
            .iter()
            .map(|cell| {
                let (matched_prefix, exact_match) =
                    matches_prefix(&cell.label, prefix, case_sensitive);
                GridCellDto {
                    code: cell.label.clone(),
                    row: cell.row,
                    col: cell.col,
                    rect: to_window_relative_rect(cell.rect, origin),
                    matched_prefix,
                    exact_match,
                }
            })
            .collect();

        let crosshair = snapshot
            .cursor_preview
            .map(|pt| to_window_relative_point(pt, origin))
            .unwrap_or_else(|| LogicalPointDto {
                x: (parent_rect.x - origin.x) + parent_rect.width * 0.5,
                y: (parent_rect.y - origin.y) + parent_rect.height * 0.5,
            });

        SubgridStateDto {
            parent_code: prefix.to_string(),
            parent_rect: to_window_relative_rect(parent_rect, origin),
            rows: config.grid.subgrid_rows,
            cols: config.grid.subgrid_cols,
            cells,
            nudge_step_px: config.nudge.normal_px,
            crosshair,
        }
    });

    let label_badges: Vec<LabelBadgeDto> = snapshot
        .label_badges
        .iter()
        .map(|badge| {
            let (matched_prefix, exact_match) = matches_prefix(&badge.label, prefix, false);
            let rel_rect = to_window_relative_rect(badge.rect, origin);
            LabelBadgeDto {
                id: badge.id.clone(),
                code: badge.label.clone(),
                role: badge.role.clone(),
                title: if badge.title.is_empty() {
                    None
                } else {
                    Some(badge.title.clone())
                },
                rect: rel_rect,
                anchor: LogicalPointDto {
                    x: rel_rect.x,
                    y: rel_rect.y,
                },
                matched_prefix,
                exact_match,
            }
        })
        .collect();

    let active_modifiers: Vec<String> = snapshot
        .active_modifiers
        .speed_keys_held
        .iter()
        .map(|m| match m {
            SpeedModifier::Slow => "A".to_string(),
            SpeedModifier::Normal => "S".to_string(),
            SpeedModifier::Fast => "D".to_string(),
            SpeedModifier::Turbo => "F".to_string(),
        })
        .collect();

    let free_mode = if snapshot.mode == EngineMode::FreeMode {
        let speed_profile = match snapshot.active_modifiers.speed {
            SpeedModifier::Slow => "slow",
            SpeedModifier::Fast | SpeedModifier::Turbo => "fast",
            SpeedModifier::Normal => "normal",
        };
        let cursor = snapshot
            .cursor_preview
            .map(|pt| to_window_relative_point(pt, origin))
            .unwrap_or(LogicalPointDto {
                x: origin.width * 0.5,
                y: origin.height * 0.5,
            });
        Some(FreeModeStateDto {
            cursor,
            velocity: VelocityDto { vx: 0.0, vy: 0.0 },
            speed_profile: speed_profile.to_string(),
            scroll_mode: false,
            drag_active: snapshot.active_modifiers.dragging,
            active_directions: Vec::new(),
            held_modifiers: active_modifiers.clone(),
        })
    } else {
        None
    };

    OverlayStatePayloadDto {
        mode: mode_str.to_string(),
        visible: snapshot.visible,
        active_monitor: MonitorDto::from(active),
        typed_prefix: snapshot.typed_prefix.clone(),
        grid_rows: config.grid.rows,
        grid_cols: config.grid.cols,
        grid_cells,
        subgrid,
        label_badges,
        free_mode,
        cursor_preview: snapshot
            .cursor_preview
            .map(|pt| to_window_relative_point(pt, origin)),
        active_modifiers,
        frame_timestamp_ms: unix_timestamp_ms(),
    }
}

/// Default frontend-compatible `camelCase` settings document for a specific OS.
/// Mirrors `frontend/src/config/defaults.ts` (`getDefaultConfigForOs`).
pub fn default_frontend_config_json_for_os(os: &str) -> Value {
    let kb = default_keybindings_for_os(os);
    json!({
        "general": {
            "launchAtLogin": true,
            "showTrayIcon": true,
            "multiMonitorStrategy": "active-follow",
            "hideCursorInOverlay": false,
            "autoExitAfterClick": true,
            "escapeCancelsMode": true,
            "confirmBeforeQuit": false
        },
        "grid": {
            "rows": 18,
            "cols": 26,
            "alphabet": "asdfghjklqwertyuiopzxcvbnmASDFGHJKLQWERTYUIOPZXCVBNM",
            "caseSensitive": true,
            "enableSubgridZoom": true,
            "subgridRows": 3,
            "subgridCols": 3,
            "subgridZoomScale": 1.85,
            "nudgeStepPx": 8,
            "fineNudgeStepPx": 2,
            "showSubgridCrosshair": true,
            "dimNonMatchingCells": true,
            "dimOpacity": 0.2
        },
        "freeMode": {
            "baseSpeedPxPerSec": 640,
            "fastMultiplier": 2.5,
            "slowMultiplier": 0.25,
            "accelerationCurve": 1.35,
            "scrollSpeed": 42,
            "naturalScroll": true,
            "showHudBadge": true,
            "hudPosition": "bottom-right",
            "keyUp": "I",
            "keyDown": "K",
            "keyLeft": "J",
            "keyRight": "L",
            "modFast": "F",
            "modSlow": "S",
            "modScroll": "D",
            "modDrag": "A"
        },
        "labelMode": {
            "enabled": true,
            "charset": "asdfghjklqweruiopzxcvbnm",
            "minElementSizePx": 14,
            "maxBadges": 160,
            "collisionAvoidance": true,
            "includeMenubar": true,
            "includeDock": true,
            "showElementRoleHint": true,
            "badgePlacement": "smart"
        },
        "keybindings": kb,
        "appearance": {
            "theme": "dark",
            "gridBgColor": "#0a0a0a",
            "gridBgOpacity": 0.46,
            "gridLineColor": "#44403b",
            "gridLineWidth": 1,
            "gridTextColor": "#f5f5f5",
            "gridAccentColor": "#e17100",
            "gridFontFamily": "\"Geist Mono\", \"JetBrains Mono\", \"SF Mono\", Menlo, Consolas, monospace",
            "gridFontSize": 13,
            "gridFontWeight": 600,
            "subgridAccentColor": "#e17100",
            "subgridBgColor": "#171717",
            "subgridBgOpacity": 0.9,
            "crosshairColor": "#f0b100",
            "labelBgColor": "#e17100",
            "labelBgOpacity": 0.96,
            "labelBorderColor": "#973c00",
            "labelTextColor": "#0a0a0a",
            "labelMatchPrefixColor": "#fafafa",
            "labelFontFamily": "\"Geist Mono\", \"JetBrains Mono\", \"SF Mono\", Menlo, Consolas, monospace",
            "labelFontSize": 12,
            "labelBorderRadius": 0,
            "hudBgColor": "#171717",
            "hudTextColor": "#f5f5f5",
            "hudAccentColor": "#e17100",
            "hudOpacity": 0.94,
            "reduceMotion": false
        },
        "updates": {
            "channel": "stable",
            "autoCheck": true,
            "autoDownload": false
        }
    })
}

/// Default frontend-compatible `camelCase` settings document for the detected host OS.
pub fn default_frontend_config_json() -> Value {
    default_frontend_config_json_for_os(detect_os())
}

/// Deep-merges `incoming` into `target` while preserving any unknown keys already in `target`.
pub fn deep_merge_preserving_unknowns(target: &mut Value, incoming: &Value) {
    match (target, incoming) {
        (Value::Object(t_map), Value::Object(i_map)) => {
            for (k, v) in i_map {
                if let Some(existing) = t_map.get_mut(k) {
                    deep_merge_preserving_unknowns(existing, v);
                } else {
                    t_map.insert(k.clone(), v.clone());
                }
            }
        }
        (t, i) => {
            *t = i.clone();
        }
    }
}

/// Looks up a dot-notation key (e.g. `"grid.rows"`) inside a JSON object.
pub fn get_json_path<'a>(root: &'a Value, dot_path: &str) -> Option<&'a Value> {
    let mut curr = root;
    for part in dot_path.split('.') {
        curr = curr.get(part)?;
    }
    Some(curr)
}

/// Sets a dot-notation key inside a JSON object, creating intermediate objects if needed.
pub fn set_json_path(root: &mut Value, dot_path: &str, value: Value) -> bool {
    let parts: Vec<&str> = dot_path.split('.').collect();
    if parts.is_empty() {
        return false;
    }
    let mut curr = root;
    for part in &parts[..parts.len() - 1] {
        if !curr.is_object() {
            *curr = Value::Object(Map::new());
        }
        let obj = curr.as_object_mut().expect("object guaranteed");
        curr = obj
            .entry((*part).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if let Some(obj) = curr.as_object_mut() {
        obj.insert(parts[parts.len() - 1].to_string(), value);
        true
    } else {
        false
    }
}

/// Lossless, atomic configuration store preserving unknown keys across upgrades & edits.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
    raw_json: Value,
}

impl ConfigStore {
    /// Loads existing config JSON from `path` (merging defaults for missing fields while keeping
    /// any hand-edited or future unknown fields intact) without writing to disk on boot.
    pub fn load_or_default(path: PathBuf) -> Self {
        let mut merged = default_frontend_config_json();
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<Value>(&contents) {
                deep_merge_preserving_unknowns(&mut merged, &parsed);
                let is_legacy_default_kb = get_json_path(&merged, "keybindings.toggleGrid")
                    .and_then(Value::as_str)
                    == Some("Alt+Space")
                    && get_json_path(&merged, "keybindings.toggleLabelMode")
                        .and_then(Value::as_str)
                        == Some("Alt+Shift+Space")
                    && get_json_path(&merged, "keybindings.toggleFreeMode")
                        .and_then(Value::as_str)
                        == Some("Alt+F")
                    && get_json_path(&merged, "keybindings.middleClick")
                        .and_then(Value::as_str)
                        == Some("Alt+M")
                    && get_json_path(&merged, "keybindings.openSettings")
                        .and_then(Value::as_str)
                        == Some("CmdOrCtrl+,");

                if is_legacy_default_kb {
                    if let Ok(native_kb) =
                        serde_json::to_value(default_keybindings_for_os(detect_os()))
                    {
                        set_json_path(&mut merged, "keybindings", native_kb);
                    }
                } else if get_json_path(&merged, "keybindings.openSettings")
                    .and_then(Value::as_str)
                    == Some("CmdOrCtrl+,")
                {
                    let native_open = default_keybindings_for_os(detect_os()).open_settings;
                    set_json_path(
                        &mut merged,
                        "keybindings.openSettings",
                        Value::String(native_open),
                    );
                }

                // Upgrade untouched legacy cyan/slate overlay appearance to Amber Mono 2.0
                if get_json_path(&merged, "appearance.gridBgColor").and_then(Value::as_str)
                    == Some("#0b0e14")
                    && get_json_path(&merged, "appearance.subgridAccentColor")
                        .and_then(Value::as_str)
                        == Some("#38bdf8")
                    && get_json_path(&merged, "appearance.hudAccentColor")
                        .and_then(Value::as_str)
                        == Some("#38bdf8")
                {
                    let fresh_defaults = default_frontend_config_json();
                    if let Some(fresh_app) = fresh_defaults.get("appearance").cloned() {
                        set_json_path(&mut merged, "appearance", fresh_app);
                    }
                }
            }
        }
        Self {
            path,
            raw_json: merged,
        }
    }

    pub fn json(&self) -> &Value {
        &self.raw_json
    }

    pub fn label_charset(&self) -> String {
        get_json_path(&self.raw_json, "labelMode.charset")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("asdfghjklqwertyuiopzxcvbnm")
            .to_string()
    }

    pub fn update_and_save(&mut self, incoming: Value) -> Result<Value, String> {
        deep_merge_preserving_unknowns(&mut self.raw_json, &incoming);
        self.save_atomic()?;
        Ok(self.raw_json.clone())
    }

    pub fn reset_key_and_save(&mut self, dot_key: &str) -> Result<Value, String> {
        let defaults = default_frontend_config_json();
        let def_val = get_json_path(&defaults, dot_key)
            .cloned()
            .ok_or_else(|| format!("Unknown setting key: {dot_key}"))?;
        set_json_path(&mut self.raw_json, dot_key, def_val);
        self.save_atomic()?;
        Ok(self.raw_json.clone())
    }

    pub fn save_atomic(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create config dir: {e}"))?;
        }
        let tmp_path = self.path.with_extension("json.tmp");
        let serialized = serde_json::to_string_pretty(&self.raw_json)
            .map_err(|e| format!("failed to serialize config: {e}"))?;
        fs::write(&tmp_path, serialized)
            .map_err(|e| format!("failed to write temporary config: {e}"))?;
        fs::rename(&tmp_path, &self.path)
            .map_err(|e| format!("failed to atomically replace config: {e}"))?;
        Ok(())
    }

    /// Projects the frontend camelCase settings JSON into a `ratpoison_core::RatPoisonConfig`
    /// so `CoreEngine` immediately reflects live grid/subgrid/nudge/free-mode edits.
    pub fn to_core_config(&self) -> RatPoisonConfig {
        let mut core = RatPoisonConfig::default();
        if let Some(rows) = get_json_path(&self.raw_json, "grid.rows").and_then(Value::as_u64) {
            core.grid.rows = (rows as u16).clamp(1, 52);
        }
        if let Some(cols) = get_json_path(&self.raw_json, "grid.cols").and_then(Value::as_u64) {
            core.grid.cols = (cols as u16).clamp(1, 52);
        }
        if let Some(sr) = get_json_path(&self.raw_json, "grid.subgridRows").and_then(Value::as_u64)
        {
            core.grid.subgrid_rows = (sr as u16).clamp(1, 9);
        }
        if let Some(sc) = get_json_path(&self.raw_json, "grid.subgridCols").and_then(Value::as_u64)
        {
            core.grid.subgrid_cols = (sc as u16).clamp(1, 9);
        }
        let case_sensitive = get_json_path(&self.raw_json, "grid.caseSensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        core.grid.label_scheme.case_sensitive = case_sensitive;
        if let Some(alpha) = get_json_path(&self.raw_json, "grid.alphabet").and_then(Value::as_str)
        {
            if !alpha.is_empty() {
                core.grid.label_scheme.alphabet = alpha.chars().collect();
                core.grid.label_scheme.second_alphabet = None;
            }
        }
        if let Some(step) =
            get_json_path(&self.raw_json, "grid.nudgeStepPx").and_then(Value::as_f64)
        {
            core.nudge.normal_px = step.max(1.0);
        }
        if let Some(fine) =
            get_json_path(&self.raw_json, "grid.fineNudgeStepPx").and_then(Value::as_f64)
        {
            core.nudge.fine_px = fine.max(0.5);
        }
        if let Some(base) =
            get_json_path(&self.raw_json, "freeMode.baseSpeedPxPerSec").and_then(Value::as_f64)
        {
            let normal = base.max(10.0);
            let fast_mult = get_json_path(&self.raw_json, "freeMode.fastMultiplier")
                .and_then(Value::as_f64)
                .unwrap_or(2.4)
                .max(1.0);
            let slow_mult = get_json_path(&self.raw_json, "freeMode.slowMultiplier")
                .and_then(Value::as_f64)
                .unwrap_or(0.3)
                .clamp(0.05, 1.0);
            core.free_mode.normal = SpeedTier::new(normal * 0.45, normal);
            core.free_mode.slow = SpeedTier::new(normal * slow_mult * 0.5, normal * slow_mult);
            core.free_mode.fast = SpeedTier::new(normal * 0.8, normal * fast_mult);
            core.free_mode.turbo = SpeedTier::new(normal * fast_mult, normal * fast_mult * 1.6);
        }
        if let Some(curve) =
            get_json_path(&self.raw_json, "freeMode.accelerationCurve").and_then(Value::as_f64)
        {
            core.free_mode.curve_exponent = curve.clamp(0.5, 4.0);
        }
        if let Some(scroll) =
            get_json_path(&self.raw_json, "freeMode.scrollSpeed").and_then(Value::as_f64)
        {
            core.free_mode.scroll_speed = scroll.max(1.0);
        }
        if let Some(max_b) =
            get_json_path(&self.raw_json, "labelMode.maxBadges").and_then(Value::as_u64)
        {
            core.label_mode.max_badges = max_b as usize;
        }
        core
    }
}

/// Resolves the platform-appropriate persistent config directory for RatPoison.
pub fn resolve_data_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.trim().is_empty() {
            return Path::new(&xdg).join("ratpoison");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        #[cfg(target_os = "macos")]
        {
            return Path::new(&home)
                .join("Library")
                .join("Application Support")
                .join("io.github.emrisca.RatPoison");
        }
        #[cfg(not(target_os = "macos"))]
        {
            return Path::new(&home).join(".config").join("ratpoison");
        }
    }
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        return Path::new(&appdata).join("RatPoison");
    }
    std::env::temp_dir().join("ratpoison")
}
