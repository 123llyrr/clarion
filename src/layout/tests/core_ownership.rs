use super::*;
use clarion_core::{CoreEvent, LayoutMode, SpaceId};

fn desktop() -> Layout<TestWindow> {
    check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
    ])
}

fn verify_core(layout: &Layout<TestWindow>) {
    let core = layout.core.borrow();
    for (_, _, workspace) in layout.workspaces() {
        assert!(Rc::ptr_eq(&workspace.core, &layout.core));
        for window in workspace.windows() {
            let record = core
                .window(&window.core_id())
                .expect("mapped tile needs a core identity");
            assert!(record.attached);
            assert_eq!(record.space, SpaceId(workspace.id().get()));
            assert_eq!(record.output, workspace.current_output().map(Output::name));
            assert_eq!(record.minimized, workspace.is_minimized(window.id()));
        }
    }
    for record in core.windows().filter(|record| record.attached) {
        assert!(layout
            .workspaces()
            .any(|(_, _, workspace)| workspace.id().get() == record.space.0
                && workspace
                    .windows()
                    .any(|window| window.core_id() == record.id)));
    }
}

#[test]
fn production_mode_dispatch_reads_the_independent_core() {
    let mut layout = check_ops([Op::AddOutput(1)]);
    let space = SpaceId(layout.active_workspace().unwrap().id().get());
    // There is no duplicate Workspace mode field to update. An event directly
    // changes where the next mapped client is placed by the platform adapter.
    layout.core.borrow_mut().apply(CoreEvent::SetLayoutMode {
        space,
        mode: LayoutMode::Dwindle,
    });
    Op::AddWindow {
        params: TestWindowParams::new(0),
    }
    .apply(&mut layout);
    let workspace = layout.active_workspace().unwrap();
    assert_eq!(workspace.tiling_mode(), clarion_ipc::TilingMode::Dwindle);
    assert!(workspace.dwindle.has_window(&0));
    assert!(workspace.scrolling().is_empty());
    verify_core(&layout);
}

#[test]
fn minimize_restore_transfer_and_destroy_keep_one_identity_owner() {
    let mut layout = desktop();
    verify_core(&layout);
    assert!(layout.minimize_window(&0));
    assert!(layout.core.borrow().window(&0).unwrap().minimized);
    verify_core(&layout);
    assert!(layout.restore_window(&0));
    assert!(!layout.core.borrow().window(&0).unwrap().minimized);
    Op::MoveWindowToOutput {
        window_id: Some(0),
        output_id: 2,
        target_ws_idx: None,
    }
    .apply(&mut layout);
    assert_eq!(
        layout.core.borrow().window(&0).unwrap().output.as_deref(),
        Some("output2")
    );
    assert_eq!(
        layout
            .core
            .borrow()
            .windows()
            .filter(|record| record.id == 0)
            .count(),
        1
    );
    verify_core(&layout);
    assert!(layout.remove_window(&0, Transaction::new()).is_some());
    assert!(layout.core.borrow().window(&0).is_none());
    verify_core(&layout);
}

#[test]
fn output_disconnect_reconnect_and_no_output_preserve_membership() {
    let mut layout = desktop();
    Op::MoveWindowToOutput {
        window_id: Some(0),
        output_id: 2,
        target_ws_idx: None,
    }
    .apply(&mut layout);
    for operation in [
        Op::RemoveOutput(2),
        Op::AddOutput(2),
        Op::RemoveOutput(1),
        Op::RemoveOutput(2),
        Op::AddOutput(1),
    ] {
        operation.apply(&mut layout);
        layout.verify_invariants();
        verify_core(&layout);
        assert_eq!(layout.core.borrow().windows().count(), 2);
    }
}

