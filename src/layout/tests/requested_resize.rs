use super::*;
use crate::layout::window_presentation::ResizeAnimation;
use crate::scene::resources::SceneResources;

fn tile_fixture(rate: f64) -> (SceneResources, Tile<TestWindow>, Clock) {
    let mut clock = Clock::with_time(Duration::ZERO);
    clock.set_rate(rate);
    let mut options = Options::default();
    options.layout.border.off = true;
    options.layout.focus_ring.off = true;
    options.layout.shadow.on = false;
    let resources = SceneResources::new();
    let window = TestWindow::new(TestWindowParams {
        bbox: Rectangle::from_size(Size::from((800, 600))),
        ..TestWindowParams::new(1)
    });
    window.0.requested_size.set(Some((800, 600).into()));
    let tile = Tile::new(
        window,
        (1280., 720.).into(),
        1.,
        clock.clone(),
        Rc::new(options),
        resources.allocator().windows.allocate(),
    );
    (resources, tile, clock)
}

fn resize(tile: &Tile<TestWindow>) -> &ResizeAnimation {
    tile.presentation.resize_animation.as_ref().unwrap()
}

fn apply_buffer(tile: &mut Tile<TestWindow>, size: (i32, i32)) {
    tile.window()
        .0
        .bbox
        .set(Rectangle::from_size(Size::from(size)));
    tile.update_window();
}

#[test]
fn requested_tiling_resize_starts_before_client_buffer_and_late_commit_keeps_time() {
    let (_resources, mut tile, mut clock) = tile_fixture(1.);
    tile.request_tile_size_with_geometry_animation((400., 300.).into(), true, None);
    let started = resize(&tile).anim.start_time();
    assert_eq!(tile.animated_window_size(), (800., 600.).into());
    clock.set_unadjusted(Duration::from_millis(80));
    let before = tile.animated_window_size();
    assert!(before.w > 400. && before.w < 800.);
    assert!(before.h > 300. && before.h < 600.);
    apply_buffer(&mut tile, (400, 300));
    assert_eq!(resize(&tile).anim.start_time(), started);
    assert_eq!(tile.animated_window_size(), before);
    assert!(!resize(&tile).request.as_ref().unwrap().pending_buffer);
}

#[test]
fn completed_requested_resize_holds_late_goal_without_idle_redraw_or_restarting() {
    let (_resources, mut tile, mut clock) = tile_fixture(1.);
    tile.request_tile_size_with_geometry_animation((400., 300.).into(), true, None);
    let started = resize(&tile).anim.start_time();
    clock.set_unadjusted(Duration::from_secs(5));
    tile.advance_animations();
    assert_eq!(tile.animated_window_size(), (400., 300.).into());
    assert!(resize(&tile).request.as_ref().unwrap().pending_buffer);
    assert!(!tile.are_animations_ongoing());
    // Routine layout maintenance is not an explicit cancellation.
    tile.request_tile_size_with_geometry_animation((400., 300.).into(), false, None);
    assert_eq!(resize(&tile).anim.start_time(), started);
    apply_buffer(&mut tile, (400, 300));
    assert_eq!(tile.animated_window_size(), (400., 300.).into());
    tile.advance_animations();
    assert!(tile.presentation.resize_animation.is_none());
    assert_eq!(tile.animated_window_size(), (400., 300.).into());
}

#[test]
fn a_small_retarget_after_completion_never_reveals_the_old_client_size() {
    let (_resources, mut tile, mut clock) = tile_fixture(1.);
    tile.request_tile_size_with_geometry_animation((400., 300.).into(), true, None);
    clock.set_unadjusted(Duration::from_secs(5));
    tile.advance_animations();
    tile.request_tile_size_with_geometry_animation((405., 305.).into(), true, None);
    assert_eq!(tile.animated_window_size(), (400., 300.).into());
    clock.set_unadjusted(Duration::from_secs(10));
    tile.advance_animations();
    assert_eq!(tile.animated_window_size(), (405., 305.).into());
    assert!(!tile.are_animations_ongoing());
}

