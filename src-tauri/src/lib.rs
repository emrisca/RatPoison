//! RatPoison Tauri v2 Desktop Runtime
//!
//! Wires `ratpoison-core`, `ratpoison-hid`, and `ratpoison-a11y`
//! to the React/TypeScript overlay & searchable settings frontend via typed IPC commands
//! and a live cross-platform global keyboard hook.

pub mod bridge;
pub mod hotkeys;
pub mod ipc;
pub mod updater;

use crate::bridge::AppRuntime;
use crate::hotkeys::{HookAction, HookRouteConfig, HookRouter};
use crate::ipc::{
    MonitorDto, OverlayStatePayloadDto, PermissionStatusDto, PlatformInfoDto, UpdateCheckResultDto,
    EVENT_CONFIG_UPDATED,
};
use ratpoison_core::{EngineEvent, EngineMode, LogicalRect, MonitorInfo, MonitorLayout, PhysicalPoint};
use ratpoison_hid::{
    check_platform_permissions, create_platform_keyboard_hook, request_platform_permissions,
    KeyboardHookBackend,
};
use serde_json::Value;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    AppHandle, Emitter, Manager, State,
};

pub struct SharedRuntime(pub Arc<Mutex<AppRuntime>>);

pub struct HookController {
    hook: Box<dyn KeyboardHookBackend>,
    router: Arc<Mutex<HookRouter>>,
    action_tx: mpsc::Sender<HookAction>,
    last_error: Option<String>,
}

impl HookController {
    pub fn new(router: Arc<Mutex<HookRouter>>, action_tx: mpsc::Sender<HookAction>) -> Self {
        Self {
            hook: create_platform_keyboard_hook(),
            router,
            action_tx,
            last_error: None,
        }
    }

    /// Attempts to install and start the OS-level global keyboard hook if not already active.
    pub fn ensure_started(&mut self) -> bool {
        if self.hook.is_running() {
            self.last_error = None;
            return true;
        }
        let router = Arc::clone(&self.router);
        let tx = self.action_tx.clone();
        match self.hook.start(Box::new(move |raw_ev| {
            let Ok(mut guard) = router.lock() else {
                return ratpoison_hid::InterceptDecision::PassThrough;
            };
            let (decision, actions) = guard.route_event(raw_ev);
            drop(guard);
            for action in actions {
                let _ = tx.send(action);
            }
            decision
        })) {
            Ok(()) => {
                self.last_error = None;
                true
            }
            Err(err) => {
                self.last_error = Some(err.to_string());
                false
            }
        }
    }

    pub fn update_config(&self, cfg_json: &Value) {
        if let Ok(mut guard) = self.router.lock() {
            guard.update_config(HookRouteConfig::from_config_json(cfg_json));
        }
    }

    pub fn sync_engine_state(&self, mode: EngineMode, dragging: bool) {
        if let Ok(mut guard) = self.router.lock() {
            guard.sync_engine_state(mode, dragging);
        }
    }

    pub fn status_dto(&self) -> PermissionStatusDto {
        let report = check_platform_permissions();
        PermissionStatusDto {
            platform: report.platform.to_string(),
            accessibility_granted: report.accessibility_granted,
            input_monitoring_granted: report.input_monitoring_granted,
            secure_input_active: report.secure_input_active,
            hook_active: self.hook.is_running(),
            hook_backend: self.hook.backend_name().to_string(),
            guidance: report.guidance,
            last_error: self.last_error.clone(),
        }
    }
}

pub struct SharedHook(pub Arc<Mutex<HookController>>);

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
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = handle.get_webview_window("settings") {
            let _ = win.show();
            let _ = win.unminimize();
            let _ = win.set_focus();
        }
    });
}

