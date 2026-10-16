//! Regression coverage for Stage drag restoration.
use super::*;

fn drag_layout(floating: bool, named: bool) -> Layout<TestWindow> {
    let options = Options {
        animations: clarion_config::Animations {
            off: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut ops = vec![Op::AddOutput(1)];
    if named {
        ops.push(Op::AddNamedWorkspace {
            ws_name: 1,
            output_name: Some(1),
            layout_config: None,
        });
        ops.push(Op::FocusWorkspace(1));
    }
    let mut params = TestWindowParams::new(0);
    params.is_floating = floating;
    ops.extend([
        Op::AddWindow { params },
        Op::Communicate(0),
        Op::CompleteAnimations,
    ]);
    let mut layout = check_ops_with_options(options, ops);
    layout.open_overview();
    Op::CompleteAnimations.apply(&mut layout);
    layout
}

fn workspace_of(layout: &Layout<TestWindow>, id: usize) -> WorkspaceId {
    layout
        .workspaces()
        .find(|(_, _, ws)| ws.has_window(&id))
        .unwrap()
        .2
        .id()
}

fn start_drag(layout: &mut Layout<TestWindow>, window: usize, target: Point<f64, Logical>) {
    let output = layout.outputs().next().unwrap().clone();
    let start = layout
        .monitors()
        .find_map(|mon| {
            mon.workspaces_with_render_geo().find_map(|(ws, geo)| {
                ws.tiles_with_render_positions().find_map(|(tile, pos, _)| {
                    (tile.window().id() == &window).then(|| {
                        geo.loc
                            + (pos
                                + tile.window_loc()
                                + tile.window_size().to_point().downscale(2.))
                            .upscale(mon.workspace_render_zoom(geo))
                    })
                })
            })
        })
        .unwrap();
    assert!(layout.interactive_move_begin(window, &output, start));
    assert!(layout.interactive_move_update(&window, target - start, output, target));
    assert!(matches!(
        layout.interactive_move,
        Some(InteractiveMoveState::Moving(_))
    ));
}

fn card_center(layout: &Layout<TestWindow>, id: WorkspaceId) -> Point<f64, Logical> {
    let mon = layout.active_monitor_ref().unwrap();
    let geo = mon
        .workspaces_render_geo()
        .nth(mon.idx_of_ws(id).unwrap())
        .unwrap();
    geo.loc + geo.size.to_point().downscale(2.)
}

#[test]
fn stage_blank_release_restores_non_current_unnamed_source_by_stable_id() {
    let mut layout = drag_layout(false, false);
    let source = workspace_of(&layout, 0);
    Op::FocusWorkspaceDown.apply(&mut layout);
    Op::CompleteAnimations.apply(&mut layout);
    start_drag(&mut layout, 0, (0., 0.).into());
    assert!(
        layout.workspaces().any(|(_, _, ws)| ws.id() == source),
        "the empty source must survive internal extraction"
    );
    layout.interactive_move_end(&0);
    assert_eq!(workspace_of(&layout, 0), source);
    assert!(layout.core.borrow().stage_drag().window().is_none());
    layout.verify_invariants();
}

#[test]
fn stage_cancel_over_valid_card_restores_floating_source_position() {
    let mut layout = drag_layout(true, true);
    let source = workspace_of(&layout, 0);
    let before = layout.session_layout().windows[0].position;
    let target = layout
        .active_monitor_ref()
        .unwrap()
        .workspaces
        .last()
        .unwrap()
        .id();
    let point = card_center(&layout, target);
    start_drag(&mut layout, 0, point);
    layout.interactive_move_cancel(&0);
    Op::CompleteAnimations.apply(&mut layout);
    assert_eq!(workspace_of(&layout, 0), source);
    assert_eq!(layout.session_layout().windows[0].position, before);
    assert!(layout.core.borrow().stage_drag().window().is_none());
    layout.verify_invariants();
}

#[test]
fn stage_valid_drop_releases_source_pin_after_insertion() {
    let mut layout = drag_layout(false, false);
    let source = workspace_of(&layout, 0);
    Op::FocusWorkspaceDown.apply(&mut layout);
    Op::CompleteAnimations.apply(&mut layout);
    let target = layout.active_workspace().unwrap().id();
    let point = card_center(&layout, target);
    start_drag(&mut layout, 0, point);
    layout.interactive_move_end(&0);
    assert_eq!(workspace_of(&layout, 0), target);
    assert!(
        layout.core.borrow().stage_drag().window().is_none(),
        "a successful drop must release the origin"
    );
    Op::CompleteAnimations.apply(&mut layout);
    assert!(!layout.workspaces().any(|(_, _, ws)| ws.id() == source));
    layout.verify_invariants();
}

#[test]
fn stage_new_workspace_drop_uses_real_card_scale() {
    let mut layout = drag_layout(true, false);
    let mut options = (*layout.options).clone();
    options.animations.off = false;
    layout.update_options(options);
    let source = workspace_of(&layout, 0);
    let mon = layout.active_monitor_ref().unwrap();
    let height = mon.view_size().h;
    let point = (0..mon.view_size().w as i32)
        .find_map(|x| {
            let point = Point::from((x as f64, height - 38.));
            matches!(mon.stage_rail_under(point), Some(InsertWorkspace::NewAt(_))).then_some(point)
        })
        .unwrap();
    start_drag(&mut layout, 0, point);
    let mon = layout.active_monitor_ref().unwrap();
    let (_, geo) = mon.stage_drop_target(point).unwrap();
    assert!(
        mon.workspace_render_zoom(geo) > 0.01,
        "new workspace target needs real overview geometry"
    );
    layout.interactive_move_end(&0);
    assert_ne!(workspace_of(&layout, 0), source);
    let tile = layout
        .workspaces()
        .find(|(_, _, ws)| ws.has_window(&0))
        .unwrap()
        .2
        .tiles()
        .next()
        .unwrap();
    let offset = tile.render_offset();
    assert!(offset.x.is_finite() && offset.y.is_finite());
    assert!(
        offset.x.abs() < 100_000. && offset.y.abs() < 100_000.,
        "invalid miniature zoom produced offset {offset:?}"
    );
    assert!(layout.core.borrow().stage_drag().window().is_none());
    layout.verify_invariants();
}

#[test]
fn stage_cancel_restores_scrolling_column_and_tile_order() {
    let mut layout = drag_layout(false, true);
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::ConsumeWindowIntoColumn,
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::CompleteAnimations,
        ],
    );
    let order = |layout: &Layout<TestWindow>| {
        layout
            .workspaces()
            .find(|(_, _, ws)| ws.has_window(&0))
            .unwrap()
            .2
            .scrolling()
            .columns()
            .map(|col| {
                col.tiles()
                    .map(|(tile, _)| *tile.window().id())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    let before = order(&layout);
    start_drag(&mut layout, 0, (0., 0.).into());
    layout.interactive_move_cancel(&0);
    assert_eq!(order(&layout), before);
    layout.verify_invariants();
}

#[test]
fn stage_cancel_restores_dwindle_split_tree() {
    let mut layout = drag_layout(false, true);
    check_ops_on_layout(
        &mut layout,
        [
            Op::SwitchTilingMode(Some(clarion_ipc::TilingMode::Dwindle)),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::CompleteAnimations,
        ],
    );
    let tree = |layout: &Layout<TestWindow>| {
        layout
            .workspaces()
            .find(|(_, _, ws)| ws.has_window(&0))
            .unwrap()
            .2
            .dwindle
            .session_tree(&|id| *id)
    };
    let before = format!("{:?}", tree(&layout));
    start_drag(&mut layout, 0, (0., 0.).into());
    layout.interactive_move_cancel(&0);
    assert_eq!(format!("{:?}", tree(&layout)), before);
    layout.verify_invariants();
}

#[test]
fn stage_destroy_after_last_output_removal_releases_empty_source() {
    let mut layout = drag_layout(false, false);
    let output = layout.outputs().next().unwrap().clone();
    start_drag(&mut layout, 0, (0., 0.).into());
    layout.remove_output(&output);
    layout.remove_window(&0, Transaction::new());
    assert!(layout.core.borrow().stage_drag().window().is_none());
    assert!(layout
        .workspaces()
        .all(|(_, _, ws)| ws.has_windows_or_name()));
    Op::AddOutput(1).apply(&mut layout);
    layout.verify_invariants();
}

#[test]
fn stage_drag_begin_then_output_reconnect_keeps_unnamed_source() {
    for empty_above in [false, true] {
        let mut layout = drag_layout(false, false);
        if empty_above {
            let mut options = (*layout.options).clone();
            options.layout.empty_workspace_above_first = true;
            layout.update_options(options);
        }
        let source = workspace_of(&layout, 0);
        let old_output = layout.outputs().next().unwrap().clone();
        let old_generation = layout
            .core
            .borrow()
            .output(&old_output.name())
            .unwrap()
            .generation;
        assert!(layout.interactive_move_begin(0, &old_output, (640., 300.).into()));
        layout.remove_output(&old_output);
        Op::AddOutput(1).apply(&mut layout);
        let new_output = layout.outputs().next().unwrap().clone();
        let new_generation = layout
            .core
            .borrow()
            .output(&new_output.name())
            .unwrap()
            .generation;
        assert!(
            new_generation > old_generation,
            "same connector replacement must have a fresh identity"
        );
        assert_eq!(
            layout.core.borrow().stage_drag_destination(),
            Some(clarion_core::SpaceId(source.get())),
            "live stable source workspace outranks the replaced connector generation"
        );
        Op::FocusWorkspaceDown.apply(&mut layout);
        Op::CompleteAnimations.apply(&mut layout);
        assert!(layout.active_workspace().unwrap().id() != source);
        assert!(layout.interactive_move_update(
            &0,
            (640., 400.).into(),
            new_output,
            (0., 0.).into()
        ));
        assert!(layout.workspaces().any(|(_, _, ws)| ws.id() == source));
        layout.interactive_move_cancel(&0);
        assert_eq!(workspace_of(&layout, 0), source);
        layout.verify_invariants();
    }
}

#[test]
fn stage_cross_output_drop_keeps_target_workspace_and_releases_origin() {
    let mut layout = drag_layout(false, false);
    Op::AddOutput(2).apply(&mut layout);
    let output = layout
        .outputs()
        .find(|output| output.name() == "output2")
        .unwrap()
        .clone();
    let mon = layout.monitor_for_output(&output).unwrap();
    let target = mon.workspaces[mon.active_workspace_idx()].id();
    let geo = mon
        .workspaces_render_geo()
        .nth(mon.active_workspace_idx())
        .unwrap();
    let point = geo.loc + geo.size.to_point().downscale(2.);
    start_drag(&mut layout, 0, (0., 0.).into());
    assert!(layout.interactive_move_update(&0, (100., 100.).into(), output.clone(), point));
    layout.interactive_move_end(&0);
    assert_eq!(workspace_of(&layout, 0), target);
    assert_eq!(
        layout
            .workspaces()
            .find(|(_, _, ws)| ws.has_window(&0))
            .unwrap()
            .0
            .unwrap()
            .output(),
        &output
    );
    assert!(layout.core.borrow().stage_drag().window().is_none());
    Op::CompleteAnimations.apply(&mut layout);
    layout.verify_invariants();
}

#[test]
fn stage_drag_threshold_uses_screen_distance_at_fractional_scales() {
    for scale in [1., 1.25, 1.5] {
        let mut layout = check_ops([
            Op::AddScaledOutput {
                id: 1,
                scale,
                layout_config: None,
            },
            Op::AddWindow {
                params: TestWindowParams::new(0),
            },
            Op::Communicate(0),
            Op::CompleteAnimations,
        ]);
        layout.open_overview();
        Op::CompleteAnimations.apply(&mut layout);
        let output = layout.outputs().next().unwrap().clone();
        let source = workspace_of(&layout, 0);
        let start = card_center(&layout, source);
        assert!(layout.interactive_move_begin(0, &output, start));
        assert!(layout.interactive_move_update(
            &0,
            (15., 0.).into(),
            output.clone(),
            start + Point::from((15., 0.))
        ));
        assert!(matches!(
            layout.interactive_move,
            Some(InteractiveMoveState::Starting { .. })
        ));
        assert!(layout.interactive_move_update(
            &0,
            (2., 0.).into(),
            output,
            start + Point::from((17., 0.))
        ));
        assert!(matches!(
            layout.interactive_move,
            Some(InteractiveMoveState::Moving(_))
        ));
        layout.interactive_move_cancel(&0);
        assert_eq!(workspace_of(&layout, 0), source);
        layout.verify_invariants();
    }
}

#[test]
fn stage_hidden_rail_drop_has_finite_placement_and_restores_cancelled_source() {
    for select_last in [false, true] {
        for distance in [4, 5, 6] {
            for cancel in [false, true] {
                let mut layout = drag_layout(true, true);
                for name in 2..=9 {
                    Op::AddNamedWorkspace {
                        ws_name: name,
                        output_name: Some(1),
                        layout_config: None,
                    }
                    .apply(&mut layout);
                }
                let mon = layout.active_monitor_ref().unwrap();
                let source_idx = if select_last {
                    mon.workspaces.len() - 1
                } else {
                    0
                };
                let source = mon.workspaces[source_idx].id();
                layout.move_to_workspace(Some(&0), source_idx, ActivateWindow::No);
                let output = layout.outputs().next().unwrap().clone();
                let selected = if select_last {
                    layout
                        .active_monitor_ref()
                        .unwrap()
                        .workspaces
                        .last()
                        .unwrap()
                        .id()
                } else {
                    source
                };
                assert!(layout.stage_select(&output, selected));
                Op::CompleteAnimations.apply(&mut layout);
                let before = layout.session_layout().windows[0].position;
                let mon = layout.active_monitor_ref().unwrap();
                let selected_idx = mon.idx_of_ws(selected).unwrap();
                let target_idx = if select_last {
                    selected_idx - distance
                } else {
                    selected_idx + distance
                };
                let target = mon.workspaces[target_idx].id();
                assert_eq!(
                    mon.workspaces_render_geo().nth(target_idx).unwrap().size.w,
                    0.
                );
                let height = mon.view_size().h;
                let point = (0..mon.view_size().w as i32)
                    .find_map(|x| {
                        let point = Point::from((x as f64, height - 38.));
                        (mon.stage_rail_under(point) == Some(InsertWorkspace::Existing(target)))
                            .then_some(point)
                    })
                    .unwrap();
                let (_, geo) = mon.stage_drop_target(point).unwrap();
                assert!(mon.workspace_render_zoom(geo) >= 0.01);
                start_drag(&mut layout, 0, point);
                if cancel {
                    layout.interactive_move_cancel(&0);
                } else {
                    layout.interactive_move_end(&0);
                }
                Op::CompleteAnimations.apply(&mut layout);
                assert_eq!(
                    workspace_of(&layout, 0),
                    if cancel { source } else { target }
                );
                let position = layout.session_layout().windows[0].position.unwrap();
                assert!(position.0.is_finite() && position.1.is_finite());
                assert!(position.0.abs() < 100_000. && position.1.abs() < 100_000.);
                if cancel {
                    assert_eq!(Some(position), before);
                } else {
                    assert_eq!(
                        layout.active_monitor_ref().unwrap().stage_selected(),
                        target
                    );
                }
                assert!(layout.core.borrow().stage_drag().window().is_none());
                layout.verify_invariants();
            }
        }
    }
}

#[test]
fn stage_canvas_cancel_preserves_original_membership_before_and_after_canvas_exit() {
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        for originally_floating in [false, true] {
            for exit_during_drag in [false, true] {
                let mut layout = drag_layout(originally_floating, false);
                Op::SwitchTilingMode(Some(mode)).apply(&mut layout);
                let source = workspace_of(&layout, 0);
                if !originally_floating && mode == clarion_ipc::TilingMode::Scrolling {
                    let workspace = layout.active_workspace_mut().unwrap();
                    workspace.set_window_width(Some(&0), SizeChange::SetProportion(73.));
                    workspace.toggle_full_width();
                    workspace.set_column_display(ColumnDisplay::Tabbed);
                }
                let original_columns = layout
                    .active_workspace()
                    .unwrap()
                    .scrolling()
                    .policy_snapshot()
                    .columns;
                layout.active_workspace_mut().unwrap().toggle_canvas();
                Op::CompleteAnimations.apply(&mut layout);
                assert!(layout.active_workspace().unwrap().is_floating(&0));
                start_drag(&mut layout, 0, (0., 0.).into());
                if exit_during_drag {
                    layout
                        .workspaces_mut()
                        .find(|workspace| workspace.id() == source)
                        .unwrap()
                        .toggle_canvas();
                }
                layout.interactive_move_cancel(&0);
                Op::CompleteAnimations.apply(&mut layout);
                assert_eq!(workspace_of(&layout, 0), source);
                let workspace = layout
                    .workspaces()
                    .find(|(_, _, workspace)| workspace.id() == source)
                    .unwrap()
                    .2;
                assert_eq!(workspace.is_floating(&0),originally_floating || !exit_during_drag,
                    "mode={mode:?}, originalfloating={originally_floating}, exit={exit_during_drag}");
                assert!(layout.core.borrow().stage_drag().window().is_none());
                if !exit_during_drag {
                    layout
                        .workspaces_mut()
                        .find(|workspace| workspace.id() == source)
                        .unwrap()
                        .toggle_canvas();
                    let workspace = layout
                        .workspaces()
                        .find(|(_, _, workspace)| workspace.id() == source)
                        .unwrap()
                        .2;
                    assert_eq!(
                        workspace.is_floating(&0),
                        originally_floating,
                        "restore must retain original canvas member until later canvas exit"
                    );
                }
                if !originally_floating && mode == clarion_ipc::TilingMode::Scrolling {
                    let workspace = layout
                        .workspaces()
                        .find(|(_, _, workspace)| workspace.id() == source)
                        .unwrap()
                        .2;
                    let restored = workspace.scrolling().policy_snapshot();
                    let column = restored
                        .columns
                        .iter()
                        .find(|column| column.windows.contains(&0))
                        .unwrap();
                    let original = original_columns
                        .iter()
                        .find(|column| column.windows.contains(&0))
                        .unwrap();
                    assert_eq!(column.width,original.width,"canvas must retain proportional width rather than freeze the floating pixel width");
                    assert_eq!(
                        column.full_width, original.full_width,
                        "canvas must retain full-width preference"
                    );
                    assert_eq!(
                        column.display, original.display,
                        "canvas must retain tabbed display preference"
                    );
                }
                layout.verify_invariants();
            }
        }
    }
}

#[test]
fn stage_retired_source_does_not_treat_same_named_replacement_as_old_output() {
    let mut layout = drag_layout(false, false);
    Op::AddOutput(2).apply(&mut layout);
    let source = workspace_of(&layout, 0);
    let old_output = layout
        .outputs()
        .find(|output| output.name() == "output1")
        .unwrap()
        .clone();
    let old_generation = layout.core.borrow().output("output1").unwrap().generation;
    start_drag(&mut layout, 0, (0., 0.).into());
    // Model an explicit removal of the now-empty source. Automatic cleanup
    // ordinarily keeps it pinned, but management may retire it during a drag.
    let monitor = layout.monitor_for_output_mut(&old_output).unwrap();
    let index = monitor.idx_of_ws(source).unwrap();
    monitor.workspaces.remove(index);
    assert!(layout
        .core
        .borrow()
        .space(clarion_core::SpaceId(source.get()))
        .is_none());
    layout.remove_output(&old_output);
    Op::FocusOutput(2).apply(&mut layout);
    let destination = layout.active_workspace().unwrap().id();
    Op::AddOutput(1).apply(&mut layout);
    assert!(layout.core.borrow().output("output1").unwrap().generation > old_generation);
    assert_eq!(
        layout.core.borrow().stage_drag_destination(),
        Some(clarion_core::SpaceId(destination.get()))
    );
    layout.interactive_move_cancel(&0);
    assert_eq!(workspace_of(&layout, 0), destination);
    assert!(layout.core.borrow().stage_drag().window().is_none());
    layout.verify_invariants();
}
