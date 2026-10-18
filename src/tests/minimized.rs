use std::time::Duration;

use clarion_config::{Action, BlockOutFrom, Config, Workspace, WorkspaceName};
use clarion_ipc::{DockEdge, DockTarget, MinimizeEffect, PositionChange, TilingMode};
use smithay::desktop::Window;
use smithay::output::Scale as OutputScale;
use smithay::utils::{Logical, Point};

use super::client::ClientId;
use super::Fixture;
use crate::layout::LayoutElement as _;
use crate::render_helpers::RenderTarget;

struct TestWindow {
    client: ClientId,
    surface: wayland_client::protocol::wl_surface::WlSurface,
    mapped: Window,
    id: u64,
}

fn fixture() -> Fixture {
    let mut config = Config::default();
    config.hotkey_overlay.skip_at_startup = true;
    config.layout.auto_stack_new_windows = false;
    config.animations.window_open.anim.off = true;
    config.animations.window_close.anim.off = true;
    config.layout.border.off = true;
    config.layout.focus_ring.off = true;
    config.layout.shadow.on = false;
    config.workspaces = ["first", "second"]
        .into_iter()
        .map(|name| Workspace {
            name: WorkspaceName(name.into()),
            open_on_output: None,
            layout: None,
        })
        .collect();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 800));
    f
}

fn add(f: &mut Fixture, floating: bool, x: f64, color: [u32; 4]) -> TestWindow {
    let client = f.add_client();
    let w = f.client(client).create_window();
    let surface = w.surface.clone();
    w.commit();
    f.double_roundtrip(client);
    let w = f.client(client).window(&surface);
    let buffer = w
        .spbm
        .create_u32_rgba_buffer(color[0], color[1], color[2], color[3], &w.qh, ());
    w.surface.attach(Some(&buffer), 0, 0);
    w.set_size(320, 240);
    w.ack_last_and_commit();
    f.double_roundtrip(client);
    let w = f.clarion().layout.active_window().unwrap();
    let id = w.id().get();
    let mapped = w.window.clone();
    if floating {
        f.clarion().layout.set_window_floating(Some(&mapped), true);
        f.clarion().layout.move_floating_window(
            Some(&mapped),
            PositionChange::SetFixed(x),
            PositionChange::SetFixed(100.),
            false,
        );
    }
    f.clarion_complete_animations();
    f.double_roundtrip(client);
    TestWindow {
        client,
        surface,
        mapped,
        id,
    }
}

fn target(f: &mut Fixture, w: &TestWindow, reduce_motion: bool) {
    target_on_output(f, w, reduce_motion, 1);
}

fn target_on_output(f: &mut Fixture, w: &TestWindow, reduce_motion: bool, n: u8) {
    target_style_on_output(
        f,
        w,
        reduce_motion,
        n,
        DockEdge::Bottom,
        MinimizeEffect::Genie,
    );
}

fn target_style_on_output(
    f: &mut Fixture,
    w: &TestWindow,
    reduce_motion: bool,
    n: u8,
    edge: DockEdge,
    effect: MinimizeEffect,
) {
    let output = f.clarion_output(n);
    let size = crate::utils::output_size(&output);
    let (x, y) = match edge {
        DockEdge::Bottom => (size.w - 120., size.h - 72.),
        DockEdge::Left => (24., size.h - 120.),
        DockEdge::Right => (size.w - 72., size.h - 120.),
        DockEdge::Top => (size.w - 120., 24.),
    };
    f.clarion_state().do_action(
        Action::SetMinimizeTarget {
            id: w.id,
            target: Some(DockTarget {
                output: output.name(),
                x,
                y,
                width: 48.,
                height: 48.,
                edge,
                effect,
            }),
            reduce_motion,
        },
        false,
    );
}

fn clear_output_redraws(f: &mut Fixture) {
    for output in f.clarion().output_state.values_mut() {
        output
            .schedule
            .test_set_redraw(crate::output_schedule::RedrawState::Idle);
    }
}

fn assert_output_redraw(f: &mut Fixture, n: u8, queued: bool) {
    let output = f.clarion_output(n);
    assert_eq!(
        f.clarion().output_state[&output].schedule.is_queued(),
        queued,
        "unexpected redraw on output {n}"
    );
}