#[test]
fn interactive_transfer_retains_history_until_placement_or_destruction() {
    for destroy in [false, true] {
        let mut layout = desktop();
        let output = layout.active_monitor_ref().unwrap().output().clone();
        assert!(layout.interactive_move_begin(0, &output, (100.0, 100.0).into()));
        assert!(layout.interactive_move_update(
            &0,
            (500.0, 100.0).into(),
            output,
            (600.0, 200.0).into()
        ));
        assert!(matches!(
            layout.interactive_move,
            Some(InteractiveMoveState::Moving(_))
        ));
        let record = layout.core.borrow().window(&0).unwrap().clone();
        assert!(!record.attached);
        assert!(record.saved_geometry.get(LayoutMode::Scrolling).is_some());
        verify_core(&layout);
        if destroy {
            assert!(layout.remove_window(&0, Transaction::new()).is_some());
            assert!(layout.core.borrow().window(&0).is_none());
        } else {
            layout.interactive_move_end(&0);
            assert!(layout.core.borrow().window(&0).unwrap().attached);
        }
        verify_core(&layout);
    }
}

#[test]
fn directional_focus_and_minimize_select_stored_core_focus() {
    let mut layout = desktop();
    Op::FocusColumnLeft.apply(&mut layout);
    let focused = layout.focus().map(LayoutElement::core_id);
    let space = SpaceId(layout.active_workspace().unwrap().id().get());
    assert_eq!(layout.core.borrow().space(space).unwrap().focused, focused);
    layout.refresh(true);
    assert_eq!(layout.core.borrow().active_window().copied(), focused);
    assert!(layout.minimize_window(&(focused.unwrap() as usize)));
    layout.refresh(true);
    assert_eq!(
        layout.core.borrow().active_window().copied(),
        layout.focus().map(LayoutElement::core_id)
    );
    verify_core(&layout);
}

#[test]
fn floating_geometry_is_committed_at_mutations_and_survives_tiling_switches() {
    let mut options = Options::default();
    options.animations.off = true;
    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams {
                    is_floating: true,
                    ..TestWindowParams::new(0)
                },
            },
            Op::Communicate(0),
        ],
    );
    let initial = layout.core.borrow().window(&0).unwrap().geometry;
    Op::MoveFloatingWindow {
        id: Some(0),
        x: PositionChange::SetFixed(333.0),
        y: PositionChange::SetFixed(177.0),
        animate: false,
    }
    .apply(&mut layout);
    let moved = layout.core.borrow().window(&0).unwrap().geometry;
    assert_ne!(moved, initial);
    assert_eq!((moved.x, moved.y), (333.0, 177.0));
    assert_eq!(
        layout
            .core
            .borrow()
            .window(&0)
            .unwrap()
            .saved_geometry
            .get(LayoutMode::Floating),
        Some(moved)
    );
    layout.set_window_width(Some(&0), SizeChange::SetFixed(444));
    let resized = layout.core.borrow().window(&0).unwrap().geometry;
    assert_ne!(resized.width, moved.width);
    assert_eq!(
        layout
            .core
            .borrow()
            .window(&0)
            .unwrap()
            .saved_geometry
            .get(LayoutMode::Floating),
        Some(resized)
    );
    for mode in [
        clarion_ipc::TilingMode::Dwindle,
        clarion_ipc::TilingMode::Scrolling,
    ] {
        Op::SwitchTilingMode(Some(mode)).apply(&mut layout);
        assert_eq!(
            layout
                .core
                .borrow()
                .window(&0)
                .unwrap()
                .saved_geometry
                .get(LayoutMode::Floating),
            Some(resized)
        );
    }
    verify_core(&layout);
}

#[test]
fn tiled_geometry_commits_target_slots_without_animation_offsets() {
    let mut layout = desktop();
    for mode in [
        clarion_ipc::TilingMode::Dwindle,
        clarion_ipc::TilingMode::Scrolling,
    ] {
        Op::SwitchTilingMode(Some(mode)).apply(&mut layout);
        let workspace = layout.active_workspace().unwrap();
        let mut compare = |tile: &Tile<TestWindow>, geometry: Rectangle<f64, Logical>| {
            assert_eq!(
                layout
                    .core
                    .borrow()
                    .window(&tile.window().core_id())
                    .unwrap()
                    .geometry,
                core_adapter::rect(geometry)
            );
        };
        match mode {
            clarion_ipc::TilingMode::Dwindle => {
                workspace.dwindle.with_layout_geometry(&mut compare)
            }
            clarion_ipc::TilingMode::Scrolling => {
                workspace.scrolling().with_layout_geometry(&mut compare)
            }
        }
        let before: Vec<_> = layout
            .core
            .borrow()
            .windows()
            .map(|record| (record.id, record.geometry))
            .collect();
        layout.clock.set_unadjusted(Duration::from_millis(30));
        layout.advance_animations();
        let after: Vec<_> = layout
            .core
            .borrow()
            .windows()
            .map(|record| (record.id, record.geometry))
            .collect();
        assert_eq!(
            before, after,
            "animation progression cannot overwrite saved layout targets"
        );
    }
}

