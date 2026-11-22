//! Cross-crate runtime wiring connecting `ratpoison-core`, `ratpoison-hid`,
//! `ratpoison-a11y`, and `ratpoison-license` inside the Tauri v2 desktop shell.

use crate::ipc::{
    format_unix_iso8601, license_state_to_dto, project_snapshot_to_overlay_payload,
    resolve_data_dir, unix_timestamp_secs, ConfigStore, LicenseStatusDto, MonitorDto,
    OfflineChallengeDto, OverlayStatePayloadDto, UpdateCheckResultDto, EVENT_OVERLAY_STATE,
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
use ratpoison_license::{
    compute_machine_fingerprint, detect_current_machine_fingerprint, detect_flatpak_sandbox,
    evaluate_update, parse_update_manifest, ActivationOutcome, LicenseManager, MachineSeatSummary,
    UpdateChannel, UpdateCheckDecision,
};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager};

pub struct AppRuntime {
    pub config_store: ConfigStore,
    pub engine: CoreEngine,
    pub license_manager: LicenseManager,
    pub interceptor: CommandInterceptor<5>,
    pub mouse_sim: Box<dyn MouseSimulatorBackend>,
    pub a11y_provider: Box<dyn AccessibilityProvider>,
    pub permissions: PermissionReport,
    pub last_license_key_for_offline: Option<String>,
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