#[test]
fn minimize_target_registration_does_not_draw_advisory_geometry() {
    let mut f = fixture();
    f.add_output(2, (1024, 768));
    let w = add(&mut f, true, 100., [u32::MAX, 0, 0, u32::MAX]);
    clear_output_redraws(&mut f);
    // Dock magnification and layout repeatedly publish targets. They affect the
    // next transition, so both initial and identical registrations stay idle.
    for (motion, edge, effect) in [
        (false, DockEdge::Bottom, MinimizeEffect::Genie),
        (false, DockEdge::Bottom, MinimizeEffect::Genie),
        (false, DockEdge::Left, MinimizeEffect::Scale),
        (true, DockEdge::Right, MinimizeEffect::Genie),
    ] {
        target_style_on_output(&mut f, &w, motion, 1, edge, effect);
        assert_output_redraw(&mut f, 1, false);
        assert_output_redraw(&mut f, 2, false);
    }
    f.clarion_state().do_action(
        Action::SetMinimizeTarget {
            id: w.id,
            target: None,
            reduce_motion: false,
        },
        false,
    );
    assert_output_redraw(&mut f, 1, false);
    assert_output_redraw(&mut f, 2, false);
    // Rejecting a target for another output also changes no visible pixels.
    target_on_output(&mut f, &w, false, 2);
    assert_output_redraw(&mut f, 1, false);
    assert_output_redraw(&mut f, 2, false);
}

#[test]
fn minimize_target_registration_draws_only_a_cancelled_animation_output() {
    for restoring in [false, true] {
        for cancellation in 0..3 {
            let reduced_motion = cancellation == 2;
            for effect in [MinimizeEffect::Genie, MinimizeEffect::Scale] {
                let mut f = fixture();
                f.add_output(2, (1024, 768));
                f.clarion_focus_output(2);
                let w = add(&mut f, true, 100., [u32::MAX, 0, 0, u32::MAX]);
                target_style_on_output(&mut f, &w, false, 2, DockEdge::Bottom, effect);
                f.clarion_state()
                    .do_action(Action::MinimizeWindowById(w.id), false);
                if restoring {
                    f.clarion_complete_animations();
                    f.clarion_state()
                        .do_action(Action::FocusWindow(w.id), false);
                }
                assert!(genie(&mut f, &w));
                clear_output_redraws(&mut f);
                // A newer valid target is advisory even during an animation:
                // the visible mesh retains the destination it began with.
                target_style_on_output(&mut f, &w, false, 2, DockEdge::Left, effect);
                assert!(genie(&mut f, &w));
                assert_output_redraw(&mut f, 1, false);
                assert_output_redraw(&mut f, 2, false);
                if reduced_motion {
                    target_style_on_output(&mut f, &w, true, 2, DockEdge::Left, effect);
                } else if cancellation == 1 {
                    // A foreign-output target is invalidated to None and must
                    // still repaint the old visible animation's final frame.
                    target_style_on_output(&mut f, &w, false, 1, DockEdge::Left, effect);
                } else {
                    f.clarion_state().do_action(
                        Action::SetMinimizeTarget {
                            id: w.id,
                            target: None,
                            reduce_motion: false,
                        },
                        false,
                    );
                }
                assert!(!genie(&mut f, &w));
                assert_eq!(is_minimized(&mut f, &w), !restoring);
                assert_output_redraw(&mut f, 1, false);
                assert_output_redraw(&mut f, 2, true);
                clear_output_redraws(&mut f);
                // Repeating the cancellation must not keep rendering an idle
                // display after the removed mesh has been repainted.
                f.clarion_state().do_action(
                    Action::SetMinimizeTarget {
                        id: w.id,
                        target: None,
                        reduce_motion: reduced_motion,
                    },
                    false,
                );
                assert_output_redraw(&mut f, 1, false);
                assert_output_redraw(&mut f, 2, false);
            }
        }
    }
}

