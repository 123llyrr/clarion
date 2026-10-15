//! Exercise accepted pure move controls through the real Tile/protocol adapter.
use super::*;

fn fixture(mode: realand_ipc::TilingMode, floating: bool) -> Layout<TestWindow> {
    let mut options = Options::default();
    options.animations.off = true;
    let mut params = TestWindowParams::new(1);
    params.is_floating = floating;
    check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddOutput(2),
            Op::FocusOutput(1),
            Op::SwitchTilingMode(Some(mode)),
            Op::AddWindow { params },
            Op::Communicate(1),
            Op::CompleteAnimations,
        ],
    )
}
fn begin(layout: &mut Layout<TestWindow>) -> (Output, Point<f64, Logical>) {
    let (output, pointer) = layout
        .monitors()
        .find_map(|monitor| {
            monitor
                .workspaces_with_render_geo()
                .find_map(|(workspace, geo)| {
                    workspace
                        .tiles_with_render_positions()
                        .find_map(|(tile, pos, _)| {
                            (tile.window().id() == &1).then(|| {
                                (
                                    monitor.output().clone(),
                                    geo.loc
                                        + (pos
                                            + tile.window_loc()
                                            + tile.window_size().to_point().downscale(2.))
                                        .upscale(monitor.workspace_render_zoom(geo)),
                                )
                            })
                        })
                })
        })
        .unwrap();
    assert!(layout.interactive_move_begin(1, &output, pointer));
    let anchor = layout
        .core
        .borrow()
        .interaction()
        .unwrap()
        .controls
        .anchor();
    assert!((anchor[0] - 0.5).abs() < 1e-12 && (anchor[1] - 0.5).abs() < 1e-12);
    (output, pointer)
}
fn detach(layout: &mut Layout<TestWindow>, output: Output, pointer: Point<f64, Logical>) {
    assert!(layout.interactive_move_update(
        &1,
        (300., 0.).into(),
        output,
        pointer + Point::from((300., 0.))
    ));
    assert!(matches!(
        layout.interactive_move,
        Some(InteractiveMoveState::Moving(_))
    ));
    assert!(
        layout
            .core
            .borrow()
            .interaction()
            .unwrap()
            .controls
            .placement()
            .width
            == layout
                .interactive_move
                .as_ref()
                .unwrap()
                .moving()
                .unwrap()
                .placement()
                .width
    );
    layout.verify_invariants();
}
#[test]
fn toggling_target_during_drag_preserves_column_preferences_and_commits_requested_lane() {
    let mut layout = fixture(realand_ipc::TilingMode::Scrolling, false);
    let workspace = layout.active_workspace_mut().unwrap();
    workspace.set_window_width(Some(&1), SizeChange::SetProportion(73.));
    workspace.toggle_full_width();
    let expected = workspace.scrolling().move_preferences(1).unwrap();
    let (output, point) = begin(&mut layout);
    detach(&mut layout, output, point);
    for floating in [true, false, true, false] {
        layout.set_window_floating(Some(&1), floating);
        let placement = layout
            .core
            .borrow()
            .interaction()
            .unwrap()
            .controls
            .placement();
        assert_eq!((placement.width, placement.full_width), expected);
        assert_eq!(placement.floating, floating);
        layout.refresh(true);
        layout.verify_invariants();
    }
    layout.interactive_move_end(&1);
    assert!(layout.core.borrow().interaction().is_none());
    let workspace = layout
        .workspaces()
        .find(|(_, _, ws)| ws.has_window(&1))
        .unwrap()
        .2;
    assert!(!workspace.is_floating(&1));
    assert_eq!(workspace.scrolling().move_preferences(1), Some(expected));
    layout.verify_invariants();
}
#[test]
fn source_layer_change_while_starting_is_used_by_the_next_accepted_motion() {
    for mode in [
        realand_ipc::TilingMode::Scrolling,
        realand_ipc::TilingMode::Dwindle,
    ] {
        let mut layout = fixture(mode, false);
        let (output, point) = begin(&mut layout);
        assert!(layout.interactive_move_update(&1, (1., 0.).into(), output.clone(), point));
        assert!(matches!(
            layout.interactive_move,
            Some(InteractiveMoveState::Starting { .. })
        ));
        layout.set_window_floating(Some(&1), true);
        assert!(layout.interactive_move_update(&1, (0., 0.).into(), output, point));
        assert!(
            layout
                .core
                .borrow()
                .interaction()
                .unwrap()
                .controls
                .placement()
                .floating
        );
        layout.interactive_move_end(&1);
        assert!(layout
            .workspaces()
            .find(|(_, _, ws)| ws.has_window(&1))
            .unwrap()
            .2
            .is_floating(&1));
        layout.verify_invariants();
    }
}
#[test]
fn normal_cancel_places_at_current_output_and_keeps_explicit_target_lane() {
    for mode in [
        realand_ipc::TilingMode::Scrolling,
        realand_ipc::TilingMode::Dwindle,
    ] {
        let mut layout = fixture(mode, false);
        let (output, point) = begin(&mut layout);
        detach(&mut layout, output, point);
        layout.set_window_floating(Some(&1), true);
        let right = layout
            .outputs()
            .find(|output| output.name() == "output2")
            .unwrap()
            .clone();
        assert!(layout.interactive_move_update(&1, (10., 0.).into(), right, (400., 300.).into()));
        layout.interactive_move_cancel(&1);
        let (output, _, workspace) = layout
            .workspaces()
            .find(|(_, _, ws)| ws.has_window(&1))
            .unwrap();
        assert_eq!(output.unwrap().output().name(), "output2");
        assert!(workspace.is_floating(&1));
        assert!(layout.core.borrow().interaction().is_none());
        layout.verify_invariants();
    }
}
#[test]
fn output_removal_and_mode_switch_keep_numeric_placement_until_no_output_drop() {
    let mut layout = fixture(realand_ipc::TilingMode::Scrolling, false);
    let (output, point) = begin(&mut layout);
    detach(&mut layout, output, point);
    layout.set_window_floating(Some(&1), true);
    Op::SwitchTilingMode(Some(realand_ipc::TilingMode::Dwindle)).apply(&mut layout);
    for index in [1, 2] {
        Op::RemoveOutput(index).apply(&mut layout);
        assert!(
            layout
                .core
                .borrow()
                .interaction()
                .unwrap()
                .controls
                .placement()
                .floating
        );
        layout.verify_invariants();
    }
    layout.interactive_move_end(&1);
    assert!(layout.core.borrow().interaction().is_none());
    assert!(layout
        .workspaces()
        .find(|(_, _, ws)| ws.has_window(&1))
        .unwrap()
        .2
        .is_floating(&1));
    layout.verify_invariants();
}
#[test]
fn fullscreen_floating_window_removal_acknowledges_restored_original_lane() {
    let mut layout = fixture(realand_ipc::TilingMode::Scrolling, true);
    layout.set_fullscreen(&1, true);
    Op::Communicate(1).apply(&mut layout);
    let (output, point) = begin(&mut layout);
    detach(&mut layout, output, point);
    assert!(
        layout
            .core
            .borrow()
            .interaction()
            .unwrap()
            .controls
            .placement()
            .floating
    );
    layout.interactive_move_end(&1);
    assert!(layout
        .workspaces()
        .find(|(_, _, ws)| ws.has_window(&1))
        .unwrap()
        .2
        .is_floating(&1));
    layout.verify_invariants();
}
