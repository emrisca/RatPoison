//! Cross-crate end-to-end integration tests bridging `ratpoison-hid`, `ratpoison-core`,
//! `ratpoison-a11y`, and `ratpoison-license`.
//!
//! Exercises:
//! 1. `ratpoison-hid` mock hook + `CommandInterceptor` -> `ratpoison-core` state machine
//!    (`Grid -> Subgrid -> Click/Drag`, `FreeMode` relative movement + speed tiers, and
//!    Quick Select + Free Mode handoff).
//! 2. `ratpoison-a11y` mock accessibility tree + Windows UIA / Linux AT-SPI2 adapters ->
//!    home-row prefix-free label badge generation -> `ratpoison-core` `LabelMode` target
//!    center coordinate mapping and auto-click synthesis.
//! 3. `ratpoison-license` 14-day trial lifecycle, clock rollback detection, 5-machine seat
//!    limit + deactivation dialog swap, offline/air-gapped Ed25519 challenge-response
//!    activation token verification, and update manifest verification.
//! 4. Cross-crate contract and serialization audit tests verifying where type conversion
//!    bridges (`ratpoison_tests::*`) are required.

use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;
use ratpoison_a11y::{
    AccessibilityBudget, AccessibilityProvider, AccessibleElement, ElementRole,
    LabelConfig as A11yLabelConfig, LogicalRect as A11yRect, MockAccessibilityProvider,
};
use ratpoison_core::{
    CoreEngine, Direction, EngineEffect, EngineEvent, EngineMode, FreeKey,
    LogicalPoint as CoreLogicalPoint, LogicalRect as CoreLogicalRect, MonitorInfo, MonitorLayout,
    MouseActionKind, MouseButton as CoreMouseButton, NudgeStep, PhysicalPoint as CorePhysicalPoint,
    RatPoisonConfig, SpeedModifier,
};
use ratpoison_hid::{
    ClickKind, CommandInterceptor, InterceptDecision, InterceptorAction, InterceptorConfig,
    KeyCode, KeyPhase, KeyboardHookBackend, MockHidBackend, MockMouseOp, Modifiers,
    MouseButton as HidMouseButton, MouseSimulatorBackend, RawKeyEvent, ScrollDelta,
};
use ratpoison_license::{
    compute_machine_fingerprint, issue_offline_activation_token, normalize_and_hash_license_key,
    ActivationMode, ActivationOutcome, LicenseError, LicenseManager, LicenseState,
    MachineSeatSummary, CLOCK_ROLLBACK_TOLERANCE_SECS, MAX_MACHINES, OFFLINE_CHALLENGE_PREFIX,
    OFFLINE_TOKEN_PREFIX, TRIAL_DURATION_SECS,
};
use ratpoison_tests::{
    bridge_a11y_badge_to_core, core_to_a11y_rect, core_to_hid_logical_point,
    core_to_hid_mouse_button, hid_click_to_core_action, hid_to_core_logical_point,
};

fn build_dual_display_layout() -> MonitorLayout {
    let primary = MonitorInfo::from_logical_and_scale(
        "PRIMARY-RETINA",
        "Primary Retina 2.0x",
        CoreLogicalRect::new(0.0, 0.0, 1440.0, 900.0),
        CorePhysicalPoint::new(0, 0),
        2.0,
        true,
    )
    .unwrap();

    let secondary = MonitorInfo::from_logical_and_scale(
        "LEFT-125",
        "Secondary Left 1.25x",
        CoreLogicalRect::new(-1536.0, 0.0, 1536.0, 864.0),
        CorePhysicalPoint::new(-1920, 0),
        1.25,
        false,
    )
    .unwrap();

    MonitorLayout::new(vec![secondary, primary]).unwrap()
}