#[test]
fn secondary_output_minimizes_independently_and_invalid_targets_finish_safely() {
    let mut f = fixture();
    f.add_output(2, (1024, 768));
    let first = add(&mut f, true, 100., [u32::MAX, 0, 0, u32::MAX]);
    f.clarion_focus_output(2);
    let second = add(&mut f, true, 100., [0, u32::MAX, 0, u32::MAX]);
    target_on_output(&mut f, &second, false, 2);
    f.clarion_focus_output(1);
    f.clarion_state()
        .do_action(Action::MinimizeWindowById(second.id), false);
    assert!(
        genie(&mut f, &second),
        "visible secondary output must animate"
    );
    assert!(is_minimized(&mut f, &second));
    assert_eq!(keyboard_window(&mut f), Some(first.mapped));
    // Reduced motion changed during a live animation completes its presentation
    // without restoring the logical window or retaining redraw work.
    target_on_output(&mut f, &second, true, 2);
    assert!(!genie(&mut f, &second));
    assert!(is_minimized(&mut f, &second));
    for (name, x, width) in [
        (f.clarion_output(1).name(), 40., 48.),
        (f.clarion_output(2).name(), -1., 48.),
        (f.clarion_output(2).name(), f64::NAN, 48.),
        (f.clarion_output(2).name(), 1000., 48.),
        (f.clarion_output(2).name(), 40., 0.),
    ] {
        f.clarion_state().do_action(
            Action::SetMinimizeTarget {
                id: second.id,
                target: Some(DockTarget {
                    output: name,
                    x,
                    y: 40.,
                    width,
                    height: 48.,
                    edge: DockEdge::Bottom,
                    effect: MinimizeEffect::Genie,
                }),
                reduce_motion: false,
            },
            false,
        );
        f.clarion_state()
            .do_action(Action::FocusWindow(second.id), false);
        assert!(!genie(&mut f, &second));
        f.clarion_state()
            .do_action(Action::MinimizeWindowById(second.id), false);
        assert!(!genie(&mut f, &second));
        assert!(is_minimized(&mut f, &second));
    }
    f.clarion_complete_animations();
    let output = f.clarion_output(2);
    assert!(!f.clarion().layout.are_transitions_ongoing(Some(&output)));
    f.clarion().layout.verify_invariants();
}

#[test]
fn restore_retains_floating_position_and_scrolling_column_order() {
    for floating in [false, true] {
        let mut f = fixture();
        let first = add(&mut f, floating, 100., [u32::MAX, 0, 0, u32::MAX]);
        let _second = add(&mut f, floating, 500., [0, u32::MAX, 0, u32::MAX]);
        let order = |f: &mut Fixture| {
            f.clarion()
                .layout
                .active_workspace()
                .unwrap()
                .tiles()
                .map(|t| t.window().id().get())
                .collect::<Vec<_>>()
        };
        let before = order(&mut f);
        let position = |f: &mut Fixture| {
            f.clarion()
                .layout
                .active_workspace()
                .unwrap()
                .tiles_with_render_positions()
                .find(|(t, _, _)| t.window().id().get() == first.id)
                .map(|(_, p, _)| p)
                .unwrap()
        };
        let original_position = position(&mut f);
        f.clarion_state()
            .do_action(Action::MinimizeWindowById(first.id), false);
        f.clarion_state()
            .do_action(Action::FocusWindow(first.id), false);
        f.clarion_complete_animations();
        f.double_roundtrip(first.client);
        if floating {
            assert_eq!(position(&mut f), original_position);
        } else {
            assert_eq!(order(&mut f), before);
        }
        f.clarion().layout.verify_invariants();
    }
}

