use std::fmt::Write as _;

use clarion_config::animations::{Curve, EasingParams, Kind};
use insta::assert_snapshot;

use super::*;

fn idle_drag_layout(is_floating: bool) -> Layout<TestWindow> {
    let mut options = Options::default();
    // Isolate frame scheduling from the wobble simulation, which advances during rendering.
    options.animations.window_wobble.off = true;
    check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddOutput(2),
            Op::FocusOutput(1),
            Op::AddWindow {
                params: TestWindowParams {
                    is_floating,
                    ..TestWindowParams::new(1)
                },
            },
            Op::CompleteAnimations,
            Op::InteractiveMoveBegin {
                window: 1,
                output_idx: 1,
                px: 500.,
                py: 400.,
            },
        ],
    )
}

fn move_idle_drag_to(layout: &mut Layout<TestWindow>, output_idx: usize) {
    Op::InteractiveMoveUpdate {
        window: 1,
        dx: 1000.,
        dy: 0.,
        output_idx,
        px: 700.,
        py: 400.,
    }
    .apply(layout);
    assert!(matches!(
        layout.interactive_move,
        Some(InteractiveMoveState::Moving(_))
    ));
    Op::CompleteAnimations.apply(layout);
}

#[test]
fn idle_drag_start_does_not_keep_settled_outputs_busy() {
    for floating in [false, true] {
        let layout = idle_drag_layout(floating);
        assert!(matches!(
            layout.interactive_move,
            Some(InteractiveMoveState::Starting { .. })
        ));
        assert!(!layout.are_animations_ongoing(None));
        assert!(!layout.are_transitions_ongoing(None));
    }
}

#[test]
fn idle_drag_tiled_edge_scrolling_follows_its_output() {
    let mut layout = idle_drag_layout(false);
    for owner in [1, 2, 1] {
        move_idle_drag_to(&mut layout, owner);
        for output in layout.outputs() {
            let expected = output.name() == format!("output{owner}");
            assert_eq!(layout.are_animations_ongoing(Some(output)), expected);
            assert_eq!(layout.are_transitions_ongoing(Some(output)), expected);
        }
        assert!(layout.are_transitions_ongoing(None));
    }
}

#[test]
fn idle_drag_floating_settles_and_retains_local_and_global_transitions() {
    let mut layout = idle_drag_layout(true);
    move_idle_drag_to(&mut layout, 2);
    assert!(!layout.are_animations_ongoing(None));
    assert!(!layout.are_transitions_ongoing(None));

    let Some(InteractiveMoveState::Moving(move_)) = &mut layout.interactive_move else {
        unreachable!();
    };
    move_.tile.animate_move_x_from(80.);
    for output in layout.outputs() {
        let expected = output.name() == "output2";
        assert_eq!(layout.are_animations_ongoing(Some(output)), expected);
        assert_eq!(layout.are_transitions_ongoing(Some(output)), expected);
    }
    Op::CompleteAnimations.apply(&mut layout);
    assert!(!layout.are_transitions_ongoing(None));

    layout.open_overview();
    layout.refresh(true);
    for output in layout.outputs() {
        assert!(layout.are_transitions_ongoing(Some(output)));
    }
    Op::CompleteAnimations.apply(&mut layout);
    for output in layout.outputs() {
        // Once the global transition ends, overview edge scrolling still needs its output.
        assert_eq!(
            layout.are_transitions_ongoing(Some(output)),
            output.name() == "output2"
        );
    }
}

#[test]
fn idle_drag_and_drop_edge_scrolling_follows_its_output() {
    let mut layout = check_ops([Op::AddOutput(1), Op::AddOutput(2)]);
    for owner in [1, 2, 1] {
        Op::DndUpdate {
            output_idx: owner,
            px: 500.,
            py: 400.,
        }
        .apply(&mut layout);
        Op::CompleteAnimations.apply(&mut layout);
        for output in layout.outputs() {
            let expected = output.name() == format!("output{owner}");
            assert_eq!(layout.are_animations_ongoing(Some(output)), expected);
            assert_eq!(layout.are_transitions_ongoing(Some(output)), expected);
        }
        assert!(layout.are_transitions_ongoing(None));
    }
    layout.dnd_end();
    Op::CompleteAnimations.apply(&mut layout);
    assert!(!layout.are_transitions_ongoing(None));
}

#[test]
fn idle_drag_workspace_gesture_only_schedules_its_internal_animation() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddOutput(2),
        Op::FocusOutput(1),
        Op::CompleteAnimations,
    ]);
    layout.open_overview();
    Op::CompleteAnimations.apply(&mut layout);
    layout.active_monitor().unwrap().dnd_scroll_gesture_begin();
    assert!(!layout.are_animations_ongoing(None));
    assert!(!layout.are_transitions_ongoing(None));

    // Switching while the gesture is held creates an animation inside the gesture.
    layout.active_monitor().unwrap().activate_workspace(1);
    for output in layout.outputs() {
        let expected = output.name() == "output1";
        assert_eq!(layout.are_animations_ongoing(Some(output)), expected);
        assert_eq!(layout.are_transitions_ongoing(Some(output)), expected);
    }
    Op::CompleteAnimations.apply(&mut layout);
    assert!(!layout.are_transitions_ongoing(None));
}

#[test]
fn idle_drag_view_gesture_only_schedules_its_internal_animation() {
    let mut options = Options::default();
    options.layout.auto_stack_new_windows = false;
    options.layout.center_focused_column = CenterFocusedColumn::Always;
    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::AddOutput(2),
            Op::FocusOutput(1),
            Op::CompleteAnimations,
        ],
    );
    layout
        .active_workspace_mut()
        .unwrap()
        .scrolling_mut()
        .dnd_scroll_gesture_begin();
    assert!(!layout.are_animations_ongoing(None));
    assert!(!layout.are_transitions_ongoing(None));

    layout.focus_column_first();
    for output in layout.outputs() {
        let expected = output.name() == "output1";
        assert_eq!(layout.are_animations_ongoing(Some(output)), expected);
        assert_eq!(layout.are_transitions_ongoing(Some(output)), expected);
    }
    Op::CompleteAnimations.apply(&mut layout);
    assert!(!layout.are_transitions_ongoing(None));
}

#[test]
fn workspace_reversal_keeps_position_and_velocity_across_refresh_rates() {
    for hz in [60., 120., 144., 240., 360.] {
        let mut layout = check_ops([Op::AddOutput(1)]);
        let monitor = layout.active_monitor().unwrap();
        monitor.add_workspace_bottom();
        monitor.activate_workspace(1);
        let t = Duration::from_secs_f64(5. / hz);
        let delta = Duration::from_micros(1);
        layout.clock.set_unadjusted(t - delta);
        let prev = layout.active_monitor_ref().unwrap().workspace_render_idx();
        layout.clock.set_unadjusted(t);
        let before = layout.active_monitor_ref().unwrap().workspace_render_idx();
        let velocity = (before - prev) / delta.as_secs_f64();
        assert!(velocity > 0.);
        layout.active_monitor().unwrap().activate_workspace(0);
        let after = layout.active_monitor_ref().unwrap().workspace_render_idx();
        assert!((before - after).abs() < 1e-10);
        layout.clock.set_unadjusted(t + delta);
        let next = layout.active_monitor_ref().unwrap().workspace_render_idx();
        let next_velocity = (next - after) / delta.as_secs_f64();
        assert!((next_velocity - velocity).abs() < 0.001, "hz={hz}");
        layout.clock.set_unadjusted(Duration::from_secs(2));
        assert_eq!(
            layout.active_monitor_ref().unwrap().workspace_render_idx(),
            0.
        );
    }
}

#[test]
fn overview_reversal_keeps_position_and_velocity() {
    let mut layout = check_ops([Op::AddOutput(1)]);
    layout.open_overview();
    layout.clock.set_unadjusted(Duration::from_micros(79999));
    let prev = layout.overview_progress_value().unwrap();
    layout.clock.set_unadjusted(Duration::from_millis(80));
    let before = layout.overview_progress_value().unwrap();
    let velocity = (before - prev) / 0.000001;
    assert!(velocity > 0.);
    layout.close_overview();
    let after = layout.overview_progress_value().unwrap();
    assert!((before - after).abs() < 1e-10);
    layout.clock.set_unadjusted(Duration::from_micros(80001));
    let next = layout.overview_progress_value().unwrap();
    assert!(((next - after) / 0.000001 - velocity).abs() < 0.001);
    layout.clock.set_unadjusted(Duration::from_secs(2));
    layout.advance_animations();
    assert!(layout.overview_progress.is_none());
}

