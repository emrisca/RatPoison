//! Multi-monitor mixed-DPI test matrix exercising `ratpoison-core` geometry, layout,
//! grid, subgrid, and nudge APIs across dual/quad negative-origin configurations,
//! fractional DPI scale factors (1.0x, 1.25x, 1.5x, 2.0x Retina), portrait/landscape
//! orientations, and verifying <= 0.5 logical px / <= 1 physical px coordinate precision.

use ratpoison_core::{
    clamp_logical_to_monitor, generate_grid, generate_subgrid, logical_to_physical,
    logical_to_physical_clamped, nudge_by_vector, nudge_in_direction, physical_to_logical,
    CoreEngine, Direction, EngineEffect, EngineEvent, EngineMode, GridConfig, LabelOrdering,
    LabelScheme, LogicalPoint, LogicalRect, MonitorInfo, MonitorLayout, MonitorSwitch,
    NudgeClampMode, NudgeConfig, NudgeStep, Orientation, PhysicalPoint, PrefixMatch,
    RatPoisonConfig,
};

/// Build the canonical 4-monitor mixed-DPI QA matrix covering:
/// 1. Primary macOS Retina 2.0x (`1512x982` logical -> `3024x1964` physical) at `(0, 0)`
/// 2. Left 1080p 1.0x display (`1920x1080` logical -> `1920x1080` physical) at negative X `(-1920, -49)`
/// 3. Top 1440p 1.5x fractional display (`2560x1440` physical -> `1706.6667x960` or `2560x1440` logical -> `3840x2160` physical) at negative Y `(-200, -1440)`
/// 4. Right Vertical Portrait 1.25x display (`1080x1920` logical -> `1350x2400` physical) at `(1512, -300)`
fn canonical_quad_mixed_dpi_matrix() -> MonitorLayout {
    let primary_retina = MonitorInfo::from_logical_and_scale(
        "RETINA-2X",
        "Built-in Retina Display (3024x1964 @ 2.0x)",
        LogicalRect::new(0.0, 0.0, 1512.0, 982.0),
        PhysicalPoint::new(0, 0),
        2.0,
        true,
    )
    .expect("primary retina valid");

    let left_1080p = MonitorInfo::from_logical_and_scale(
        "LEFT-1X",
        "External 1080p Left (-X, -Y @ 1.0x)",
        LogicalRect::new(-1920.0, -49.0, 1920.0, 1080.0),
        PhysicalPoint::new(-1920, -49),
        1.0,
        false,
    )
    .expect("left 1080p valid");

    let top_150_pct = MonitorInfo::from_logical_and_scale(
        "TOP-150",
        "Top QHD Display (-Y @ 1.5x)",
        LogicalRect::new(-200.0, -1440.0, 2560.0, 1440.0),
        PhysicalPoint::new(-300, -2160),
        1.5,
        false,
    )
    .expect("top 1.5x valid");

    let right_portrait_125 = MonitorInfo::from_logical_and_scale(
        "RIGHT-PORTRAIT-125",
        "Right Vertical Portrait (1080x1920 @ 1.25x)",
        LogicalRect::new(1512.0, -300.0, 1080.0, 1920.0),
        PhysicalPoint::new(3024, -375),
        1.25,
        false,
    )
    .expect("right portrait 1.25x valid");

    MonitorLayout::new(vec![
        left_1080p,
        primary_retina,
        top_150_pct,
        right_portrait_125,
    ])
    .expect("quad mixed-dpi matrix layout valid")
}

#[test]
fn matrix_monitor_dimensions_orientations_and_negative_origins() {
    let layout = canonical_quad_mixed_dpi_matrix();
    assert_eq!(layout.len(), 4);

    let retina = layout.by_id("RETINA-2X").unwrap();
    assert_eq!(retina.physical_rect.width, 3024);
    assert_eq!(retina.physical_rect.height, 1964);
    assert_eq!(retina.orientation(), Orientation::Landscape);

    let left = layout.by_id("LEFT-1X").unwrap();
    assert_eq!(left.logical_rect.x, -1920.0);
    assert_eq!(left.logical_rect.y, -49.0);
    assert_eq!(left.physical_rect.x, -1920);
    assert_eq!(left.physical_rect.y, -49);
    assert_eq!(left.orientation(), Orientation::Landscape);

    let top = layout.by_id("TOP-150").unwrap();
    assert_eq!(top.physical_rect.width, 3840);
    assert_eq!(top.physical_rect.height, 2160);
    assert_eq!(top.orientation(), Orientation::Landscape);

    let portrait = layout.by_id("RIGHT-PORTRAIT-125").unwrap();
    assert_eq!(portrait.physical_rect.width, 1350);
    assert_eq!(portrait.physical_rect.height, 2400);
    assert_eq!(portrait.orientation(), Orientation::Portrait);
}