fn spawn_hook_worker(
    app_handle: AppHandle,
    runtime: Arc<Mutex<AppRuntime>>,
    router: Arc<Mutex<HookRouter>>,
    action_rx: mpsc::Receiver<HookAction>,
) {
    let _ = std::thread::Builder::new()
        .name("ratpoison-hook-worker".to_string())
        .spawn(move || loop {
            let in_free_mode = router
                .lock()
                .map(|r| r.mode() == EngineMode::FreeMode)
                .unwrap_or(false);

            let maybe_action = if in_free_mode {
                match action_rx.recv_timeout(Duration::from_millis(16)) {
                    Ok(action) => Some(action),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        Some(HookAction::Engine(EngineEvent::FreeModeTick { dt_ms: 16 }))
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match action_rx.recv() {
                    Ok(action) => Some(action),
                    Err(_) => break,
                }
            };

            let Some(action) = maybe_action else {
                continue;
            };

            match action {
                HookAction::OpenSettings => {
                    open_settings_window(&app_handle);
                }
                HookAction::ToggleGrid => {
                    if let Ok(mut rt) = runtime.lock() {
                        let event = if matches!(
                            rt.engine.mode(),
                            EngineMode::Grid | EngineMode::Subgrid | EngineMode::Nudge
                        ) {
                            EngineEvent::ExitToNormal
                        } else {
                            EngineEvent::ActivateGrid {
                                monitor_id: None,
                                preset_id: None,
                            }
                        };
                        let _ = rt.dispatch_event(event, Some(&app_handle));
                        let mode = rt.engine.mode();
                        let dragging = rt.engine.snapshot().active_modifiers.dragging;
                        drop(rt);
                        if let Ok(mut r) = router.lock() {
                            r.sync_engine_state(mode, dragging);
                        }
                    }
                }
                HookAction::ToggleLabelMode => {
                    if let Ok(mut rt) = runtime.lock() {
                        let _ = rt.dispatch_event(EngineEvent::ActivateLabelMode, Some(&app_handle));
                        let mode = rt.engine.mode();
                        let dragging = rt.engine.snapshot().active_modifiers.dragging;
                        drop(rt);
                        if let Ok(mut r) = router.lock() {
                            r.sync_engine_state(mode, dragging);
                        }
                    }
                }
                HookAction::ToggleFreeMode => {
                    if let Ok(mut rt) = runtime.lock() {
                        let _ = rt.dispatch_event(
                            EngineEvent::ActivateFreeMode { preset_id: None },
                            Some(&app_handle),
                        );
                        let mode = rt.engine.mode();
                        let dragging = rt.engine.snapshot().active_modifiers.dragging;
                        drop(rt);
                        if let Ok(mut r) = router.lock() {
                            r.sync_engine_state(mode, dragging);
                        }
                    }
                }
                HookAction::Engine(engine_event) => {
                    let trigger_immediate_step = matches!(
                        engine_event,
                        EngineEvent::FreeModeKeyDown {
                            key: ratpoison_core::FreeKey::MoveUp
                                | ratpoison_core::FreeKey::MoveDown
                                | ratpoison_core::FreeKey::MoveLeft
                                | ratpoison_core::FreeKey::MoveRight
                                | ratpoison_core::FreeKey::ScrollUp
                                | ratpoison_core::FreeKey::ScrollDown
                                | ratpoison_core::FreeKey::ScrollLeft
                                | ratpoison_core::FreeKey::ScrollRight
                        }
                    );
                    let (dirs, scroll_held) = router
                        .lock()
                        .map(|r| (r.active_directions_strings(), r.is_scroll_held()))
                        .unwrap_or_default();
                    if let Ok(mut rt) = runtime.lock() {
                        rt.active_free_directions = dirs;
                        rt.free_scroll_active = scroll_held;
                        let _ = rt.dispatch_event(engine_event, Some(&app_handle));
                        if trigger_immediate_step && rt.engine.mode() == EngineMode::FreeMode {
                            let _ = rt.dispatch_event(
                                EngineEvent::FreeModeTick { dt_ms: 16 },
                                Some(&app_handle),
                            );
                        }
                        let mode = rt.engine.mode();
                        let dragging = rt.engine.snapshot().active_modifiers.dragging;
                        drop(rt);
                        if let Ok(mut r) = router.lock() {
                            r.sync_engine_state(mode, dragging);
                        }
                    }
                }
            }
        });
}

fn spawn_permission_watcher(hook_ctrl: Arc<Mutex<HookController>>) {
    let _ = std::thread::Builder::new()
        .name("ratpoison-perm-watcher".to_string())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(1500));
            let Ok(mut ctrl) = hook_ctrl.lock() else {
                break;
            };
            if ctrl.hook.is_running() {
                // Keep monitoring at a slow cadence in case the hook ever stops.
                drop(ctrl);
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
            let report = check_platform_permissions();
            if report.accessibility_granted {
                let _ = ctrl.ensure_started();
            }
        });
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
    hook: State<'_, SharedHook>,
    config: Value,
) -> Result<Value, String> {
    let mut rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    let updated = rt.config_store.update_and_save(config)?;
    rt.sync_core_config_from_store();
    drop(rt);
    if let Ok(ctrl) = hook.0.lock() {
        ctrl.update_config(&updated);
    }
    let _ = app.emit(EVENT_CONFIG_UPDATED, &updated);
    Ok(updated)
}

