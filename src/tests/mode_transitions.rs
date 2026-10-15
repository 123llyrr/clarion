use realand_config::Config;
use smithay::backend::allocator::Fourcc;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::ExtSessionLockManagerV1,
    ext_session_lock_v1::{self, ExtSessionLockV1},
};
use smithay::utils::Scale;
use wayland_client::{Connection, Dispatch, QueueHandle};

use super::Fixture;
use crate::layout::OverviewMode;
use crate::overview::OverviewCommand;
use crate::render_helpers::{render_to_vec, RenderCtx, RenderTarget};
use crate::ui::screen_transition::DURATION;

const OVERVIEW_DURATION: std::time::Duration = std::time::Duration::from_millis(180);

fn setup() -> Fixture {
    let mut config = Config::default();
    config.animations.overview_open_close.0.kind =
        realand_config::animations::Kind::Easing(realand_config::animations::EasingParams {
            duration_ms: OVERVIEW_DURATION.as_millis() as u32,
            curve: realand_config::animations::Curve::Linear,
        });
    setup_with_config(config)
}

fn setup_with_config(config: Config) -> Fixture {
    let native = config.renderer_backend() == Some(realand_config::RendererBackendConfig::Vulkan);
    let mut f = Fixture::with_config(config);
    // Fixture's server deliberately defaults to GLES. Select the actual native
    // backend before adding outputs or clients for the explicit hardware case.
    if native {
        let mut headless = crate::backend::Headless::new(
            f.realand().config.clone(),
            crate::backend::RendererBackendType::Vulkan,
        );
        headless.add_renderer().unwrap();
        headless.init(&mut crate::platform::device_bridge::DeviceCoordinator::new(
            f.realand(),
        ));
        f.realand_state().backend = crate::backend::Backend::Headless(headless);
    } else {
        f.init_gles_renderer();
    }
    f.add_output(1, (640, 400));
    let client = f.add_client();
    let window = f.client(client).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.double_roundtrip(client);
    let window = f.client(client).window(&surface);
    let buffer = window
        .spbm
        .create_u32_rgba_buffer(u32::MAX, 0, 0, u32::MAX, &window.qh, ());
    window.surface.attach(Some(&buffer), 0, 0);
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    f.realand_complete_animations();
    f.realand().layout.toggle_expo();
    f.realand_complete_animations();
    f
}

fn pixels(f: &mut Fixture, target: RenderTarget) -> Vec<u8> {
    let output = f.realand_output(1);
    let state = f.realand_state();
    let realand = &mut state.realand;
    realand.update_render_elements(None);
    macro_rules! draw {
        ($renderer:expr) => {{
            let renderer = $renderer;
            let elements = realand.render_to_vec(
                RenderCtx {
                    renderer,
                    target,
                    xray: None,
                },
                &output,
                false,
            );
            render_to_vec(
                renderer,
                output.current_mode().unwrap().size,
                Scale::from(output.current_scale().fractional_scale()),
                output.current_transform(),
                Fourcc::Abgr8888,
                elements.iter().rev(),
            )
            .unwrap()
        }};
    }
    state
        .backend
        .with_renderer(|renderer| match renderer {
            crate::render_helpers::renderer::AnyRenderer::Gles(renderer) => draw!(renderer),
            crate::render_helpers::renderer::AnyRenderer::Native(renderer) => draw!(renderer),
        })
        .unwrap()
}

#[test]
fn actions_and_hot_corners_share_the_composed_frame_handoff() {
    use crate::compositor::HotCornerKind;
    use realand_config::{Action, OverviewStyle};

    for (style, expected, corner) in [
        (
            OverviewStyle::Stage,
            OverviewMode::Stage,
            HotCornerKind::TopLeft,
        ),
        (
            OverviewStyle::Strip,
            OverviewMode::Strip,
            HotCornerKind::TopLeft,
        ),
        (
            OverviewStyle::Stage,
            OverviewMode::Expo,
            HotCornerKind::TopRight,
        ),
    ] {
        for via_hot_corner in [false, true] {
            let mut config = Config::default();
            config.overview.style = style;
            config.gestures.hot_corners.top_left = true;
            config.gestures.hot_corners.top_right = true;
            let mut f = setup_with_config(config);
            f.realand_state()
                .overview_command(OverviewCommand::ToggleSpread);
            f.realand_complete_animations();
            f.realand().clock.set_rate(0.);
            let targets = [
                RenderTarget::Output,
                RenderTarget::ScreenCapture,
                RenderTarget::Screencast,
            ];
            let before = targets.map(|target| pixels(&mut f, target));
            let workspace = f.realand().layout.active_workspace().unwrap().id();
            if via_hot_corner {
                let x = if corner == HotCornerKind::TopRight {
                    636.
                } else {
                    4.
                };
                super::hot_corners::motion(&mut f, (x, 4.));
                assert_eq!(f.realand().hot_corner_latch().unwrap().1, corner);
            } else {
                let action = if expected == OverviewMode::Expo {
                    Action::ToggleExpo
                } else {
                    Action::ToggleOverview
                };
                f.realand_state().do_action(action, false);
            }
            assert_eq!(f.realand().layout.overview_mode(), expected);
            assert!(f.realand().layout.is_overview_open());
            assert_eq!(
                f.realand().layout.active_workspace().unwrap().id(),
                workspace
            );
            for (target, before) in targets.into_iter().zip(before) {
                assert_eq!(
                    pixels(&mut f, target),
                    before,
                    "{expected:?}, hot_corner={via_hot_corner}, target={target:?}"
                );
            }
        }
    }
}