#[test]
fn synchronized_zoom_and_workspace_switch_keep_velocity_when_interrupted() {
    for interrupt_overview in [false, true] {
        for hz in [60., 120., 144., 240., 360.] {
            let mut layout = check_ops([Op::AddOutput(1)]);
            layout.active_monitor().unwrap().add_workspace_bottom();
            layout.open_overview();
            let config = clarion_config::animations::OverviewOpenCloseAnim::default().0;
            layout
                .active_monitor()
                .unwrap()
                .activate_workspace_with_anim_config(1, Some(config));
            let t = Duration::from_secs_f64(5. / hz);
            let delta = Duration::from_nanos(100);
            layout.clock.set_unadjusted(t - delta);
            let prev = layout.active_monitor_ref().unwrap().workspace_render_idx();
            layout.clock.set_unadjusted(t);
            let before = layout.active_monitor_ref().unwrap().workspace_render_idx();
            let velocity = (before - prev) / delta.as_secs_f64();
            assert!(velocity > 0.);
            if interrupt_overview {
                layout.close_overview();
            } else {
                layout.active_monitor().unwrap().activate_workspace(0);
            }
            let after = layout.active_monitor_ref().unwrap().workspace_render_idx();
            assert!((before - after).abs() < 1e-10);
            layout.clock.set_unadjusted(t + delta);
            let next = layout.active_monitor_ref().unwrap().workspace_render_idx();
            let next_velocity = (next - after) / delta.as_secs_f64();
            assert!(
                (next_velocity - velocity).abs() < 0.002,
                "hz={hz}, overview={interrupt_overview}, before={velocity}, after={next_velocity}"
            );
        }
    }
}

#[test]
fn horizontal_view_reversal_keeps_position_and_velocity() {
    let mut options = Options::default();
    options.layout.auto_stack_new_windows = false;
    options.layout.center_focused_column = CenterFocusedColumn::Always;
    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::AddWindow {
                params: TestWindowParams::new(3),
            },
            Op::CompleteAnimations,
        ],
    );
    layout.focus_column_first();
    layout.clock.set_unadjusted(Duration::from_micros(79999));
    let prev = layout.active_workspace().unwrap().scrolling().view_pos();
    layout.clock.set_unadjusted(Duration::from_millis(80));
    let before = layout.active_workspace().unwrap().scrolling().view_pos();
    let velocity = (before - prev) / 0.000001;
    assert!(velocity.abs() > 1.);
    layout.focus_column_last();
    let after = layout.active_workspace().unwrap().scrolling().view_pos();
    assert!((before - after).abs() < 1e-8);
    layout.clock.set_unadjusted(Duration::from_micros(80001));
    let next = layout.active_workspace().unwrap().scrolling().view_pos();
    assert!(((next - after) / 0.000001 - velocity).abs() < 1.);
}

#[test]
fn workspace_dnd_release_keeps_the_visible_offset_and_momentum() {
    for hz in [60., 120., 144., 240., 360.] {
        for rate in [0.5, 1., 2.] {
            let mut layout = check_ops([
                Op::AddOutput(1),
                Op::AddWindow {
                    params: TestWindowParams::new(1),
                },
                Op::CompleteAnimations,
            ]);
            layout.open_overview();
            Op::CompleteAnimations.apply(&mut layout);
            layout.clock.set_rate(rate);
            let monitor = layout.active_monitor().unwrap();
            monitor.dnd_scroll_gesture_begin();
            monitor.activate_workspace(1);

            let t = Duration::from_secs_f64(5. / hz);
            let dt = Duration::from_micros(1);
            layout.clock.set_unadjusted(t - dt);
            let prev = layout.active_monitor_ref().unwrap().workspace_render_idx();
            layout.clock.set_unadjusted(t);
            let before = layout.active_monitor_ref().unwrap().workspace_render_idx();
            let velocity = (before - prev) / dt.as_secs_f64();
            assert!(velocity > 0.01, "exercise the in-flight DnD spring");

            layout.active_monitor().unwrap().dnd_scroll_gesture_end();
            let after = layout.active_monitor_ref().unwrap().workspace_render_idx();
            assert!((before - after).abs() < 1e-10, "hz={hz}, rate={rate}");
            layout.clock.set_unadjusted(t + dt);
            let next = layout.active_monitor_ref().unwrap().workspace_render_idx();
            assert!(
                ((next - after) / dt.as_secs_f64() - velocity).abs() < 0.005,
                "hz={hz}, rate={rate}"
            );
            layout.clock.set_unadjusted(Duration::from_secs(5));
            layout.advance_animations();
            assert_eq!(
                layout.active_monitor_ref().unwrap().workspace_render_idx(),
                1.
            );
        }
    }
}

#[test]
fn horizontal_dnd_release_combines_scroll_and_layout_momentum() {
    for hz in [60., 120., 144., 240., 360.] {
        for rate in [0.5, 1., 2.] {
            let mut options = Options::default();
            options.layout.auto_stack_new_windows = false;
            options.layout.center_focused_column = CenterFocusedColumn::Always;
            options.gestures.dnd_edge_view_scroll.delay_ms = 0;
            let scroll_delta = options.gestures.dnd_edge_view_scroll.max_speed * 0.01;
            let mut layout = check_ops_with_options(
                options,
                [
                    Op::AddOutput(1),
                    Op::AddWindow {
                        params: TestWindowParams::new(1),
                    },
                    Op::AddWindow {
                        params: TestWindowParams::new(2),
                    },
                    Op::AddWindow {
                        params: TestWindowParams::new(3),
                    },
                    Op::CompleteAnimations,
                ],
            );
            layout.clock.set_rate(rate);
            layout
                .active_workspace_mut()
                .unwrap()
                .scrolling_mut()
                .dnd_scroll_gesture_begin();
            // Two real edge-scroll readings, followed by a column activation
            // while the drag is still held, exercise both velocity sources.
            for millis in [10, 20] {
                layout.clock.set_unadjusted(Duration::from_millis(millis));
                assert!(layout
                    .active_workspace_mut()
                    .unwrap()
                    .scrolling_mut()
                    .dnd_scroll_gesture_scroll(1.));
            }
            layout.focus_column_first();

            let t = Duration::from_millis(20) + Duration::from_secs_f64(5. / hz);
            let dt = Duration::from_micros(1);
            layout.clock.set_unadjusted(t - dt);
            let prev = layout.active_workspace().unwrap().scrolling().view_pos();
            layout.clock.set_unadjusted(t);
            let before = layout.active_workspace().unwrap().scrolling().view_pos();
            let layout_velocity = (before - prev) / dt.as_secs_f64();
            assert!(
                layout_velocity.abs() > 1.,
                "exercise the in-flight DnD spring"
            );
            // The first reading establishes the tracker interval; the second
            // contributes scroll_delta and release contributes zero movement.
            let scroll_velocity = scroll_delta / (t - Duration::from_millis(10)).as_secs_f64();

            layout
                .active_workspace_mut()
                .unwrap()
                .scrolling_mut()
                .dnd_scroll_gesture_end();
            let after = layout.active_workspace().unwrap().scrolling().view_pos();
            assert!((before - after).abs() < 1e-8, "hz={hz}, rate={rate}");
            layout.clock.set_unadjusted(t + dt);
            let next = layout.active_workspace().unwrap().scrolling().view_pos();
            assert!(
                ((next - after) / dt.as_secs_f64() - layout_velocity - scroll_velocity).abs() < 1.,
                "hz={hz}, rate={rate}"
            );
            layout.clock.set_unadjusted(Duration::from_secs(5));
            layout.advance_animations();
            let scrolling = layout.active_workspace().unwrap().scrolling();
            assert_eq!(scrolling.view_pos(), scrolling.target_view_pos());
        }
    }
}

#[test]
fn tile_retarget_keeps_momentum_even_when_new_offset_is_zero() {
    let mut clock = Clock::with_time(Duration::ZERO);
    let scene_resources = crate::scene::resources::SceneResources::new();
    let mut tile = Tile::new(
        TestWindow::new(TestWindowParams::new(1)),
        Size::from((1920., 1080.)),
        1.,
        clock.clone(),
        Rc::new(Options::default()),
        scene_resources.allocator().windows.allocate(),
    );
    tile.animate_move_from(Point::from((400., -200.)));
    clock.set_unadjusted(Duration::from_micros(59999));
    let prev = tile.render_offset();
    clock.set_unadjusted(Duration::from_millis(60));
    let before = tile.render_offset();
    let velocity = (before - prev).upscale(1_000_000.);
    assert!(velocity.x < -1. && velocity.y > 1.);
    tile.animate_move_from(Point::from((-before.x, -before.y)));
    let after = tile.render_offset();
    assert!(after.x.abs() < 1e-10 && after.y.abs() < 1e-10);
    clock.set_unadjusted(Duration::from_micros(60001));
    let next = tile.render_offset();
    let next_velocity = (next - after).upscale(1_000_000.);
    assert!((next_velocity.x - velocity.x).abs() < 0.2);
    assert!((next_velocity.y - velocity.y).abs() < 0.2);
    clock.set_unadjusted(Duration::from_secs(2));
    assert_eq!(tile.render_offset(), Point::from((0., 0.)));
}