#[tauri::command]
fn reset_setting_key(
    app: AppHandle,
    state: State<'_, SharedRuntime>,
    hook: State<'_, SharedHook>,
    key: String,
) -> Result<Value, String> {
    let mut rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    let updated = rt.config_store.reset_key_and_save(&key)?;
    rt.sync_core_config_from_store();
    drop(rt);
    if let Ok(ctrl) = hook.0.lock() {
        ctrl.update_config(&updated);
    }
    let _ = app.emit(EVENT_CONFIG_UPDATED, &updated);
    Ok(updated)
}

#[tauri::command]
fn get_monitors(
    app: AppHandle,
    state: State<'_, SharedRuntime>,
) -> Result<Vec<MonitorDto>, String> {
    let live = query_tauri_monitors(&app);
    let mut rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    if !live.is_empty() {
        if let Ok(layout) = MonitorLayout::new(live) {
            let _ = rt
                .engine
                .handle_event(EngineEvent::MonitorsUpdated { layout });
        }
    }
    Ok(rt.monitors_dto())
}

#[tauri::command]
fn get_platform_info(state: State<'_, SharedRuntime>) -> Result<PlatformInfoDto, String> {
    let rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    Ok(rt.platform_info_dto())
}

#[tauri::command]
fn get_overlay_state(state: State<'_, SharedRuntime>) -> Result<OverlayStatePayloadDto, String> {
    let rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    Ok(rt.current_overlay_payload())
}

#[tauri::command]
fn get_permissions(hook: State<'_, SharedHook>) -> Result<PermissionStatusDto, String> {
    let mut ctrl = hook.0.lock().map_err(|_| "hook lock poisoned".to_string())?;
    if !ctrl.hook.is_running() && check_platform_permissions().accessibility_granted {
        let _ = ctrl.ensure_started();
    }
    Ok(ctrl.status_dto())
}

#[tauri::command]
fn request_permissions(hook: State<'_, SharedHook>) -> Result<PermissionStatusDto, String> {
    let _ = request_platform_permissions();
    let mut ctrl = hook.0.lock().map_err(|_| "hook lock poisoned".to_string())?;
    let _ = ctrl.ensure_started();
    Ok(ctrl.status_dto())
}