#[test]
fn overview_command_claims_stationary_gestures_and_cancel_restores_the_entry_state() {
    use std::time::Duration;
    let mut f = setup();
    assert!(!f
        .realand_state()
        .overview_command(OverviewCommand::GestureUpdate {
            delta: 0.,
            timestamp: Duration::ZERO
        }));
    for initially_open in [true, false] {
        if !initially_open {
            f.realand_state().overview_command(OverviewCommand::Close);
            f.realand_complete_animations();
        }
        let workspace = f.realand().layout.active_workspace().unwrap().id();
        assert!(f
            .realand_state()
            .overview_command(OverviewCommand::GestureBegin));
        assert!(f
            .realand_state()
            .overview_command(OverviewCommand::GestureUpdate {
                delta: 0.,
                timestamp: Duration::ZERO
            }));
        assert!(f
            .realand_state()
            .overview_command(OverviewCommand::GestureUpdate {
                delta: if initially_open { -60. } else { 60. },
                timestamp: Duration::from_millis(10)
            }));
        assert!(f
            .realand_state()
            .overview_command(OverviewCommand::GestureEnd { cancelled: true }));
        f.realand_complete_animations();
        assert_eq!(f.realand().layout.is_overview_open(), initially_open);
        assert_eq!(
            f.realand().layout.active_workspace().unwrap().id(),
            workspace
        );
        assert!(!f.realand().layout.are_animations_ongoing(None));
    }
}

#[test]
fn overview_handoff_preserves_composed_pixels_and_repeated_switches() {
    let mut f = setup();
    let before = pixels(&mut f, RenderTarget::Output);
    f.realand_state()
        .overview_command(OverviewCommand::ToggleSpread);
    assert_eq!(pixels(&mut f, RenderTarget::Output), before);

    let now = f.realand().clock.now_unadjusted();
    f.realand()
        .clock
        .set_unadjusted(now + OVERVIEW_DURATION / 2);
    f.realand().advance_animations();
    let halfway = pixels(&mut f, RenderTarget::Output);
    assert_ne!(halfway, before);
    f.realand_state()
        .overview_command(OverviewCommand::ToggleExpo);
    assert_eq!(pixels(&mut f, RenderTarget::Output), halfway);
    let now = f.realand().clock.now_unadjusted();
    f.realand().clock.set_unadjusted(now + OVERVIEW_DURATION);
    f.realand().advance_animations();
    let output = f.realand_output(1);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_none());
}

#[test]
fn identical_transition_targets_share_the_captured_image() {
    use smithay::backend::renderer::element::Element;

    let mut f = setup();
    // Expo's unchanged unprotected scene is identical for all three targets.
    let output = f.realand_output(1);
    f.realand_state()
        .capture_overview_test_frame(OverviewMode::Windows);
    let transition = f.realand().output_state[&output]
        .screen_transition
        .as_ref()
        .unwrap();
    let shown = transition.render(RenderTarget::Output);
    assert_eq!(shown.id(), transition.render(RenderTarget::Screencast).id());
    assert_eq!(
        shown.id(),
        transition.render(RenderTarget::ScreenCapture).id()
    );
}

#[test]
fn overview_handoff_respects_disabled_animations_and_output_changes() {
    let mut f = setup();
    let output = f.realand_output(1);
    f.realand_state()
        .capture_overview_test_frame(OverviewMode::Windows);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_some());
    // Ordinary shell refreshes use this path too, without resizing the output.
    f.realand().output_resized(&output);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_some());
    output.change_current_state(
        None,
        None,
        Some(smithay::output::Scale::Fractional(1.25)),
        None,
    );
    f.realand().output_resized(&output);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_none());
    f.realand().config.borrow_mut().animations.off = true;
    f.realand_state()
        .capture_overview_test_frame(OverviewMode::Windows);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_none());
}

#[test]
fn layer_privacy_changes_clear_handoff_pixels_without_rearming_hot_corners() {
    use realand_config::{BlockOutFrom, LayerRule};
    use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::{
        zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
    };

    use super::client::LayerConfigureProps;
    use crate::compositor::HotCornerKind;

    for remap in [false, true] {
        let mut config = Config::default();
        config.layer_rules.push(LayerRule {
            matches: vec![realand_config::layer_rule::Match {
                layer: Some(realand_ipc::Layer::Bottom),
                ..Default::default()
            }],
            block_out_from: Some(BlockOutFrom::ScreenCapture),
            ..Default::default()
        });
        let mut f = setup_with_config(config);
        let output = f.realand_output(1);
        let client = f.add_client();
        let panel = f
            .client(client)
            .create_layer(None, Layer::Top, "private-panel");
        let surface = panel.surface.clone();
        panel.set_configure_props(LayerConfigureProps {
            size: Some((100, 40)),
            anchor: Some(Anchor::Bottom | Anchor::Left),
            ..Default::default()
        });
        panel.commit();
        f.double_roundtrip(client);
        let panel = f.client(client).layer(&surface);
        let buffer = panel
            .spbm
            .create_u32_rgba_buffer(0, u32::MAX, 0, u32::MAX, &panel.qh, ());
        panel.surface.attach(Some(&buffer), 0, 0);
        panel.set_size(100, 40);
        panel.ack_last_and_commit();
        f.double_roundtrip(client);
        f.realand_complete_animations();
        // Protocol dispatch clears the cached wall clock. Freeze adjusted time
        // so a slow roundtrip cannot finish the handoff before privacy changes.
        f.realand().clock.set_rate(0.);
        let has_green = |image: &[u8]| {
            image
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[0] < 10 && pixel[1] > 240 && pixel[2] < 10)
        };
        assert!(has_green(&pixels(&mut f, RenderTarget::ScreenCapture)));
        let latch = Some((output.clone(), HotCornerKind::TopLeft));
        assert!(f
            .realand()
            .hot_corner_motion_entered((4., 4.).into(), latch.clone()));
        f.realand_state().overview_command(OverviewCommand::Toggle);
        assert!(f.realand().output_state[&output]
            .screen_transition
            .is_some());

        if remap {
            let panel = f.client(client).layer(&surface);
            panel.attach_null();
            panel.commit();
            f.double_roundtrip(client);
            assert!(f.realand().output_state[&output]
                .screen_transition
                .is_some());
        }
        let panel = f.client(client).layer(&surface);
        panel.set_configure_props(LayerConfigureProps {
            layer: Some(Layer::Bottom),
            size: Some((100, 40)),
            anchor: Some(Anchor::Bottom | Anchor::Left),
            ..Default::default()
        });
        panel.commit();
        f.double_roundtrip(client);
        if remap {
            let panel = f.client(client).layer(&surface);
            panel.surface.attach(Some(&buffer), 0, 0);
            panel.set_size(100, 40);
            panel.ack_last_and_commit();
            f.double_roundtrip(client);
        }
        assert!(
            f.realand().output_state[&output].screen_transition.is_none(),
            "a newly protected layer must invalidate previously unblocked handoff textures, remap={remap}"
        );
        assert_eq!(f.realand().hot_corner_latch(), latch);
        assert!(has_green(&pixels(&mut f, RenderTarget::Output)));
        for target in [RenderTarget::ScreenCapture, RenderTarget::Screencast] {
            assert!(
                !has_green(&pixels(&mut f, target)),
                "protected pixels leaked to {target:?}, remap={remap}"
            );
        }
    }
}

