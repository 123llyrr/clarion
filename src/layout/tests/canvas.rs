//! Actual layout targets belong to one pure camera; output forecasts only sample.
use super::*;

fn fixture(mode: realand_ipc::TilingMode) -> Layout<TestWindow> {
    let mut options = Options::default();
    options.layout.auto_stack_new_windows = false;
    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddScaledOutput {
                id: 1,
                scale: 1.25,
                layout_config: None,
            },
            Op::SwitchTilingMode(Some(mode)),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::CompleteAnimations,
        ],
    );
    layout.active_workspace_mut().unwrap().toggle_canvas();
    Op::CompleteAnimations.apply(&mut layout);
    layout
}

fn targets(layout: &Layout<TestWindow>) -> Vec<(usize, Point<f64, Logical>)> {
    layout
        .active_workspace()
        .unwrap()
        .floating()
        .tiles_with_offsets()
        .map(|(tile, point)| (*tile.window().id(), point))
        .collect()
}

fn rendered(layout: &Layout<TestWindow>) -> Vec<(usize, Point<f64, Logical>)> {
    layout
        .active_workspace()
        .unwrap()
        .floating()
        .tiles_with_render_positions()
        .map(|(tile, point)| (*tile.window().id(), point))
        .collect()
}

fn verify_core_targets(layout: &Layout<TestWindow>) {
    let core = layout.core.borrow();
    for (tile, position) in layout
        .active_workspace()
        .unwrap()
        .floating()
        .tiles_with_offsets()
    {
        let geometry = core.window(&tile.window().core_id()).unwrap().geometry;
        assert_eq!([geometry.x, geometry.y], [position.x, position.y]);
    }
}

fn kinetic_input(layout: &mut Layout<TestWindow>) {
    layout.clock.set_unadjusted(Duration::ZERO);
    layout
        .active_workspace_mut()
        .unwrap()
        .pan_canvas((10., -20.).into(), true);
    layout.clock.set_unadjusted(Duration::from_millis(16));
    layout
        .active_workspace_mut()
        .unwrap()
        .pan_canvas((10., -20.).into(), true);
}

#[test]
fn canvas_presentation_forecasts_change_render_pose_without_mutating_targets_or_next_owner_tick() {
    for mode in [
        realand_ipc::TilingMode::Scrolling,
        realand_ipc::TilingMode::Dwindle,
    ] {
        let mut layout = fixture(mode);
        kinetic_input(&mut layout);
        layout.clock.set_unadjusted(Duration::from_millis(120));
        layout.advance_animations();
        let accepted = targets(&layout);
        verify_core_targets(&layout);
        let core_targets: Vec<_> = layout
            .core
            .borrow()
            .windows()
            .map(|window| (window.id, window.geometry))
            .collect();
        let canonical_render = rendered(&layout);
        let clock = layout.clock.clone();
        let next_render;
        {
            let _projection = clock.project_to(Duration::from_millis(140));
            next_render = rendered(&layout);
            assert_ne!(next_render, canonical_render);
            assert_eq!(targets(&layout), accepted);
            // Decorations/clipping also sample the same camera forecast.
            layout.update_render_elements(None);
            assert_eq!(targets(&layout), accepted);
            assert_eq!(
                layout
                    .core
                    .borrow()
                    .windows()
                    .map(|window| (window.id, window.geometry))
                    .collect::<Vec<_>>(),
                core_targets
            );
        }
        {
            let _projection = clock.project_to(Duration::from_millis(125));
            assert_ne!(rendered(&layout), next_render);
            assert_eq!(targets(&layout), accepted);
        }
        assert_eq!(rendered(&layout), canonical_render);
        assert_eq!(targets(&layout), accepted);
        layout.clock.set_unadjusted(Duration::from_millis(140));
        layout.advance_animations();
        assert_eq!(rendered(&layout), next_render);
        verify_core_targets(&layout);
        layout.verify_invariants();
    }
}

#[test]
fn new_canvas_gesture_owns_its_first_displacement_without_old_coasting_afterward() {
    let mut layout = fixture(realand_ipc::TilingMode::Scrolling);
    kinetic_input(&mut layout);
    layout.clock.set_unadjusted(Duration::from_millis(250));
    layout.advance_animations();
    assert!(layout
        .active_workspace()
        .unwrap()
        .floating()
        .are_animations_ongoing());
    let before = targets(&layout);
    layout.clock.set_unadjusted(Duration::from_millis(260));
    layout
        .active_workspace_mut()
        .unwrap()
        .pan_canvas((-1., 2.).into(), true);
    let accepted = targets(&layout);
    verify_core_targets(&layout);
    for ((id, old), (next_id, new)) in before.iter().zip(&accepted) {
        assert_eq!(id, next_id);
        // The input moves the camera. Scene targets move in the opposite
        // direction, just as direct pan, pointer grabs and reveal intents do.
        assert_eq!(*new, *old - Point::from((-1., 2.)));
    }
    assert!(!layout
        .active_workspace()
        .unwrap()
        .floating()
        .are_animations_ongoing());
    let canonical_render = rendered(&layout);
    let clock = layout.clock.clone();
    {
        let _projection = clock.project_to(Duration::from_secs(10));
        assert_eq!(rendered(&layout), canonical_render);
    }
    assert_eq!(targets(&layout), accepted);
    layout.clock.set_unadjusted(Duration::from_secs(10));
    layout.advance_animations();
    assert_eq!(targets(&layout), accepted);
    layout.verify_invariants();
}

#[test]
fn canvas_reset_and_toggle_remove_pending_forecast_without_reusing_an_old_camera() {
    for mode in [
        realand_ipc::TilingMode::Scrolling,
        realand_ipc::TilingMode::Dwindle,
    ] {
        let mut layout = fixture(mode);
        let initial = targets(&layout);
        kinetic_input(&mut layout);
        layout.clock.set_unadjusted(Duration::from_millis(120));
        layout.advance_animations();
        assert_ne!(targets(&layout), initial);
        layout.active_workspace_mut().unwrap().reset_canvas();
        assert_eq!(targets(&layout), initial);
        verify_core_targets(&layout);
        assert!(!layout
            .active_workspace()
            .unwrap()
            .floating()
            .are_animations_ongoing());
        assert_eq!(layout.core.borrow().windows().count(), 2);
        layout.active_workspace_mut().unwrap().toggle_canvas();
        assert!(!layout.active_workspace().unwrap().canvas_enabled());
        assert!(targets(&layout).is_empty());
        layout.active_workspace_mut().unwrap().toggle_canvas();
        Op::CompleteAnimations.apply(&mut layout);
        let restarted = targets(&layout);
        let restarted_render = rendered(&layout);
        let clock = layout.clock.clone();
        {
            let _projection = clock.project_to(Duration::from_secs(10));
            assert_eq!(rendered(&layout), restarted_render);
        }
        assert_eq!(targets(&layout), restarted);
        assert!(!layout
            .active_workspace()
            .unwrap()
            .floating()
            .are_animations_ongoing());
        assert_eq!(layout.core.borrow().windows().count(), 2);
        layout.verify_invariants();
    }
}