#[test]
fn reservation_and_same_extent_scale_changes_keep_logical_target() {
    use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
    let mut f = fixture();
    let w = add(&mut f, true, 100., [u32::MAX, 0, 0, u32::MAX]);
    target(&mut f, &w, false);
    let mut bar = super::stage_render::StageTestBar::new(&mut f);
    bar.configure(&mut f, false, 40, Layer::Top);
    f.clarion_state()
        .do_action(Action::MinimizeWindowById(w.id), false);
    assert!(
        genie(&mut f, &w),
        "new exclusive reservation must keep the logical Dock target"
    );
    bar.configure(&mut f, true, 400, Layer::Top);
    assert!(
        genie(&mut f, &w),
        "working-area-only reflow must not complete the mesh"
    );
    f.clarion_complete_animations();
    f.clarion_state()
        .do_action(Action::FocusWindow(w.id), false);
    assert!(genie(&mut f, &w));
    f.clarion_complete_animations();
    f.clarion_state()
        .do_action(Action::MinimizeWindowById(w.id), false);
    assert!(genie(&mut f, &w));
    let output = f.clarion_output(1);
    output.change_current_state(
        Some(smithay::output::Mode {
            size: (1600, 1000).into(),
            refresh: 60_000,
        }),
        None,
        Some(OutputScale::Fractional(1.25)),
        None,
    );
    f.clarion().output_resized(&output);
    assert_eq!(crate::utils::output_size(&output), (1280., 800.).into());
    assert!(
        !genie(&mut f, &w),
        "scale change must release old-scale mesh resources"
    );
    f.clarion_state()
        .do_action(Action::FocusWindow(w.id), false);
    assert!(
        genie(&mut f, &w),
        "unchanged logical extent must retain the target"
    );
    output.change_current_state(None, Some(smithay::utils::Transform::_180), None, None);
    f.clarion().output_resized(&output);
    assert!(!genie(&mut f, &w));
    f.clarion_state()
        .do_action(Action::MinimizeWindowById(w.id), false);
    assert!(
        genie(&mut f, &w),
        "same-extent transform must retain logical target"
    );
    f.clarion().layout.verify_invariants();
}

#[test]
fn destroying_window_during_live_genie_removes_animation_and_repaint_work() {
    let mut f = fixture();
    let w = add(&mut f, true, 100., [u32::MAX, 0, 0, u32::MAX]);
    target(&mut f, &w, false);
    f.clarion_state()
        .do_action(Action::MinimizeWindowById(w.id), false);
    assert!(genie(&mut f, &w));
    let c = f.client(w.client);
    let idx = c
        .state
        .windows
        .iter()
        .position(|x| x.surface == w.surface)
        .unwrap();
    let window = c.state.windows.remove(idx);
    window.xdg_toplevel.destroy();
    window.xdg_surface.destroy();
    window.surface.destroy();
    f.double_roundtrip(w.client);
    assert_eq!(f.clarion().layout.windows().count(), 0);
    let output = f.clarion_output(1);
    assert!(!f.clarion().layout.are_transitions_ongoing(Some(&output)));
    f.clarion().layout.verify_invariants();
}

fn save_image(
    native: bool,
    scale: f64,
    edge: DockEdge,
    effect: MinimizeEffect,
    phase: &str,
    pixels: &[u8],
) {
    let Some(dir) = std::env::var_os("CLARION_GENIE_PNG_DIR") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let path = std::path::Path::new(&dir).join(format!(
        "{}-{scale}-{edge:?}-{effect:?}-{phase}.png",
        if native { "vulkan" } else { "gles" }
    ));
    let mut encoder = png::Encoder::new(std::fs::File::create(&path).unwrap(), 1280, 800);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(pixels)
        .unwrap();
}

fn is_minimized(f: &mut Fixture, w: &TestWindow) -> bool {
    f.clarion()
        .layout
        .windows()
        .find(|(_, m)| m.id().get() == w.id)
        .unwrap()
        .1
        .is_minimized()
}

fn keyboard_window(f: &mut Fixture) -> Option<Window> {
    match &f.clarion().keyboard_focus {
        crate::compositor::KeyboardFocus::Layout {
            surface: Some(surface),
        } => {
            let surface = surface.clone();
            f.clarion()
                .layout
                .find_window_and_output(&surface)
                .map(|(m, _)| m.window.clone())
        }
        _ => None,
    }
}

fn genie(f: &mut Fixture, w: &TestWindow) -> bool {
    f.clarion()
        .layout
        .workspaces()
        .flat_map(|(_, _, ws)| ws.tiles())
        .find(|t| t.window().id().get() == w.id)
        .unwrap()
        .genie_ongoing()
}