#[test]
fn reduced_motion_and_animation_reload_release_snapshots_immediately() {
    for disable_global in [false, true] {
        let mut f = setup();
        let output = f.realand_output(1);
        f.realand_state()
            .capture_overview_test_frame(OverviewMode::Windows);
        assert!(f.realand().output_state[&output]
            .screen_transition
            .is_some());
        let config = f.realand().config.clone();
        if disable_global {
            config.borrow_mut().animations.off = true;
        } else {
            config.borrow_mut().animations.overview_open_close.0.off = true;
        }
        f.realand().layout.update_config(&config.borrow());
        f.realand().advance_animations();
        assert!(f.realand().output_state[&output]
            .screen_transition
            .is_none());
        f.realand_state()
            .overview_command(OverviewCommand::ToggleSpread);
        f.realand().advance_animations();
        assert!(!f.realand().overview_mode_transition_active());
        assert!(!f.realand().layout.are_transitions_ongoing(Some(&output)));
    }
}

#[test]
fn overview_handoff_uses_adjusted_time_without_changing_explicit_transition_timing() {
    let mut f = setup();
    let output = f.realand_output(1);
    f.realand().clock.set_rate(0.5);
    f.realand_state()
        .capture_overview_test_frame(OverviewMode::Windows);
    let start = f.realand().clock.now_unadjusted();
    f.realand().clock.set_unadjusted(start + OVERVIEW_DURATION);
    let transition = f.realand().output_state[&output]
        .screen_transition
        .as_ref()
        .unwrap();
    assert!(!transition.is_done());
    assert!((transition.alpha() - 0.5).abs() < 0.001);
    f.realand()
        .clock
        .set_unadjusted(start + OVERVIEW_DURATION * 2);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .as_ref()
        .unwrap()
        .is_done());
    f.realand().advance_animations();

    let state = f.realand_state();
    state
        .backend
        .with_primary_renderer(|renderer| state.realand.do_screen_transition(renderer, Some(0)));
    let start = f.realand().clock.now_unadjusted();
    f.realand().clock.set_unadjusted(start + DURATION);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .as_ref()
        .unwrap()
        .is_done());
}

#[test]
fn hidden_or_unchanged_modes_do_not_allocate_handoffs() {
    let mut f = Fixture::with_config(Config::default());
    f.add_output(1, (640, 400));
    let output = f.realand_output(1);
    f.realand_state()
        .capture_overview_test_frame(OverviewMode::Windows);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_none());
    f.realand().layout.toggle_expo();
    f.realand_state()
        .capture_overview_test_frame(OverviewMode::Expo);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_none());
}

#[test]
fn switching_modes_releases_old_workspace_and_view_gestures_without_committing_focus() {
    let mut f = setup();
    let output = f.realand_output(1);
    let initial_workspace = f.realand().layout.active_workspace().unwrap().id();
    let initial_window = f.realand().layout.active_window().unwrap().id();
    f.realand()
        .layout
        .workspace_switch_gesture_begin(&output, true);
    f.realand().layout.workspace_switch_gesture_update(
        300.,
        std::time::Duration::from_millis(10),
        true,
    );
    f.realand_state()
        .overview_command(OverviewCommand::ToggleSpread);
    assert!(f
        .realand()
        .layout
        .workspace_switch_gesture_end(Some(true))
        .is_none());
    f.realand_complete_animations();
    assert_eq!(
        f.realand().layout.active_workspace().unwrap().id(),
        initial_workspace
    );

    f.realand()
        .layout
        .view_offset_gesture_begin(&output, None, true);
    f.realand()
        .layout
        .view_offset_gesture_update(300., std::time::Duration::from_millis(20), true);
    f.realand_state()
        .overview_command(OverviewCommand::ToggleExpo);
    assert!(f
        .realand()
        .layout
        .view_offset_gesture_end(Some(true))
        .is_none());
    f.realand_complete_animations();
    assert_eq!(
        f.realand().layout.active_window().unwrap().id(),
        initial_window
    );
}