#[test]
fn instantly_completed_pending_goal_does_not_revive_when_motion_is_reenabled() {
    let (_resources, mut tile, mut clock) = tile_fixture(1.);
    tile.request_tile_size_with_geometry_animation((400., 300.).into(), true, None);
    clock.set_unadjusted(Duration::from_millis(20));
    clock.set_complete_instantly(true);
    tile.advance_animations();
    assert_eq!(tile.animated_window_size(), (400., 300.).into());
    clock.set_complete_instantly(false);
    assert_eq!(tile.animated_window_size(), (400., 300.).into());
    assert!(!tile.are_animations_ongoing());
    assert!(resize(&tile).request.as_ref().unwrap().pending_buffer);
}

#[test]
fn constrained_clients_keep_legal_visible_goals_when_protocol_cells_are_too_small() {
    let (_resources, mut tile, mut clock) = tile_fixture(1.);
    // A genuine fixed-size client cannot fit into the requested half cell.
    Rc::get_mut(&mut tile.window_mut().0).unwrap().min_size = (800, 600).into();
    Rc::get_mut(&mut tile.window_mut().0).unwrap().max_size = (800, 600).into();
    tile.request_tile_size_with_geometry_animation((400., 300.).into(), true, None);
    assert_eq!(tile.window().requested_size(), Some((400, 300).into()));
    assert_eq!(tile.animated_window_size(), (800., 600.).into());
    clock.set_unadjusted(Duration::from_millis(100));
    assert_eq!(tile.animated_window_size(), (800., 600.).into());
    // The buffer honors its legal size, rather than the unfulfillable hint.
    tile.window()
        .0
        .expected_size_override
        .set(Some((800, 600).into()));
    apply_buffer(&mut tile, (800, 600));
    assert_eq!(tile.animated_window_size(), (800., 600.).into());
    assert!(tile.presentation.resize_animation.is_none());
}

#[test]
fn ordinary_vertical_columns_keep_their_commit_driven_resize_policy() {
    let (_resources, mut tile, mut clock) = tile_fixture(1.);
    tile.request_tile_size((400., 300.).into(), true, None);
    clock.set_unadjusted(Duration::from_millis(80));
    assert_eq!(tile.animated_window_size(), (800., 600.).into());
    assert!(tile.presentation.resize_animation.is_none());
}

#[test]
fn requested_resize_retargets_visible_size_and_velocity_in_each_clock_domain() {
    for rate in [0.25, 1., 2.] {
        for hz in [60., 120.] {
            let (_resources, mut tile, mut clock) = tile_fixture(rate);
            tile.request_tile_size_with_geometry_animation((400., 300.).into(), true, None);
            let interrupted = Duration::from_secs_f64(4. / hz);
            clock.set_unadjusted(interrupted);
            let before = resize(&tile).size_state(tile.window_size(), false);
            tile.request_tile_size_with_geometry_animation((900., 500.).into(), true, None);
            let after = resize(&tile).size_state(tile.window_size(), false);
            assert!((before.0.w - after.0.w).abs() < 1e-9);
            assert!((before.0.h - after.0.h).abs() < 1e-9);
            assert!((before.1.x - after.1.x).abs() < 1e-8);
            assert!((before.1.y - after.1.y).abs() < 1e-8);
            clock.set_unadjusted(interrupted + Duration::from_nanos(100));
            let next = resize(&tile).size_state(tile.window_size(), false).0;
            assert!(((next.w - after.0.w) / 0.0000001 - after.1.x).abs() < 0.2);
            assert!(((next.h - after.0.h) / 0.0000001 - after.1.y).abs() < 0.2);
        }
    }
}