#[test]
fn xdg_minimize_preserves_mapping_commits_workspace_and_switches_real_focus() {
    for mode in [TilingMode::Scrolling, TilingMode::Dwindle] {
        for floating in [false, true] {
            let mut f = fixture();
            f.clarion()
                .layout
                .active_workspace_mut()
                .unwrap()
                .set_tiling_mode(mode);
            let first = add(&mut f, floating, 100., [u32::MAX, 0, 0, u32::MAX]);
            let second = add(&mut f, floating, 500., [0, u32::MAX, 0, u32::MAX]);
            let ws_id = f.clarion().layout.active_workspace().unwrap().id();
            f.client(second.client)
                .window(&second.surface)
                .xdg_toplevel
                .set_minimized();
            f.double_roundtrip(second.client);
            assert!(is_minimized(&mut f, &second));
            assert_eq!(f.clarion().layout.windows().count(), 2);
            assert_eq!(keyboard_window(&mut f), Some(first.mapped.clone()));
            assert_eq!(f.clarion().layout.active_workspace().unwrap().id(), ws_id);
            // The client is still mapped and can commit new content without losing
            // hooks or mutating the workspace's ordinary tiling/focus geometry.
            let w = f.client(second.client).window(&second.surface);
            w.ack_last();
            w.set_size(321, 241);
            w.commit();
            f.double_roundtrip(second.client);
            let (m, _) = f
                .clarion()
                .layout
                .windows()
                .find(|(_, m)| m.id().get() == second.id)
                .map(|(o, m)| (m, o))
                .unwrap();
            assert_eq!(m.size().w, 321);
            assert!(m.is_minimized());
            let output = f.clarion_output(1);
            for point in [
                Point::<f64, Logical>::from((510., 120.)),
                (600., 220.).into(),
            ] {
                assert!(f
                    .clarion()
                    .layout
                    .window_under(&output, point)
                    .is_none_or(|(m, _)| m.id().get() != second.id));
            }
            f.clarion_state()
                .do_action(Action::FocusWindow(second.id), false);
            f.double_roundtrip(second.client);
            assert!(!is_minimized(&mut f, &second));
            assert_eq!(keyboard_window(&mut f), Some(second.mapped.clone()));
            f.clarion().layout.verify_invariants();
        }
    }
}

#[test]
fn minimizing_last_window_keeps_workspace_and_ipc_identity_and_can_close_hidden() {
    let mut f = fixture();
    let w = add(&mut f, true, 100., [u32::MAX, 0, 0, u32::MAX]);
    let ws_id = f.clarion().layout.active_workspace().unwrap().id();
    f.clarion_state()
        .do_action(Action::MinimizeWindowById(w.id), false);
    f.double_roundtrip(w.client);
    assert!(keyboard_window(&mut f).is_none());
    assert!(f.clarion().layout.focus().is_none());
    let mut ids = Vec::new();
    f.clarion().layout.with_windows(|m, _, ws, layout| {
        ids.push((m.id().get(), ws, m.is_minimized()));
        assert!(layout.tile_pos_in_workspace_view.is_none());
    });
    assert_eq!(ids, vec![(w.id, Some(ws_id), true)]);
    f.clarion().layout.switch_workspace(1);
    f.clarion_complete_animations();
    assert!(f.clarion().layout.find_workspace_by_id(ws_id).is_some());
    f.clarion_state()
        .do_action(Action::CloseWindowById(w.id), false);
    f.double_roundtrip(w.client);
    assert!(f.client(w.client).window(&w.surface).close_requested);
    let c = f.client(w.client);
    let idx = c
        .state
        .windows
        .iter()
        .position(|x| x.surface == w.surface)
        .unwrap();
    let w = c.state.windows.remove(idx);
    w.xdg_toplevel.destroy();
    w.xdg_surface.destroy();
    w.surface.destroy();
    let client = c.id;
    f.double_roundtrip(client);
    assert_eq!(f.clarion().layout.windows().count(), 0);
    f.clarion().layout.verify_invariants();
}