#[tauri::command]
fn open_accessibility_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn()
            .map_err(|e| format!("Failed to open macOS Accessibility settings: {e}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

#[tauri::command]
async fn check_for_updates(channel: String) -> Result<UpdateCheckResultDto, String> {
    tauri::async_runtime::spawn_blocking(move || updater::check_github_releases(&channel))
        .await
        .map_err(|err| format!("Update check task failed: {err}"))?
}

#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    updater::open_external_url(&url)
}

#[tauri::command]
fn toggle_overlay_mode(
    app: AppHandle,
    state: State<'_, SharedRuntime>,
    hook: State<'_, SharedHook>,
    mode: String,
) -> Result<OverlayStatePayloadDto, String> {
    let mut rt = state.0.lock().map_err(|_| "runtime lock poisoned".to_string())?;
    let event = match mode.as_str() {
        "grid" => {
            if matches!(
                rt.engine.mode(),
                EngineMode::Grid | EngineMode::Subgrid | EngineMode::Nudge
            ) {
                EngineEvent::ExitToNormal
            } else {
                EngineEvent::ActivateGrid {
                    monitor_id: None,
                    preset_id: None,
                }
            }
        }
        "label" => EngineEvent::ActivateLabelMode,
        "free" => EngineEvent::ActivateFreeMode { preset_id: None },
        _ => EngineEvent::ExitToNormal,
    };
    let payload = rt.dispatch_event(event, Some(&app));
    let active_mode = rt.engine.mode();
    let dragging = rt.engine.snapshot().active_modifiers.dragging;
    drop(rt);
    if let Ok(ctrl) = hook.0.lock() {
        ctrl.sync_engine_state(active_mode, dragging);
    }
    Ok(payload)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Harden process memory before initializing UI or hooks (RLIMIT_CORE=0 / PR_SET_DUMPABLE=0).
    let _ = ratpoison_hid::harden_process_memory();

    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle();

            // Strictly enforce click-through on the overlay window from the moment it exists.
            if let Some(overlay) = app.get_webview_window("overlay") {
                let _ = overlay.set_ignore_cursor_events(true);
                let _ = overlay.set_always_on_top(true);
                let _ = overlay.set_visible_on_all_workspaces(true);
            }

            let monitors = query_tauri_monitors(handle);
            let runtime_inner = AppRuntime::initialize(monitors);
            let route_cfg = HookRouteConfig::from_config_json(runtime_inner.config_store.json());

            let runtime = Arc::new(Mutex::new(runtime_inner));
            let router = Arc::new(Mutex::new(HookRouter::new(route_cfg)));
            let (action_tx, action_rx) = mpsc::channel::<HookAction>();

            let mut controller = HookController::new(Arc::clone(&router), action_tx);
            let _ = controller.ensure_started();
            let hook_arc = Arc::new(Mutex::new(controller));

            spawn_hook_worker(
                handle.clone(),
                Arc::clone(&runtime),
                Arc::clone(&router),
                action_rx,
            );
            spawn_permission_watcher(Arc::clone(&hook_arc));

            app.manage(SharedRuntime(runtime));
            app.manage(SharedHook(hook_arc));

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
                                let ev = if matches!(
                                    guard.engine.mode(),
                                    EngineMode::Grid | EngineMode::Subgrid | EngineMode::Nudge
                                ) {
                                    EngineEvent::ExitToNormal
                                } else {
                                    EngineEvent::ActivateGrid {
                                        monitor_id: None,
                                        preset_id: None,
                                    }
                                };
                                guard.dispatch_event(ev, Some(app_handle));
                                let mode = guard.engine.mode();
                                let dragging = guard.engine.snapshot().active_modifiers.dragging;
                                drop(guard);
                                if let Some(hk) = app_handle.try_state::<SharedHook>() {
                                    if let Ok(ctrl) = hk.0.lock() {
                                        ctrl.sync_engine_state(mode, dragging);
                                    }
                                }
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
                                let mode = guard.engine.mode();
                                let dragging = guard.engine.snapshot().active_modifiers.dragging;
                                drop(guard);
                                if let Some(hk) = app_handle.try_state::<SharedHook>() {
                                    if let Ok(ctrl) = hk.0.lock() {
                                        ctrl.sync_engine_state(mode, dragging);
                                    }
                                }
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
                                let mode = guard.engine.mode();
                                let dragging = guard.engine.snapshot().active_modifiers.dragging;
                                drop(guard);
                                if let Some(hk) = app_handle.try_state::<SharedHook>() {
                                    if let Ok(ctrl) = hk.0.lock() {
                                        ctrl.sync_engine_state(mode, dragging);
                                    }
                                }
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
            get_platform_info,
            get_overlay_state,
            get_permissions,
            request_permissions,
            open_accessibility_settings,
            check_for_updates,
            open_external_url,
            toggle_overlay_mode,
        ])
        .run(tauri::generate_context!())
        .expect("error while running RatPoison Tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{
        default_keybindings_for_os, detect_os, project_snapshot_to_overlay_payload, ConfigStore,
    };
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

        let first = &payload.grid_cells[0];
        assert!(first.rect.x.abs() < 1e-6, "expected 0.0, got {}", first.rect.x);
        assert!(first.rect.y.abs() < 1e-6, "expected 0.0, got {}", first.rect.y);

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
    fn os_detection_selects_native_non_conflicting_keybindings() {
        let mac = default_keybindings_for_os("macos");
        assert_eq!(mac.toggle_grid, "Option+Space");
        assert_eq!(mac.open_settings, "Cmd+,");

        let win = default_keybindings_for_os("windows");
        assert_eq!(win.toggle_grid, "Ctrl+Alt+Space");
        assert_eq!(win.open_settings, "Ctrl+,");

        let lin = default_keybindings_for_os("linux");
        assert_eq!(lin.toggle_grid, "Super+Alt+Space");
        assert_eq!(lin.open_settings, "Ctrl+,");

        let rt = AppRuntime::initialize(Vec::new());
        let platform = rt.platform_info_dto();
        assert_eq!(platform.os, detect_os());
        assert_eq!(
            platform.default_keybindings,
            default_keybindings_for_os(detect_os())
        );

        let upd = updater::resolve_release_from_list(&[], "0.1.0", "stable", detect_os(), false);
        assert_eq!(upd.current_version, "0.1.0");
        assert!(!upd.update_available);
    }
}
