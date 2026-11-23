//! Cross-crate runtime wiring connecting `ratpoison-core`, `ratpoison-hid`,
//! and `ratpoison-a11y` inside the Tauri v2 desktop shell.

use crate::ipc::{
    detect_platform_info, project_snapshot_to_overlay_payload, resolve_data_dir, ConfigStore,
    MonitorDto, OverlayStatePayloadDto, PlatformInfoDto, EVENT_OVERLAY_STATE,
};
use ratpoison_a11y::{
    default_platform_provider, generate_label_badges, AccessibilityBudget, AccessibilityProvider,
    LabelConfig, LogicalRect as A11yRect,
};
use ratpoison_core::{
    CoreEngine, EngineEffect, EngineEvent, EngineMode, LabelBadge as CoreLabelBadge, LogicalPoint,
    LogicalRect, MonitorInfo, MonitorLayout, MouseButton as CoreMouseButton, PhysicalPoint,
};
use ratpoison_hid::{
    check_platform_permissions, create_platform_mouse_simulator, ClickKind, CommandInterceptor,
    InterceptorConfig, LogicalPoint as HidLogicalPoint, MouseButton as HidMouseButton,
    MouseSimulatorBackend, PermissionReport, ScrollDelta,
};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager};

pub struct AppRuntime {
    pub config_store: ConfigStore,
    pub engine: CoreEngine,
    pub interceptor: CommandInterceptor<5>,
    pub mouse_sim: Box<dyn MouseSimulatorBackend>,
    pub a11y_provider: Box<dyn AccessibilityProvider>,
    pub permissions: PermissionReport,
    pub active_free_directions: Vec<String>,
    pub free_scroll_active: bool,
}

impl AppRuntime {
    /// Initializes all RatPoison subsystems cleanly without writing to disk at startup.
    pub fn initialize(monitors: Vec<MonitorInfo>) -> Self {
        let data_dir = resolve_data_dir();
        let config_store = ConfigStore::load_or_default(data_dir.join("config.json"));
        let core_config = config_store.to_core_config();

        let fallback = MonitorInfo::from_logical_and_scale(
            "display-1",
            "Primary Display",
            LogicalRect::new(0.0, 0.0, 1920.0, 1080.0),
            PhysicalPoint::new(0, 0),
            1.0,
            true,
        )
        .expect("default monitor is valid");

        let layout = if monitors.is_empty() {
            MonitorLayout::new(vec![fallback]).expect("single monitor layout is valid")
        } else {
            MonitorLayout::new(monitors)
                .unwrap_or_else(|_| MonitorLayout::new(vec![fallback]).expect("fallback valid"))
        };

        let engine = CoreEngine::new(core_config, layout);
        let interceptor = CommandInterceptor::<5>::new(InterceptorConfig::default());
        let mouse_sim = create_platform_mouse_simulator();
        let a11y_provider = default_platform_provider();
        let permissions = check_platform_permissions();

        Self {
            config_store,
            engine,
            interceptor,
            mouse_sim,
            a11y_provider,
            permissions,
            active_free_directions: Vec::new(),
            free_scroll_active: false,
        }
    }

    pub fn monitors_dto(&self) -> Vec<MonitorDto> {
        self.engine
            .layout()
            .monitors()
            .iter()
            .map(MonitorDto::from)
            .collect()
    }

    pub fn platform_info_dto(&self) -> PlatformInfoDto {
        detect_platform_info()
    }

    pub fn sync_core_config_from_store(&mut self) {
        let updated = self.config_store.to_core_config();
        let _ = self.engine.handle_event(EngineEvent::ConfigUpdated {
            config: Box::new(updated),
        });
    }

    /// Reads the live OS cursor position and syncs it into `CoreEngine` so overlays open on
    /// the cursor's active monitor and Free Mode glides from the actual pointer location.
    pub fn sync_live_cursor(&mut self) {
        if let Ok(pt) = self.mouse_sim.cursor_position() {
            let _ = self.engine.handle_event(EngineEvent::SyncCursor {
                logical: LogicalPoint::new(pt.x, pt.y),
            });
        }
    }

    pub fn current_overlay_payload(&self) -> OverlayStatePayloadDto {
        let mut payload =
            project_snapshot_to_overlay_payload(&self.engine.snapshot(), self.engine.config());
        self.enrich_free_mode_payload(&mut payload);
        payload
    }