#[test]
fn targets_reduced_motion_output_changes_and_explicit_mutations_finish_cleanly() {
    let mut f = fixture();
    let w = add(&mut f, true, 100., [u32::MAX, 0, 0, u32::MAX]);
    target(&mut f, &w, false);
    f.clarion_state()
        .do_action(Action::MinimizeWindowById(w.id), false);
    assert!(genie(&mut f, &w));
    f.clarion_state().do_action(
        Action::SetMinimizeTarget {
            id: w.id,
            target: None,
            reduce_motion: false,
        },
        false,
    );
    assert!(!genie(&mut f, &w));
    assert!(is_minimized(&mut f, &w));
    f.clarion_state()
        .do_action(Action::FocusWindow(w.id), false);
    assert!(!genie(&mut f, &w));
    target(&mut f, &w, true);
    f.clarion_state()
        .do_action(Action::MinimizeWindowById(w.id), false);
    assert!(!genie(&mut f, &w));
    assert!(is_minimized(&mut f, &w));
    // An explicit sizing mutation safely restores the parked ID before the
    // ordinary floating/scrolling handlers dereference it.
    f.clarion_state().do_action(
        Action::SetWindowWidthById {
            id: w.id,
            change: clarion_ipc::SizeChange::SetFixed(400),
        },
        false,
    );
    assert!(!is_minimized(&mut f, &w));
    target(&mut f, &w, false);
    f.clarion_state()
        .do_action(Action::MinimizeWindowById(w.id), false);
    assert!(genie(&mut f, &w));
    f.clarion().layout.complete_minimize_animations();
    assert!(!genie(&mut f, &w));
    assert!(is_minimized(&mut f, &w));
    let output = f.clarion_output(1);
    f.clarion_state()
        .do_action(Action::FocusWindow(w.id), false);
    f.clarion_complete_animations();
    f.clarion_state()
        .do_action(Action::MinimizeWindowById(w.id), false);
    assert!(genie(&mut f, &w));
    f.clarion().remove_output(&output);
    assert!(!genie(&mut f, &w));
    assert_eq!(f.clarion().layout.windows().count(), 1);
    f.add_output(2, (1024, 768));
    f.clarion_state()
        .do_action(Action::FocusWindow(w.id), false);
    f.double_roundtrip(w.client);
    assert!(!is_minimized(&mut f, &w));
    assert_eq!(keyboard_window(&mut f), Some(w.mapped.clone()));
    f.clarion().layout.verify_invariants();
}

#[test]
fn dock_activation_restores_minimized_window_and_exits_every_overview() {
    for style in [
        clarion_config::OverviewStyle::Stage,
        clarion_config::OverviewStyle::Strip,
    ] {
        for action in [
            Action::ToggleOverview,
            Action::ToggleExpo,
            Action::ToggleWindowSpread,
        ] {
            let mut f = fixture();
            f.clarion().config.borrow_mut().overview.style = style;
            let config = f.clarion().config.clone();
            f.clarion().layout.update_config(&config.borrow());
            let _first = add(&mut f, true, 500., [0, u32::MAX, 0, u32::MAX]);
            let w = add(&mut f, true, 100., [u32::MAX, 0, 0, u32::MAX]);
            target(&mut f, &w, false);
            f.clarion_state()
                .do_action(Action::MinimizeWindowById(w.id), false);
            f.clarion_complete_animations();
            f.clarion_state().do_action(action, false);
            f.clarion_complete_animations();
            assert!(f.clarion().layout.is_overview_open());
            f.clarion_state()
                .do_action(Action::FocusWindow(u64::MAX), false);
            assert!(
                f.clarion().layout.is_overview_open(),
                "invalid IDs must retain overview"
            );
            f.clarion_state()
                .do_action(Action::FocusWindow(w.id), false);
            f.clarion_complete_animations();
            f.double_roundtrip(w.client);
            assert!(!f.clarion().layout.is_overview_open());
            assert!(!is_minimized(&mut f, &w));
            assert_eq!(keyboard_window(&mut f), Some(w.mapped.clone()));
            f.clarion().layout.verify_invariants();
        }
    }
}

