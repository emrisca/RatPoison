//! RatPoison Tauri v2 Desktop Application Shell.
//!
//! Wires `ratpoison-core`, `ratpoison-hid`, `ratpoison-a11y`, and `ratpoison-license`
//! to the React/TypeScript overlay & searchable settings frontend via 10 typed IPC commands.

pub mod bridge;
pub mod ipc;

use crate::bridge::AppRuntime;
use crate::ipc::{
    LicenseStatusDto, MonitorDto, OfflineChallengeDto, OverlayStatePayloadDto,
    UpdateCheckResultDto, EVENT_CONFIG_UPDATED,
};
use ratpoison_core::{EngineEvent, LogicalRect, MonitorInfo, MonitorLayout, PhysicalPoint};
use serde_json::Value;
use std::sync::Mutex;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    AppHandle, Emitter, Manager, State,
};

pub struct SharedRuntime(pub Mutex<AppRuntime>);

fn query_tauri_monitors(app: &AppHandle) -> Vec<MonitorInfo> {
    let primary_name = app
        .primary_monitor()
        .ok()
        .flatten()
        .and_then(|m| m.name().cloned());

    match app.available_monitors() {
        Ok(list) if !list.is_empty() => list
            .into_iter()
            .enumerate()
            .filter_map(|(idx, mon)| {
                let scale = mon.scale_factor().max(0.5);
                let pos = mon.position();
                let size = mon.size();
                let name = mon
                    .name()
                    .cloned()
                    .unwrap_or_else(|| format!("Display {}", idx + 1));
                let is_primary = match &primary_name {
                    Some(p) => *p == name,
                    None => idx == 0,
                };
                MonitorInfo::from_logical_and_scale(
                    format!("display-{}", idx + 1),
                    name,
                    LogicalRect::new(
                        pos.x as f64 / scale,
                        pos.y as f64 / scale,
                        size.width as f64 / scale,
                        size.height as f64 / scale,
                    ),
                    PhysicalPoint::new(pos.x, pos.y),
                    scale,
                    is_primary,
                )
                .ok()
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn open_settings_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

#[tauri::command]
fn get_config(state: State<'_, SharedRuntime>) -> Result<Value, String> {
    let rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    Ok(rt.config_store.json().clone())
}

#[tauri::command]
fn update_config(
    app: AppHandle,
    state: State<'_, SharedRuntime>,
    config: Value,
) -> Result<Value, String> {
    let mut rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    let saved = rt.config_store.update_and_save(config)?;
    let core_cfg = rt.config_store.to_core_config();
    rt.dispatch_event(
        EngineEvent::ConfigUpdated {
            config: Box::new(core_cfg),
        },
        Some(&app),
    );
    let _ = app.emit(EVENT_CONFIG_UPDATED, &saved);
    Ok(saved)
}

#[tauri::command]
fn reset_setting_key(
    app: AppHandle,
    state: State<'_, SharedRuntime>,
    key: String,
) -> Result<Value, String> {
    let mut rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    let saved = rt.config_store.reset_key_and_save(&key)?;
    let core_cfg = rt.config_store.to_core_config();
    rt.dispatch_event(
        EngineEvent::ConfigUpdated {
            config: Box::new(core_cfg),
        },
        Some(&app),
    );
    let _ = app.emit(EVENT_CONFIG_UPDATED, &saved);
    Ok(saved)
}

#[tauri::command]
fn get_monitors(app: AppHandle, state: State<'_, SharedRuntime>) -> Result<Vec<MonitorDto>, String> {
    let live = query_tauri_monitors(&app);
    let mut rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    if !live.is_empty() {
        if let Ok(layout) = MonitorLayout::new(live) {
            rt.dispatch_event(EngineEvent::MonitorsUpdated { layout }, None);
        }
    }
    Ok(rt.monitors_dto())
}

#[tauri::command]
fn get_license_status(state: State<'_, SharedRuntime>) -> Result<LicenseStatusDto, String> {
    let mut rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    rt.license_status_dto()
}

#[tauri::command]
fn activate_license(
    state: State<'_, SharedRuntime>,
    license_key: String,
) -> Result<LicenseStatusDto, String> {
    let mut rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    rt.activate_license(&license_key)
}

#[tauri::command]
fn deactivate_machine(state: State<'_, SharedRuntime>) -> Result<LicenseStatusDto, String> {
    let mut rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    rt.deactivate_current_machine()
}

#[tauri::command]
fn generate_offline_challenge(
    state: State<'_, SharedRuntime>,
) -> Result<OfflineChallengeDto, String> {
    let rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    rt.generate_offline_challenge()
}

#[tauri::command]
fn apply_offline_response(
    state: State<'_, SharedRuntime>,
    response_blob: String,
) -> Result<LicenseStatusDto, String> {
    let mut rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    rt.apply_offline_response(&response_blob)
}

#[tauri::command]
fn check_for_updates(
    state: State<'_, SharedRuntime>,
    channel: String,
) -> Result<UpdateCheckResultDto, String> {
    let rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    rt.check_for_updates(&channel)
}

/// Optional helper command allowing tray or dev harness to toggle modes cleanly.
#[tauri::command]
fn toggle_overlay_mode(
    app: AppHandle,
    state: State<'_, SharedRuntime>,
    mode: String,
) -> Result<OverlayStatePayloadDto, String> {
    let mut rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    let event = match mode.as_str() {
        "grid" => EngineEvent::ActivateGrid {
            monitor_id: None,
            preset_id: None,
        },
        "free" => EngineEvent::ActivateFreeMode { preset_id: None },
        "label" => EngineEvent::ActivateLabelMode,
        _ => EngineEvent::ExitToNormal,
    };
    Ok(rt.dispatch_event(event, Some(&app)))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Enforce anti-core-dump Memory Hygiene hardening before any window or hook starts.
    let _ = ratpoison_hid::harden_process_memory();

    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle();

            // Enforce click-through on the transparent overlay window from boot.
            if let Some(overlay) = app.get_webview_window("overlay") {
                let _ = overlay.set_ignore_cursor_events(true);
            }

            let monitors = query_tauri_monitors(handle);
            app.manage(SharedRuntime(Mutex::new(AppRuntime::initialize(monitors))));

            // Wire native system tray / menu bar menu onto `ratpoison-tray`.
            if let Some(tray) = app.tray_by_id("ratpoison-tray") {
                let grid_item = MenuItemBuilder::with_id("toggle-grid", "Toggle Grid Mode").build(app)?;
                let label_item =
                    MenuItemBuilder::with_id("toggle-label", "Toggle Label Mode").build(app)?;
                let free_item =
                    MenuItemBuilder::with_id("toggle-free", "Toggle Free Mode").build(app)?;
                let settings_item =
                    MenuItemBuilder::with_id("open-settings", "Settings…").build(app)?;
                let quit_item = MenuItemBuilder::with_id("quit", "Quit RatPoison").build(app)?;

                let menu = MenuBuilder::new(app)
                    .item(&grid_item)
                    .item(&label_item)
                    .item(&free_item)
                    .separator()
                    .item(&settings_item)
                    .separator()
                    .item(&quit_item)
                    .build()?;

                tray.set_menu(Some(menu))?;
                tray.on_menu_event(|app_handle, event| match event.id().as_ref() {
                    "toggle-grid" => {
                        if let Some(rt) = app_handle.try_state::<SharedRuntime>() {
                            if let Ok(mut guard) = rt.0.lock() {
                                guard.dispatch_event(
                                    EngineEvent::ActivateGrid {
                                        monitor_id: None,
                                        preset_id: None,
                                    },
                                    Some(app_handle),
                                );
                            }
                        }
                    }
                    "toggle-label" => {
                        if let Some(rt) = app_handle.try_state::<SharedRuntime>() {
                            if let Ok(mut guard) = rt.0.lock() {
                                guard.dispatch_event(
                                    EngineEvent::ActivateLabelMode,
                                    Some(app_handle),
                                );
                            }
                        }
                    }
                    "toggle-free" => {
                        if let Some(rt) = app_handle.try_state::<SharedRuntime>() {
                            if let Ok(mut guard) = rt.0.lock() {
                                guard.dispatch_event(
                                    EngineEvent::ActivateFreeMode { preset_id: None },
                                    Some(app_handle),
                                );
                            }
                        }
                    }
                    "open-settings" => open_settings_window(app_handle),
                    "quit" => app_handle.exit(0),
                    _ => {}
                });
            }

            // Show Settings window on first manual launch so the user sees immediate feedback.
            open_settings_window(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            update_config,
            reset_setting_key,
            get_monitors,
            get_license_status,
            activate_license,
            deactivate_machine,
            generate_offline_challenge,
            apply_offline_response,
            check_for_updates,
            toggle_overlay_mode,
        ])
        .run(tauri::generate_context!())
        .expect("error while running RatPoison Tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{project_snapshot_to_overlay_payload, ConfigStore};
    use ratpoison_core::{CoreEngine, EngineEvent, LogicalRect, MonitorInfo, MonitorLayout};
    use serde_json::json;

    #[test]
    fn overlay_payload_uses_camel_case_and_window_relative_coordinates() {
        let left_monitor = MonitorInfo::from_logical_and_scale(
            "display-left",
            "External 1080p",
            LogicalRect::new(-1920.0, -200.0, 1920.0, 1080.0),
            PhysicalPoint::new(-1920, -200),
            1.0,
            false,
        )
        .expect("valid left monitor");
        let primary = MonitorInfo::from_logical_and_scale(
            "display-1",
            "Retina Primary",
            LogicalRect::new(0.0, 0.0, 1512.0, 982.0),
            PhysicalPoint::new(0, 0),
            2.0,
            true,
        )
        .expect("valid primary monitor");

        let mut engine = CoreEngine::new(
            ratpoison_core::RatPoisonConfig::default(),
            MonitorLayout::new(vec![primary, left_monitor]).expect("valid layout"),
        );

        let step = engine.handle_event(EngineEvent::ActivateGrid {
            monitor_id: Some("display-left".to_string()),
            preset_id: None,
        });

        let payload = project_snapshot_to_overlay_payload(&step.snapshot, engine.config());
        assert_eq!(payload.mode, "grid");
        assert!(payload.visible);
        assert_eq!(payload.active_monitor.id, "display-left");
        assert!(!payload.grid_cells.is_empty());

        // First grid cell must start at overlay window-relative (0, 0), NOT (-1920, -200)
        let first = &payload.grid_cells[0];
        assert!(first.rect.x.abs() < 1e-6, "expected 0.0, got {}", first.rect.x);
        assert!(first.rect.y.abs() < 1e-6, "expected 0.0, got {}", first.rect.y);

        // Verify strict camelCase JSON keys matching frontend/src/types/ipc.ts
        let serialized = serde_json::to_value(&payload).expect("serialize payload");
        assert!(serialized.get("activeMonitor").is_some());
        assert!(serialized.get("gridCells").is_some());
        assert!(serialized.get("typedPrefix").is_some());
        assert!(serialized.get("active_monitor").is_none());
        assert!(serialized.get("grid_cells").is_none());
    }

    #[test]
    fn config_store_preserves_unknown_fields_and_resets_single_key() {
        let tmp = std::env::temp_dir().join(format!(
            "ratpoison_cfg_test_{}",
            crate::ipc::unix_timestamp_ms()
        ));
        let path = tmp.join("config.json");
        let mut store = ConfigStore::load_or_default(path.clone());

        let updated = store
            .update_and_save(json!({
                "grid": { "rows": 24 },
                "customFuturePlugin": { "enabled": true, "level": 7 }
            }))
            .expect("save config");
        assert_eq!(updated["grid"]["rows"], 24);
        assert_eq!(updated["customFuturePlugin"]["level"], 7);

        let mut reloaded = ConfigStore::load_or_default(path.clone());
        assert_eq!(reloaded.json()["grid"]["rows"], 24);
        assert_eq!(reloaded.json()["customFuturePlugin"]["enabled"], true);

        let reset = reloaded.reset_key_and_save("grid.rows").expect("reset key");
        assert_eq!(reset["grid"]["rows"], 18);
        assert_eq!(reset["customFuturePlugin"]["level"], 7);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn runtime_license_and_updater_roundtrip_without_network() {
        let mut rt = AppRuntime::initialize(Vec::new());
        let status = rt.license_status_dto().expect("trial status");
        assert_eq!(status.state, "trial");
        assert_eq!(status.trial_days_remaining, 14);
        assert_eq!(status.machines_max, 5);

        let challenge = rt
            .generate_offline_challenge()
            .expect("offline challenge");
        assert!(challenge.challenge_code.starts_with("RP-CHALLENGE-V1:"));

        let upd = rt.check_for_updates("stable").expect("update check");
        assert_eq!(upd.current_version, "0.1.0");
        assert!(!upd.update_available);
    }
}