#[test]
fn logical_physical_roundtrip_and_precision_bounds_across_all_dpi_scales() {
    let layout = canonical_quad_mixed_dpi_matrix();

    for monitor in layout.monitors() {
        let scale = monitor.scale_factor;
        let lrect = monitor.logical_rect;
        let prect = monitor.physical_rect;

        // 1. Every sampled physical pixel must round-trip cleanly:
        //    logical_to_physical(physical_to_logical(p)) == p
        let phys_samples = [
            PhysicalPoint::new(prect.x, prect.y),
            PhysicalPoint::new(prect.x + 1, prect.y + 1),
            PhysicalPoint::new(
                prect.x + (prect.width / 3) as i32,
                prect.y + (prect.height / 3) as i32,
            ),
            PhysicalPoint::new(
                prect.x + (prect.width / 2) as i32,
                prect.y + (prect.height / 2) as i32,
            ),
            PhysicalPoint::new(
                prect.x + ((prect.width * 3) / 4) as i32,
                prect.y + ((prect.height * 3) / 4) as i32,
            ),
            PhysicalPoint::new(prect.max_x_inclusive(), prect.max_y_inclusive()),
        ];

        for p in phys_samples {
            assert!(
                prect.contains_point(p),
                "physical sample {p:?} must be inside {}",
                monitor.id
            );
            let logical = physical_to_logical(p, monitor);
            let roundtrip = logical_to_physical(logical, monitor);
            assert_eq!(
                roundtrip, p,
                "physical -> logical -> physical roundtrip drifted on monitor {}",
                monitor.id
            );
        }

        // 2. Arbitrary fractional logical coordinates inside the monitor must round to
        //    within <= 0.5 physical pixels (and round-trip back to within <= 0.5 / scale_factor
        //    <= 0.5 logical px for scale_factor >= 1.0).
        for step_x in 0..=20 {
            for step_y in 0..=20 {
                let fx = step_x as f64 / 21.0;
                let fy = step_y as f64 / 21.0;
                let logical_pt = LogicalPoint::new(
                    lrect.x + fx * (lrect.width - 1.0),
                    lrect.y + fy * (lrect.height - 1.0),
                );
                let phys_pt = logical_to_physical(logical_pt, monitor);
                let exact_px = prect.x as f64 + (logical_pt.x - lrect.x) * scale;
                let exact_py = prect.y as f64 + (logical_pt.y - lrect.y) * scale;

                let phys_err_x = (phys_pt.x as f64 - exact_px).abs();
                let phys_err_y = (phys_pt.y as f64 - exact_py).abs();
                assert!(
                    phys_err_x <= 0.5 + 1e-9 && phys_err_y <= 0.5 + 1e-9,
                    "physical rounding error exceeded 0.5 px on {}: ({phys_err_x}, {phys_err_y})",
                    monitor.id
                );

                let back_logical = physical_to_logical(phys_pt, monitor);
                let log_err_x = (back_logical.x - logical_pt.x).abs();
                let log_err_y = (back_logical.y - logical_pt.y).abs();
                let max_logical_err = 0.5 / scale + 1e-9;
                assert!(
                    log_err_x <= max_logical_err && log_err_y <= max_logical_err,
                    "logical roundtrip error exceeded 0.5/scale ({max_logical_err}) on {}: ({log_err_x}, {log_err_y})",
                    monitor.id
                );
                assert!(
                    log_err_x <= 0.5 + 1e-9 && log_err_y <= 0.5 + 1e-9,
                    "logical roundtrip error exceeded 0.5 logical px on {}",
                    monitor.id
                );
            }
        }
    }
}