fn format_tiles(layout: &Layout<TestWindow>) -> String {
    let mut buf = String::new();
    let ws = layout.active_workspace().unwrap();
    let mut tiles: Vec<_> = ws.tiles_with_render_positions().collect();

    // We sort by id since that gives us a consistent order (from first opened to last), but we
    // don't print the id since it's nondeterministic (the id is a global counter across all
    // running tests in the same binary).
    tiles.sort_by_key(|(tile, _, _)| tile.window().id());
    for (tile, pos, _visible) in tiles {
        let Size { w, h, .. } = tile.animated_tile_size();
        let Point { x, y, .. } = pos;
        writeln!(&mut buf, "{w:>3.0} × {h:>3.0} at x:{x:>3.0} y:{y:>3.0}").unwrap();
    }
    buf
}

fn window_y(layout: &mut Layout<TestWindow>, id: usize) -> f64 {
    layout
        .active_workspace_mut()
        .unwrap()
        .tiles_with_render_positions_mut(false)
        .find(|(tile, _)| *tile.window().id() == id)
        .unwrap()
        .1
        .y
}

fn incoming_y(layout: &mut Layout<TestWindow>, id: usize) -> (f64, f64) {
    let at = layout.clock.now_unadjusted();
    let value = window_y(layout, id);
    let dt = Duration::from_micros(1);
    layout.clock.set_unadjusted(at - dt);
    let previous = window_y(layout, id);
    layout.clock.set_unadjusted(at);
    (value, (value - previous) / dt.as_secs_f64())
}

fn assert_y_continuity(layout: &mut Layout<TestWindow>, id: usize, incoming: (f64, f64)) {
    let (value, velocity) = incoming;
    assert!((window_y(layout, id) - value).abs() < 1e-9);
    let at = layout.clock.now_unadjusted();
    let dt = Duration::from_micros(1);
    layout.clock.set_unadjusted(at + dt);
    let measured = (window_y(layout, id) - value) / dt.as_secs_f64();
    layout.clock.set_unadjusted(at);
    // Adjacent secants differ by acceleration over two microseconds; this is
    // below 0.01 px/s for these fixtures, far below a discarded launch velocity.
    assert!(
        (measured - velocity).abs() < 0.01,
        "expected={velocity}, actual={measured}"
    );
}

fn assert_y_continuation(
    layout: &mut Layout<TestWindow>,
    id: usize,
    incoming: (f64, f64),
    target: f64,
    elapsed: f64,
) {
    let (from, velocity) = incoming;
    // Fixture presets last one second. This independent endpoint/tangent
    // equation checks the adapter's compound motion and amplitude conversion.
    let expected = from
        + (target - from) * elapsed.powi(2) * (3. - 2. * elapsed)
        + velocity * elapsed * (1. - elapsed).powi(2);
    // Incoming velocity is a one-microsecond secant, so its contribution can
    // differ from the analytic tangent by less than 0.001 logical pixels.
    assert!(
        (window_y(layout, id) - expected).abs() < 0.001,
        "expected={expected}, actual={}",
        window_y(layout, id)
    );
}

fn make_options() -> Options {
    const LINEAR: Kind = Kind::Easing(EasingParams {
        duration_ms: 1000,
        curve: Curve::Linear,
    });

    let mut options = Options {
        layout: clarion_config::Layout {
            gaps: 0.0,
            auto_stack_new_windows: false,
            ..Default::default()
        },
        ..Options::default()
    };
    options.animations.window_resize.anim.kind = LINEAR;
    options.animations.window_movement.0.kind = LINEAR;

    options
}

fn set_up_two_in_column() -> Layout<TestWindow> {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusColumnLeft,
        Op::ConsumeWindowIntoColumn,
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        Op::SetForcedSize {
            id: 2,
            size: Some(Size::new(200, 200)),
        },
        Op::Communicate(1),
        Op::Communicate(2),
        Op::CompleteAnimations,
    ];

    check_ops_with_options(make_options(), ops)
}

#[test]
fn dwindle_tile_transitions_remain_active_while_idle() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::SwitchTilingMode(Some(clarion_ipc::TilingMode::Dwindle)),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::CompleteAnimations,
    ]);
    assert!(!layout
        .active_workspace()
        .unwrap()
        .dwindle
        .are_transitions_ongoing());

    Op::AddWindow {
        params: TestWindowParams::new(1),
    }
    .apply(&mut layout);
    assert!(layout
        .active_workspace()
        .unwrap()
        .dwindle
        .are_transitions_ongoing());
}

#[test]
fn height_resize_animates_next_y() {
    let mut layout = set_up_two_in_column();

    let ops = [
        // Issue a resize.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::AdjustFixed(-50),
        },
        // The top window shrinks in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 50)),
        },
        Op::Communicate(1),
        Op::Communicate(2),
    ];
    check_ops_on_layout(&mut layout, ops);

    // No time had passed yet, so we're at the initial state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");

    // Advance the time halfway.
    Op::AdvanceAnimations { msec_delta: 500 }.apply(&mut layout);

    // Top window is half-resized at 75 px tall, bottom window is at y=75 matching it.
    assert_snapshot!(format_tiles(&layout), @r"
    100 ×  75 at x:  0 y:  0
    200 × 200 at x:  0 y: 75
    ");

    // Advance the time to completion.
    Op::AdvanceAnimations { msec_delta: 500 }.apply(&mut layout);

    // Final state at 50 px.
    assert_snapshot!(format_tiles(&layout), @r"
    100 ×  50 at x:  0 y:  0
    200 × 200 at x:  0 y: 50
    ");
}

#[test]
fn clientside_height_change_doesnt_animate() {
    let mut layout = set_up_two_in_column();

    // The initial state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");

    let ops = [
        // The top window shrinks by itself, without a clarion-issued resize.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 50)),
        },
        // This does not start any animations.
        Op::Communicate(1),
        Op::Communicate(2),
    ];
    check_ops_on_layout(&mut layout, ops);

    // No time had passed yet, but we are at the final state right away.
    assert_snapshot!(format_tiles(&layout), @r"
    100 ×  50 at x:  0 y:  0
    200 × 200 at x:  0 y: 50
    ");
}

#[test]
fn height_resize_and_back() {
    let mut layout = set_up_two_in_column();

    // The initial state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");

    let ops = [
        // Issue a resize.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(200),
        },
        // The top window grows in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 200)),
        },
        // This starts the resize animation.
        Op::Communicate(1),
        Op::Communicate(2),
        // Advance the time halfway.
        Op::AdvanceAnimations { msec_delta: 500 },
    ];
    check_ops_on_layout(&mut layout, ops);

    // Top window is half-resized at 150 px tall, bottom window is at y=150 matching it.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 150 at x:  0 y:  0
    200 × 200 at x:  0 y:150
    ");

    let incoming = incoming_y(&mut layout, 2);
    let ops = [
        // Issue a resize back.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(100),
        },
        // The top window shrinks in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        // This starts a new resize animation.
        Op::Communicate(1),
        Op::Communicate(2),
    ];
    check_ops_on_layout(&mut layout, ops);

    assert_y_continuity(&mut layout, 2, incoming);

    // No time had passed yet, and we expect no animation jumps, so this state matches the last.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 150 at x:  0 y:  0
    200 × 200 at x:  0 y:150
    ");

    // Advance the time halfway.
    Op::AdvanceAnimations { msec_delta: 500 }.apply(&mut layout);

    // Height is halfway resized; Y retains its incoming +100 px/s momentum.
    assert_y_continuation(&mut layout, 2, incoming, 100., 0.5);
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 125 at x:  0 y:  0
    200 × 200 at x:  0 y:138
    ");

    // Advance the time to completion.
    Op::AdvanceAnimations { msec_delta: 500 }.apply(&mut layout);

    // Final state back at 100px.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");
}