#[test]
fn real_session_lock_releases_minimize_presentation_and_blocks_restore() {
    use smithay::reexports::wayland_protocols::ext::session_lock::v1::client::
        ext_session_lock_manager_v1::ExtSessionLockManagerV1;
    let mut f = fixture();
    let w = add(&mut f, true, 100., [u32::MAX, 0, 0, u32::MAX]);
    target(&mut f, &w, false);
    f.clarion_state()
        .do_action(Action::MinimizeWindowById(w.id), false);
    assert!(genie(&mut f, &w));
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
        f.clarion().lock_state,
        crate::compositor::LockState::Unlocked
    ));
    assert!(!genie(&mut f, &w));
    assert!(is_minimized(&mut f, &w));
    f.clarion_state()
        .do_action(Action::FocusWindow(w.id), false);
    assert!(is_minimized(&mut f, &w));
    assert_ne!(keyboard_window(&mut f), Some(w.mapped.clone()));
    // Client lifecycle requests must not restart presentation after lock
    // cleared existing effects. Retain real xdg configure handling.
    f.client(w.client).window(&w.surface).set_maximized();
    f.double_roundtrip(w.client);
    assert!(!genie(&mut f, &w));
    f.clarion().unlock();
    f.clarion_state()
        .do_action(Action::FocusWindow(w.id), false);
    f.clarion_complete_animations();
    target(&mut f, &w, false);
    f.clarion_state()
        .do_action(Action::MinimizeWindowById(w.id), false);
    assert!(
        genie(&mut f, &w),
        "unlock must re-enable minimize presentation"
    );
}