#[test]
fn grid_and_subgrid_cell_center_precision_across_mixed_dpi_displays() {
    let layout = canonical_quad_mixed_dpi_matrix();
    let configs = [
        GridConfig {
            cols: 16,
            rows: 12,
            subgrid_cols: 3,
            subgrid_rows: 3,
            ..Default::default()
        },
        GridConfig {
            cols: 26,
            rows: 26,
            subgrid_cols: 4,
            subgrid_rows: 4,
            ..Default::default()
        },
        GridConfig {
            cols: 11,
            rows: 7, // prime dimensions that produce non-integer logical cell widths/heights
            subgrid_cols: 3,
            subgrid_rows: 3,
            ..Default::default()
        },
    ];

    for monitor in layout.monitors() {
        let lrect = monitor.logical_rect;
        let scale = monitor.scale_factor;

        for cfg in &configs {
            let grid = generate_grid(monitor, cfg).expect("grid generation succeeds");
            assert_eq!(
                grid.cells.len(),
                (cfg.cols as usize) * (cfg.rows as usize)
            );

            // Top-left and bottom-right boundary invariants
            let first = &grid.cells[0];
            assert!((first.rect.x - lrect.x).abs() < 1e-12);
            assert!((first.rect.y - lrect.y).abs() < 1e-12);

            let last = grid.cells.last().unwrap();
            assert!((last.rect.right() - lrect.right()).abs() < 1e-12);
            assert!((last.rect.bottom() - lrect.bottom()).abs() < 1e-12);

            for (idx, cell) in grid.cells.iter().enumerate() {
                let row = (idx / cfg.cols as usize) as u16;
                let col = (idx % cfg.cols as usize) as u16;
                assert_eq!(cell.row, row);
                assert_eq!(cell.col, col);

                // Theoretical cell center in monitor-global logical coordinates:
                let expected_cx =
                    lrect.x + ((col as f64 + 0.5) * lrect.width) / (cfg.cols as f64);
                let expected_cy =
                    lrect.y + ((row as f64 + 0.5) * lrect.height) / (cfg.rows as f64);

                assert!(
                    (cell.center.x - expected_cx).abs() < 1e-9
                        && (cell.center.y - expected_cy).abs() < 1e-9,
                    "grid cell center drift on {} cell {}",
                    monitor.id,
                    cell.label
                );

                // Physical pixel projection must be within <= 1 physical pixel (and <= 0.5 px rounding)
                let phys_center = logical_to_physical_clamped(cell.center, monitor);
                assert!(
                    monitor.physical_rect.contains_point(phys_center),
                    "grid cell physical center {phys_center:?} escaped monitor {}",
                    monitor.id
                );
                let expected_phys_x =
                    monitor.physical_rect.x as f64 + (expected_cx - lrect.x) * scale;
                let expected_phys_y =
                    monitor.physical_rect.y as f64 + (expected_cy - lrect.y) * scale;
                assert!((phys_center.x as f64 - expected_phys_x).abs() <= 0.5 + 1e-9);
                assert!((phys_center.y as f64 - expected_phys_y).abs() <= 0.5 + 1e-9);

                // Test nested subgrid precision on corner, edge, and center cells
                if (row == 0 && col == 0)
                    || (row == cfg.rows / 2 && col == cfg.cols / 2)
                    || (row + 1 == cfg.rows && col + 1 == cfg.cols)
                {
                    let subgrid =
                        generate_subgrid(cell, &monitor.id, cfg).expect("subgrid generation");
                    assert_eq!(
                        subgrid.cells.len(),
                        (cfg.subgrid_cols as usize) * (cfg.subgrid_rows as usize)
                    );

                    // Subgrid center cell (when odd 3x3) must align with parent cell center
                    if cfg.subgrid_cols == 3 && cfg.subgrid_rows == 3 {
                        let sub_center = &subgrid.cells[4];
                        assert!((sub_center.center.x - cell.center.x).abs() < 1e-9);
                        assert!((sub_center.center.y - cell.center.y).abs() < 1e-9);
                    }

                    for sub_cell in &subgrid.cells {
                        let expected_scx = cell.rect.x
                            + ((sub_cell.col as f64 + 0.5) * cell.rect.width)
                                / (cfg.subgrid_cols as f64);
                        let expected_scy = cell.rect.y
                            + ((sub_cell.row as f64 + 0.5) * cell.rect.height)
                                / (cfg.subgrid_rows as f64);
                        assert!((sub_cell.center.x - expected_scx).abs() < 1e-9);
                        assert!((sub_cell.center.y - expected_scy).abs() < 1e-9);

                        let sub_phys = logical_to_physical_clamped(sub_cell.center, monitor);
                        assert!(monitor.physical_rect.contains_point(sub_phys));
                        let exp_spx =
                            monitor.physical_rect.x as f64 + (expected_scx - lrect.x) * scale;
                        let exp_spy =
                            monitor.physical_rect.y as f64 + (expected_scy - lrect.y) * scale;
                        assert!((sub_phys.x as f64 - exp_spx).abs() <= 0.5 + 1e-9);
                        assert!((sub_phys.y as f64 - exp_spy).abs() <= 0.5 + 1e-9);
                    }
                }
            }
        }
    }
}