#[test]
fn policy_round_trip_preserves_scrolling_groups_sizes_tabs_and_view() {
    let mut options = Options::default();
    options.animations.off = true;
    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(0),
            },
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::Communicate(0),
            Op::Communicate(1),
            Op::Communicate(2),
        ],
    );
    // Customize a real column rather than comparing default topology alone.
    layout.set_column_width(SizeChange::SetFixed(777));
    layout.set_window_height(Some(&1), SizeChange::SetFixed(233));
    layout.set_column_display(ColumnDisplay::Tabbed);
    let original = layout
        .active_workspace()
        .unwrap()
        .scrolling()
        .policy_snapshot();
    for _ in 0..3 {
        Op::SwitchTilingMode(Some(clarion_ipc::TilingMode::Dwindle)).apply(&mut layout);
        Op::SwitchTilingMode(Some(clarion_ipc::TilingMode::Scrolling)).apply(&mut layout);
        layout.verify_invariants();
        assert_eq!(
            layout
                .active_workspace()
                .unwrap()
                .scrolling()
                .policy_snapshot(),
            original
        );
        verify_core(&layout);
    }
}

#[test]
fn policy_history_prunes_closed_ids_and_inserts_new_members_once() {
    let mut layout = desktop();
    Op::SwitchTilingMode(Some(clarion_ipc::TilingMode::Dwindle)).apply(&mut layout);
    let original = layout.active_workspace().unwrap().dwindle.policy_snapshot();
    Op::SwitchTilingMode(Some(clarion_ipc::TilingMode::Scrolling)).apply(&mut layout);
    assert!(layout.remove_window(&0, Transaction::new()).is_some());
    Op::AddWindow {
        params: TestWindowParams::new(5),
    }
    .apply(&mut layout);
    Op::SwitchTilingMode(Some(clarion_ipc::TilingMode::Dwindle)).apply(&mut layout);
    layout.verify_invariants();
    assert_ne!(
        layout
            .active_workspace()
            .unwrap()
            .dwindle
            .policy_snapshot()
            .topology,
        original.topology
    );
    assert!(layout.active_workspace().unwrap().dwindle.has_window(&1));
    assert!(layout.active_workspace().unwrap().dwindle.has_window(&5));
    assert!(!layout.active_workspace().unwrap().dwindle.has_window(&0));
    assert_eq!(layout.active_workspace().unwrap().windows().count(), 2);
    verify_core(&layout);
}

#[test]
fn empty_workspace_retirement_survives_an_outstanding_core_reader() {
    let context = core_adapter::new_context::<usize>();
    let workspace = Workspace::<TestWindow>::new_no_outputs(
        Clock::with_time(Duration::ZERO),
        Rc::new(Options::default()),
    )
    .with_core(context.clone());
    let id = SpaceId(workspace.id().get());
    let reader = context.borrow();
    drop(workspace);
    assert!(
        reader.space(id).is_some(),
        "retirement waits for the owning reader"
    );
    drop(reader);
    assert!(
        context.borrow().space(id).is_none(),
        "the next observation drains queued retirement"
    );
}