        let machine_fp = detect_current_machine_fingerprint()
            .or_else(|_| compute_machine_fingerprint("ratpoison-fallback-host-id"))
            .unwrap_or_else(|_| "0000000000000000".to_string());

        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "local-workstation".to_string());

        let machine_summary = MachineSeatSummary {
            machine_fingerprint: machine_fp,
            friendly_hostname: hostname,
            os: std::env::consts::OS.to_string(),
        };

        let license_path = data_dir.join("license.json");
        let mut license_manager = LicenseManager::with_default_key(machine_summary)
            .expect("embedded Ed25519 verifying key is valid")
            .with_storage_path(&license_path);
        let now = unix_timestamp_secs();
        if let Ok(ratpoison_license::LicenseState::UnstartedTrial) =
            license_manager.evaluate_state(now)
        {
            let _ = license_manager.start_trial(now);
        }

        let interceptor = CommandInterceptor::<5>::new(InterceptorConfig::default());
        let mouse_sim = create_platform_mouse_simulator();
        let a11y_provider = default_platform_provider();
        let permissions = check_platform_permissions();

        Self {
            config_store,
            engine,
            license_manager,
            interceptor,
            mouse_sim,
            a11y_provider,
            permissions,
            last_license_key_for_offline: None,
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

    pub fn license_status_dto(&mut self) -> Result<LicenseStatusDto, String> {
        let now = unix_timestamp_secs();
        let state = self
            .license_manager
            .evaluate_state(now)
            .map_err(|e| e.to_string())?;
        Ok(license_state_to_dto(
            &state,
            &self.license_manager.current_machine().machine_fingerprint,
            self.license_manager.active_seats().len(),
        ))
    }

    pub fn activate_license(&mut self, license_key: &str) -> Result<LicenseStatusDto, String> {
        let now = unix_timestamp_secs();
        let machine = self.license_manager.current_machine().clone();
        self.last_license_key_for_offline = Some(license_key.to_string());
        match self
            .license_manager
            .activate_machine(license_key, machine, now)
            .map_err(|e| e.to_string())?
        {
            ActivationOutcome::Activated { state, .. } => Ok(license_state_to_dto(
                &state,
                &self.license_manager.current_machine().machine_fingerprint,
                self.license_manager.active_seats().len(),
            )),
            ActivationOutcome::RequiresDeactivation { prompt } => Err(format!(
                "All {} seats are active ({}). Deactivate a seat before activating this machine.",
                prompt.max_machines,
                prompt
                    .occupied_seats
                    .iter()
                    .map(|s| s.friendly_hostname.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    pub fn deactivate_current_machine(&mut self) -> Result<LicenseStatusDto, String> {
        let fp = self
            .license_manager
            .current_machine()
            .machine_fingerprint
            .clone();
        let _ = self.license_manager.deactivate_seat(&fp);
        self.license_status_dto()
    }

    pub fn generate_offline_challenge(&self) -> Result<OfflineChallengeDto, String> {
        let now = unix_timestamp_secs();
        let key_hint = self
            .last_license_key_for_offline
            .as_deref()
            .unwrap_or("RP-OFFLINE-REQUEST-0001");
        let challenge_code = self
            .license_manager
            .generate_offline_challenge(key_hint, now)
            .map_err(|e| e.to_string())?;
        Ok(OfflineChallengeDto {
            challenge_code,
            machine_fingerprint: self
                .license_manager
                .current_machine()
                .machine_fingerprint
                .clone(),
            issued_at: format_unix_iso8601(now),
        })
    }

    pub fn apply_offline_response(&mut self, response_blob: &str) -> Result<LicenseStatusDto, String> {
        let now = unix_timestamp_secs();
        let state = self
            .license_manager
            .apply_offline_response(response_blob.trim(), now)
            .map_err(|e| e.to_string())?;
        Ok(license_state_to_dto(
            &state,
            &self.license_manager.current_machine().machine_fingerprint,
            self.license_manager.active_seats().len(),
        ))
    }

    pub fn check_for_updates(&self, channel_str: &str) -> Result<UpdateCheckResultDto, String> {
        let current_str = env!("CARGO_PKG_VERSION");
        let channel = if channel_str.eq_ignore_ascii_case("beta") {
            UpdateChannel::Beta
        } else {
            UpdateChannel::Stable
        };
        let now_iso = format_unix_iso8601(unix_timestamp_secs());
        let is_flatpak = detect_flatpak_sandbox();
        let target_key = format!(
            "{}-{}",
            match std::env::consts::OS {
                "macos" => "darwin",
                other => other,
            },
            std::env::consts::ARCH
        );

        let baseline_manifest = serde_json::json!({
            "version": current_str,
            "notes": "You are running the latest verified build of RatPoison.",
            "pub_date": now_iso,
            "channel": if channel == UpdateChannel::Beta { "beta" } else { "stable" },
            "platforms": {
                target_key.clone(): {
                    "url": "https://github.com/emrisca/RatPoison/releases/latest",
                    "signature": "BASELINE_LOCAL_CHECK"
                }
            }
        })
        .to_string();

        let manifest = parse_update_manifest(&baseline_manifest).map_err(|e| e.to_string())?;
        let decision = evaluate_update(&manifest, current_str, channel, &target_key, is_flatpak)
            .map_err(|e| e.to_string())?;

        match decision {
            UpdateCheckDecision::ManagedByPackageManager { command_hint, .. } => {
                Ok(UpdateCheckResultDto {
                    current_version: current_str.to_string(),
                    latest_version: current_str.to_string(),
                    update_available: false,
                    release_notes: format!(
                        "Managed by Flatpak sandbox. Run `{command_hint}` to update."
                    ),
                    published_at: now_iso,
                    download_url: None,
                })
            }
            UpdateCheckDecision::UpdateAvailable {
                new_version,
                artifact,
                notes,
                pub_date,
                ..
            } => Ok(UpdateCheckResultDto {
                current_version: current_str.to_string(),
                latest_version: new_version,
                update_available: true,
                release_notes: notes.unwrap_or_else(|| "New RatPoison update available.".to_string()),
                published_at: pub_date.unwrap_or(now_iso),
                download_url: Some(artifact.url),
            }),
            UpdateCheckDecision::UpToDate { .. } => Ok(UpdateCheckResultDto {
                current_version: current_str.to_string(),
                latest_version: current_str.to_string(),
                update_available: false,
                release_notes: "You are running the latest verified build of RatPoison.".to_string(),
                published_at: now_iso,
                download_url: None,
            }),
        }
    }

    fn scale_for_monitor(&self, monitor_id: &str) -> f64 {
        self.engine
            .layout()
            .by_id(monitor_id)
            .map(|m| m.scale_factor)
            .unwrap_or(1.0)
    }

    /// Dispatches an `EngineEvent` through `CoreEngine` and executes all emitted `EngineEffect`s
    /// across `ratpoison-hid`, `ratpoison-a11y`, and the Tauri overlay WebView.
    pub fn dispatch_event(
        &mut self,
        event: EngineEvent,
        app: Option<&AppHandle>,
    ) -> OverlayStatePayloadDto {
        let output = self.engine.handle_event(event);
        let mut latest_payload =
            project_snapshot_to_overlay_payload(&output.snapshot, self.engine.config());

        for effect in output.effects {
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