/// Applies `EngineEffect`s emitted by `ratpoison-core` directly against `MockHidBackend`,
/// bridging crate boundary types via `ratpoison-tests` conversion helpers.
fn dispatch_core_effects_to_hid(
    effects: &[EngineEffect],
    layout: &MonitorLayout,
    hid: &mut MockHidBackend,
) {
    for effect in effects {
        match effect {
            EngineEffect::MoveCursor {
                logical,
                monitor_id,
                ..
            } => {
                let scale = layout
                    .by_id(monitor_id)
                    .map(|m| m.scale_factor)
                    .unwrap_or(1.0);
                hid.move_to(core_to_hid_logical_point(*logical), scale)
                    .unwrap();
            }
            EngineEffect::Click {
                button,
                count,
                logical,
                monitor_id,
                ..
            } => {
                let scale = layout
                    .by_id(monitor_id)
                    .map(|m| m.scale_factor)
                    .unwrap_or(1.0);
                let kind = match (*button, *count) {
                    (CoreMouseButton::Left, 2..) => ClickKind::DoubleLeft,
                    (CoreMouseButton::Left, _) => ClickKind::Left,
                    (CoreMouseButton::Right, _) => ClickKind::Right,
                    (CoreMouseButton::Middle, _) => ClickKind::Middle,
                };
                hid.perform_click(kind, core_to_hid_logical_point(*logical), scale)
                    .unwrap();
            }
            EngineEffect::MouseDown {
                button,
                logical,
                monitor_id,
                ..
            } => {
                let scale = layout
                    .by_id(monitor_id)
                    .map(|m| m.scale_factor)
                    .unwrap_or(1.0);
                hid.mouse_down(
                    core_to_hid_mouse_button(*button),
                    core_to_hid_logical_point(*logical),
                    scale,
                )
                .unwrap();
            }
            EngineEffect::MouseUp {
                button,
                logical,
                monitor_id,
                ..
            } => {
                let scale = layout
                    .by_id(monitor_id)
                    .map(|m| m.scale_factor)
                    .unwrap_or(1.0);
                hid.mouse_up(
                    core_to_hid_mouse_button(*button),
                    core_to_hid_logical_point(*logical),
                    scale,
                )
                .unwrap();
            }
            EngineEffect::Scroll { delta_x, delta_y } => {
                hid.scroll(ScrollDelta::new(*delta_x, *delta_y)).unwrap();
            }
            EngineEffect::ShowOverlay { .. }
            | EngineEffect::HideOverlay
            | EngineEffect::QueryAccessibility { .. }
            | EngineEffect::ClearKeyBuffer
            | EngineEffect::EmitOverlaySnapshot { .. } => {}
        }
    }
}