    fn enrich_free_mode_payload(&self, payload: &mut OverlayStatePayloadDto) {
        if self.engine.mode() == EngineMode::FreeMode {
            if self.config_store.show_free_mode_hud() {
                payload.visible = true;
            }
            if let Some(ref mut fm) = payload.free_mode {
                fm.active_directions = self.active_free_directions.clone();
                fm.scroll_mode = self.free_scroll_active;
            }
        }
    }

    fn scale_for_monitor(&self, monitor_id: &str) -> f64 {
        self.engine
            .layout()
            .by_id(monitor_id)
            .map(|m| m.scale_factor)
            .unwrap_or(1.0)
    }

    /// Feeds an `EngineEvent` into `ratpoison-core`, executes all resulting HID/A11y/window
    /// effects, and emits a camelCase, window-relative `ratpoison://overlay-state` payload.
    pub fn dispatch_event(
        &mut self,
        event: EngineEvent,
        app: Option<&AppHandle>,
    ) -> OverlayStatePayloadDto {
        if matches!(
            event,
            EngineEvent::ActivateGrid { .. }
                | EngineEvent::ActivateFreeMode { .. }
                | EngineEvent::ActivateLabelMode
        ) {
            self.sync_live_cursor();
        }

        let step = self.engine.handle_event(event);

        // If both `ShowOverlay` and `QueryAccessibility` are emitted in the same step
        // (e.g. entering Label Mode), run `QueryAccessibility` BEFORE showing the overlay
        // so `overlay.show()` never perturbs frontmost app focus before snapshotting the AX tree.
        let mut ordered_effects = step.effects;
        let has_query_a11y = ordered_effects
            .iter()
            .any(|e| matches!(e, EngineEffect::QueryAccessibility { .. }));
        if has_query_a11y {
            let mut deferred_show = Vec::new();
            ordered_effects.retain(|e| {
                if matches!(e, EngineEffect::ShowOverlay { .. }) {
                    deferred_show.push(e.clone());
                    false
                } else {
                    true
                }
            });
            ordered_effects.extend(deferred_show);
        }

        for effect in ordered_effects {
            match effect {
                EngineEffect::MoveCursor {
                    logical,
                    monitor_id,
                    ..
                } => {
                    let scale = self.scale_for_monitor(&monitor_id);
                    let _ = self
                        .mouse_sim
                        .move_to(HidLogicalPoint::new(logical.x, logical.y), scale);
                }
                EngineEffect::Click {
                    button,
                    count,
                    logical,
                    monitor_id,
                    ..
                } => {
                    let scale = self.scale_for_monitor(&monitor_id);
                    let pt = HidLogicalPoint::new(logical.x, logical.y);
                    let click_kind = match (button, count) {
                        (CoreMouseButton::Left, 2) => ClickKind::DoubleLeft,
                        (CoreMouseButton::Left, _) => ClickKind::Left,
                        (CoreMouseButton::Right, _) => ClickKind::Right,
                        (CoreMouseButton::Middle, _) => ClickKind::Middle,
                    };
                    let _ = self.mouse_sim.perform_click(click_kind, pt, scale);
                }
                EngineEffect::MouseDown {
                    button,
                    logical,
                    monitor_id,
                    ..
                } => {
                    let scale = self.scale_for_monitor(&monitor_id);
                    let pt = HidLogicalPoint::new(logical.x, logical.y);
                    let _ = self.mouse_sim.mouse_down(map_mouse_button(button), pt, scale);
                }
                EngineEffect::MouseUp {
                    button,
                    logical,
                    monitor_id,
                    ..
                } => {
                    let scale = self.scale_for_monitor(&monitor_id);
                    let pt = HidLogicalPoint::new(logical.x, logical.y);
                    let _ = self.mouse_sim.mouse_up(map_mouse_button(button), pt, scale);
                }
                EngineEffect::Scroll { delta_x, delta_y } => {
                    let _ = self.mouse_sim.scroll(ScrollDelta {
                        dx: delta_x,
                        dy: delta_y,
                    });
                }
                EngineEffect::ShowOverlay { monitor_id } => {
                    if let Some(handle) = app {
                        if let Some(mon) = self.engine.layout().by_id(&monitor_id) {
                            position_and_show_overlay(handle, mon);
                        }
                    }
                }
                EngineEffect::HideOverlay => {
                    if self.engine.mode() == EngineMode::FreeMode
                        && self.config_store.show_free_mode_hud()
                    {
                        if let Some(handle) = app {
                            let active_mon = self
                                .engine
                                .snapshot()
                                .active_monitor
                                .or_else(|| self.engine.layout().primary().cloned());
                            if let Some(mon) = active_mon {
                                position_and_show_overlay(handle, &mon);
                            }
                        }
                    } else if let Some(handle) = app {
                        hide_overlay_window(handle);
                    }
                }
                EngineEffect::QueryAccessibility { monitor_id } => {
                    let active_mon = self
                        .engine
                        .layout()
                        .by_id(&monitor_id)
                        .or_else(|| self.engine.layout().primary())
                        .cloned();
                    if let Some(mon) = active_mon {
                        let a11y_budget = AccessibilityBudget {
                            max_depth: 28,
                            max_nodes: 3000,
                            timeout: std::time::Duration::from_millis(500),
                        };
                        match self.a11y_provider.snapshot(&a11y_budget) {
                            Ok(a11y_snap) => {
                                let cfg = LabelConfig {
                                    viewport: A11yRect::new(
                                        mon.logical_rect.x,
                                        mon.logical_rect.y,
                                        mon.logical_rect.width,
                                        mon.logical_rect.height,
                                    ),
                                    hint_alphabet: self.config_store.label_charset(),
                                    max_badges: self.engine.config().label_mode.max_badges,
                                    min_dimension: self.config_store.label_min_element_size_px(),
                                    ..LabelConfig::default()
                                };
                                let generated = generate_label_badges(&a11y_snap.elements, &cfg);
                                let core_badges: Vec<CoreLabelBadge> = generated
                                    .into_iter()
                                    .map(|b| CoreLabelBadge {
                                        id: b.element.id,
                                        label: b.hint,
                                        title: b.element.title.unwrap_or_default(),
                                        role: format!("{:?}", b.element.role),
                                        rect: LogicalRect::new(
                                            b.bounds.x,
                                            b.bounds.y,
                                            b.bounds.width,
                                            b.bounds.height,
                                        ),
                                        center: LogicalPoint::new(b.target.x, b.target.y),
                                        monitor_id: mon.id.clone(),
                                    })
                                    .collect();
                                let _ = self.engine.handle_event(
                                    EngineEvent::AccessibilityLabelsLoaded {
                                        badges: core_badges,
                                    },
                                );
                            }
                            Err(err) => {
                                eprintln!(
                                    "[RatPoison] Accessibility snapshot failed on {}: {err}",
                                    self.a11y_provider.platform_name()
                                );
                                let _ = self.engine.handle_event(
                                    EngineEvent::AccessibilityLabelsLoaded { badges: Vec::new() },
                                );
                            }
                        }
                    }
                }
                EngineEffect::ClearKeyBuffer => {
                    let _ = self.interceptor.deactivate();
                }
                EngineEffect::EmitOverlaySnapshot { .. } => {}
            }
        }

        // Always project from the authoritative post-effect engine snapshot so follow-up events
        // (such as `AccessibilityLabelsLoaded`) are never overwritten by an earlier `EmitOverlaySnapshot`.
        let mut latest_payload =
            project_snapshot_to_overlay_payload(&self.engine.snapshot(), self.engine.config());

        if self.engine.mode() != EngineMode::FreeMode {
            self.active_free_directions.clear();
            self.free_scroll_active = false;
        }
        self.enrich_free_mode_payload(&mut latest_payload);

        if let Some(handle) = app {
            let _ = handle.emit(EVENT_OVERLAY_STATE, &latest_payload);
        }
        latest_payload
    }
}

fn map_mouse_button(btn: CoreMouseButton) -> HidMouseButton {
    match btn {
        CoreMouseButton::Left => HidMouseButton::Left,
        CoreMouseButton::Right => HidMouseButton::Right,
        CoreMouseButton::Middle => HidMouseButton::Middle,
    }
}

fn position_and_show_overlay(app: &AppHandle, monitor: &MonitorInfo) {
    let app_handle = app.clone();
    let x = monitor.logical_rect.x;
    let y = monitor.logical_rect.y;
    let w = monitor.logical_rect.width;
    let h = monitor.logical_rect.height;
    let _ = app.run_on_main_thread(move || {
        if let Some(overlay) = app_handle.get_webview_window("overlay") {
            let _ = overlay.set_ignore_cursor_events(true);
            let _ = overlay.set_always_on_top(true);
            let _ = overlay.set_visible_on_all_workspaces(true);
            let _ = overlay.set_position(LogicalPosition::new(x, y));
            let _ = overlay.set_size(LogicalSize::new(w, h));
            let _ = overlay.show();
        }
    });
}

fn hide_overlay_window(app: &AppHandle) {
    let app_handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(overlay) = app_handle.get_webview_window("overlay") {
            let _ = overlay.hide();
        }
    });
}