#[test]
fn dynamic_workspace_cycles_do_not_retain_retired_identities_or_history() {
    let mut layout = check_ops([Op::AddOutput(1)]);
    for id in 20..120 {
        Op::AddWindow {
            params: TestWindowParams::new(id),
        }
        .apply(&mut layout);
        Op::FocusWorkspaceDown.apply(&mut layout);
        assert!(layout.remove_window(&id, Transaction::new()).is_some());
        layout.verify_invariants();
        let physical: std::collections::HashSet<_> = layout
            .workspaces()
            .map(|(_, _, workspace)| SpaceId(workspace.id().get()))
            .collect();
        let model: std::collections::HashSet<_> = layout
            .core
            .borrow()
            .spaces()
            .map(|space| space.id)
            .collect();
        assert_eq!(model, physical);
        assert_eq!(layout.core.borrow().windows().count(), 0);
    }
}

#[test]
fn moving_focus_survives_source_output_removal_without_a_new_pointer_packet() {
    let mut layout = desktop();
    let output = layout.active_monitor_ref().unwrap().output().clone();
    assert!(layout.interactive_move_begin(0, &output, (100.0, 100.0).into()));
    assert!(layout.queue_interactive_move_update(
        &0,
        (500.0, 100.0).into(),
        output.clone(),
        (600.0, 200.0).into()
    ));
    assert!(matches!(
        layout.interactive_move,
        Some(InteractiveMoveState::Moving(_))
    ));
    assert_eq!(layout.core.borrow().interaction().unwrap().id, 0);
    assert_eq!(layout.core.borrow().active_window(), Some(&0));
    layout.remove_output(&output);
    let (window, focused_output) = layout.focus_with_output().unwrap();
    assert_eq!(window.core_id(), 0);
    assert_eq!(focused_output.name(), "output2");
    assert_eq!(
        layout
            .core
            .borrow()
            .interaction()
            .unwrap()
            .output
            .as_deref(),
        Some("output2")
    );
    layout.verify_invariants();
    layout.interactive_move_cancel(&0);
    assert!(layout.core.borrow().interaction().is_none());
    verify_core(&layout);
}

#[test]
fn minimized_canvas_restore_preserves_original_layer_before_and_after_exit() {
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        for originally_floating in [false, true] {
            for exit_before_restore in [false, true] {
                let mut options = Options::default();
                options.animations.off = true;
                let mut layout = check_ops_with_options(
                    options,
                    [
                        Op::AddOutput(1),
                        Op::SwitchTilingMode(Some(mode)),
                        Op::AddWindow {
                            params: TestWindowParams {
                                is_floating: originally_floating,
                                ..TestWindowParams::new(0)
                            },
                        },
                        Op::AddWindow {
                            params: TestWindowParams::new(1),
                        },
                        Op::Communicate(0),
                        Op::Communicate(1),
                    ],
                );
                layout.active_workspace_mut().unwrap().toggle_canvas();
                assert!(layout.active_workspace().unwrap().is_floating(&0));
                assert!(layout.minimize_window(&0));
                if exit_before_restore {
                    layout.active_workspace_mut().unwrap().toggle_canvas();
                }
                assert!(layout.restore_window(&0));
                assert_eq!(layout.active_workspace().unwrap().is_floating(&0),
                    originally_floating || !exit_before_restore,
                    "mode={mode:?}, originally_floating={originally_floating}, exit_before_restore={exit_before_restore}");
                if !exit_before_restore {
                    layout.active_workspace_mut().unwrap().toggle_canvas();
                    assert_eq!(
                        layout.active_workspace().unwrap().is_floating(&0),
                        originally_floating
                    );
                }
                layout.verify_invariants();
                verify_core(&layout);
                let core = layout.core.borrow();
                assert_eq!(core.windows().filter(|record| record.id == 0).count(), 1);
                assert!(!core.window(&0).unwrap().minimized);
            }
        }
    }
}