#[test]
fn end_to_end_hid_hook_and_core_grid_subgrid_nudge_click_and_drag_pipeline() {
    let layout = build_dual_display_layout();
    let mut engine = CoreEngine::new(RatPoisonConfig::default(), layout.clone());
    let mut hid = MockHidBackend::new();

    let interceptor = Arc::new(Mutex::new(CommandInterceptor::new(
        InterceptorConfig::default(),
    )));
    let cb_ref = Arc::clone(&interceptor);
    hid.start(Box::new(move |ev| cb_ref.lock().unwrap().handle_event(ev)))
        .unwrap();

    // 1. Press Alt+Space -> MockHidBackend invokes CommandInterceptor -> emits ActivateOverlay
    hid.dispatch_event(RawKeyEvent::new(
        KeyCode::ALT_L,
        Modifiers::ALT,
        KeyPhase::Down,
        10,
    ))
    .unwrap();
    let act = hid
        .dispatch_event(RawKeyEvent::new(
            KeyCode::SPACE,
            Modifiers::ALT,
            KeyPhase::Down,
            20,
        ))
        .unwrap();
    assert_eq!(
        act,
        InterceptDecision::SwallowAndEmit(InterceptorAction::ActivateOverlay)
    );
    let step = engine.handle_event(EngineEvent::ActivateGrid {
        monitor_id: None,
        preset_id: Some("standard".to_string()),
    });
    assert_eq!(step.snapshot.mode, EngineMode::Grid);
    assert!(step.snapshot.visible);

    // Release Space & Alt
    hid.dispatch_event(RawKeyEvent::new(
        KeyCode::SPACE,
        Modifiers::ALT,
        KeyPhase::Up,
        25,
    ))
    .unwrap();
    hid.dispatch_event(RawKeyEvent::new(
        KeyCode::ALT_L,
        Modifiers::EMPTY,
        KeyPhase::Up,
        28,
    ))
    .unwrap();

    // 2. Type 'a', 'b' (Row 0, Col 1 on 16x12 grid over 1440x900 primary monitor)
    for (idx, ascii) in b"AB".iter().enumerate() {
        let ts = 40 + (idx as u64) * 10;
        let kc = KeyCode::from_ascii_upper(*ascii);
        let dec = hid
            .dispatch_event(RawKeyEvent::new(kc, Modifiers::EMPTY, KeyPhase::Down, ts))
            .unwrap();
        if let InterceptDecision::SwallowAndEmit(InterceptorAction::GridKeyInput { key, .. }) = dec
        {
            let ch = (key.code().raw() as u8 as char).to_ascii_lowercase();
            let out = engine.handle_event(EngineEvent::KeyChar { ch });
            dispatch_core_effects_to_hid(&out.effects, &layout, &mut hid);
        } else {
            panic!("expected GridKeyInput for {ascii}");
        }
        hid.dispatch_event(RawKeyEvent::new(
            kc,
            Modifiers::EMPTY,
            KeyPhase::Up,
            ts + 4,
        ))
        .unwrap();
    }
    assert_eq!(engine.mode(), EngineMode::Subgrid);
    // Cell (0, 1) on 1440x900 (16x12 -> 90x75 cells) has center (135.0, 37.5)
    assert_eq!(
        hid_to_core_logical_point(hid.cursor_position().unwrap()),
        CoreLogicalPoint::new(135.0, 37.5)
    );

    // 3. Type 'e' (top-right subgrid cell: col 2, row 0 in 3x3 subgrid of 90x75 -> 30x25 subcells)
    //    Subcell center = (90 + 2.5*30, 0 + 0.5*25) = (165.0, 12.5)
    let kc_e = KeyCode::from_ascii_upper(b'E');
    if let InterceptDecision::SwallowAndEmit(InterceptorAction::GridKeyInput { key, .. }) = hid
        .dispatch_event(RawKeyEvent::new(kc_e, Modifiers::EMPTY, KeyPhase::Down, 80))
        .unwrap()
    {
        let ch = (key.code().raw() as u8 as char).to_ascii_lowercase();
        let out = engine.handle_event(EngineEvent::KeyChar { ch });
        dispatch_core_effects_to_hid(&out.effects, &layout, &mut hid);
    }
    hid.dispatch_event(RawKeyEvent::new(kc_e, Modifiers::EMPTY, KeyPhase::Up, 84))
        .unwrap();
    assert_eq!(engine.mode(), EngineMode::Nudge);
    assert_eq!(
        hid_to_core_logical_point(hid.cursor_position().unwrap()),
        CoreLogicalPoint::new(165.0, 12.5)
    );

    // 4. Fine Nudge Right (+1.5 px -> 166.5, 12.5)
    let out_nudge = engine.handle_event(EngineEvent::Nudge {
        direction: Direction::Right,
        step: NudgeStep::Fine,
    });
    dispatch_core_effects_to_hid(&out_nudge.effects, &layout, &mut hid);
    assert_eq!(
        hid_to_core_logical_point(hid.cursor_position().unwrap()),
        CoreLogicalPoint::new(166.5, 12.5)
    );

    // 5. Hold 'R' (Hold-For Right Click override) + press RETURN -> executes RightClick!
    let kc_r = KeyCode::from_ascii_upper(b'R');
    assert_eq!(
        hid.dispatch_event(RawKeyEvent::new(kc_r, Modifiers::EMPTY, KeyPhase::Down, 100))
            .unwrap(),
        InterceptDecision::SwallowAndEmit(InterceptorAction::SetClickMode(ClickKind::Right))
    );
    let exec = hid
        .dispatch_event(RawKeyEvent::new(
            KeyCode::RETURN,
            Modifiers::EMPTY,
            KeyPhase::Down,
            110,
        ))
        .unwrap();
    if let InterceptDecision::SwallowAndEmit(InterceptorAction::ExecuteClick { kind, .. }) = exec {
        let action = hid_click_to_core_action(kind);
        assert_eq!(action, MouseActionKind::RightClick);
        let out_click = engine.handle_event(EngineEvent::MouseAction { action });
        dispatch_core_effects_to_hid(&out_click.effects, &layout, &mut hid);
    } else {
        panic!("expected ExecuteClick");
    }
    assert_eq!(engine.mode(), EngineMode::Normal);

    // 6. Test Drag lifecycle: activate grid -> start drag -> nudge -> escape releases drag cleanly
    engine.handle_event(EngineEvent::ActivateGrid {
        monitor_id: None,
        preset_id: None,
    });
    let drag_start = engine.handle_event(EngineEvent::MouseAction {
        action: MouseActionKind::DragStart,
    });
    dispatch_core_effects_to_hid(&drag_start.effects, &layout, &mut hid);
    assert!(engine.is_dragging());
    assert_eq!(hid.pressed_buttons(), &[HidMouseButton::Left]);

    let drag_move = engine.handle_event(EngineEvent::Nudge {
        direction: Direction::Down,
        step: NudgeStep::Coarse,
    });
    dispatch_core_effects_to_hid(&drag_move.effects, &layout, &mut hid);

    let drag_cancel = engine.handle_event(EngineEvent::ExitToNormal);
    dispatch_core_effects_to_hid(&drag_cancel.effects, &layout, &mut hid);
    assert!(!engine.is_dragging());
    assert!(
        hid.pressed_buttons().is_empty(),
        "Escape / ExitToNormal must release held mouse drag button"
    );
}