#[test]
fn height_resize_and_cancel() {
    let mut layout = set_up_two_in_column();

    // The initial state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");

    let ops = [
        // Issue a resize.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(200),
        },
        // The top window grows in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 200)),
        },
        // This starts the resize animation.
        Op::Communicate(1),
        Op::Communicate(2),
        // Advance the time slightly.
        Op::AdvanceAnimations { msec_delta: 50 },
    ];
    check_ops_on_layout(&mut layout, ops);

    // Top window is half-resized at 105 px tall, bottom window is at y=105 matching it.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 105 at x:  0 y:  0
    200 × 200 at x:  0 y:105
    ");

    let ops = [
        // Issue a resize back.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(100),
        },
        // The top window shrinks in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        // This cancels the resize animation since the change of 5 px is less than the resize
        // animation threshold.
        Op::Communicate(1),
        Op::Communicate(2),
    ];
    check_ops_on_layout(&mut layout, ops);

    // Since the resize animation is cancelled, the height goes to the new value immediately. The Y
    // position doesn't jump, instead the animation is offset to preserve the current position.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:105
    ");

    // Advance to the end of the move animation.
    Op::AdvanceAnimations { msec_delta: 1000 }.apply(&mut layout);

    assert!(!layout.are_transitions_ongoing(None));
    // Final state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");
}

#[test]
fn height_resize_and_back_during_another_y_anim() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusColumnLeft,
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        Op::SetForcedSize {
            id: 2,
            size: Some(Size::new(200, 200)),
        },
        Op::Communicate(1),
        Op::Communicate(2),
        Op::CompleteAnimations,
    ];
    let mut layout = check_ops_with_options(make_options(), ops);

    // The initial state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");

    // Consume second window into column, starting the X/Y move anim down.
    Op::ConsumeWindowIntoColumn.apply(&mut layout);

    // No time had passed, so no change in coordinates yet.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");

    // Advance the time halfway.
    Op::AdvanceAnimations { msec_delta: 500 }.apply(&mut layout);

    // Second window halfway to the bottom.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x: 50 y: 50
    ");

    let first_incoming = incoming_y(&mut layout, 2);
    let ops = [
        // Issue a resize.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(200),
        },
        // The top window grows in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 200)),
        },
        // This starts the resize animation.
        Op::Communicate(1),
        Op::Communicate(2),
    ];
    check_ops_on_layout(&mut layout, ops);

    assert_y_continuity(&mut layout, 2, first_incoming);

    // No time had passed, so no change in state yet.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x: 50 y: 50
    ");

    // Advance the time a bit.
    Op::AdvanceAnimations { msec_delta: 200 }.apply(&mut layout);

    // The compound Y motion follows the retained endpoint and incoming velocity.
    assert_y_continuation(&mut layout, 2, first_incoming, 200., 0.2);
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 120 at x:  0 y:  0
    200 × 200 at x: 30 y: 78
    ");

    let incoming = incoming_y(&mut layout, 2);
    let ops = [
        // Issue a resize back.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(100),
        },
        // The top window shrinks in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        // This starts the resize animation.
        Op::Communicate(1),
        Op::Communicate(2),
    ];
    check_ops_on_layout(&mut layout, ops);

    assert_y_continuity(&mut layout, 2, incoming);

    // No time had passed, so no change in state yet.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 120 at x:  0 y:  0
    200 × 200 at x: 30 y: 78
    ");

    // Advance the time a bit. Both resize and consume movement are still ongoing.
    Op::AdvanceAnimations { msec_delta: 200 }.apply(&mut layout);

    assert_y_continuation(&mut layout, 2, incoming, 100., 0.2);
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 116 at x:  0 y:  0
    200 × 200 at x: 10 y:103
    ");

    // Advance the time to complete the consume movement.
    Op::AdvanceAnimations { msec_delta: 100 }.apply(&mut layout);

    // The continuation is still turning its original downward momentum around.
    assert_y_continuation(&mut layout, 2, incoming, 100., 0.3);
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 114 at x:  0 y:  0
    200 × 200 at x:  0 y:109
    ");

    // Advance the time to complete the resize.
    Op::AdvanceAnimations { msec_delta: 700 }.apply(&mut layout);

    // Final state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");
}

#[test]
fn height_resize_and_cancel_during_another_y_anim() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusColumnLeft,
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        Op::SetForcedSize {
            id: 2,
            size: Some(Size::new(200, 200)),
        },
        Op::Communicate(1),
        Op::Communicate(2),
        Op::CompleteAnimations,
    ];
    let mut layout = check_ops_with_options(make_options(), ops);

    // The initial state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");

    // Consume second window into column, starting the X/Y move anim down.
    Op::ConsumeWindowIntoColumn.apply(&mut layout);

    // No time had passed, so no change in coordinates yet.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");

    // Advance the time halfway.
    Op::AdvanceAnimations { msec_delta: 500 }.apply(&mut layout);

    // Second window halfway to the bottom.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x: 50 y: 50
    ");

    let first_incoming = incoming_y(&mut layout, 2);
    let ops = [
        // Issue a resize.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(200),
        },
        // The top window grows in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 200)),
        },
        // This starts the resize animation.
        Op::Communicate(1),
        Op::Communicate(2),
        // Advance the time slightly.
        Op::AdvanceAnimations { msec_delta: 50 },
    ];
    check_ops_on_layout(&mut layout, ops);

    assert_y_continuation(&mut layout, 2, first_incoming, 200., 0.05);
    // X retains its independent movement; Y carries the prior downward velocity.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 105 at x:  0 y:  0
    200 × 200 at x: 45 y: 56
    ");

    let incoming = incoming_y(&mut layout, 2);
    let ops = [
        // Issue a resize back.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(100),
        },
        // The top window shrinks in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        // This cancels the resize animation since the change of 5 px is less than the resize
        // animation threshold.
        Op::Communicate(1),
        Op::Communicate(2),
    ];
    check_ops_on_layout(&mut layout, ops);

    assert_y_continuity(&mut layout, 2, incoming);
    // Cancel retargets Y without changing its visible position or incoming velocity.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x: 45 y: 56
    ");

    // Advance the time to complete the consume movement.
    Op::AdvanceAnimations { msec_delta: 450 }.apply(&mut layout);

    // A cancelled resize uses the same retained-duration continuation as other retargets.
    assert_y_continuation(&mut layout, 2, incoming, 100., 0.45);
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y: 91
    ");

    // Advance the time to complete the resize-induced anim.
    Op::AdvanceAnimations { msec_delta: 550 }.apply(&mut layout);

    // Final state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");
}

#[test]
fn height_resize_before_another_y_anim_then_back() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusColumnLeft,
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        Op::SetForcedSize {
            id: 2,
            size: Some(Size::new(200, 200)),
        },
        Op::Communicate(1),
        Op::Communicate(2),
        Op::CompleteAnimations,
        // Issue a resize.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(200),
        },
        // The top window grows in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 200)),
        },
        // This starts the resize animation.
        Op::Communicate(1),
        Op::Communicate(2),
        // Advance the time a bit.
        Op::AdvanceAnimations { msec_delta: 200 },
    ];
    let mut layout = check_ops_with_options(make_options(), ops);

    // The resize is in progress.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 120 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");

    // Consume second window into column, starting the X/Y move anim down.
    Op::ConsumeWindowIntoColumn.apply(&mut layout);

    // No time had passed, so no change in coordinates yet.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 120 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");

    // Advance the time halfway.
    Op::AdvanceAnimations { msec_delta: 600 }.apply(&mut layout);

    // Second window halfway to the bottom. Since consume happened after the start of the first
    // window's resize, the second window's Y is unaffected by it and is animating towards the
    // final position right away.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 180 at x:  0 y:  0
    200 × 200 at x: 40 y:120
    ");

    let incoming = incoming_y(&mut layout, 2);
    let ops = [
        // Issue a resize back.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(100),
        },
        // The top window shrinks in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        // This starts the resize animation.
        Op::Communicate(1),
        Op::Communicate(2),
    ];
    check_ops_on_layout(&mut layout, ops);

    assert_y_continuity(&mut layout, 2, incoming);

    // No time had passed, so no change in state yet.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 180 at x:  0 y:  0
    200 × 200 at x: 40 y:120
    ");

    // Advance the time a bit. Both resize and consume movement are still ongoing.
    Op::AdvanceAnimations { msec_delta: 200 }.apply(&mut layout);

    assert_y_continuation(&mut layout, 2, incoming, 100., 0.2);
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 164 at x:  0 y:  0
    200 × 200 at x: 20 y:144
    ");

    // Advance the time to complete the consume movement.
    Op::AdvanceAnimations { msec_delta: 200 }.apply(&mut layout);

    assert_y_continuation(&mut layout, 2, incoming, 100., 0.4);
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 148 at x:  0 y:  0
    200 × 200 at x:  0 y:142
    ");

    // Advance the time to complete the resize.
    Op::AdvanceAnimations { msec_delta: 600 }.apply(&mut layout);

    // Final state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");
}