fn enter_mode(f: &mut Fixture, mode: OverviewMode) {
    if matches!(mode, OverviewMode::Stage | OverviewMode::Strip) {
        let config = f.realand().config.clone();
        config.borrow_mut().overview.style = if mode == OverviewMode::Stage {
            realand_config::OverviewStyle::Stage
        } else {
            realand_config::OverviewStyle::Strip
        };
        f.realand().layout.update_config(&config.borrow());
    }
    let command = match mode {
        OverviewMode::Stage | OverviewMode::Strip => OverviewCommand::Toggle,
        OverviewMode::Expo => OverviewCommand::ToggleExpo,
        OverviewMode::Windows => OverviewCommand::ToggleSpread,
    };
    f.realand_state().overview_command(command);
    assert_eq!(f.realand().layout.overview_mode(), mode);
}

#[test]
fn all_four_mode_handoffs_preserve_pixels_at_each_animation_state_and_scale() {
    let modes = [
        OverviewMode::Stage,
        OverviewMode::Strip,
        OverviewMode::Expo,
        OverviewMode::Windows,
    ];
    let mut scenarios = 0;
    for scale in [1., 1.25, 1.5] {
        for source in modes {
            for target in modes.into_iter().filter(|mode| *mode != source) {
                for phase in ["opening", "open", "closing", "gesture"] {
                    let mut f = setup();
                    let output = f.realand_output(1);
                    output.change_current_state(
                        None,
                        None,
                        Some(smithay::output::Scale::Fractional(scale)),
                        None,
                    );
                    f.realand().output_resized(&output);
                    f.realand().layout.close_overview();
                    f.realand_complete_animations();
                    enter_mode(&mut f, source);
                    if phase != "opening" {
                        f.realand_complete_animations();
                    }
                    if phase == "closing" {
                        f.realand().layout.close_overview();
                        let now = f.realand().clock.now_unadjusted();
                        f.realand()
                            .clock
                            .set_unadjusted(now + OVERVIEW_DURATION / 3);
                        f.realand().advance_animations();
                    } else if phase == "gesture" {
                        f.realand().layout.overview_gesture_begin();
                        f.realand()
                            .layout
                            .overview_gesture_update(-90., std::time::Duration::from_millis(10));
                    }
                    let workspace = f.realand().layout.active_workspace().unwrap().id();
                    let window = f.realand().layout.active_window().unwrap().id();
                    let before = pixels(&mut f, RenderTarget::Output);
                    enter_mode(&mut f, target);
                    assert!(pixels(&mut f, RenderTarget::Output) == before,
                        "discontinuous first frame: {source:?} -> {target:?}, {phase}, scale {scale}");
                    assert!(f
                        .realand()
                        .layout
                        .overview_gesture_update(80., std::time::Duration::from_millis(20))
                        .is_none());
                    assert!(!f.realand().layout.overview_gesture_end());
                    f.realand_complete_animations();
                    assert!(f.realand().layout.is_overview_open());
                    assert!(!f.realand().overview_mode_transition_active());
                    assert_eq!(
                        f.realand().layout.active_workspace().unwrap().id(),
                        workspace
                    );
                    assert_eq!(f.realand().layout.active_window().unwrap().id(), window);
                    scenarios += 1;
                }
            }
        }
    }
    assert_eq!(scenarios, 144);
}

#[test]
fn cancelled_overview_gestures_restore_original_open_state_without_inertia() {
    for initially_open in [false, true] {
        let mut f = setup();
        if !initially_open {
            f.realand().layout.close_overview();
            f.realand_complete_animations();
        }
        let workspace = f.realand().layout.active_workspace().unwrap().id();
        f.realand().layout.overview_gesture_begin();
        let delta = if initially_open { -200. } else { 200. };
        f.realand()
            .layout
            .overview_gesture_update(delta, std::time::Duration::from_millis(10));
        assert!(f.realand().layout.overview_gesture_cancel());
        assert!(!f.realand().layout.overview_gesture_end());
        f.realand_complete_animations();
        assert_eq!(f.realand().layout.is_overview_open(), initially_open);
        assert_eq!(
            f.realand().layout.active_workspace().unwrap().id(),
            workspace
        );
        assert_eq!(f.realand().layout.is_overview_visible(), initially_open);
    }
}

#[test]
fn stage_cross_output_preview_preserves_actual_focus_until_confirmation() {
    let mut f = setup();
    f.realand().layout.close_overview();
    f.realand_complete_animations();
    f.add_output(2, (640, 400));
    let first = f.realand_output(1);
    let second = f.realand_output(2);
    f.realand().layout.focus_output(&second);
    let client = f.add_client();
    let window = f.client(client).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.double_roundtrip(client);
    let window = f.client(client).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    f.realand_complete_animations();
    let entry = f.realand().layout.active_window().unwrap().id();
    let workspace = f.realand().layout.active_workspace().unwrap().id();
    enter_mode(&mut f, OverviewMode::Stage);
    f.realand_complete_animations();
    assert!(f.realand().layout.stage_navigate(Some(&first), 1));
    assert_eq!(f.realand().layout.active_output(), Some(&second));
    assert_eq!(
        f.realand().layout.active_workspace().unwrap().id(),
        workspace
    );
    assert_eq!(f.realand().layout.active_window().unwrap().id(), entry);
    assert!(f.realand().layout.stage_navigate(None, -1));
    assert_eq!(f.realand().layout.active_window().unwrap().id(), entry);
    assert!(f.realand().layout.confirm_stage());
    f.realand_complete_animations();
    assert_eq!(f.realand().layout.active_output(), Some(&first));

    let entry = f.realand().layout.active_window().unwrap().id();
    enter_mode(&mut f, OverviewMode::Stage);
    f.realand_complete_animations();
    f.realand().layout.stage_navigate(Some(&second), 1);
    assert!(f.realand().layout.cancel_stage());
    f.realand_complete_animations();
    assert_eq!(f.realand().layout.active_output(), Some(&first));
    assert_eq!(f.realand().layout.active_window().unwrap().id(), entry);
}