#[test]
fn end_to_end_free_mode_relative_movement_speed_tiers_and_quick_select_handoff() {
    let layout = build_dual_display_layout();
    let mut engine = CoreEngine::new(RatPoisonConfig::default(), layout.clone());
    let mut hid = MockHidBackend::new();

    // 1. Activate Quick Select + Free Mode preset ("quick_select_free" -> 6x4 single-key grid)
    let act = engine.handle_event(EngineEvent::ActivateGrid {
        monitor_id: None,
        preset_id: Some("quick_select_free".to_string()),
    });
    assert_eq!(act.snapshot.mode, EngineMode::Grid);
    assert_eq!(act.snapshot.grid_cells.len(), 24);

    // Press single key 'q' -> jumps directly to zone center and hands off into FreeMode with overlay hidden!
    let handoff = engine.handle_event(EngineEvent::KeyChar { ch: 'q' });
    dispatch_core_effects_to_hid(&handoff.effects, &layout, &mut hid);
    assert_eq!(handoff.snapshot.mode, EngineMode::FreeMode);
    assert!(!handoff.snapshot.visible);

    let start_pos = engine.cursor_logical();

    // 2. Compare displacement across Slow ('a'), Normal ('s'), Fast ('d'), and Turbo ('f') speed tiers
    let tiers = [
        ('a', SpeedModifier::Slow),
        ('s', SpeedModifier::Normal),
        ('d', SpeedModifier::Fast),
        ('f', SpeedModifier::Turbo),
    ];
    let mut deltas = Vec::new();

    for (ch, expected_mod) in tiers {
        engine.handle_event(EngineEvent::SyncCursor { logical: start_pos });
        let speed_key = FreeKey::from_qwerty_char(ch).unwrap();
        let move_right = FreeKey::from_qwerty_char('l').unwrap();

        engine.handle_event(EngineEvent::FreeModeKeyDown { key: speed_key });
        let snap = engine
            .handle_event(EngineEvent::FreeModeKeyDown { key: move_right })
            .snapshot;
        assert_eq!(snap.active_modifiers.speed, expected_mod);

        let tick_out = engine.handle_event(EngineEvent::FreeModeTick { dt_ms: 100 });
        dispatch_core_effects_to_hid(&tick_out.effects, &layout, &mut hid);
        let moved_dx = engine.cursor_logical().x - start_pos.x;
        assert!(moved_dx > 0.0);
        deltas.push(moved_dx);

        engine.handle_event(EngineEvent::FreeModeKeyUp { key: move_right });
        engine.handle_event(EngineEvent::FreeModeKeyUp { key: speed_key });
    }

    // Verify strict monotonic speed progression: Slow < Normal < Fast < Turbo
    assert!(
        deltas[0] < deltas[1] && deltas[1] < deltas[2] && deltas[2] < deltas[3],
        "FreeMode speed tier displacements must satisfy Slow < Normal < Fast < Turbo, got {deltas:?}"
    );

    // 3. Test FreeMode scrolling
    engine.handle_event(EngineEvent::FreeModeKeyDown {
        key: FreeKey::ScrollUp,
    });
    let scroll_out = engine.handle_event(EngineEvent::FreeModeTick { dt_ms: 100 });
    dispatch_core_effects_to_hid(&scroll_out.effects, &layout, &mut hid);
    assert!(hid
        .mouse_ops()
        .iter()
        .any(|op| matches!(op, MockMouseOp::Scroll(d) if d.dy > 0.0)));
}

