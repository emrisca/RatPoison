//! Cross-crate runtime wiring connecting `ratpoison-core`, `ratpoison-hid`,
//! and `ratpoison-a11y` inside the Tauri v2 desktop shell.

use crate::ipc::{
    detect_platform_info, format_unix_iso8601, project_snapshot_to_overlay_payload,
    resolve_data_dir, unix_timestamp_secs, ConfigStore, MonitorDto, OverlayStatePayloadDto,
    PlatformInfoDto, UpdateCheckResultDto, EVENT_OVERLAY_STATE,
};
use ratpoison_a11y::{
    default_platform_provider, generate_label_badges, AccessibilityBudget, AccessibilityProvider,
    LabelConfig, LogicalRect as A11yRect,
};
use ratpoison_core::{
    CoreEngine, EngineEffect, EngineEvent, LabelBadge as CoreLabelBadge, LogicalPoint, LogicalRect,
    MonitorInfo, MonitorLayout, MouseButton as CoreMouseButton, PhysicalPoint,
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

    pub fn check_for_updates(&self, channel_str: &str) -> Result<UpdateCheckResultDto, String> {
        let current_str = env!("CARGO_PKG_VERSION");
        let channel = if channel_str.eq_ignore_ascii_case("beta") {
            "beta"
        } else {
            "stable"
        };
        let now_iso = format_unix_iso8601(unix_timestamp_secs());
        let is_flatpak =
            std::env::var_os("FLATPAK_ID").is_some() || std::path::Path::new("/.flatpak-info").exists();

        let release_notes = if is_flatpak {
            format!(
                "RatPoison v{current_str} ({channel} channel) is managed via Flatpak (`flatpak update io.github.emrisca.RatPoison`)."
            )
        } else {
            format!(
                "RatPoison v{current_str} ({channel} channel) — open-source MIT builds are published at https://github.com/emrisca/RatPoison/releases."
            )
        };

        Ok(UpdateCheckResultDto {
            current_version: current_str.to_string(),
            latest_version: current_str.to_string(),
            update_available: false,
            release_notes,
            published_at: now_iso,
            download_url: Some("https://github.com/emrisca/RatPoison/releases".to_string()),
        })
    }

    pub fn sync_core_config_from_store(&mut self) {
        let updated = self.config_store.to_core_config();
        let _ = self
            .engine
            .handle_event(EngineEvent::ConfigUpdated { config: Box::new(updated) });
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
        let step = self.engine.handle_event(event);
        let mut latest_payload =
            project_snapshot_to_overlay_payload(&step.snapshot, self.engine.config());

        for effect in step.effects {
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
                    if let Some(handle) = app {
                        if let Some(win) = handle.get_webview_window("overlay") {
                            let _ = win.hide();
                        }
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
                        if let Ok(a11y_snap) =
                            self.a11y_provider.snapshot(&AccessibilityBudget::default())
                        {
                            let cfg = LabelConfig {
                                viewport: A11yRect::new(
                                    mon.logical_rect.x,
                                    mon.logical_rect.y,
                                    mon.logical_rect.width,
                                    mon.logical_rect.height,
                                ),
                                hint_alphabet: self.config_store.label_charset(),
                                max_badges: self.engine.config().label_mode.max_badges,
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
                            let follow = self.engine.handle_event(
                                EngineEvent::AccessibilityLabelsLoaded {
                                    badges: core_badges,
                                },
                            );
                            latest_payload = project_snapshot_to_overlay_payload(
                                &follow.snapshot,
                                self.engine.config(),
                            );
                        }
                    }
                }
                EngineEffect::ClearKeyBuffer => {
                    let _ = self.interceptor.deactivate();
                }
                EngineEffect::EmitOverlaySnapshot { snapshot } => {
                    latest_payload =
                        project_snapshot_to_overlay_payload(&snapshot, self.engine.config());
                }
            }
        }

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
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.set_ignore_cursor_events(true);
        let _ = overlay.set_position(LogicalPosition::new(
            monitor.logical_rect.x,
            monitor.logical_rect.y,
        ));
        let _ = overlay.set_size(LogicalSize::new(
            monitor.logical_rect.width,
            monitor.logical_rect.height,
        ));
        let _ = overlay.show();
    }
}