#[test]
fn height_resize_before_another_y_anim_then_cancel() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusColumnLeft,
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        Op::SetForcedSize {
            id: 2,
            size: Some(Size::new(200, 200)),
        },
        Op::Communicate(1),
        Op::Communicate(2),
        Op::CompleteAnimations,
        // Issue a resize.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(200),
        },
        // The top window grows in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 200)),
        },
        // This starts the resize animation.
        Op::Communicate(1),
        Op::Communicate(2),
        // Advance the time a bit.
        Op::AdvanceAnimations { msec_delta: 20 },
    ];
    let mut layout = check_ops_with_options(make_options(), ops);

    // The resize is in progress.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 102 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");

    // Consume second window into column, starting the X/Y move anim down.
    Op::ConsumeWindowIntoColumn.apply(&mut layout);

    // No time had passed, so no change in coordinates yet.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 102 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");

    // Advance the time a little.
    Op::AdvanceAnimations { msec_delta: 20 }.apply(&mut layout);

    // Second window on its way to the bottom.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 104 at x:  0 y:  0
    200 × 200 at x: 98 y:  4
    ");

    let ops = [
        // Issue a resize back.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(100),
        },
        // The top window shrinks in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        // This cancels the resize animation since the change of 4 px is less than the resize
        // animation threshold.
        Op::Communicate(1),
        Op::Communicate(2),
    ];
    check_ops_on_layout(&mut layout, ops);

    // The second window's trajectory readjusts to the new final position at 100 px, without jumps.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x: 98 y:  4
    ");

    // Advance the time to complete the consume movement.
    Op::AdvanceAnimations { msec_delta: 980 }.apply(&mut layout);

    // Final state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");
}

#[test]
fn clientside_height_change_during_another_y_anim() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusColumnLeft,
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        Op::SetForcedSize {
            id: 2,
            size: Some(Size::new(200, 200)),
        },
        Op::Communicate(1),
        Op::Communicate(2),
        Op::CompleteAnimations,
        Op::ConsumeWindowIntoColumn,
        // Clear the animate next configure flag.
        Op::Communicate(1),
        Op::Communicate(2),
        // Advance the time a bit.
        Op::AdvanceAnimations { msec_delta: 200 },
    ];
    let mut layout = check_ops_with_options(make_options(), ops);

    // Second window on its way to the bottom.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x: 80 y: 20
    ");

    let incoming = incoming_y(&mut layout, 2);
    let ops = [
        // The top window suddenly grows.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 200)),
        },
        Op::Communicate(1),
        Op::Communicate(2),
    ];
    check_ops_on_layout(&mut layout, ops);

    assert_y_continuity(&mut layout, 2, incoming);
    // The second window's trajectory readjusts to the new final position at 200 px, without jumps.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 200 at x:  0 y:  0
    200 × 200 at x: 80 y: 20
    ");

    // X completes at its original deadline; Y follows the retained retarget
    // duration with incoming momentum, rather than changing amplitude/velocity.
    Op::AdvanceAnimations { msec_delta: 800 }.apply(&mut layout);
    assert_y_continuation(&mut layout, 2, incoming, 200., 0.8);
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 200 at x:  0 y:  0
    200 × 200 at x:  0 y:184
    ");
    Op::AdvanceAnimations { msec_delta: 200 }.apply(&mut layout);
    assert!(!layout.are_transitions_ongoing(None));

    // Final state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 200 at x:  0 y:  0
    200 × 200 at x:  0 y:200
    ");
}

#[test]
fn height_resize_cancel_with_stationary_second_window() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusColumnLeft,
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        Op::SetForcedSize {
            id: 2,
            size: Some(Size::new(200, 200)),
        },
        Op::Communicate(1),
        Op::Communicate(2),
        Op::CompleteAnimations,
        // Issue a resize.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(200),
        },
        // The top window grows in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 200)),
        },
        // This starts the resize animation.
        Op::Communicate(1),
        Op::Communicate(2),
        // Advance the time a bit.
        Op::AdvanceAnimations { msec_delta: 20 },
    ];
    let mut options = make_options();
    // Window movement will happen instantly.
    options.animations.window_movement.0.off = true;
    let mut layout = check_ops_with_options(options, ops);

    // The resize is in progress.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 102 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");

    // Consume second window into column, starting the X/Y move anim down.
    Op::ConsumeWindowIntoColumn.apply(&mut layout);

    // No time had passed, so no change in coordinates yet.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 102 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");

    // Advance the time a little.
    Op::AdvanceAnimations { msec_delta: 20 }.apply(&mut layout);

    // The window movement anim is off, so the second window is already at the bottom. Since
    // consume started after the resize, the second window is unaffected by the resize-induced Y
    // movement, and sits at the final position at 200 px.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 104 at x:  0 y:  0
    200 × 200 at x:  0 y:200
    ");

    let ops = [
        // Issue a resize back.
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(100),
        },
        // The top window shrinks in response, the bottom remains as is.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        // This cancels the resize animation since the change of 4 px is less than the resize
        // animation threshold.
        Op::Communicate(1),
        Op::Communicate(2),
    ];
    check_ops_on_layout(&mut layout, ops);

    // This causes the second window to jump down, which is correct because it hadn't been in an
    // animation, and as far as it's concerned, this is the same case as a window just deciding to
    // do a clientside resize on its own, which is not animated.
    //
    // Since the resize is also cancelled, this is the final state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");
}

#[test]
fn width_resize_and_cancel() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusColumnLeft,
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        Op::SetForcedSize {
            id: 2,
            size: Some(Size::new(200, 200)),
        },
        Op::Communicate(1),
        Op::Communicate(2),
        Op::CompleteAnimations,
    ];
    let mut layout = check_ops_with_options(make_options(), ops);

    // The initial state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");

    let ops = [
        // Issue a resize.
        Op::SetWindowWidth {
            id: None,
            change: SizeChange::SetFixed(200),
        },
        // The left window grows in response.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(200, 100)),
        },
        // This starts the resize animation.
        Op::Communicate(1),
        Op::Communicate(2),
        // Advance the time slightly.
        Op::AdvanceAnimations { msec_delta: 50 },
    ];
    check_ops_on_layout(&mut layout, ops);

    // Left window is half-resized at 105 px wide, right window is at x=105 matching it.
    assert_snapshot!(format_tiles(&layout), @r"
    105 × 100 at x:  0 y:  0
    200 × 200 at x:105 y:  0
    ");

    let ops = [
        // Issue a resize back.
        Op::SetWindowWidth {
            id: None,
            change: SizeChange::SetFixed(100),
        },
        // The top window shrinks in response.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        // This cancels the resize animation since the change of 5 px is less than the resize
        // animation threshold.
        Op::Communicate(1),
        Op::Communicate(2),
    ];
    check_ops_on_layout(&mut layout, ops);

    // Since the resize animation is cancelled, the width goes to the new value immediately. The X
    // position doesn't jump, instead the animation is restarted to preserve the current position.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:105 y:  0
    ");

    // Advance to the end of the move animation.
    Op::AdvanceAnimations { msec_delta: 1000 }.apply(&mut layout);

    // Final state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");
}

#[test]
fn width_resize_and_cancel_of_column_to_the_left() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        Op::SetForcedSize {
            id: 2,
            size: Some(Size::new(200, 200)),
        },
        Op::Communicate(1),
        Op::Communicate(2),
        Op::CompleteAnimations,
    ];
    let mut layout = check_ops_with_options(make_options(), ops);

    // The initial state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");

    let ops = [
        // Issue a resize.
        Op::SetWindowWidth {
            id: Some(1),
            change: SizeChange::SetFixed(200),
        },
        // The left window grows in response.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(200, 100)),
        },
        // This starts the resize animation.
        Op::Communicate(1),
        Op::Communicate(2),
        // Advance the time slightly.
        Op::AdvanceAnimations { msec_delta: 50 },
    ];
    check_ops_on_layout(&mut layout, ops);

    // Left window is half-resized at 105 px wide, it's at x=-5 matching the right edge position.
    assert_snapshot!(format_tiles(&layout), @r"
    105 × 100 at x: -5 y:  0
    200 × 200 at x:100 y:  0
    ");

    let ops = [
        // Issue a resize back.
        Op::SetWindowWidth {
            id: Some(1),
            change: SizeChange::SetFixed(100),
        },
        // The top window shrinks in response.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::new(100, 100)),
        },
        // This cancels the resize animation since the change of 5 px is less than the resize
        // animation threshold.
        Op::Communicate(1),
        Op::Communicate(2),
    ];
    check_ops_on_layout(&mut layout, ops);

    // Since the resize animation is cancelled, the width goes to the new value immediately. The X
    // position doesn't jump, instead the animation is restarted to preserve the current position.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x: -5 y:  0
    200 × 200 at x:100 y:  0
    ");

    // Advance to the end of the move animation.
    Op::AdvanceAnimations { msec_delta: 1000 }.apply(&mut layout);

    // Final state.
    assert_snapshot!(format_tiles(&layout), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:100 y:  0
    ");
}