#[test]
fn end_to_end_a11y_tree_to_core_label_mode_and_hid_auto_click() {
    let layout = build_dual_display_layout();
    let primary = layout.by_id("PRIMARY-RETINA").unwrap();
    let mut engine = CoreEngine::new(RatPoisonConfig::default(), layout.clone());
    let mut hid = MockHidBackend::new();

    // 1. Activate LabelMode in ratpoison-core -> emits QueryAccessibility for PRIMARY-RETINA
    let act = engine.handle_event(EngineEvent::ActivateLabelMode);
    assert_eq!(act.snapshot.mode, EngineMode::LabelMode);
    assert!(act.effects.iter().any(|e| matches!(
        e,
        EngineEffect::QueryAccessibility { monitor_id } if monitor_id == "PRIMARY-RETINA"
    )));

    // 2. Query MockAccessibilityProvider from ratpoison-a11y using the active monitor's viewport
    let a11y_provider = MockAccessibilityProvider::standard_desktop_fixture();
    let budget = AccessibilityBudget::default();
    let label_cfg = A11yLabelConfig {
        viewport: core_to_a11y_rect(primary.logical_rect),
        ..A11yLabelConfig::default()
    };

    let (a11y_snap, a11y_badges) = a11y_provider
        .collect_label_badges(&budget, &label_cfg)
        .expect("a11y snapshot succeeds");
    assert!(!a11y_snap.truncated_by_budget);
    assert_eq!(a11y_badges.len(), 5);

    // Verify home-row hints ("a", "s", "d", "f", "g") and bridge into ratpoison-core::LabelBadge
    let core_badges: Vec<ratpoison_core::LabelBadge> = a11y_badges
        .iter()
        .map(|b| bridge_a11y_badge_to_core(b, &primary.id))
        .collect();
    assert_eq!(core_badges[0].label, "a");
    assert_eq!(core_badges[0].id, "tab-1");

    // Find the "modal-confirm-btn" badge and its assigned home-row hint
    let modal_badge = core_badges
        .iter()
        .find(|b| b.id == "modal-confirm-btn")
        .expect("modal-confirm-btn present");
    let modal_hint = modal_badge.label.clone();
    let modal_center = modal_badge.center;
    assert_eq!(modal_center, CoreLogicalPoint::new(480.0, 324.0));

    // 3. Deliver badges to CoreEngine
    let loaded = engine.handle_event(EngineEvent::AccessibilityLabelsLoaded {
        badges: core_badges,
    });
    assert_eq!(loaded.snapshot.label_badges.len(), 5);

    // 4. Type the modal button's home-row hint -> CoreEngine moves cursor to target center,
    //    synthesizes auto-click, and returns to Normal mode!
    let mut last_out = None;
    for ch in modal_hint.chars() {
        let out = engine.handle_event(EngineEvent::KeyChar { ch });
        dispatch_core_effects_to_hid(&out.effects, &layout, &mut hid);
        last_out = Some(out);
    }
    let final_out = last_out.unwrap();
    assert_eq!(final_out.snapshot.mode, EngineMode::Normal);
    assert_eq!(engine.cursor_logical(), modal_center);
    assert_eq!(
        hid_to_core_logical_point(hid.cursor_position().unwrap()),
        modal_center
    );
    assert!(hid.mouse_ops().iter().any(|op| matches!(
        op,
        MockMouseOp::MouseDown {
            button: HidMouseButton::Left,
            point,
            scale_factor,
        } if (point.x - 480.0).abs() < 1e-9 && (point.y - 324.0).abs() < 1e-9 && (*scale_factor - 2.0).abs() < 1e-9
    )));
}