#[test]
fn requested_resize_ignores_old_buffers_and_converges_client_selected_latest_size() {
    let (_resources, mut tile, mut clock) = tile_fixture(1.);
    tile.request_tile_size_with_geometry_animation((400., 300.).into(), true, None);
    clock.set_unadjusted(Duration::from_millis(40));
    tile.request_tile_size_with_geometry_animation((900., 500.).into(), true, None);
    let started = resize(&tile).anim.start_time();
    clock.set_unadjusted(Duration::from_millis(60));
    let before_old = tile.animated_window_size();
    apply_buffer(&mut tile, (400, 300));
    assert_eq!(resize(&tile).anim.start_time(), started);
    assert_eq!(tile.animated_window_size(), before_old);
    assert_eq!(
        resize(&tile).request.as_ref().unwrap().window_size,
        (900., 500.).into()
    );

    clock.set_unadjusted(Duration::from_millis(80));
    let before = resize(&tile).size_state(tile.window_size(), false);
    // Like Mapped::expected_size once the latest configure is committed: an
    // honest size hint need not match terminal cell increments or fixed clients.
    tile.window()
        .0
        .expected_size_override
        .set(Some((870, 480).into()));
    apply_buffer(&mut tile, (870, 480));
    let after = resize(&tile).size_state(tile.window_size(), false);
    assert!((before.0.w - after.0.w).abs() < 1e-9);
    assert!((before.1.x - after.1.x).abs() < 1e-8);
    assert!(!resize(&tile).request.as_ref().unwrap().pending_buffer);
    clock.set_unadjusted(Duration::from_secs(5));
    tile.advance_animations();
    assert!(tile.presentation.resize_animation.is_none());
    assert_eq!(tile.animated_window_size(), (870., 480.).into());
}

#[test]
fn requested_resize_cancels_on_explicit_instant_change_and_keeps_legacy_modes() {
    let (_resources, mut tile, mut clock) = tile_fixture(1.);
    tile.request_tile_size_with_geometry_animation((400., 300.).into(), true, None);
    clock.set_unadjusted(Duration::from_millis(50));
    tile.request_tile_size_with_geometry_animation((700., 550.).into(), false, None);
    assert!(tile.presentation.resize_animation.is_none());
    assert_eq!(tile.animated_window_size(), (800., 600.).into());

    tile.request_tile_size_with_geometry_animation((400., 300.).into(), true, None);
    tile.request_fullscreen(true, None);
    assert!(tile.presentation.resize_animation.is_none());
    tile.window().0.pending_sizing_mode.set(SizingMode::Normal);
    let mut options = Options::clone(&tile.presentation.options);
    options.animations.window_resize.custom_shader = Some(String::from("custom"));
    tile.update_config((1280., 720.).into(), 1., Rc::new(options));
    tile.request_tile_size_with_geometry_animation((500., 350.).into(), true, None);
    assert!(tile.presentation.resize_animation.is_none());
}

#[test]
fn scrolling_client_commits_do_not_cancel_an_existing_requested_size_timeline() {
    let clock = Clock::with_time(Duration::ZERO);
    let mut layout = Layout::with_options(clock.clone(), Options::default());
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::Communicate(1),
            Op::CompleteAnimations,
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
        ],
    );
    let mut clock = clock;
    let started = {
        let (tile, _, _) = layout
            .active_workspace()
            .unwrap()
            .tiles_with_render_positions()
            .find(|(tile, _, _)| *tile.window().id() == 1)
            .unwrap();
        resize(tile).anim.start_time()
    };
    clock.set_unadjusted(started + Duration::from_millis(80));
    let before = {
        let (tile, _, _) = layout
            .active_workspace()
            .unwrap()
            .tiles_with_render_positions()
            .find(|(tile, _, _)| *tile.window().id() == 1)
            .unwrap();
        tile.animated_window_size()
    };
    Op::Communicate(1).apply(&mut layout);
    let (tile, _, _) = layout
        .active_workspace()
        .unwrap()
        .tiles_with_render_positions()
        .find(|(tile, _, _)| *tile.window().id() == 1)
        .unwrap();
    assert_eq!(resize(tile).anim.start_time(), started);
    assert_eq!(tile.animated_window_size(), before);
}