#[test]
fn interrupted_resize_preserves_each_size_velocity() {
    for rate in [0.25, 1., 2.] {
        for hz in [60., 120., 144., 240., 360.] {
            let mut clock = Clock::with_time(Duration::ZERO);
            clock.set_rate(rate);
            // Expose subpixel motion to the derivative check without a GPU.
            let scene_resources = crate::scene::resources::SceneResources::new();
            let mut tile = Tile::new(
                TestWindow::new(TestWindowParams::new(1)),
                Size::from((1920., 1080.)),
                1_000_000.,
                clock.clone(),
                Rc::new(Options::default()),
                scene_resources.allocator().windows.allocate(),
            );
            tile.window_mut()
                .request_size((1000, 700).into(), SizingMode::Fullscreen, true, None);
            tile.window().communicate();
            tile.update_window();
            let at = Duration::from_secs_f64(5. / hz);
            let delta = Duration::from_micros(1);
            clock.set_unadjusted(at - delta);
            let previous = [tile.animated_window_size(), tile.animated_tile_size()];
            clock.set_unadjusted(at);
            let before = [tile.animated_window_size(), tile.animated_tile_size()];
            tile.window_mut()
                .request_size((300, 1200).into(), SizingMode::Normal, true, None);
            tile.window().communicate();
            tile.update_window();
            let after = [tile.animated_window_size(), tile.animated_tile_size()];
            assert_eq!(before, after);
            clock.set_unadjusted(at + delta);
            let next = [tile.animated_window_size(), tile.animated_tile_size()];
            for i in 0..2 {
                for (prev, start, finish) in [
                    (previous[i].w, before[i].w, next[i].w),
                    (previous[i].h, before[i].h, next[i].h),
                ] {
                    let old_velocity = (start - prev) / delta.as_secs_f64();
                    let new_velocity = (finish - start) / delta.as_secs_f64();
                    assert!(old_velocity.abs() > 100.);
                    assert!(
                        (old_velocity - new_velocity).abs() < 10.,
                        "rate={rate}, hz={hz}, old={old_velocity}, new={new_velocity}"
                    );
                }
            }
            clock.set_unadjusted(Duration::from_secs(5));
            tile.advance_animations();
            assert!(tile.resize_animation().is_none());
            assert!(!tile.are_transitions_ongoing());
            assert_eq!(tile.animated_window_size(), Size::from((300., 1200.)));
            assert_eq!(tile.animated_tile_size(), tile.tile_size());
        }
    }
}

#[test]
fn resize_to_current_size_keeps_momentum_and_eventually_stops() {
    let mut clock = Clock::with_time(Duration::ZERO);
    let scene_resources = crate::scene::resources::SceneResources::new();
    let mut tile = Tile::new(
        TestWindow::new(TestWindowParams::new(1)),
        Size::from((1920., 1080.)),
        1.,
        clock.clone(),
        Rc::new(Options::default()),
        scene_resources.allocator().windows.allocate(),
    );
    tile.window_mut()
        .request_size((1000, 700).into(), SizingMode::Normal, true, None);
    tile.window().communicate();
    tile.update_window();
    clock.set_unadjusted(Duration::from_millis(60));
    let current = tile.animated_window_size();
    tile.window_mut()
        .request_size(current.to_i32_round(), SizingMode::Normal, true, None);
    tile.window().communicate();
    tile.update_window();
    assert!(tile.resize_animation().is_some());
    assert_eq!(tile.animated_window_size(), current);
    clock.set_unadjusted(Duration::from_millis(61));
    assert!(tile.animated_window_size().w > current.w);
    clock.set_unadjusted(Duration::from_secs(2));
    tile.advance_animations();
    assert!(!tile.are_transitions_ongoing());
    assert!(tile.resize_animation().is_none());
    assert_eq!(tile.animated_window_size(), current);
}

#[test]
fn clamped_tile_alpha_stops_redrawing_and_retains_held_target() {
    for held in [false, true] {
        let mut clock = Clock::with_time(Duration::ZERO);
        let scene_resources = crate::scene::resources::SceneResources::new();
        let mut tile = Tile::new(
            TestWindow::new(TestWindowParams::new(1)),
            Size::from((1920., 1080.)),
            1.,
            clock.clone(),
            Rc::new(Options::default()),
            scene_resources.allocator().windows.allocate(),
        );
        let mut config = clarion_config::animations::WindowMovementAnim::default().0;
        let Kind::Spring(params) = &mut config.kind else {
            unreachable!()
        };
        params.damping_ratio = 0.2;
        // Pin the oscillation phase being tested, independently of UI defaults.
        params.stiffness = 800;
        tile.animate_alpha(
            if held { 1. } else { 0.75 },
            if held { 0.75 } else { 1. },
            config,
        );
        if held {
            tile.hold_alpha_animation_after_done();
        }
        clock.set_unadjusted(Duration::from_millis(100));
        let alpha = tile.presentation.alpha_animation.as_ref().unwrap();
        assert!(alpha.anim.is_clamped_done());
        assert!(!alpha.anim.is_done());
        assert!(!tile.are_transitions_ongoing());
        tile.advance_animations();
        if held {
            assert_eq!(
                tile.presentation
                    .alpha_animation
                    .as_ref()
                    .unwrap()
                    .anim
                    .clamped_value(),
                0.75
            );
        } else {
            assert!(tile.presentation.alpha_animation.is_none());
        }
    }
}

#[test]
fn shrinking_resize_keeps_signed_velocity_through_repeated_reversals() {
    let mut clock = Clock::with_time(Duration::ZERO);
    let scene_resources = crate::scene::resources::SceneResources::new();
    let mut tile = Tile::new(
        TestWindow::new(TestWindowParams::new(1)),
        Size::from((1920., 1080.)),
        1_000_000.,
        clock.clone(),
        Rc::new(Options::default()),
        scene_resources.allocator().windows.allocate(),
    );
    tile.window_mut()
        .request_size((1000, 700).into(), SizingMode::Normal, true, None);
    tile.window().communicate();
    tile.update_window();
    clock.set_unadjusted(Duration::from_secs(2));
    tile.advance_animations();
    tile.window_mut()
        .request_size((300, 150).into(), SizingMode::Normal, true, None);
    tile.window().communicate();
    tile.update_window();
    for (at_ms, target) in [(2060, (1100, 900)), (2064, (500, 400)), (2068, (900, 300))] {
        let at = Duration::from_millis(at_ms);
        let delta = Duration::from_micros(1);
        clock.set_unadjusted(at - delta);
        let previous = tile.animated_window_size();
        clock.set_unadjusted(at);
        let before = tile.animated_window_size();
        tile.window_mut()
            .request_size(target.into(), SizingMode::Normal, true, None);
        tile.window().communicate();
        tile.update_window();
        assert_eq!(before, tile.animated_window_size());
        clock.set_unadjusted(at + delta);
        let next = tile.animated_window_size();
        for (previous, before, next) in [
            (previous.w, before.w, next.w),
            (previous.h, before.h, next.h),
        ] {
            let old_velocity = (before - previous) / delta.as_secs_f64();
            let new_velocity = (next - before) / delta.as_secs_f64();
            assert!(old_velocity < -100., "at={at_ms}, velocity={old_velocity}");
            assert!(
                (old_velocity - new_velocity).abs() < 5.,
                "at={at_ms}, before={old_velocity}, after={new_velocity}"
            );
        }
    }
    clock.set_unadjusted(Duration::from_secs(4));
    tile.advance_animations();
    assert!(!tile.are_transitions_ongoing());
    assert_eq!(tile.animated_window_size(), Size::from((900., 300.)));
}

#[test]
fn shrinking_resize_clamps_negative_overshoot_and_hidden_inward_velocity() {
    let mut clock = Clock::with_time(Duration::ZERO);
    let mut options = Options::default();
    let Kind::Spring(params) = &mut options.animations.window_resize.anim.kind else {
        unreachable!()
    };
    params.damping_ratio = 0.1;
    // At 150 ms this spring is past its first peak and returning inward.
    params.stiffness = 800;
    let scene_resources = crate::scene::resources::SceneResources::new();
    let mut tile = Tile::new(
        TestWindow::new(TestWindowParams::new(1)),
        Size::from((1920., 1080.)),
        1_000_000.,
        clock.clone(),
        Rc::new(options),
        scene_resources.allocator().windows.allocate(),
    );
    tile.window_mut()
        .request_size((1, 1).into(), SizingMode::Normal, true, None);
    tile.window().communicate();
    tile.update_window();
    clock.set_unadjusted(Duration::from_millis(150));
    // This phase has overshot below zero and is already moving back inward.
    let progress = tile.resize_animation().unwrap();
    assert!(progress.value() > 1.);
    assert!(progress.velocity() < 0.);
    assert_eq!(tile.animated_window_size(), Size::from((1., 1.)));
    tile.window_mut()
        .request_size((500, 500).into(), SizingMode::Normal, true, None);
    tile.window().communicate();
    tile.update_window();
    assert_eq!(tile.animated_window_size(), Size::from((1., 1.)));
    clock.set_unadjusted(Duration::from_micros(150001));
    let next = tile.animated_window_size();
    assert!(next.w < 1.001 && next.h < 1.001);
    clock.set_unadjusted(Duration::from_secs(10));
    tile.advance_animations();
    assert!(!tile.are_transitions_ongoing());
    assert_eq!(tile.animated_window_size(), Size::from((500., 500.)));
}

