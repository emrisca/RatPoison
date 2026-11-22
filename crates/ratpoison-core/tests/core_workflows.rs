//! End-to-end public API tests for `ratpoison-core`.

use ratpoison_core::{
    generate_grid, generate_subgrid, logical_to_physical, physical_to_logical, CoreEngine,
    Direction, EngineEffect, EngineEvent, EngineMode, FreeKey, GridConfig, LabelBadge,
    LabelOrdering, LabelScheme, LogicalPoint, LogicalRect, MonitorInfo, MonitorLayout,
    MouseActionKind, MouseButton, NudgeStep, PhysicalPoint, QuickSelectAction, RatPoisonConfig,
    SpeedModifier, WorkflowPreset,
};

fn build_mixed_dpi_layout() -> MonitorLayout {
    let left_portrait = MonitorInfo::from_logical_and_scale(
        "LEFT-PORTRAIT",
        "Vertical 150%",
        LogicalRect::new(-720.0, -200.0, 720.0, 1280.0),
        PhysicalPoint::new(-1080, -300),
        1.5,
        false,
    )
    .unwrap();
    let center_retina = MonitorInfo::from_logical_and_scale(
        "CENTER-2X",
        "Retina Primary",
        LogicalRect::new(0.0, 0.0, 1440.0, 900.0),
        PhysicalPoint::new(0, 0),
        2.0,
        true,
    )
    .unwrap();
    let right_125 = MonitorInfo::from_logical_and_scale(
        "RIGHT-125",
        "Office 125%",
        LogicalRect::new(1440.0, 36.0, 1536.0, 864.0),
        PhysicalPoint::new(2880, 72),
        1.25,
        false,
    )
    .unwrap();
    MonitorLayout::new(vec![left_portrait, center_retina, right_125]).unwrap()
}

#[test]
fn sequential_mixed_case_grid_over_1000_cells_maps_cleanly_on_portrait_negative_origin() {
    let layout = build_mixed_dpi_layout();
    let left = layout.by_id("LEFT-PORTRAIT").unwrap();

    let mut scheme = LabelScheme::mixed_case_az();
    scheme.ordering = LabelOrdering::Sequential;
    let cfg = GridConfig {
        cols: 20,
        rows: 55, // 1,100 cells (> 676 and rows > 52 so Sequential handles it cleanly!)
        label_length: 2,
        label_scheme: scheme,
        subgrid_cols: 4,
        subgrid_rows: 4,
        subgrid_keys: vec![], // forces automatic fallback to label_scheme
        ..Default::default()
    };

    let grid = generate_grid(left, &cfg).unwrap();
    assert_eq!(grid.cells.len(), 1100);

    // Subgrid inside the top-left cell on negative origin
    let sub = generate_subgrid(&grid.cells[0], &left.id, &cfg).unwrap();
    assert_eq!(sub.cells.len(), 16);

    // Every subgrid center maps to a physical pixel inside LEFT-PORTRAIT and round-trips accurately
    for cell in &sub.cells {
        let phys = logical_to_physical(cell.center, left);
        assert!(left.physical_rect.contains_point(phys));
        let back = physical_to_logical(phys, left);
        assert_eq!(logical_to_physical(back, left), phys);
    }
}

#[test]
fn app_scoped_subgrid_then_free_mode_preset_activates_automatically() {
    let layout = build_mixed_dpi_layout();
    let mut cfg = RatPoisonConfig::default();

    let mut figma_preset = WorkflowPreset::precision_design();
    figma_preset.id = "figma_auto".to_string();
    figma_preset.quick_select_action = QuickSelectAction::SubgridThenFreeMode;
    figma_preset.scope.app_ids = vec!["com.figma.Desktop".to_string()];
    cfg.presets.push(figma_preset);

    let mut engine = CoreEngine::new(cfg, layout);
    engine.handle_event(EngineEvent::ActiveContextChanged {
        app_id: Some("com.figma.Desktop".to_string()),
        monitor_id: Some("CENTER-2X".to_string()),
    });

    // Activate Grid without explicit preset_id -> automatically resolves figma_auto!
    let out = engine.handle_event(EngineEvent::ActivateGrid {
        monitor_id: None,
        preset_id: None,
    });
    assert_eq!(out.snapshot.mode, EngineMode::Grid);
    assert_eq!(out.snapshot.active_preset_id.as_deref(), Some("figma_auto"));

    // Select primary cell "aa" -> enters Subgrid
    engine.handle_event(EngineEvent::KeyChar { ch: 'a' });
    let out = engine.handle_event(EngineEvent::KeyChar { ch: 'a' });
    assert_eq!(out.snapshot.mode, EngineMode::Subgrid);

    // Select subgrid cell "s" -> automatically hides overlay and hands off to FreeMode!
    let out = engine.handle_event(EngineEvent::KeyChar { ch: 's' });
    assert_eq!(out.snapshot.mode, EngineMode::FreeMode);
    assert!(!out.snapshot.visible);
    assert!(out
        .effects
        .iter()
        .any(|e| matches!(e, EngineEffect::HideOverlay)));
}