fn test_pixels(native: bool) {
    for edge in [
        DockEdge::Bottom,
        DockEdge::Left,
        DockEdge::Right,
        DockEdge::Top,
    ] {
        for effect in [MinimizeEffect::Genie, MinimizeEffect::Scale] {
            for scale in [1., 1.25, 1.5, 2.] {
                let mut f = fixture();
                if native {
                    let mut backend = crate::backend::Headless::new(
                        f.clarion().config.clone(),
                        crate::backend::RendererBackendType::Vulkan,
                    );
                    backend.add_renderer().unwrap();
                    backend.init(&mut crate::platform::device_bridge::DeviceCoordinator::new(
                        f.clarion(),
                    ));
                    f.clarion_state().backend = crate::backend::Backend::Headless(backend);
                } else {
                    f.init_gles_renderer();
                }
                let output = f.clarion_output(1);
                output.change_current_state(None, None, Some(OutputScale::Fractional(scale)), None);
                f.clarion().output_resized(&output);
                let w = add(&mut f, true, 80. / scale, [u32::MAX, 0, 0, u32::MAX]);
                target_style_on_output(&mut f, &w, false, 1, edge, effect);
                let before = super::stage_render::pixels(&mut f, RenderTarget::Output);
                save_image(native, scale, edge, effect, "before", &before);
                let red = |pixels: &[u8]| {
                    pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .filter(|p| p[0] > 180 && p[1] < 60 && p[2] < 60)
                        .count()
                };
                assert!(
                    red(&before) > 20_000,
                    "expected real red source pixels at {scale}"
                );
                let now = f.clarion().clock.now_unadjusted();
                f.clarion().clock.set_unadjusted(now);
                f.clarion_state()
                    .do_action(Action::MinimizeWindowById(w.id), false);
                let start = super::stage_render::pixels(&mut f, RenderTarget::Output);
                save_image(native, scale, edge, effect, "start", &start);
                if before != start {
                    let bbox = |pixels: &[u8]| {
                        pixels
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .enumerate()
                            .filter(|(_, p)| p[0] > 180 && p[1] < 60 && p[2] < 60)
                            .fold((1280, 800, 0, 0), |(l, t, r, b), (i, _)| {
                                (
                                    l.min(i % 1280),
                                    t.min(i / 1280),
                                    r.max(i % 1280),
                                    b.max(i / 1280),
                                )
                            })
                    };
                    eprintln!(
                        "Genie start {native}/{scale}: red {} vs {}, bbox {:?} vs {:?}, diff {}",
                        red(&before),
                        red(&start),
                        bbox(&before),
                        bbox(&start),
                        before.iter().zip(&start).filter(|(a, b)| a != b).count()
                    );
                }
                assert!(
                    before == start,
                    "start frame must preserve the complete source image"
                );
                f.clarion().clock.set_unadjusted(
                    now + Duration::from_millis(match effect {
                        // Geometry-dependent narrowing may already finish
                        // before half-time, notably on a small 2x viewport.
                        // Sample the bending stage for this fixture's targets.
                        MinimizeEffect::Genie => 125,
                        MinimizeEffect::Scale => 175,
                    }),
                );
                f.clarion().advance_animations();
                let middle = super::stage_render::pixels(&mut f, RenderTarget::Output);
                save_image(native, scale, edge, effect, "middle", &middle);
                let red_bounds = |pixels: &[u8]| {
                    pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p[0] > 180 && p[1] < 60 && p[2] < 60)
                        .fold((1280, 800, 0, 0), |(l, t, r, b), (i, _)| {
                            (
                                l.min(i % 1280),
                                t.min(i / 1280),
                                r.max(i % 1280),
                                b.max(i / 1280),
                            )
                        })
                };
                let (l, t, r, b) = red_bounds(&middle);
                let rectangle_area = (r - l + 1) * (b - t + 1);
                if effect == MinimizeEffect::Scale {
                    assert!(
                        red(&middle) as f64 / rectangle_area as f64 > 0.97,
                        "Scale must keep actual client content rectangular at {edge:?}/{scale}"
                    );
                    let (sl, st, sr, sb) = red_bounds(&before);
                    let old_ratio = (sr - sl + 1) as f64 / (sb - st + 1) as f64;
                    let new_ratio = (r - l + 1) as f64 / (b - t + 1) as f64;
                    assert!((old_ratio-new_ratio).abs() < 0.035,
                        "Scale must preserve actual content aspect at {edge:?}/{scale}: {old_ratio} vs {new_ratio}");
                } else {
                    assert!(
                        red(&middle) as f64 / (rectangle_area as f64) < 0.95,
                        "Genie must bend actual client content at {edge:?}/{scale}"
                    );
                }
                assert!(
                    red(&middle) > 500 && red(&middle) < red(&before),
                    "{edge:?}/{effect:?} must deform genuine client content at {scale}"
                );
                f.clarion_state()
                    .do_action(Action::FocusWindow(w.id), false);
                let reversed = super::stage_render::pixels(&mut f, RenderTarget::Output);
                save_image(native, scale, edge, effect, "reversed", &reversed);
                assert!(
                    middle == reversed,
                    "reversal must continue from the current composed frame at {scale}"
                );
                f.clarion_complete_animations();
                f.double_roundtrip(w.client);
                let restored = super::stage_render::pixels(&mut f, RenderTarget::Output);
                save_image(native, scale, edge, effect, "restored", &restored);
                assert_eq!(red(&restored), red(&before));
                assert_eq!(keyboard_window(&mut f), Some(w.mapped.clone()));
                target_style_on_output(&mut f, &w, false, 1, edge, effect);
                // Verify target-specific privacy during mesh flattening as well as in
                // the normal window path: capturing Output first must not leak its image.

                let rule = clarion_config::WindowRule {
                    block_out_from: Some(BlockOutFrom::ScreenCapture),
                    ..Default::default()
                };
                f.clarion()
                    .config
                    .borrow_mut()
                    .window_rules
                    .push(rule.clone());
                f.clarion().layout.with_windows_mut(|m, _| {
                    m.recompute_window_rules(std::slice::from_ref(&rule), false);
                });
                f.clarion().layout.update_window(&w.mapped, None);
                f.clarion_state()
                    .do_action(Action::MinimizeWindowById(w.id), false);
                let on_screen = super::stage_render::pixels(&mut f, RenderTarget::Output);
                assert!(red(&on_screen) > 500);
                let private = super::stage_render::pixels(&mut f, RenderTarget::ScreenCapture);
                save_image(native, scale, edge, effect, "private", &private);
                assert_eq!(
                    red(&private),
                    0,
                    "blocked source leaked through Genie on scale {scale}"
                );
                f.clarion_complete_animations();
                let hidden = super::stage_render::pixels(&mut f, RenderTarget::Output);
                save_image(native, scale, edge, effect, "hidden", &hidden);
                assert_eq!(red(&hidden), 0);
                assert!(!genie(&mut f, &w));
                assert!(!f.clarion().layout.are_transitions_ongoing(Some(&output)));
            }
        }
    }
}

#[test]
fn genie_real_window_pixels_and_reversal_on_gles() {
    test_pixels(false);
}

#[test]
#[ignore = "requires a Vulkan render node; never acquires display master"]
fn genie_real_window_pixels_and_reversal_on_vulkan() {
    test_pixels(true);
}