#[test]
#[ignore = "requires real DRM render node, EGL and Vulkan"]
fn interrupted_resize_keeps_pixels_when_commit_application_is_delayed() {
    use smithay::backend::allocator::Fourcc;
    use smithay::backend::renderer::gles::GlesRenderer;
    use smithay::utils::Transform;

    use crate::render_helpers::renderer::AnyRenderer;
    use crate::render_helpers::solid_color::SolidColorBuffer;
    use crate::render_helpers::texture::TextureBuffer;
    use crate::render_helpers::xray::XrayPos;
    use crate::render_helpers::{render_to_vec, shaders, BakedBuffer, RenderTarget};

    fn snapshot(
        renderer: &mut GlesRenderer,
        size: (i32, i32),
        color: [u8; 4],
    ) -> LayoutElementRenderSnapshot {
        let buffer = TextureBuffer::from_memory(
            renderer,
            &color.repeat((size.0 * size.1) as usize),
            Fourcc::Abgr8888,
            size,
            false,
            1.,
            Transform::Normal,
            Vec::new(),
        )
        .unwrap();
        RenderSnapshot {
            contents: vec![BakedBuffer {
                buffer,
                location: (0., 0.).into(),
                src: None,
                dst: None,
            }],
            contents_with_blocked_out_bg: None,
            blocked_out_contents: vec![BakedBuffer {
                buffer: SolidColorBuffer::new(Size::from(size).to_f64(), [0., 0., 0., 1.]),
                location: (0., 0.).into(),
                src: None,
                dst: None,
            }],
            block_out_from: None,
            size: Size::from(size).to_f64(),
            texture: Default::default(),
            texture_with_blocked_out_bg: Default::default(),
            blocked_out_texture: Default::default(),
        }
    }

    fn draw(tile: &mut Tile<TestWindow>, renderer: &mut GlesRenderer, scale: f64) -> Vec<u8> {
        tile.update_render_elements(false, Rectangle::from_size((64., 64.).into()));
        let mut elements = Vec::new();
        tile.render(
            RenderCtx {
                renderer,
                target: RenderTarget::Output,
                xray: None,
            },
            (8., 8.).into(),
            XrayPos::default(),
            false,
            &mut |e| elements.push(e),
        );
        render_to_vec(
            renderer,
            (80, 80).into(),
            scale.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            elements.iter().rev(),
        )
        .unwrap()
    }

    let (_vk, mut renderer, _allocator) = crate::render_helpers::vulkan::hardware_tests::setup();
    shaders::init(&mut renderer);
    crate::render_helpers::resources::init(&mut renderer);
    for (initial_scale, scale) in [(1., 1.), (2., 1.25)] {
        let mut clock = Clock::with_time(Duration::ZERO);
        let mut options = make_options();
        options.layout.border.off = true;
        options.layout.focus_ring.off = true;
        options.layout.shadow.on = false;
        options.animations.window_resize.anim.kind = Kind::Easing(EasingParams {
            duration_ms: 300,
            curve: Curve::Linear,
        });
        let options = Rc::new(options);
        let mut params = TestWindowParams::new(1);
        params.bbox = Rectangle::from_size((16, 16).into());
        let scene_resources = crate::scene::resources::SceneResources::new();
        let mut tile = Tile::new(
            TestWindow::new(params),
            (64., 64.).into(),
            initial_scale,
            clock.clone(),
            options.clone(),
            scene_resources.allocator().windows.allocate(),
        );
        tile.window_mut()
            .request_size((40, 40).into(), SizingMode::Normal, true, None);
        tile.window().communicate();
        tile.window().0.animation_snapshot.replace(Some(snapshot(
            &mut renderer,
            (16, 16),
            [255, 0, 0, 255],
        )));
        tile.window().0.render_color.set(Some([0., 1., 0., 1.]));
        tile.update_window();
        clock.set_unadjusted(Duration::from_millis(80));
        // Prime the original snapshot cache before moving to a differently scaled output.
        draw(&mut tile, &mut renderer, initial_scale);
        let precommit = snapshot(&mut renderer, (40, 40), [0, 255, 0, 255]);
        tile.update_config((64., 64.).into(), scale, options);
        // The next buffer is blocked for 40 ms while the old animation continues.
        clock.set_unadjusted(Duration::from_millis(120));
        let before_size = tile.animated_window_size();
        let before = draw(&mut tile, &mut renderer, scale);
        tile.window_mut()
            .request_size((20, 50).into(), SizingMode::Normal, true, None);
        tile.window().communicate();
        tile.window().0.animation_snapshot.replace(Some(precommit));
        tile.window().0.render_color.set(Some([0., 0., 1., 1.]));
        tile.prepare_resize_retarget(AnyRenderer::Gles(&mut renderer));
        tile.update_window();
        assert_eq!(tile.animated_window_size(), before_size);
        let after = draw(&mut tile, &mut renderer, scale);
        // Check interior pixels to avoid edge coverage changing with fractional geometry.
        for y in 14..22 {
            for x in 14..22 {
                let index = (y * 80 + x) * 4;
                let previous = &before[index..index + 4];
                assert!(
                    previous[0].abs_diff(153) <= 2 && previous[1].abs_diff(102) <= 2,
                    "expected the 120 ms mix, got {previous:?}"
                );
                assert!(
                    previous
                        .iter()
                        .zip(&after[index..index + 4])
                        .all(|(a, b)| a.abs_diff(*b) <= 2),
                    "scale {initial_scale} → {scale}: {previous:?} → {:?}",
                    &after[index..index + 4]
                );
            }
        }
        clock.set_unadjusted(Duration::from_millis(150));
        let progressing = draw(&mut tile, &mut renderer, scale);
        assert!(progressing[(16 * 80 + 16) * 4 + 2] > 20);
        clock.set_unadjusted(Duration::from_secs(1));
        tile.advance_animations();
        assert!(!tile.are_transitions_ongoing());
        let final_pixels = draw(&mut tile, &mut renderer, scale);
        assert_eq!(
            &final_pixels[(16 * 80 + 16) * 4..(16 * 80 + 16) * 4 + 4],
            &[0, 0, 255, 255]
        );
    }
}

#[test]
fn builtin_opening_releases_its_tile_transition_after_visible_spring_motion() {
    use crate::animation::{Spring, SpringParams};

    for damping_ratio in [0., 0.4] {
        for custom in [false, true] {
            let mut clock = Clock::with_time(Duration::ZERO);
            let mut options = Options::default();
            options.animations.window_open.anim.kind =
                Kind::Spring(clarion_config::animations::SpringParams {
                    damping_ratio,
                    stiffness: 400,
                    epsilon: 0.0001,
                });
            options.animations.window_open.custom_shader = custom.then(String::new);
            let scene_resources = crate::scene::resources::SceneResources::new();
            let mut tile = Tile::new(
                TestWindow::new(TestWindowParams::new(1)),
                (1920., 1080.).into(),
                1.,
                clock.clone(),
                Rc::new(options),
                scene_resources.allocator().windows.allocate(),
            );
            let visible_end = Spring {
                from: 0.,
                to: 1.,
                initial_velocity: 0.,
                params: SpringParams::new(damping_ratio, 400., 0.0001),
            }
            .clamped_duration()
            .unwrap();

            tile.start_open_animation();
            assert!(tile.are_transitions_ongoing());
            clock.set_unadjusted(visible_end - Duration::from_millis(1));
            tile.advance_animations();
            assert!(tile.are_transitions_ongoing());

            clock.set_unadjusted(visible_end);
            tile.advance_animations();
            // A damped built-in reveal keeps its bounded rebound until the
            // spring settles; an undamped one stops at first arrival.
            assert_eq!(
                tile.are_transitions_ongoing(),
                custom || damping_ratio > 0.,
                "damping={damping_ratio}, custom={custom}"
            );

            // Continued clock ticks do not resurrect the completed built-in reveal. The
            // custom undamped trajectory deliberately remains ongoing.
            clock.set_unadjusted(Duration::from_secs(10));
            tile.advance_animations();
            assert_eq!(
                tile.are_transitions_ongoing(),
                custom && damping_ratio == 0.
            );
        }
    }
}