#[test]
fn cross_monitor_switching_and_boundary_clamping_prevent_seam_spillover() {
    let layout = canonical_quad_mixed_dpi_matrix();

    // 1. Half-open boundary containment: seam points at x = 0.0 (between LEFT-1X and RETINA-2X)
    //    and x = 1512.0 (between RETINA-2X and RIGHT-PORTRAIT-125) are claimed by exactly ONE monitor.
    let seam_left_primary = LogicalPoint::new(0.0, 200.0);
    let owners_left_seam: Vec<&str> = layout
        .monitors()
        .iter()
        .filter(|m| m.logical_rect.contains_point(seam_left_primary))
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(owners_left_seam, vec!["RETINA-2X"]);

    let seam_primary_right = LogicalPoint::new(1512.0, 200.0);
    let owners_right_seam: Vec<&str> = layout
        .monitors()
        .iter()
        .filter(|m| m.logical_rect.contains_point(seam_primary_right))
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(owners_right_seam, vec!["RIGHT-PORTRAIT-125"]);

    // 2. Clamping an out-of-bounds point to each monitor keeps the point strictly inside that monitor
    //    and maps to its inclusive max physical pixel without spilling onto a neighbor.
    for monitor in layout.monitors() {
        let far_bottom_right = LogicalPoint::new(99_999.0, 99_999.0);
        let clamped = clamp_logical_to_monitor(far_bottom_right, monitor);
        assert!(
            monitor.logical_rect.contains_point(clamped),
            "clamped point {clamped:?} fell outside half-open rect of {}",
            monitor.id
        );
        assert_eq!(
            layout.monitor_at_logical(clamped).map(|m| m.id.as_str()),
            Some(monitor.id.as_str())
        );
        let phys = logical_to_physical(clamped, monitor);
        assert_eq!(phys.x, monitor.physical_rect.max_x_inclusive());
        assert_eq!(phys.y, monitor.physical_rect.max_y_inclusive());
    }

    // 3. Directional neighbor switching from RETINA-2X reaches LEFT-1X, TOP-150, and RIGHT-PORTRAIT-125.
    assert_eq!(
        layout.neighbor_in_direction("RETINA-2X", Direction::Left),
        Some("LEFT-1X")
    );
    assert_eq!(
        layout.neighbor_in_direction("RETINA-2X", Direction::Up),
        Some("TOP-150")
    );
    assert_eq!(
        layout.neighbor_in_direction("RETINA-2X", Direction::Right),
        Some("RIGHT-PORTRAIT-125")
    );
    assert_eq!(
        layout.neighbor_in_direction("RETINA-2X", Direction::Down),
        None
    );

    // 4. Deterministic spatial cycling (Next / Prev) visits all 4 monitors in left-to-right order
    let spatial_ids: Vec<&str> = layout
        .spatially_ordered_monitors()
        .into_iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(
        spatial_ids,
        vec![
            "LEFT-1X",
            "TOP-150",
            "RETINA-2X",
            "RIGHT-PORTRAIT-125"
        ]
    );
    assert_eq!(layout.next_monitor_id("LEFT-1X"), Some("TOP-150"));
    assert_eq!(layout.next_monitor_id("TOP-150"), Some("RETINA-2X"));
    assert_eq!(
        layout.next_monitor_id("RETINA-2X"),
        Some("RIGHT-PORTRAIT-125")
    );
    assert_eq!(
        layout.next_monitor_id("RIGHT-PORTRAIT-125"),
        Some("LEFT-1X")
    );
    assert_eq!(
        layout.prev_monitor_id("LEFT-1X"),
        Some("RIGHT-PORTRAIT-125")
    );
}

