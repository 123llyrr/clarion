use super::*;

#[test]
fn undo_after_all_outputs_and_captured_windows_disappear_cleans_empty_workspaces() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
    ]);
    layout.checkpoint_layout();
    check_ops_on_layout(&mut layout, [Op::RemoveOutput(1), Op::CloseWindow(0)]);

    assert!(layout.undo_layout(false));
    layout.verify_invariants();
    assert_eq!(layout.workspaces().count(), 0);
    assert!(layout.undo_layout(true));
    layout.verify_invariants();
    assert_eq!(layout.workspaces().count(), 0);

    // Reconnecting a screen after the history operation must still have a valid empty layout.
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);
    assert!(layout.windows().next().is_none());
}

#[test]
fn group_moves_without_outputs_and_undo_restores_workspace_membership() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_name: Some(1),
            layout_config: None,
        },
        Op::AddWindowToNamedWorkspace {
            params: TestWindowParams::new(0),
            ws_name: 1,
        },
        Op::AddNamedWorkspace {
            ws_name: 2,
            output_name: Some(1),
            layout_config: None,
        },
        Op::AddWindowToNamedWorkspace {
            params: TestWindowParams::new(1),
            ws_name: 2,
        },
        Op::RemoveOutput(1),
    ]);
    let source = layout.find_workspace_by_name("ws1").unwrap().1.id();
    let target = layout.find_workspace_by_name("ws2").unwrap().1.id();
    layout.set_window_group("pair".into(), vec![0, 1]);
    layout.move_window_group("pair", target);
    layout.verify_invariants();
    let ws = layout.find_workspace_by_id(target).unwrap().1;
    assert!(ws.has_window(&0));
    assert!(ws.has_window(&1));

    assert!(layout.undo_layout(false));
    layout.verify_invariants();
    assert!(layout
        .find_workspace_by_id(source)
        .unwrap()
        .1
        .has_window(&0));
    assert!(layout
        .find_workspace_by_id(target)
        .unwrap()
        .1
        .has_window(&1));
    assert!(layout.undo_layout(true));
    layout.verify_invariants();
    assert!(layout
        .find_workspace_by_id(target)
        .unwrap()
        .1
        .has_window(&0));
}

#[test]
fn group_move_across_outputs_recomputes_target_index_after_source_cleanup() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::MoveWindowToWorkspace {
            window_id: Some(0),
            workspace_idx: 1,
        },
        Op::FocusWorkspace(0),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_name: Some(2),
            layout_config: None,
        },
        Op::CompleteAnimations,
    ]);
    let target = layout.find_workspace_by_name("ws1").unwrap().1.id();
    layout.set_window_group("pair".into(), vec![0, 1]);
    layout.move_window_group("pair", target);
    layout.verify_invariants();
    let ws = layout.find_workspace_by_id(target).unwrap().1;
    assert!(ws.has_window(&0));
    assert!(ws.has_window(&1));
    assert_eq!(ws.current_output().unwrap().name(), "output2");

    assert!(layout.undo_layout(false));
    layout.verify_invariants();
    assert!(layout
        .workspaces()
        .filter(|(_, _, ws)| ws.has_windows())
        .all(|(_, _, ws)| ws.current_output().unwrap().name() == "output1"));
    assert!(layout.undo_layout(true));
    layout.verify_invariants();
    assert!(layout
        .find_workspace_by_id(target)
        .unwrap()
        .1
        .has_window(&0));
    assert!(layout
        .find_workspace_by_id(target)
        .unwrap()
        .1
        .has_window(&1));
}

#[test]
fn undo_recreates_unnamed_workspaces_in_saved_order_without_outputs() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddNamedWorkspace {
            ws_name: 3,
            output_name: Some(1),
            layout_config: None,
        },
        Op::AddNamedWorkspace {
            ws_name: 2,
            output_name: Some(1),
            layout_config: None,
        },
        Op::AddWindowToNamedWorkspace {
            params: TestWindowParams::new(1),
            ws_name: 2,
        },
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_name: Some(1),
            layout_config: None,
        },
        Op::AddWindowToNamedWorkspace {
            params: TestWindowParams::new(0),
            ws_name: 1,
        },
        Op::RemoveOutput(1),
        Op::UnnameWorkspace { ws_name: 1 },
        Op::UnnameWorkspace { ws_name: 2 },
    ]);
    let membership = |layout: &Layout<TestWindow>| {
        layout
            .workspaces()
            .filter(|(_, _, ws)| ws.has_windows_or_name())
            .map(|(_, _, ws)| {
                (
                    ws.name().map(|name| (*name).clone()),
                    ws.windows().map(|w| *w.id()).collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    let before = membership(&layout);
    let target = layout.find_workspace_by_name("ws3").unwrap().1.id();
    layout.set_window_group("pair".into(), vec![0, 1]);
    layout.move_window_group("pair", target);
    layout.verify_invariants();
    assert_eq!(layout.workspaces().count(), 1);

    assert!(layout.undo_layout(false));
    layout.verify_invariants();
    assert_eq!(membership(&layout), before);
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);
    assert_eq!(membership(&layout), before);
}

#[test]
fn closed_history_slots_do_not_capture_new_clients_or_restore_dead_group_members() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::Communicate(0),
        Op::Communicate(1),
    ]);
    layout.set_window_group("pair".into(), vec![0, 1]);
    layout.checkpoint_layout();
    check_ops_on_layout(
        &mut layout,
        [
            Op::CloseWindow(0),
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::Communicate(2),
        ],
    );
    assert_eq!(layout.core.borrow().management().groups()[0].members, [1]);
    assert!(layout.undo_layout(false));
    assert!(layout.has_window(&1));
    assert!(
        layout.has_window(&2),
        "uncaptured clients remain live and keep their own stable identity"
    );
    assert!(!layout.has_window(&0));
    let (groups, _, redo) = layout.management_status();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].windows, [1]);
    assert_eq!(redo, 1);
    layout.verify_invariants();
}

#[test]
fn production_history_is_bounded_and_a_new_checkpoint_releases_the_redo_branch() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::Communicate(0),
    ]);
    for _ in 0..40 {
        layout.checkpoint_layout();
    }
    assert_eq!(layout.core.borrow().management().history_steps(), (32, 0));
    for _ in 0..32 {
        assert!(layout.undo_layout(false));
        layout.verify_invariants();
    }
    assert!(!layout.undo_layout(false));
    assert_eq!(layout.core.borrow().management().history_steps(), (0, 32));
    layout.checkpoint_layout();
    assert_eq!(layout.core.borrow().management().history_steps(), (1, 0));
    assert!(!layout.undo_layout(true));
    layout.verify_invariants();
}