#[test]
fn overview_gesture_interruptions_keep_live_animation_position_and_velocity() {
    for rate in [0.5, 1., 2.] {
        for easing in [false, true] {
            for mode_change in [false, true] {
                let mut options = Options::default();
                if easing {
                    options.animations.overview_open_close.0 = clarion_config::Animation {
                        off: false,
                        kind: Kind::Easing(EasingParams {
                            duration_ms: 300,
                            curve: Curve::EaseOutExpo,
                        }),
                    };
                }
                let mut layout =
                    check_ops_with_options(options, [Op::AddOutput(1), Op::AddOutput(2)]);
                layout.clock.set_rate(rate);
                layout.clock.set_unadjusted(Duration::from_millis(20));
                layout.overview_gesture_begin();
                for (ms, delta) in [(20, 15.), (30, 15.), (40, 15.)] {
                    layout.clock.set_unadjusted(Duration::from_millis(ms));
                    layout.overview_gesture_update(delta, Duration::from_millis(ms));
                }
                let before = layout.overview_progress_value().unwrap();
                let velocity = layout.core.borrow().overview().gesture_velocity();
                assert_eq!(velocity, 5.);
                if mode_change {
                    layout.toggle_window_spread();
                } else {
                    assert!(layout.overview_gesture_cancel());
                }
                let Some(OverviewProgress::Animation(animation)) = &layout.overview_progress else {
                    panic!("gesture must settle through actual production Animation");
                };
                assert_eq!(animation.value().to_bits(), before.to_bits());
                assert!((animation.velocity() - velocity).abs() < 1e-9);
                layout.clock.set_unadjusted(Duration::from_micros(40001));
                let next = layout.overview_progress_value().unwrap();
                assert!(
                    ((next - before) / 0.000001 - velocity).abs() < 0.003,
                    "rate={rate} easing={easing} mode_change={mode_change}"
                );
                // Both output projections sample the same owner transition; no
                // monitor can select a separate target or discard the velocity.
                for monitor in layout.monitors() {
                    assert_eq!(
                        monitor.overview_progress_value(),
                        layout.overview_progress_value()
                    );
                    assert_eq!(monitor.overview_mode(), layout.overview_mode());
                }
                Op::CompleteAnimations.apply(&mut layout);
                assert_eq!(layout.is_overview_visible(), mode_change);
            }
        }
    }
}

#[test]
fn workspace_cancel_settles_from_core_pose_with_live_tracker_velocity() {
    use clarion_core::gestures::swipe_tracker::SwipeTracker;
    for rate in [0.5, 1., 2.] {
        for easing in [false, true] {
            let mut options = Options::default();
            if easing {
                options.animations.workspace_switch.0 = clarion_config::Animation {
                    off: false,
                    kind: Kind::Easing(EasingParams {
                        duration_ms: 300,
                        curve: Curve::EaseOutExpo,
                    }),
                };
            }
            let mut layout = check_ops_with_options(
                options,
                [
                    Op::AddOutput(1),
                    Op::AddWindow {
                        params: TestWindowParams::new(1),
                    },
                    Op::CompleteAnimations,
                ],
            );
            layout.clock.set_rate(rate);
            layout.clock.set_unadjusted(Duration::from_millis(20));
            layout
                .active_monitor()
                .unwrap()
                .workspace_switch_gesture_begin(true);
            let mut oracle = SwipeTracker::new();
            for (ms, delta) in [(20, 10.), (30, 15.), (40, 15.)] {
                layout.clock.set_unadjusted(Duration::from_millis(ms));
                assert_eq!(
                    layout
                        .active_monitor()
                        .unwrap()
                        .workspace_switch_gesture_update(delta, Duration::from_millis(ms), true),
                    Some(true)
                );
                oracle.push(delta, Duration::from_millis(ms));
            }
            oracle.push(0., Duration::from_millis(40));
            let before = layout.active_monitor_ref().unwrap().workspace_render_idx();
            let velocity = oracle.velocity() / 300.;
            assert_eq!(velocity, 5.);
            assert!(layout
                .active_monitor()
                .unwrap()
                .workspace_switch_gesture_cancel(Some(true)));
            let monitor = layout.active_monitor_ref().unwrap();
            let Some(WorkspaceSwitch::Animation(animation)) = &monitor.workspace_switch else {
                panic!("cancel must use actual scene animation");
            };
            assert_eq!(animation.value().to_bits(), before.to_bits());
            assert!((animation.velocity() - velocity).abs() < 1e-9);
            assert!(layout
                .core
                .borrow()
                .workspace_switch()
                .gesture(monitor.output_name())
                .is_none());
            layout.clock.set_unadjusted(Duration::from_micros(40001));
            let next = layout.active_monitor_ref().unwrap().workspace_render_idx();
            assert!(
                ((next - before) / 0.000001 - velocity).abs() < 0.003,
                "rate={rate}, easing={easing}"
            );
            Op::CompleteAnimations.apply(&mut layout);
            assert_eq!(
                layout.active_monitor_ref().unwrap().workspace_render_idx(),
                0.
            );
            layout.verify_invariants();
        }
    }
}

#[test]
fn runtime_reduced_motion_settles_windows_and_desktops_without_reviving_them() {
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        for is_floating in [false, true] {
            let mut layout = check_ops([
                Op::AddOutput(1),
                Op::SwitchTilingMode(Some(mode)),
                Op::AddWindow {
                    params: TestWindowParams {
                        is_floating,
                        ..TestWindowParams::new(1)
                    },
                },
                Op::CompleteAnimations,
            ]);
            layout.start_open_animation_for_window(&1);
            layout.active_monitor().unwrap().activate_workspace(1);
            assert!(layout.are_transitions_ongoing(None));
            layout.set_reduced_motion(true);
            assert!(layout.reduced_motion());
            assert!(!layout.are_transitions_ongoing(None));
            // Config reload must not erase the runtime accessibility choice.
            layout.update_config(&Config::default());
            layout.clock.set_complete_instantly(false);
            assert!(layout.clock.should_complete_instantly());
            layout.start_open_animation_for_window(&1);
            layout.set_reduced_motion(false);
            assert!(!layout.reduced_motion());
            assert!(!layout.are_transitions_ongoing(None));
            layout.start_open_animation_for_window(&1);
            assert!(layout.are_transitions_ongoing(None));
            layout.set_reduced_motion(true);
            assert!(!layout.are_transitions_ongoing(None));
            layout.verify_invariants();
        }
    }
}

#[test]
fn expanded_border_thickness_and_geometry_reverse_continuously() {
    for scale in [1., 1.25, 2.] {
        let mut clock = Clock::with_time(Duration::ZERO);
        let mut options = Options::default();
        options.layout.border.off = false;
        options.layout.border.width = 8.;
        let resources = crate::scene::resources::SceneResources::new();
        let mut tile = Tile::new(
            TestWindow::new(TestWindowParams {
                bbox: Rectangle::from_size((800, 600).into()),
                ..TestWindowParams::new(1)
            }),
            (1920., 1080.).into(),
            scale,
            clock.clone(),
            Rc::new(options),
            resources.allocator().windows.allocate(),
        );
        tile.window_mut()
            .request_size((1920, 1080).into(), SizingMode::Fullscreen, true, None);
        tile.window().communicate();
        tile.update_window();
        assert_eq!(tile.presentation.visual_border_width(), Some(8.));
        clock.set_unadjusted(Duration::from_millis(60));
        let pose = |tile: &Tile<TestWindow>| {
            (
                tile.animated_window_size(),
                tile.animated_tile_size(),
                tile.window_loc(),
                tile.presentation.visual_border_width(),
            )
        };
        let before = pose(&tile);
        let width = before.3.unwrap();
        assert!(width > 0. && width < 8.);
        tile.update_render_elements(true, Rectangle::from_size((1920., 1080.).into()));
        assert_eq!(
            tile.scene().resources.decorations.borrow().border.width(),
            width
        );
        tile.window_mut()
            .request_size((800, 600).into(), SizingMode::Normal, true, None);
        tile.window().communicate();
        tile.update_window();
        assert_eq!(pose(&tile), before);
        clock.set_unadjusted(Duration::from_secs(2));
        tile.advance_animations();
        assert_eq!(tile.presentation.visual_border_width(), Some(8.));
        assert_eq!(tile.animated_window_size(), (800., 600.).into());
        tile.window_mut()
            .request_size((1920, 1080).into(), SizingMode::Fullscreen, true, None);
        tile.window().communicate();
        tile.update_window();
        clock.set_unadjusted(Duration::from_secs(4));
        tile.advance_animations();
        assert_eq!(tile.presentation.visual_border_width(), None);
    }
}