#[test]
fn diagonal_free_mode_movement_is_unit_normalized() {
    let layout = build_mixed_dpi_layout();
    let cfg = RatPoisonConfig::default();
    let mut engine = CoreEngine::new(cfg.clone(), layout);
    engine.handle_event(EngineEvent::SyncCursor {
        logical: LogicalPoint::new(500.0, 400.0),
    });
    engine.handle_event(EngineEvent::ActivateFreeMode { preset_id: None });

    let start = engine.cursor_logical();
    engine.handle_event(EngineEvent::FreeModeKeyDown {
        key: FreeKey::MoveRight,
    });
    engine.handle_event(EngineEvent::FreeModeKeyDown {
        key: FreeKey::MoveDown,
    });
    engine.handle_event(EngineEvent::FreeModeTick { dt_ms: 100 });

    let end = engine.cursor_logical();
    let expected_speed = cfg.free_mode.velocity_px_per_sec(SpeedModifier::Normal, 100);
    let expected_dist = expected_speed * 0.1;
    let actual_dist = ((end.x - start.x).powi(2) + (end.y - start.y).powi(2)).sqrt();

    assert!(
        (actual_dist - expected_dist).abs() < 1e-6,
        "expected diagonal distance {expected_dist}, got {actual_dist}"
    );
    assert!(((end.x - start.x) - (end.y - start.y)).abs() < 1e-9);
}

#[test]
fn label_mode_auto_assigns_labels_and_respects_auto_click_false() {
    let layout = build_mixed_dpi_layout();
    let mut cfg = RatPoisonConfig::default();
    cfg.label_mode.auto_click = false;

    let mut engine = CoreEngine::new(cfg, layout);
    engine.handle_event(EngineEvent::ActivateLabelMode);

    let badges = vec![
        LabelBadge {
            id: "a11y-1".to_string(),
            label: String::new(), // auto-assigned
            title: "File".to_string(),
            role: "AXMenuBarItem".to_string(),
            rect: LogicalRect::new(20.0, 5.0, 40.0, 20.0),
            center: LogicalPoint::new(40.0, 15.0),
            monitor_id: "CENTER-2X".to_string(),
        },
        LabelBadge {
            id: "a11y-2".to_string(),
            label: String::new(), // auto-assigned
            title: "Edit".to_string(),
            role: "AXMenuBarItem".to_string(),
            rect: LogicalRect::new(70.0, 5.0, 40.0, 20.0),
            center: LogicalPoint::new(90.0, 15.0),
            monitor_id: "CENTER-2X".to_string(),
        },
    ];

    let loaded = engine.handle_event(EngineEvent::AccessibilityLabelsLoaded { badges });
    assert_eq!(loaded.snapshot.label_badges.len(), 2);
    let second_label = loaded.snapshot.label_badges[1].label.clone();
    assert!(!second_label.is_empty());

    // Type the assigned label -> moves without clicking (since auto_click = false)
    let mut last = loaded;
    for ch in second_label.chars() {
        last = engine.handle_event(EngineEvent::KeyChar { ch });
    }
    assert_eq!(last.snapshot.mode, EngineMode::Normal);
    assert_eq!(engine.cursor_logical(), LogicalPoint::new(90.0, 15.0));
    assert!(!last
        .effects
        .iter()
        .any(|e| matches!(e, EngineEffect::Click { .. })));
    assert!(last
        .effects
        .iter()
        .any(|e| matches!(e, EngineEffect::MoveCursor { .. })));
}

#[test]
fn hotplug_monitor_removal_while_in_grid_recovers_cleanly() {
    let layout = build_mixed_dpi_layout();
    let mut engine = CoreEngine::new(RatPoisonConfig::default(), layout);

    engine.handle_event(EngineEvent::ActivateGrid {
        monitor_id: Some("RIGHT-125".to_string()),
        preset_id: None,
    });
    assert_eq!(engine.active_monitor_id(), Some("RIGHT-125"));

    // Unplug RIGHT-125 while Grid is visible
    let single = MonitorLayout::new(vec![MonitorInfo::from_logical_and_scale(
        "CENTER-2X",
        "Retina Primary",
        LogicalRect::new(0.0, 0.0, 1440.0, 900.0),
        PhysicalPoint::new(0, 0),
        2.0,
        true,
    )
    .unwrap()])
    .unwrap();

    let out = engine.handle_event(EngineEvent::MonitorsUpdated { layout: single });
    assert_eq!(out.snapshot.mode, EngineMode::Grid);
    assert_eq!(out.snapshot.active_monitor_id.as_deref(), Some("CENTER-2X"));
    assert!(out
        .effects
        .iter()
        .any(|e| matches!(e, EngineEffect::ShowOverlay { monitor_id } if monitor_id == "CENTER-2X")));

    // Nudge + right-click works cleanly on recovered monitor
    engine.handle_event(EngineEvent::Nudge {
        direction: Direction::Down,
        step: NudgeStep::Coarse,
    });
    let click = engine.handle_event(EngineEvent::MouseAction {
        action: MouseActionKind::RightClick,
    });
    assert!(click.effects.iter().any(|e| matches!(
        e,
        EngineEffect::Click {
            button: MouseButton::Right,
            count: 1,
            ..
        }
    )));
}

#[test]
fn free_key_qwerty_char_helper_maps_ijkl_and_asdf() {
    assert_eq!(FreeKey::from_qwerty_char('I'), Some(FreeKey::MoveUp));
    assert_eq!(FreeKey::from_qwerty_char('j'), Some(FreeKey::MoveLeft));
    assert_eq!(FreeKey::from_qwerty_char('k'), Some(FreeKey::MoveDown));
    assert_eq!(FreeKey::from_qwerty_char('L'), Some(FreeKey::MoveRight));
    assert_eq!(FreeKey::from_qwerty_char('a'), Some(FreeKey::SpeedSlow));
    assert_eq!(FreeKey::from_qwerty_char('s'), Some(FreeKey::SpeedNormal));
    assert_eq!(FreeKey::from_qwerty_char('d'), Some(FreeKey::SpeedFast));
    assert_eq!(FreeKey::from_qwerty_char('f'), Some(FreeKey::SpeedTurbo));
    assert_eq!(FreeKey::from_qwerty_char('z'), None);
}