#[test]
fn overview_handoff_uses_configured_duration_and_curve() {
    let mut config = Config::default();
    config.animations.overview_open_close.0.kind =
        realand_config::animations::Kind::Easing(realand_config::animations::EasingParams {
            duration_ms: 400,
            curve: realand_config::animations::Curve::EaseOutQuad,
        });
    let mut f = setup_with_config(config);
    let output = f.realand_output(1);
    f.realand_state()
        .capture_overview_test_frame(OverviewMode::Windows);
    let start = f.realand().clock.now_unadjusted();
    f.realand()
        .clock
        .set_unadjusted(start + std::time::Duration::from_millis(200));
    let transition = f.realand().output_state[&output]
        .screen_transition
        .as_ref()
        .unwrap();
    assert!(!transition.is_done());
    assert!((transition.alpha() - 0.25).abs() < 0.001);
    f.realand()
        .clock
        .set_unadjusted(start + std::time::Duration::from_millis(400));
    f.realand().advance_animations();
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_none());
}

#[test]
fn dual_output_virtual_refresh_finishes_handoffs_and_retires_animation_timers() {
    use crate::compositor::{PointerVisibility, RedrawState};
    use crate::frame_clock::FrameClock;

    let mut f = setup();
    f.add_output(2, (640, 400));
    f.realand_complete_animations();
    let outputs = [f.realand_output(1), f.realand_output(2)];
    for (output, hz) in outputs.iter().zip([60., 144.]) {
        let mut mode = output.current_mode().unwrap();
        mode.refresh = (hz * 1000.) as i32;
        output.change_current_state(Some(mode), None, None, None);
        f.realand()
            .output_state
            .get_mut(output)
            .unwrap()
            .schedule
            .test_set_clock(FrameClock::new(
                Some(std::time::Duration::from_secs_f64(1. / hz)),
                false,
            ));
    }
    f.realand().pointer_visibility = PointerVisibility::Hidden;
    f.realand_state()
        .overview_command(OverviewCommand::ToggleSpread);
    assert!(outputs
        .iter()
        .all(|output| f.realand().output_state[output].screen_transition.is_some()));
    let start = f.realand().clock.now_unadjusted();
    let mut rendered = [0, 0];
    // 720 ticks per second represents both output clocks exactly. Drive their
    // actual headless submission/timer paths at independent virtual vblanks.
    for tick in 0..=720 {
        f.realand()
            .clock
            .set_unadjusted(start + std::time::Duration::from_secs_f64(tick as f64 / 720.));
        f.realand().advance_animations();
        for (index, (output, interval)) in outputs.iter().zip([12, 5]).enumerate() {
            if tick % interval != 0 {
                continue;
            }
            let state = f.realand_state();
            state.realand.queue_redraw(output);
            let unfinished = state.realand.animations_need_redraw(output);
            state
                .realand
                .output_state
                .get_mut(output)
                .unwrap()
                .schedule
                .set_animations_remain(unfinished);
            let mut coordinator =
                crate::frame_pipeline::FrameCoordinator::new(&mut state.realand, output);
            state.backend.headless().render(&mut coordinator, output);
            rendered[index] += 1;
        }
    }
    assert_eq!(rendered, [61, 145]);
    for output in &outputs {
        let state = &f.realand().output_state[output];
        assert!(state.screen_transition.is_none());
        assert!(!state.schedule.animations_remain());
        assert!(matches!(state.schedule.redraw_state(), RedrawState::Idle));
    }
    let sequences = outputs.each_ref().map(|output| {
        f.realand().output_state[output]
            .schedule
            .callback_sequence()
    });
    let server = &mut f.state.server;
    server
        .event_loop
        .dispatch(std::time::Duration::from_millis(30), &mut server.state)
        .unwrap();
    for (output, sequence) in outputs.iter().zip(sequences) {
        assert_eq!(
            f.realand().output_state[output]
                .schedule
                .callback_sequence(),
            sequence
        );
        assert!(matches!(
            f.realand().output_state[output].schedule.redraw_state(),
            RedrawState::Idle
        ));
    }

    // Removal while an animation timer exists must retire it along with caches.
    f.realand_state()
        .overview_command(OverviewCommand::ToggleExpo);
    let removed = outputs[1].clone();
    let state = f.realand_state();
    state.realand.queue_redraw(&removed);
    state
        .realand
        .output_state
        .get_mut(&removed)
        .unwrap()
        .schedule
        .set_animations_remain(true);
    let mut coordinator =
        crate::frame_pipeline::FrameCoordinator::new(&mut state.realand, &removed);
    state.backend.headless().render(&mut coordinator, &removed);
    assert!(matches!(
        f.realand().output_state[&removed].schedule.redraw_state(),
        RedrawState::WaitingForEstimatedVBlank(_)
    ));
    f.realand().remove_output(&removed);
    let server = &mut f.state.server;
    server
        .event_loop
        .dispatch(std::time::Duration::from_millis(30), &mut server.state)
        .unwrap();
    assert!(!f.realand().output_state.contains_key(&removed));
    f.realand_complete_animations();
    assert!(!f.realand().overview_mode_transition_active());
}

