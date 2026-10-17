//! The production clock/config adapter must retain the pure viewport's C1 handoff.
use clarion_config::animations::{Curve, EasingParams, Kind};

use super::*;

fn fixture(rate: f64, easing: bool) -> Layout<TestWindow> {
    let mut options = Options::default();
    options.layout.auto_stack_new_windows = false;
    if easing {
        options.animations.horizontal_view_movement.0.kind = Kind::Easing(EasingParams {
            duration_ms: 400,
            curve: Curve::Linear,
        });
    }
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
}
#[test]
fn viewport_cancel_inherits_real_velocity_through_actual_config_conversion() {
    for rate in [0.5, 1., 2.] {
        for easing in [false, true] {
            let mut layout = fixture(rate, easing);
            let mut clock = layout.clock.clone();
            let scrolling = layout.active_workspace_mut().unwrap().scrolling_mut();
            scrolling.view_offset_gesture_begin(false);
            scrolling
                .view_offset_gesture_update(10., Duration::ZERO, false)
                .unwrap();
            scrolling
                .view_offset_gesture_update(20., Duration::from_millis(20), false)
                .unwrap();
            clock.set_unadjusted(Duration::from_millis(30));
            let before = scrolling.view_offset().current();
            assert!(scrolling.view_offset_gesture_cancel(None));
            assert_eq!(scrolling.view_offset().current(), before);
            clock.set_unadjusted(Duration::from_micros(30001));
            let velocity = (scrolling.view_offset().current() - before) / 0.000001;
            assert!(
                (velocity - 20. / 0.03).abs() < 0.2,
                "rate={rate} easing={easing} velocity={velocity}"
            );
        }
    }
}
#[test]
fn unscrolled_dnd_release_moves_the_owned_relative_trajectory_without_a_pose_or_speed_jump() {
    for rate in [0.5, 1., 2.] {
        for easing in [false, true] {
            let mut layout = fixture(rate, easing);
            let mut clock = layout.clock.clone();
            let scrolling = layout.active_workspace_mut().unwrap().scrolling_mut();
            scrolling.dnd_scroll_gesture_begin();
            scrolling.focus_column_first();
            clock.set_unadjusted(Duration::from_micros(79999));
            let previous = scrolling.view_pos();
            clock.set_unadjusted(Duration::from_micros(80000));
            let before = scrolling.view_pos();
            let incoming = (before - previous) / 0.000001;
            scrolling.dnd_scroll_gesture_end();
            assert_eq!(scrolling.view_pos(), before);
            clock.set_unadjusted(Duration::from_micros(80001));
            let outgoing = (scrolling.view_pos() - before) / 0.000001;
            assert!(
                (incoming - outgoing).abs() < 0.2,
                "rate={rate} easing={easing} incoming={incoming} outgoing={outgoing}"
            );
        }
    }
}
#[test]
fn future_then_earlier_output_sampling_cannot_change_the_accepted_viewport_target() {
    let mut layout = fixture(1., true);
    let clock = layout.clock.clone();
    let scrolling = layout.active_workspace_mut().unwrap().scrolling_mut();
    scrolling.focus_column_first();
    let from = scrolling.view_offset().current();
    let target = scrolling.view_offset().target();
    {
        let _prediction = clock.project_to(Duration::from_millis(300));
        assert!((scrolling.view_offset().current() - (from + (target - from) * 0.75)).abs() < 1e-8);
    }
    {
        let _prediction = clock.project_to(Duration::from_millis(50));
        assert!(
            (scrolling.view_offset().current() - (from + (target - from) * 0.125)).abs() < 1e-8
        );
    }
    assert_eq!(scrolling.view_offset().current(), from);
    assert_eq!(scrolling.view_offset().target(), target);
}