#[test]
fn nudge_precision_and_cross_monitor_traversal_across_mixed_dpi_seams() {
    let layout = canonical_quad_mixed_dpi_matrix();
    let retina = layout.by_id("RETINA-2X").unwrap();

    // 1. Sub-pixel Fine (1.5px), Normal (8.0px), and Coarse (24.0px) nudge precision on Retina 2.0x
    let nudge_cfg = NudgeConfig {
        coarse_px: 24.0,
        normal_px: 8.0,
        fine_px: 1.5,
        clamp_mode: NudgeClampMode::ActiveMonitor,
        ..Default::default()
    };

    let start = LogicalPoint::new(500.0, 400.0);
    let (_, after_fine_right) = nudge_in_direction(
        start,
        Direction::Right,
        NudgeStep::Fine,
        &nudge_cfg,
        retina,
        &layout,
    );
    assert!((after_fine_right.x - 501.5).abs() < 1e-12);
    assert!((after_fine_right.y - 400.0).abs() < 1e-12);
    // On 2.0x Retina, +1.5 logical px equals exactly +3 physical px (1000 -> 1003)
    assert_eq!(
        logical_to_physical(after_fine_right, retina),
        PhysicalPoint::new(1003, 800)
    );

    let (_, after_normal_up) = nudge_in_direction(
        after_fine_right,
        Direction::Up,
        NudgeStep::Normal,
        &nudge_cfg,
        retina,
        &layout,
    );
    assert!((after_normal_up.x - 501.5).abs() < 1e-12);
    assert!((after_normal_up.y - 392.0).abs() < 1e-12);

    let (_, after_coarse_down) = nudge_in_direction(
        after_normal_up,
        Direction::Down,
        NudgeStep::Coarse,
        &nudge_cfg,
        retina,
        &layout,
    );
    assert!((after_coarse_down.y - 416.0).abs() < 1e-12);

    // 2. Diagonal vector nudge precision
    let (_, diag_pt) = nudge_by_vector(
        LogicalPoint::new(100.0, 100.0),
        0.6,
        -0.8,
        NudgeStep::Normal,
        &nudge_cfg,
        retina,
        &layout,
    );
    assert!((diag_pt.x - 104.8).abs() < 1e-12);
    assert!((diag_pt.y - 93.6).abs() < 1e-12);

    // 3. ActiveMonitor clamp prevents crossing into LEFT-1X (-X) or TOP-150 (-Y)
    let near_top_left = LogicalPoint::new(5.0, 5.0);
    let (mon_clamped_x, pt_clamped_x) = nudge_in_direction(
        near_top_left,
        Direction::Left,
        NudgeStep::Coarse,
        &nudge_cfg,
        retina,
        &layout,
    );
    assert_eq!(mon_clamped_x, "RETINA-2X");
    assert_eq!(pt_clamped_x, LogicalPoint::new(0.0, 5.0));

    // 4. VirtualDesktop clamp crosses cleanly into LEFT-1X (-X) and TOP-150 (-Y),
    //    and snaps out of L-shaped dead space!
    let vd_cfg = NudgeConfig {
        clamp_mode: NudgeClampMode::VirtualDesktop,
        ..nudge_cfg
    };
    let (mon_cross_left, pt_cross_left) = nudge_in_direction(
        near_top_left,
        Direction::Left,
        NudgeStep::Coarse,
        &vd_cfg,
        retina,
        &layout,
    );
    assert_eq!(mon_cross_left, "LEFT-1X");
    assert_eq!(pt_cross_left, LogicalPoint::new(-19.0, 5.0));

    let (mon_cross_up, pt_cross_up) = nudge_in_direction(
        near_top_left,
        Direction::Up,
        NudgeStep::Coarse,
        &vd_cfg,
        retina,
        &layout,
    );
    assert_eq!(mon_cross_up, "TOP-150");
    assert_eq!(pt_cross_up, LogicalPoint::new(5.0, -19.0));

    // Dead-space point at (-1500, -1000) (above LEFT-1X and left of TOP-150) clamps into nearest valid monitor
    let (dead_owner, dead_clamped) = layout
        .clamp_logical_to_virtual_desktop(LogicalPoint::new(-1500.0, -1000.0))
        .unwrap();
    let owner_info = layout.by_id(&dead_owner).unwrap();
    assert!(owner_info.logical_rect.contains_point(dead_clamped));
}