#[test]
fn canvas_roundtrip_and_parked_restore_preserve_proportional_width_full_width_display_and_focus() {
    for parked in [false, true] {
        let mut options = Options::default();
        options.animations.off = true;
        let mut layout = check_ops_with_options(
            options,
            [
                Op::AddOutput(1),
                Op::AddWindow {
                    params: TestWindowParams::new(0),
                },
                Op::AddWindow {
                    params: TestWindowParams::new(1),
                },
                Op::Communicate(0),
                Op::Communicate(1),
                Op::FocusColumnLeft,
            ],
        );
        {
            let workspace = layout.active_workspace_mut().unwrap();
            workspace.set_window_width(Some(&0), SizeChange::SetProportion(73.));
            workspace.toggle_full_width();
            workspace.set_column_display(ColumnDisplay::Tabbed);
        }
        let original = layout
            .active_workspace()
            .unwrap()
            .scrolling()
            .policy_snapshot()
            .columns
            .into_iter()
            .find(|column| column.windows.contains(&0))
            .unwrap();
        layout.active_workspace_mut().unwrap().toggle_canvas();
        if parked {
            assert!(layout.minimize_window(&0));
        }
        layout.active_workspace_mut().unwrap().toggle_canvas();
        let focus = layout.focus().map(LayoutElement::core_id);
        if parked {
            assert!(layout.restore_window(&0));
        }
        let restored = layout
            .active_workspace()
            .unwrap()
            .scrolling()
            .policy_snapshot()
            .columns
            .into_iter()
            .find(|column| column.windows.contains(&0))
            .unwrap();
        assert_eq!(restored.width, original.width, "parked={parked}");
        assert_eq!(restored.full_width, original.full_width, "parked={parked}");
        assert_eq!(restored.display, original.display, "parked={parked}");
        assert_eq!(
            layout.focus().map(LayoutElement::core_id),
            focus,
            "restoration metadata must not steal focus"
        );
        assert!(!layout.active_workspace().unwrap().is_floating(&0));
        layout.verify_invariants();
        verify_core(&layout);
    }
}

#[test]
fn canvas_exit_keeps_runtime_expanded_window_from_an_original_normal_group() {
    for fullscreen in [false, true] {
        let mut options = Options::default();
        options.animations.off = true;
        options.layout.auto_stack_new_windows = false;
        let mut layout = check_ops_with_options(
            options,
            [
                Op::AddOutput(1),
                Op::AddWindow {
                    params: TestWindowParams::new(0),
                },
                Op::AddWindow {
                    params: TestWindowParams::new(1),
                },
                Op::Communicate(0),
                Op::Communicate(1),
                Op::FocusColumnFirst,
                Op::ConsumeWindowIntoColumn,
            ],
        );
        let workspace = layout.active_workspace_mut().unwrap();
        workspace.set_window_width(Some(&0), SizeChange::SetProportion(73.));
        workspace.toggle_full_width();
        let original = workspace
            .scrolling()
            .policy_snapshot()
            .columns
            .into_iter()
            .find(|column| column.windows.contains(&0))
            .unwrap();
        assert_eq!(original.windows.len(), 2);
        assert_eq!(
            original.display,
            clarion_core::policy_memory::ColumnDisplay::Normal
        );
        workspace.toggle_canvas();
        if fullscreen {
            workspace.set_fullscreen(&0, true);
        } else {
            workspace.set_maximized(&0, true);
        }
        let mode_before = workspace
            .windows()
            .find(|window| *window.id() == 0)
            .unwrap()
            .pending_sizing_mode();
        assert!(if fullscreen {
            mode_before.is_fullscreen()
        } else {
            mode_before.is_maximized()
        });
        workspace.toggle_canvas();
        let mode_after = workspace
            .windows()
            .find(|window| *window.id() == 0)
            .unwrap()
            .pending_sizing_mode();
        assert!(
            if fullscreen {
                mode_after.is_fullscreen()
            } else {
                mode_after.is_maximized()
            },
            "fullscreen={fullscreen}: restoring a Normal group cannot revoke a live expanded mode"
        );
        assert!(workspace
            .windows()
            .find(|window| *window.id() == 1)
            .unwrap()
            .pending_sizing_mode()
            .is_normal());
        let restored = workspace
            .scrolling()
            .policy_snapshot()
            .columns
            .into_iter()
            .find(|column| column.windows.contains(&0))
            .unwrap();
        assert_eq!(restored.width, original.width);
        assert_eq!(restored.full_width, original.full_width);
        assert_eq!(restored.display, original.display);
        layout.verify_invariants();
    }
}