#[test]
fn end_to_end_large_a11y_tree_prefix_free_two_char_hints_in_core_label_mode() {
    let layout = build_dual_display_layout();
    let primary = layout.by_id("PRIMARY-RETINA").unwrap();
    let mut engine = CoreEngine::new(RatPoisonConfig::default(), layout.clone());

    // Build 40 distinct buttons (> 26 single-letter capacity) so ratpoison-a11y emits a mix
    // of 1-char and 2-char prefix-free home-row hints
    let mut elements = Vec::with_capacity(40);
    for idx in 0..40 {
        let col = idx % 8;
        let row = idx / 8;
        elements.push(AccessibleElement::new(
            format!("btn-{idx}"),
            ElementRole::Button,
            Some(format!("Button {idx}")),
            A11yRect::new(20.0 + col as f64 * 150.0, 20.0 + row as f64 * 80.0, 100.0, 40.0),
            true,
            true,
            0,
            2000,
        ));
    }

    let provider = MockAccessibilityProvider::from_flat_elements(elements);
    let (_, a11y_badges) = provider
        .collect_label_badges(&AccessibilityBudget::default(), &A11yLabelConfig::default())
        .unwrap();
    assert_eq!(a11y_badges.len(), 40);

    // Pick a 2-character badge and verify typing its first character filters candidates,
    // and typing its second character triggers the click at its exact center
    let two_char_badge = a11y_badges
        .iter()
        .find(|b| b.hint.len() == 2)
        .expect("at least one 2-char hint generated for 40 elements")
        .clone();

    let core_badges: Vec<ratpoison_core::LabelBadge> = a11y_badges
        .iter()
        .map(|b| bridge_a11y_badge_to_core(b, &primary.id))
        .collect();

    engine.handle_event(EngineEvent::ActivateLabelMode);
    engine.handle_event(EngineEvent::AccessibilityLabelsLoaded {
        badges: core_badges,
    });

    let mut chars = two_char_badge.hint.chars();
    let c1 = chars.next().unwrap();
    let c2 = chars.next().unwrap();

    let step1 = engine.handle_event(EngineEvent::KeyChar { ch: c1 });
    assert_eq!(step1.snapshot.mode, EngineMode::LabelMode);
    assert_eq!(step1.snapshot.typed_prefix, c1.to_string());

    let step2 = engine.handle_event(EngineEvent::KeyChar { ch: c2 });
    assert_eq!(step2.snapshot.mode, EngineMode::Normal);
    assert_eq!(
        engine.cursor_logical(),
        CoreLogicalPoint::new(two_char_badge.target.x, two_char_badge.target.y)
    );
}

#[test]
fn end_to_end_license_trial_5_machine_seats_and_airgapped_ed25519_verification() {
    let secret_bytes: [u8; 32] = [0x5A; 32];
    let signing_key = SigningKey::from_bytes(&secret_bytes);
    let verifying_key = signing_key.verifying_key();

    let make_seat = |idx: usize| -> MachineSeatSummary {
        let fp = compute_machine_fingerprint(&format!("HW-UUID-2026-{idx:04}")).unwrap();
        MachineSeatSummary {
            machine_fingerprint: fp,
            friendly_hostname: format!("dev-node-{idx}.local"),
            os: "macos".to_string(),
        }
    };

    let mut mgr = LicenseManager::new(make_seat(1), verifying_key);
    let t0: u64 = 1_750_000_000;

    // 1. 14-day trial lifecycle
    assert_eq!(
        mgr.evaluate_state(t0).unwrap(),
        LicenseState::UnstartedTrial
    );
    let trial = mgr.start_trial(t0).unwrap();
    assert!(trial.is_feature_unlocked());
    assert_eq!(
        mgr.evaluate_state(t0 + TRIAL_DURATION_SECS).unwrap(),
        LicenseState::TrialExpired {
            started_at: t0,
            expired_at: t0 + TRIAL_DURATION_SECS,
        }
    );

    // 2. Clock rollback protection (> 300s backward jump rejected)
    let last_obs = t0 + TRIAL_DURATION_SECS;
    let rollback_err = mgr
        .evaluate_state(last_obs - CLOCK_ROLLBACK_TOLERANCE_SECS - 10)
        .unwrap_err();
    assert!(matches!(
        rollback_err,
        LicenseError::ClockRollbackDetected { .. }
    ));

    // 3. 5-machine seat limit and 6th-machine deactivation prompt
    let license_key = "RP-TEAM-2026-1111-2222-3333";
    let mut now = last_obs + 60;
    for seat_idx in 1..=MAX_MACHINES {
        now += 10;
        let out = mgr
            .activate_machine(license_key, make_seat(seat_idx), now)
            .unwrap();
        assert!(matches!(out, ActivationOutcome::Activated { .. }));
    }
    assert_eq!(mgr.active_seats().len(), 5);

    // 6th machine triggers RequiresDeactivation with all 5 occupied seats
    now += 10;
    let seat_6 = make_seat(6);
    let out_6 = mgr
        .activate_machine(license_key, seat_6.clone(), now)
        .unwrap();
    match out_6 {
        ActivationOutcome::RequiresDeactivation { prompt } => {
            assert_eq!(prompt.max_machines, 5);
            assert_eq!(prompt.occupied_seats.len(), 5);
            assert_eq!(
                prompt.attempting_machine.machine_fingerprint,
                seat_6.machine_fingerprint
            );
        }
        other => panic!("expected RequiresDeactivation, got {other:?}"),
    }

    // Deactivate seat #1 and replace with machine #6
    let seat_1_fp = make_seat(1).machine_fingerprint;
    now += 10;
    let swapped = mgr
        .deactivate_and_replace_seat(license_key, &seat_1_fp, now)
        .unwrap();
    assert!(matches!(
        swapped,
        ActivationOutcome::Activated { ref seat, .. }
        if seat.seat_id == 1 && seat.machine_fingerprint == seat_6.machine_fingerprint
    ));

    // 4. Offline / Air-Gapped Ed25519 Challenge-Response activation
    let airgap_seat = make_seat(42);
    let mut airgap_mgr = LicenseManager::new(airgap_seat.clone(), verifying_key);
    let challenge = airgap_mgr
        .generate_offline_challenge(license_key, now)
        .unwrap();
    assert!(challenge.starts_with(OFFLINE_CHALLENGE_PREFIX));

    let expires_at = Some(now + 180 * 86_400);
    let token = issue_offline_activation_token(
        &signing_key,
        &challenge,
        license_key,
        2,
        now + 30,
        expires_at,
    )
    .unwrap();
    assert!(token.starts_with(OFFLINE_TOKEN_PREFIX));

    let activated_state = airgap_mgr.apply_offline_response(&token, now + 60).unwrap();
    assert!(activated_state.is_feature_unlocked());
    assert_eq!(
        activated_state,
        LicenseState::Licensed {
            mode: ActivationMode::OfflineChallengeResponse,
            license_key_hash: normalize_and_hash_license_key(license_key).unwrap(),
            machine_fingerprint: airgap_seat.machine_fingerprint,
            seat_id: 2,
            activated_at: now + 30,
            expires_at,
        }
    );
}