#[test]
fn overview_handoff_keeps_each_privacy_target_separate() {
    use realand_config::appearance::BlockOutFrom;
    for privacy in [BlockOutFrom::Screencast, BlockOutFrom::ScreenCapture] {
        let mut f = setup();
        f.realand()
            .config
            .borrow_mut()
            .window_rules
            .push(realand_config::WindowRule {
                block_out_from: Some(privacy),
                ..Default::default()
            });
        f.realand().recompute_window_rules();
        f.realand().layout.toggle_overview();
        f.realand_complete_animations();
        assert_eq!(f.realand().layout.overview_mode(), OverviewMode::Stage);
        let targets = [
            RenderTarget::Output,
            RenderTarget::Screencast,
            RenderTarget::ScreenCapture,
        ];
        let before = targets.map(|target| pixels(&mut f, target));
        assert_ne!(
            before[0], before[1],
            "the protected red client must differ from cast output"
        );
        if privacy == BlockOutFrom::Screencast {
            assert_eq!(before[0], before[2]);
        } else {
            assert_ne!(before[0], before[2]);
        }
        f.realand_state()
            .overview_command(OverviewCommand::ToggleSpread);
        for (target, expected) in targets.into_iter().zip(before) {
            assert_eq!(pixels(&mut f, target), expected, "target {target:?}");
        }
    }
}

fn preview_point(f: &mut Fixture) -> smithay::utils::Point<f64, smithay::utils::Logical> {
    let output = f.realand_output(1);
    for y in (8..400).step_by(4) {
        for x in (8..640).step_by(4) {
            let point = (x as f64, y as f64).into();
            if f.realand().layout.window_under(&output, point).is_some() {
                return point;
            }
        }
    }
    panic!("overview has no visible client preview");
}

#[test]
fn closing_previews_in_every_mode_keep_client_input_and_activation_blocked() {
    for mode in [
        OverviewMode::Stage,
        OverviewMode::Strip,
        OverviewMode::Expo,
        OverviewMode::Windows,
    ] {
        let mut f = setup();
        match mode {
            OverviewMode::Expo => (),
            OverviewMode::Windows => f.realand().layout.toggle_window_spread(),
            OverviewMode::Stage | OverviewMode::Strip => {
                let config = f.realand().config.clone();
                config.borrow_mut().overview.style = if mode == OverviewMode::Stage {
                    realand_config::OverviewStyle::Stage
                } else {
                    realand_config::OverviewStyle::Strip
                };
                f.realand().layout.update_config(&config.borrow());
                f.realand().layout.toggle_overview();
            }
        }
        f.realand_complete_animations();
        let point = preview_point(&mut f);
        let stale = f.realand().contents_under(point);
        assert!(stale.window.is_some(), "mode {mode:?}");
        assert!(f.realand().layout.close_overview());
        assert!(f.realand().window_under(point).is_none(), "mode {mode:?}");
        assert!(f.realand().workspace_under(false, point).is_none());
        let under = f.realand().contents_under(point);
        assert!(under.window.is_none());
        assert!(under.surface.is_none());
        f.realand().handle_focus_follows_mouse(&stale);
        f.realand_complete_animations();
        assert!(!f.realand().layout.is_overview_visible());
    }
}

#[test]
fn mode_handoff_blocks_layout_input_until_the_visible_snapshot_is_released() {
    let mut f = setup();
    let point = preview_point(&mut f);
    assert!(f.realand().window_under(point).is_some());
    f.realand_state()
        .capture_overview_test_frame(OverviewMode::Windows);
    assert!(f.realand().overview_mode_transition_active());
    assert!(f.realand().window_under(point).is_none());
    assert!(f.realand().workspace_under(false, point).is_none());
    let under = f.realand().contents_under(point);
    assert!(under.window.is_none());
    assert!(under.surface.is_none());
    f.realand_complete_animations();
    assert!(!f.realand().overview_mode_transition_active());
    assert!(f.realand().window_under(point).is_some());
}

#[test]
fn privacy_rule_changes_release_previously_unprotected_snapshot_pixels() {
    let mut f = setup();
    let output = f.realand_output(1);
    f.realand_state()
        .capture_overview_test_frame(OverviewMode::Windows);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_some());
    f.realand()
        .config
        .borrow_mut()
        .window_rules
        .push(realand_config::WindowRule {
            block_out_from: Some(realand_config::appearance::BlockOutFrom::ScreenCapture),
            ..Default::default()
        });
    f.realand().recompute_window_rules();
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_none());
    let normal = pixels(&mut f, RenderTarget::Output);
    let capture = pixels(&mut f, RenderTarget::ScreenCapture);
    assert_ne!(normal, capture);
}

#[test]
fn destroying_the_previewed_window_during_handoff_releases_focus_and_caches() {
    let mut f = setup();
    enter_mode(&mut f, OverviewMode::Stage);
    f.realand_complete_animations();
    f.realand_state()
        .overview_command(OverviewCommand::ToggleSpread);
    assert!(f.realand().overview_mode_transition_active());
    let client = f.state.clients[0].id;
    let window = &f.client(client).state.windows[0];
    window.viewport.destroy();
    window.xdg_toplevel.destroy();
    window.xdg_surface.destroy();
    window.surface.destroy();
    f.double_roundtrip(client);
    assert_eq!(f.realand().layout.windows().count(), 0);
    assert!(f.realand().layout.active_window().is_none());
    f.realand_complete_animations();
    assert!(!f.realand().overview_mode_transition_active());
    let output = f.realand_output(1);
    assert!(!f.realand().layout.are_transitions_ongoing(Some(&output)));
    assert!(f.realand().window_under((320., 200.).into()).is_none());
    if let Some((_, workspace)) = f.realand().workspace_under(false, (320., 200.).into()) {
        assert!(!workspace.has_windows());
    }
    for target in [
        RenderTarget::Output,
        RenderTarget::Screencast,
        RenderTarget::ScreenCapture,
    ] {
        assert_eq!(pixels(&mut f, target).len(), 640 * 400 * 4);
    }
}