#[test]
fn engine_state_machine_switches_monitors_and_scales_coordinates_accurately() {
    let layout = canonical_quad_mixed_dpi_matrix();
    let mut engine = CoreEngine::new(RatPoisonConfig::default(), layout.clone());

    // Activate Grid on default primary (RETINA-2X)
    let out = engine.handle_event(EngineEvent::ActivateGrid {
        monitor_id: None,
        preset_id: None,
    });
    assert_eq!(out.snapshot.mode, EngineMode::Grid);
    assert_eq!(
        out.snapshot.active_monitor_id.as_deref(),
        Some("RETINA-2X")
    );

    // Switch Left to negative-origin 1080p monitor (LEFT-1X)
    let out_left = engine.handle_event(EngineEvent::SwitchMonitor {
        switch: MonitorSwitch::Direction {
            direction: Direction::Left,
        },
    });
    assert_eq!(
        out_left.snapshot.active_monitor_id.as_deref(),
        Some("LEFT-1X")
    );
    assert!(out_left.effects.iter().any(|e| matches!(
        e,
        EngineEffect::MoveCursor {
            monitor_id,
            logical,
            physical,
        } if monitor_id == "LEFT-1X"
            && (logical.x - (-960.0)).abs() < 1e-9
            && (logical.y - 491.0).abs() < 1e-9
            && *physical == PhysicalPoint::new(-960, 491)
    )));

    // Switch to Right Vertical Portrait 1.25x display by ID and select grid cell "aa" -> subgrid "s"
    let out_portrait = engine.handle_event(EngineEvent::SwitchMonitor {
        switch: MonitorSwitch::ById {
            id: "RIGHT-PORTRAIT-125".to_string(),
        },
    });
    assert_eq!(
        out_portrait.snapshot.active_monitor_id.as_deref(),
        Some("RIGHT-PORTRAIT-125")
    );
    engine.handle_event(EngineEvent::KeyChar { ch: 'a' });
    let out_cell = engine.handle_event(EngineEvent::KeyChar { ch: 'a' });
    assert_eq!(out_cell.snapshot.mode, EngineMode::Subgrid);

    let out_sub = engine.handle_event(EngineEvent::KeyChar { ch: 's' });
    assert_eq!(out_sub.snapshot.mode, EngineMode::Nudge);

    // Verify the cursor sits inside RIGHT-PORTRAIT-125's top-left cell center and physical pixel matches 1.25x scale
    let portrait_mon = layout.by_id("RIGHT-PORTRAIT-125").unwrap();
    let cursor = engine.cursor_logical();
    assert!(portrait_mon.logical_rect.contains_point(cursor));
    let expected_cx = 1512.0 + (1080.0 / 16.0) * 0.5; // 1545.75
    let expected_cy = -300.0 + (1920.0 / 12.0) * 0.5; // -220.0
    assert!((cursor.x - expected_cx).abs() < 1e-9);
    assert!((cursor.y - expected_cy).abs() < 1e-9);
}

#[test]
fn large_case_sensitive_grid_over_676_cells_on_4k_display() {
    let layout = canonical_quad_mixed_dpi_matrix();
    let top_qhd = layout.by_id("TOP-150").unwrap();

    // 40x30 = 1,200 cells (> 676 limit of 26x26 lowercase alphabet) using mixed-case 52-key alphabet
    let mut scheme = LabelScheme::mixed_case_az();
    scheme.ordering = LabelOrdering::Sequential;
    let cfg = GridConfig {
        cols: 40,
        rows: 30,
        label_length: 2,
        label_scheme: scheme,
        ..Default::default()
    };

    let grid = generate_grid(top_qhd, &cfg).expect("1200-cell mixed-case grid succeeds");
    assert_eq!(grid.cells.len(), 1200);

    // Verify every cell center maps uniquely and accurately on the 1.5x monitor
    let first_cell = &grid.cells[0];
    let last_cell = grid.cells.last().unwrap();
    assert!(matches!(
        grid.match_prefix(&first_cell.label),
        PrefixMatch::Exact(c) if c.row == 0 && c.col == 0
    ));
    assert!(matches!(
        grid.match_prefix(&last_cell.label),
        PrefixMatch::Exact(c) if c.row == 29 && c.col == 39
    ));
}