#[test]
fn cross_crate_contract_serialization_and_type_bridge_audit() {
    // 1. Verify `LogicalPoint`, `LogicalRect`, and `PhysicalPoint` across `ratpoison-core`,
    //    `ratpoison-hid`, and `ratpoison-a11y` share identical JSON representations (`{x, y}` / `{x, y, width, height}`)
    //    even though they are distinct Rust nominal structs.
    let core_pt = CoreLogicalPoint::new(-320.5, 144.25);
    let json_pt = serde_json::to_string(&core_pt).unwrap();
    let hid_pt: ratpoison_hid::LogicalPoint = serde_json::from_str(&json_pt).unwrap();
    let a11y_pt: ratpoison_a11y::LogicalPoint = serde_json::from_str(&json_pt).unwrap();
    assert_eq!(core_to_hid_logical_point(core_pt), hid_pt);
    assert_eq!(a11y_pt.x, core_pt.x);
    assert_eq!(a11y_pt.y, core_pt.y);

    let core_rect = CoreLogicalRect::new(-1920.0, 0.0, 1920.0, 1080.0);
    let json_rect = serde_json::to_string(&core_rect).unwrap();
    let hid_rect: ratpoison_hid::LogicalRect = serde_json::from_str(&json_rect).unwrap();
    let a11y_rect: ratpoison_a11y::LogicalRect = serde_json::from_str(&json_rect).unwrap();
    assert_eq!(hid_rect.width, 1920.0);
    assert_eq!(core_to_a11y_rect(core_rect), a11y_rect);

    // 2. Documented contract friction: `MouseButton` serde casing differs between `ratpoison-core`
    //    (`#[serde(rename_all = "snake_case")]` -> `"left"`) and `ratpoison-hid` (default PascalCase -> `"Left"`).
    //    Therefore direct Rust enum mapping via `core_to_hid_mouse_button` MUST be used instead of JSON punning.
    let core_btn_json = serde_json::to_string(&CoreMouseButton::Left).unwrap();
    let hid_btn_json = serde_json::to_string(&HidMouseButton::Left).unwrap();
    assert_eq!(core_btn_json, "\"left\"");
    assert_eq!(hid_btn_json, "\"Left\"");
    assert_eq!(
        core_to_hid_mouse_button(CoreMouseButton::Left),
        HidMouseButton::Left
    );
}