#[test]
fn failed_snapshot_allocation_releases_the_previous_mode_image() {
    let mut f = setup();
    let output = f.realand_output(1);
    f.realand_state()
        .capture_overview_test_frame(OverviewMode::Windows);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_some());
    // Exceed GLES texture width limits while keeping the attempted byte size small.
    // The old mode image must be dropped even when one target cannot be allocated.
    let original_mode = output.current_mode().unwrap();
    output.change_current_state(
        Some(smithay::output::Mode {
            size: (262_144, 1).into(),
            refresh: original_mode.refresh,
        }),
        None,
        None,
        None,
    );
    f.realand_state()
        .capture_overview_test_frame(OverviewMode::Windows);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_none());
    output.change_current_state(Some(original_mode), None, None, None);
}

impl Dispatch<ExtSessionLockManagerV1, ()> for super::client::State {
    fn event(
        _: &mut Self,
        _: &ExtSessionLockManagerV1,
        _: <ExtSessionLockManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtSessionLockV1, ()> for super::client::State {
    fn event(
        _: &mut Self,
        _: &ExtSessionLockV1,
        _: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

fn request_session_lock(f: &mut Fixture) -> ExtSessionLockV1 {
    let client = f.add_client();
    let c = f.client(client);
    let global = c
        .state
        .globals
        .iter()
        .find(|global| global.interface == "ext_session_lock_manager_v1")
        .unwrap();
    let registry = c.display.get_registry(&c.qh, ());
    let manager: ExtSessionLockManagerV1 = registry.bind(global.name, 1, &c.qh, ());
    let lock = manager.lock(&c.qh, ());
    f.double_roundtrip(client);
    lock
}

#[test]
fn session_lock_and_mode_actions_cancel_all_compositor_grabs_before_any_final_motion() {
    use crate::input::{move_grab::MoveGrab, touch_overview_grab::TouchOverviewGrab, AnyStartData};
    use smithay::backend::input::{
        ButtonState, TabletToolCapabilities, TabletToolDescriptor, TabletToolType,
    };
    use smithay::input::pointer::{ButtonEvent, Focus, GrabStartData, MotionEvent};
    use smithay::input::tablet::{tool, TabletDescriptor, TabletSeatTrait};
    use smithay::input::touch::{DownEvent, UpEvent};
    use smithay::utils::SERIAL_COUNTER;

    for (mode_switch, moving) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut f = setup();
        enter_mode(&mut f, OverviewMode::Stage);
        f.realand_complete_animations();
        let output = f.realand_output(1);
        let position = preview_point(&mut f);
        let workspace = f.realand().layout.active_workspace().unwrap().id();
        let window = f.realand().layout.active_window().unwrap().window.clone();
        let pointer = f.realand().seat.get_pointer().unwrap();
        pointer.button(
            f.realand_state(),
            &ButtonEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: 10,
                button: 0x110,
                state: ButtonState::Pressed,
            },
        );
        let grab = MoveGrab::new(
            f.realand_state(),
            AnyStartData::Pointer(GrabStartData {
                focus: None,
                button: 0x110,
                location: position,
            }),
            window.clone(),
            false,
            None,
        )
        .unwrap();
        pointer.set_grab(
            f.realand_state(),
            grab,
            SERIAL_COUNTER.next_serial(),
            Focus::Clear,
        );
        if moving {
            pointer.motion(
                f.realand_state(),
                None,
                &MotionEvent {
                    location: position
                        + smithay::utils::Point::<f64, smithay::utils::Logical>::from((24., 24.)),
                    serial: SERIAL_COUNTER.next_serial(),
                    time: 20,
                },
            );
            pointer.frame(f.realand_state());
            assert!(f
                .realand()
                .layout
                .interactive_move_is_moving_above_output(&output));
        }

        let touch = f.realand().seat.add_touch();
        touch.down(
            f.realand_state(),
            None,
            &DownEvent {
                slot: Some(1).into(),
                location: position,
                serial: SERIAL_COUNTER.next_serial(),
                time: 10,
            },
        );
        let grab = TouchOverviewGrab::new(
            AnyStartData::Touch(smithay::input::touch::GrabStartData {
                focus: None,
                slot: Some(1).into(),
                location: position,
            }),
            std::time::Duration::from_millis(10),
            output.clone(),
            position,
            Some(workspace),
            false,
            Some(window.clone()),
            OverviewMode::Stage,
        );
        touch.set_grab(f.realand_state(), grab, SERIAL_COUNTER.next_serial());

        let tablet_seat = f.realand().seat.tablet_seat();
        let tablet = tablet_seat.add_tablet(&TabletDescriptor {
            name: "Lock cancellation tablet".into(),
            usb_id: None,
            syspath: None,
        });
        let pen = tablet_seat.add_tool(&TabletToolDescriptor {
            tool_type: TabletToolType::Pen,
            hardware_serial: 17,
            hardware_id_wacom: 0,
            capabilities: TabletToolCapabilities::empty(),
        });
        pen.proximity_in(
            f.realand_state(),
            None,
            tablet,
            &tool::ProximityInEvent {
                location: position,
                axis: None,
                serial: SERIAL_COUNTER.next_serial(),
                time: 10,
            },
        );
        pen.down(
            f.realand_state(),
            &tool::DownEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: 10,
            },
        );
        let grab = TouchOverviewGrab::new(
            AnyStartData::TabletTool(tool::GrabStartData {
                focus: None,
                trigger: tool::GrabTrigger::Tip,
                location: position,
            }),
            std::time::Duration::from_millis(10),
            output.clone(),
            position,
            Some(workspace),
            false,
            Some(window.clone()),
            OverviewMode::Stage,
        );
        pen.set_grab(
            f.realand_state(),
            grab,
            10,
            SERIAL_COUNTER.next_serial(),
            Focus::Clear,
        );
        assert!(pointer.is_grabbed() && touch.is_grabbed() && pen.is_grabbed());

        let _lock = if mode_switch {
            f.realand_state()
                .do_action(realand_config::Action::ToggleExpo, false);
            None
        } else {
            let lock = request_session_lock(&mut f);
            assert!(matches!(
                f.realand().lock_state,
                crate::compositor::LockState::WaitingForSurfaces { .. }
            ));
            assert!(
                !f.realand().is_locked(),
                "surface wait is distinct from a completed lock"
            );
            Some(lock)
        };
        assert!(!pointer.is_grabbed() && !touch.is_grabbed() && !pen.is_grabbed());
        assert!(!f
            .realand()
            .layout
            .interactive_move_is_moving_above_output(&output));
        assert!(f.realand().layout.is_overview_open());
        let expected_mode = if mode_switch {
            OverviewMode::Expo
        } else {
            OverviewMode::Stage
        };
        assert_eq!(f.realand().layout.overview_mode(), expected_mode);
        pointer.button(
            f.realand_state(),
            &ButtonEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: 21,
                button: 0x110,
                state: ButtonState::Released,
            },
        );
        touch.up(
            f.realand_state(),
            &UpEvent {
                slot: Some(1).into(),
                serial: SERIAL_COUNTER.next_serial(),
                time: 21,
            },
        );
        pen.up(
            f.realand_state(),
            &tool::UpEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: 21,
            },
        );
        f.realand_complete_animations();
        assert!(f.realand().layout.is_overview_open());
        assert_eq!(f.realand().layout.overview_mode(), expected_mode);
        assert_eq!(
            f.realand().layout.active_workspace().unwrap().id(),
            workspace
        );
        assert_eq!(f.realand().layout.active_window().unwrap().window, window);
    }
}

#[test]
fn session_lock_request_releases_snapshots_and_blocks_corner_actions() {
    let mut f = setup();
    let output = f.realand_output(1);
    f.realand_state()
        .capture_overview_test_frame(OverviewMode::Windows);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_some());
    let client = f.add_client();
    let c = f.client(client);
    let global = c
        .state
        .globals
        .iter()
        .find(|global| global.interface == "ext_session_lock_manager_v1")
        .unwrap();
    let registry = c.display.get_registry(&c.qh, ());
    let manager: ExtSessionLockManagerV1 = registry.bind(global.name, 1, &c.qh, ());
    let _lock = manager.lock(&c.qh, ());
    f.double_roundtrip(client);
    assert!(!matches!(
        f.realand().lock_state,
        crate::compositor::LockState::Unlocked
    ));
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_none());
    f.realand_state().process_hot_corner_motion(
        (636., 4.).into(),
        Some((output.clone(), crate::compositor::HotCornerKind::TopRight)),
    );
    assert!(f.realand().hot_corner_latch().is_none());
    f.realand_state()
        .capture_overview_test_frame(OverviewMode::Windows);
    assert!(f.realand().output_state[&output]
        .screen_transition
        .is_none());
}

#[test]
#[ignore = "requires a real DRM render node and Vulkan; never acquires display master"]
fn native_stage_handoff_preserves_pixels_and_privacy_targets() {
    let mut config = Config {
        renderer: Some(realand_config::RendererBackendConfig::Vulkan),
        ..Default::default()
    };
    config.window_rules.push(realand_config::WindowRule {
        block_out_from: Some(realand_config::appearance::BlockOutFrom::ScreenCapture),
        ..Default::default()
    });
    let mut f = setup_with_config(config);
    assert_eq!(
        f.realand_state().backend.renderer_backend(),
        crate::render_helpers::renderer::RendererBackendType::Vulkan
    );
    f.realand_state()
        .backend
        .with_renderer(|renderer| match renderer {
            crate::render_helpers::renderer::AnyRenderer::Native(renderer) => eprintln!(
                "HARDWARE: native Stage handoff {}",
                renderer.vulkan().info.device_name
            ),
            crate::render_helpers::renderer::AnyRenderer::Gles(_) => {
                panic!("native Stage handoff must not fall back to GLES")
            }
        })
        .unwrap();
    f.realand().layout.toggle_overview();
    f.realand_complete_animations();
    assert_eq!(f.realand().layout.overview_mode(), OverviewMode::Stage);
    let targets = [
        RenderTarget::Output,
        RenderTarget::Screencast,
        RenderTarget::ScreenCapture,
    ];
    let before = targets.map(|target| pixels(&mut f, target));
    assert_ne!(before[0], before[1]);
    assert_ne!(before[0], before[2]);
    f.realand_state()
        .overview_command(OverviewCommand::ToggleSpread);
    for (target, expected) in targets.into_iter().zip(before) {
        assert_eq!(pixels(&mut f, target), expected, "native target {target:?}");
    }
    f.realand_complete_animations();
    assert!(!f.realand().overview_mode_transition_active());
    eprintln!("HARDWARE: native Stage handoff captured composed pixels separately for Output/Screencast/ScreenCapture");
}
