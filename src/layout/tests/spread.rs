use super::*;

fn moving_spread(scale: f64) -> Layout<TestWindow> {
    let mut options = Options::default();
    options.layout.auto_stack_new_windows = false;
    options.animations.overview_open_close.0.kind =
        clarion_config::animations::Kind::Easing(clarion_config::animations::EasingParams {
            duration_ms: 1000,
            curve: clarion_config::animations::Curve::Linear,
        });
    check_ops_with_options(
        options,
        [
            Op::AddScaledOutput {
                id: 1,
                scale,
                layout_config: None,
            },
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::FocusWorkspaceDown,
            Op::AddWindow {
                params: TestWindowParams::new(3),
            },
            Op::FocusWorkspace(0),
            Op::CompleteAnimations,
        ],
    )
}

fn spread_geometry(layout: &Layout<TestWindow>) -> Vec<(usize, Rectangle<f64, Logical>, f64)> {
    layout
        .active_monitor_ref()
        .unwrap()
        .spread_tiles()
        .into_iter()
        .map(|(tile, rect, zoom)| (*tile.window().id(), rect, zoom))
        .collect()
}

#[test]
fn spread_opens_from_actual_desktop_positions_and_moves_each_frame() {
    for scale in [1., 1.25, 1.5] {
        let mut layout = moving_spread(scale);
        let actual = layout
            .active_monitor_ref()
            .unwrap()
            .workspaces_with_render_geo_cull(false)
            .flat_map(|(ws, geo)| {
                ws.tiles_with_render_positions()
                    .map(move |(tile, pos, visible)| (*tile.window().id(), geo.loc + pos, visible))
            })
            .collect::<Vec<_>>();
        layout.toggle_window_spread();
        let start = spread_geometry(&layout);
        for (id, loc, visible) in actual {
            if visible {
                assert_eq!(
                    start
                        .iter()
                        .find(|(window, _, _)| *window == id)
                        .unwrap()
                        .1
                        .loc,
                    loc
                );
            }
        }
        layout.clock.set_unadjusted(Duration::from_millis(400));
        layout.advance_animations();
        let middle = spread_geometry(&layout);
        assert_ne!(middle, start);
        Op::CompleteAnimations.apply(&mut layout);
        assert_ne!(spread_geometry(&layout), middle);
        layout.verify_invariants();
    }
}

#[test]
fn spread_early_cancel_and_reopen_preserve_current_window_positions() {
    use smithay::input::keyboard::Keysym;
    let mut layout = moving_spread(1.25);
    let focused = *layout.active_window().unwrap().id();
    layout.toggle_window_spread();
    layout.clock.set_unadjusted(Duration::from_millis(400));
    layout.advance_animations();
    let middle = spread_geometry(&layout);
    let Some(OverviewProgress::Animation(opening)) = &layout.overview_progress else {
        unreachable!();
    };
    let incoming_progress = opening.value();
    let incoming_velocity = opening.velocity();
    layout.spread_key(Keysym::Escape);
    assert_eq!(spread_geometry(&layout), middle);
    let Some(OverviewProgress::Animation(closing)) = &layout.overview_progress else {
        unreachable!();
    };
    assert_eq!(closing.value(), incoming_progress);
    assert_eq!(closing.velocity(), incoming_velocity);
    layout.clock.set_unadjusted(Duration::from_millis(450));
    layout.advance_animations();
    let closing = spread_geometry(&layout);
    assert_ne!(closing, middle);
    // The first closing frames retain the opening direction before turning.
    let Some(OverviewProgress::Animation(closing)) = &layout.overview_progress else {
        unreachable!();
    };
    assert!(closing.value() > incoming_progress);
    let closing = spread_geometry(&layout);
    layout.toggle_window_spread();
    assert_eq!(spread_geometry(&layout), closing);
    Op::CompleteAnimations.apply(&mut layout);
    layout.spread_key(Keysym::Escape);
    Op::CompleteAnimations.apply(&mut layout);
    assert_eq!(layout.active_window().unwrap().id(), &focused);
    layout.verify_invariants();
}

#[test]
fn spread_search_confirmation_returns_to_selected_workspace_continuously() {
    use smithay::input::keyboard::Keysym;
    let mut layout = moving_spread(1.5);
    layout.toggle_window_spread();
    Op::CompleteAnimations.apply(&mut layout);
    for ch in "app3".chars() {
        layout.spread_key(Keysym::from_char(ch));
    }
    let selected = spread_geometry(&layout)[0];
    layout.spread_key(Keysym::Return);
    assert!(layout.is_window_spread_closing());
    assert_eq!(layout.active_window().unwrap().id(), &3);
    let first = spread_geometry(&layout);
    assert_eq!(*first.iter().find(|(id, _, _)| *id == 3).unwrap(), selected);
    assert!(first
        .iter()
        .filter(|(id, _, _)| *id != 3)
        .all(|(_, _, zoom)| *zoom == 0.));
    layout.clock.set_unadjusted(Duration::from_millis(500));
    layout.advance_animations();
    let middle = spread_geometry(&layout);
    assert_ne!(
        *middle.iter().find(|(id, _, _)| *id == 3).unwrap(),
        selected
    );
    assert!(layout.are_animations_ongoing(None));
    let mon = layout.active_monitor_ref().unwrap();
    let (_, rect, _) = middle.iter().find(|(id, _, _)| *id == 3).unwrap();
    assert!(matches!(
        mon.window_under(rect.loc + rect.size.to_point().downscale(2.))
            .unwrap()
            .1,
        HitType::Activate { .. }
    ));
    Op::CompleteAnimations.apply(&mut layout);
    assert!(!layout.is_window_spread_visible());
    assert_eq!(layout.active_window().unwrap().id(), &3);
    layout.verify_invariants();
}

#[test]
fn spread_closing_gesture_takeover_tracks_both_directions_without_jumping() {
    let mut layout = moving_spread(1.25);
    layout.toggle_window_spread();
    Op::CompleteAnimations.apply(&mut layout);
    layout.close_overview();
    layout.clock.set_unadjusted(Duration::from_millis(400));
    layout.advance_animations();
    let closing = spread_geometry(&layout);
    layout.overview_gesture_begin();
    assert_eq!(spread_geometry(&layout), closing);
    layout.overview_gesture_update(-60., Duration::from_millis(410));
    let lower = spread_geometry(&layout);
    assert_ne!(lower, closing);
    layout.overview_gesture_update(120., Duration::from_millis(420));
    let upper = spread_geometry(&layout);
    assert_ne!(upper, closing);
    assert_ne!(upper, lower);
    layout.clock.set_unadjusted(Duration::from_millis(430));
    layout.overview_gesture_end();
    assert_eq!(spread_geometry(&layout), upper);
    Op::CompleteAnimations.apply(&mut layout);
    layout.verify_invariants();
}

#[test]
fn filtered_spread_gesture_keeps_returning_windows_visible_and_scheduled() {
    use smithay::input::keyboard::Keysym;
    let mut layout = moving_spread(1.25);
    layout.toggle_window_spread();
    Op::CompleteAnimations.apply(&mut layout);
    for ch in "app3".chars() {
        layout.spread_key(Keysym::from_char(ch));
    }
    layout.close_overview();
    layout.clock.set_unadjusted(Duration::from_millis(400));
    layout.advance_animations();
    let closing = spread_geometry(&layout);
    assert!(closing.iter().any(|(id, _, zoom)| *id == 1 && *zoom > 0.));
    layout.overview_gesture_begin();
    assert_eq!(spread_geometry(&layout), closing);
    layout.overview_gesture_update(-60., Duration::from_millis(410));
    start_preview_animation(&mut layout, 1);
    assert!(layout
        .active_monitor_ref()
        .unwrap()
        .are_animations_ongoing());
    assert!(layout
        .active_monitor_ref()
        .unwrap()
        .are_transitions_ongoing());
    layout.update_render_elements(None);
    let lower = spread_geometry(&layout);
    layout.overview_gesture_update(120., Duration::from_millis(420));
    assert!(spread_geometry(&layout)
        .iter()
        .any(|(id, _, zoom)| *id == 1 && *zoom > 0.));
    layout.overview_gesture_end();
    Op::CompleteAnimations.apply(&mut layout);
    assert!(lower.iter().any(|(id, _, zoom)| *id == 1 && *zoom > 0.));
    assert_eq!(spread_geometry(&layout).len(), 1);
    // Fully open search still stops scheduling the excluded window.
    start_preview_animation(&mut layout, 1);
    assert!(!layout
        .active_monitor_ref()
        .unwrap()
        .are_animations_ongoing());
    layout.verify_invariants();
}

#[test]
fn spread_close_uses_current_frame_after_an_earlier_focus_capture() {
    let mut layout = moving_spread(1.25);
    layout.toggle_window_spread();
    layout.clock.set_unadjusted(Duration::from_millis(200));
    layout.advance_animations();
    Op::FocusWindow(1).apply(&mut layout);
    layout.clock.set_unadjusted(Duration::from_millis(800));
    layout.advance_animations();
    let current = spread_geometry(&layout);
    layout.close_overview();
    assert_eq!(spread_geometry(&layout), current);
    layout.verify_invariants();
}

#[test]
fn spread_uses_animated_rectangles_for_input_and_keeps_disabled_endpoints() {
    for scale in [1., 1.25, 1.5, 2.] {
        let mut layout = moving_spread(scale);
        layout.toggle_window_spread();
        layout.clock.set_unadjusted(Duration::from_millis(400));
        layout.advance_animations();
        let mon = layout.active_monitor_ref().unwrap();
        for (id, rect, _) in spread_geometry(&layout) {
            let center = rect.loc + rect.size.to_point().downscale(2.);
            if Rectangle::from_size(mon.view_size()).contains(center) {
                assert_eq!(mon.window_under(center).unwrap().0.id(), &id);
            }
            let rounded = rect.loc.to_physical_precise_round(scale).to_logical(scale);
            assert_eq!(rect.loc, rounded);
        }
        let mut options = Options::clone(&layout.options);
        options.animations.overview_open_close.0.off = true;
        layout.update_options(options);
        layout.close_overview();
        layout.advance_animations();
        assert!(!layout.is_window_spread_visible());
        layout.toggle_window_spread();
        layout.advance_animations();
        assert!(layout.is_window_spread_visible());
    }
}

#[test]
#[ignore = "CPU-only motion microbenchmark"]
fn spread_motion_geometry_update_cost_for_128_windows() {
    let mut options = Options::default();
    options.layout.auto_stack_new_windows = false;
    let mut ops = vec![Op::AddOutput(1)];
    ops.extend((1..=128).map(|id| Op::AddWindow {
        params: TestWindowParams::new(id),
    }));
    ops.push(Op::CompleteAnimations);
    let mut layout = check_ops_with_options(options, ops);
    layout.toggle_window_spread();
    layout.clock.set_unadjusted(Duration::from_millis(60));
    layout.advance_animations();
    let mut samples = Vec::new();
    for _ in 0..5 {
        let start = std::time::Instant::now();
        for _ in 0..100 {
            std::hint::black_box(spread_geometry(&layout));
            layout.update_render_elements(None);
        }
        samples.push(start.elapsed().as_secs_f64() * 10_000.);
    }
    samples.sort_by(f64::total_cmp);
    eprintln!(
        "PASS: Spread moving geometry+update 128 windows median_us={:.3}",
        samples[2]
    );
}

#[test]
#[ignore = "requires real DRM render node, EGL and Vulkan"]
fn spread_motion_pixels_match_gles_and_preserve_desktop_endpoints() {
    use crate::layout::monitor::MonitorInnerRenderElement;
    use crate::render_helpers::vulkan::{hardware_tests::setup, native::NativeRenderer};
    use crate::render_helpers::{render_to_vec, shaders, RenderTarget};
    use smithay::backend::allocator::Fourcc;
    use smithay::backend::renderer::element::utils::{
        Relocate, RelocateRenderElement, RescaleRenderElement,
    };
    use smithay::input::keyboard::Keysym;
    let (vk, mut gles, _) = setup();
    shaders::init(&mut gles);
    let mut native = NativeRenderer::new(vk).unwrap();
    for scale in [1., 1.25, 1.5, 2.] {
        let mut layout = moving_spread(scale);
        let mut options = Options::clone(&layout.options);
        options.layout.border.off = true;
        options.layout.focus_ring.off = true;
        options.layout.shadow.on = false;
        options.animations.window_wobble.off = true;
        layout.update_options(options);
        for (_, window) in layout.windows() {
            window.0.render_color.set(Some(match *window.id() {
                1 => [0.75, 0.2, 0.2, 1.],
                2 => [0.2, 0.75, 0.2, 1.],
                _ => [0.2, 0.2, 0.75, 1.],
            }));
            window
                .0
                .bbox
                .set(Rectangle::from_size(Size::from((650, 360))));
        }
        for id in 1..=3 {
            layout.update_window(&id, None);
        }
        Op::CompleteAnimations.apply(&mut layout);
        macro_rules! draw {
            ($renderer:expr) => {{
                let renderer = &mut $renderer;
                layout.update_render_elements(None);
                let monitor = layout.active_monitor_ref().unwrap();
                let mut elements = Vec::new();
                monitor.render_workspaces(
                    RenderCtx {
                        renderer,
                        target: RenderTarget::Output,
                        xray: None,
                    },
                    true,
                    &mut |element| elements.push(element),
                );
                if !layout.is_window_spread_visible() {
                    for (ws, geo) in monitor.workspaces_with_render_geo() {
                        elements.push(RelocateRenderElement::from_element(
                            RescaleRenderElement::from_element(
                                MonitorInnerRenderElement::SolidColor(ws.render_background()),
                                Point::default(),
                                1.,
                            ),
                            geo.loc.to_physical_precise_round(scale),
                            Relocate::Relative,
                        ));
                    }
                }
                render_to_vec(
                    renderer,
                    (1280, 720).into(),
                    scale.into(),
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    elements.iter().rev(),
                )
                .unwrap()
            }};
        }
        let desktop = draw!(native);
        layout.toggle_window_spread();
        let mut frames = Vec::new();
        for (stage, millis) in [
            ("opening", 0),
            ("opening-middle", 400),
            ("grid", 1000),
            ("confirm-middle", 1400),
            ("confirm-end", 2000),
        ] {
            layout.clock.set_unadjusted(Duration::from_millis(millis));
            layout.advance_animations();
            if stage == "grid" {
                for ch in "app3".chars() {
                    layout.spread_key(Keysym::from_char(ch));
                }
            }
            let actual = draw!(native);
            let expected = draw!(gles);
            let worst = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(worst <= 2, "scale={scale}, stage={stage}, worst={worst}");
            assert!(
                actual
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[..3].iter().any(|channel| *channel > 100)),
                "scale={scale}, stage={stage}"
            );
            if stage == "opening" {
                assert!(
                    actual == desktop,
                    "Spread first frame must preserve desktop pixels"
                );
            }
            if let Some(directory) = std::env::var_os("CLARION_SPREAD_MORPH_PNG_DIR") {
                let path = std::path::Path::new(&directory)
                    .join(format!("supreme-spread-{scale}-{stage}.png"));
                let file = std::fs::File::create(path).unwrap();
                let mut encoder = png::Encoder::new(file, 1280, 720);
                encoder.set_color(png::ColorType::Rgba);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(&actual)
                    .unwrap();
            }
            frames.push(actual);
            if stage == "grid" {
                layout.spread_key(Keysym::Return);
            }
        }
        for pair in frames.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
        assert!(!layout.is_window_spread_visible());
        assert_eq!(layout.active_window().unwrap().id(), &3);
    }
    eprintln!("PASS: actual Spread opening/grid/selected-workspace closing pixels match GLES/Vulkan at four scales");
}

#[test]
#[ignore = "requires real DRM render node, EGL and Vulkan"]
fn spread_filtered_returning_window_closes_at_preview_geometry_on_both_renderers() {
    use crate::render_helpers::vulkan::{hardware_tests::setup, native::NativeRenderer};
    use crate::render_helpers::{render_to_vec, shaders, RenderTarget};
    use crate::utils::transaction::TransactionBlocker;
    use smithay::backend::allocator::Fourcc;
    use smithay::input::keyboard::Keysym;
    let (vk, mut gles, _) = setup();
    shaders::init(&mut gles);
    crate::render_helpers::resources::init(&mut gles);
    let mut native = NativeRenderer::new(vk).unwrap();
    macro_rules! exercise {
        ($renderer:expr, $store:ident, $close:ident, $native:expr, $name:literal) => {{
            let renderer = &mut $renderer;
            let scale = 1.25;
            let mut layout = moving_spread(scale);
            let mut options = Options::clone(&layout.options);
            options.layout.border.off = true;
            options.layout.focus_ring.off = true;
            options.layout.shadow.on = false;
            options.animations.window_wobble.off = true;
            options.animations.window_close.effect = clarion_config::WindowCloseEffect::Fade;
            options.animations.window_close.glide.off = true;
            layout.update_options(options);
            for (_, window) in layout.windows() {
                window.0.render_color.set(Some(match *window.id() {
                    1 => [0.75, 0.2, 0.2, 1.],
                    2 => [0.2, 0.75, 0.2, 1.],
                    _ => [0.2, 0.2, 0.75, 1.],
                }));
            }
            layout.toggle_window_spread();
            Op::CompleteAnimations.apply(&mut layout);
            for ch in "app3".chars() {
                layout.spread_key(Keysym::from_char(ch));
            }
            layout.close_overview();
            layout.clock.set_unadjusted(Duration::from_millis(400));
            layout.advance_animations();
            layout.overview_gesture_begin();
            layout.overview_gesture_update(-60., Duration::from_millis(410));
            layout.update_render_elements(None);
            let (_, rect, zoom) = spread_geometry(&layout)
                .into_iter()
                .find(|(id, _, _)| *id == 1)
                .unwrap();
            assert!(zoom > 0. && zoom < 1.);
            layout.$store(renderer, None, false, &1);
            layout.$close(renderer, &1, TransactionBlocker::completed());
            let mon = layout.active_monitor_ref().unwrap();
            let closing = mon
                .workspaces
                .iter()
                .flat_map(Workspace::closing_windows)
                .collect::<Vec<_>>();
            assert_eq!(
                closing.len(),
                1,
                "visible excluded preview must receive a real close snapshot"
            );
            assert_eq!(closing[0].is_vulkan(), $native);
            Op::CloseWindow(1).apply(&mut layout);
            let mut frames = Vec::new();
            for (stage, millis) in [("initial", 400), ("middle", 480)] {
                layout.clock.set_unadjusted(Duration::from_millis(millis));
                layout.advance_animations();
                layout.update_render_elements(None);
                let mon = layout.active_monitor_ref().unwrap();
                let mut elements = Vec::new();
                mon.render_workspaces(
                    RenderCtx {
                        renderer,
                        target: RenderTarget::Output,
                        xray: None,
                    },
                    true,
                    &mut |element| elements.push(element),
                );
                let pixels = render_to_vec(
                    renderer,
                    (1280, 720).into(),
                    scale.into(),
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    elements.iter().rev(),
                )
                .unwrap();
                if let Some(directory) = std::env::var_os("CLARION_SPREAD_MORPH_PNG_DIR") {
                    let file = std::fs::File::create(std::path::Path::new(&directory).join(format!(
                        "supreme-spread-{}-closing-filtered-window-{stage}.png", $name
                    ))).unwrap();
                    let mut encoder = png::Encoder::new(file, 1280, 720);
                    encoder.set_color(png::ColorType::Rgba);
                    encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
                }
                let center =
                    (rect.loc + rect.size.to_point().downscale(2.)).to_physical_precise_round::<f64, i32>(scale);
                let offset = (center.y as usize * 1280 + center.x as usize) * 4;
                assert!(
                    pixels[offset] > 100 && pixels[offset + 1] < 100 && pixels[offset + 2] < 100,
                    "{}: red close snapshot stays at the scaled preview center: {:?}; rect={rect:?}, zoom={zoom}",
                    $name,
                    &pixels[offset..offset + 4]
                );
                if !frames.is_empty() {
                    assert_ne!(&frames[..pixels.len()], pixels.as_slice(), "closing animation must change the actual pixels while the overview gesture stays still");
                }
                frames.extend(pixels);
            }
            Op::CompleteAnimations.apply(&mut layout);
            assert!(!layout
                .workspaces()
                .any(|(_, _, ws)| ws.has_closing_windows()));
            layout.verify_invariants();
            frames
        }};
    }
    let expected = exercise!(
        gles,
        store_unmap_snapshot,
        start_close_animation_for_window,
        false,
        "gles"
    );
    let actual = exercise!(
        native,
        store_unmap_snapshot_native,
        start_close_animation_for_window_native,
        true,
        "vulkan"
    );
    let worst = actual
        .iter()
        .zip(&expected)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(
        worst <= 2,
        "scaled Spread close snapshot GLES/Vulkan difference={worst}"
    );
}

fn spread_across_workspaces() -> Layout<TestWindow> {
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=8 {
        ops.push(Op::AddWindow {
            params: TestWindowParams::new(id),
        });
        if id < 8 {
            ops.push(Op::FocusWorkspaceDown);
        }
    }
    ops.extend([Op::AddOutput(2), Op::FocusOutput(1), Op::CompleteAnimations]);
    let mut layout = check_ops(ops);
    layout.toggle_window_spread();
    Op::CompleteAnimations.apply(&mut layout);
    layout.update_render_elements(None);
    assert!(!layout.are_animations_ongoing(None));
    layout
}

fn start_preview_animation(layout: &mut Layout<TestWindow>, id: usize) {
    layout
        .workspaces_mut()
        .flat_map(|ws| ws.tiles_mut())
        .find(|tile| *tile.window().id() == id)
        .unwrap()
        .start_open_animation();
}

#[test]
fn spread_animates_previews_from_distant_workspaces() {
    let mut layout = spread_across_workspaces();
    let output = layout.active_monitor_ref().unwrap().output().clone();
    assert!(!layout
        .active_monitor_ref()
        .unwrap()
        .workspaces_with_render_geo()
        .any(|(ws, _)| ws.has_window(&1)));
    start_preview_animation(&mut layout, 1);
    assert!(layout.are_animations_ongoing(Some(&output)));
    assert!(layout.are_transitions_ongoing(Some(&output)));
    let other = layout.outputs().find(|o| o.name() == "output2").unwrap();
    assert!(!layout.are_animations_ongoing(Some(other)));
    assert!(!layout.are_transitions_ongoing(Some(other)));
    Op::CompleteAnimations.apply(&mut layout);
    assert!(!layout.are_animations_ongoing(None));
    assert!(!layout.are_transitions_ongoing(None));
}

#[test]
fn spread_search_stops_redrawing_for_excluded_previews() {
    let mut layout = spread_across_workspaces();
    start_preview_animation(&mut layout, 8);
    assert!(layout.are_animations_ongoing(None));
    for ch in "app1".chars() {
        layout.spread_key(smithay::input::keyboard::Keysym::from_char(ch));
    }
    assert_eq!(
        layout.active_monitor_ref().unwrap().spread_selected(),
        Some(1)
    );
    assert!(!layout.are_animations_ongoing(None));
    assert!(!layout.are_transitions_ongoing(None));
    for _ in 0..4 {
        layout.spread_key(smithay::input::keyboard::Keysym::BackSpace);
    }
    assert!(layout.are_animations_ongoing(None));
    assert!(layout.are_transitions_ongoing(None));
}

#[test]
fn spread_finds_dwindle_windows_behind_fullscreen() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::SwitchTilingMode(Some(clarion_ipc::TilingMode::Dwindle)),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::SetFullscreenWindow {
            window: 3,
            is_fullscreen: true,
        },
        Op::CompleteAnimations,
    ]);
    let before = serde_json::to_value(layout.session_layout()).unwrap();
    layout.toggle_window_spread();
    Op::CompleteAnimations.apply(&mut layout);
    let mon = layout.active_monitor_ref().unwrap();
    let mut ids: Vec<_> = mon
        .spread_tiles()
        .iter()
        .map(|(tile, _, _)| *tile.window().id())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, [1, 2, 3]);
    for ch in "APP2 窗口".chars() {
        layout.spread_key(smithay::input::keyboard::Keysym::from_char(ch));
    }
    let mon = layout.active_monitor_ref().unwrap();
    assert_eq!(mon.spread_tiles().len(), 1);
    assert_eq!(mon.spread_selected(), Some(2));
    let (_, rect, _) = mon.spread_tiles()[0];
    assert_eq!(
        mon.window_under(rect.loc + rect.size.to_point().downscale(2.))
            .unwrap()
            .0
            .id(),
        &2
    );
    assert_eq!(
        serde_json::to_value(layout.session_layout()).unwrap(),
        before
    );
    layout.spread_key(smithay::input::keyboard::Keysym::Return);
    assert!(!layout.is_overview_open());
    assert_eq!(layout.active_window().unwrap().id(), &2);
    layout.verify_invariants();
}

#[test]
fn spread_clamps_selection_after_a_window_closes_and_accepts_empty_results() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::CompleteAnimations,
    ]);
    layout.toggle_window_spread();
    Op::CompleteAnimations.apply(&mut layout);
    layout.spread_key(smithay::input::keyboard::Keysym::Right);
    let selected = layout
        .active_monitor_ref()
        .unwrap()
        .spread_selected()
        .unwrap();
    Op::CloseWindow(selected).apply(&mut layout);
    layout.update_render_elements(None);
    assert!(layout
        .active_monitor_ref()
        .unwrap()
        .spread_selected()
        .is_some());
    for ch in "missing".chars() {
        layout.spread_key(smithay::input::keyboard::Keysym::from_char(ch));
    }
    layout.update_render_elements(None);
    assert!(layout
        .active_monitor_ref()
        .unwrap()
        .spread_tiles()
        .is_empty());
    assert_eq!(layout.active_monitor_ref().unwrap().spread_selected(), None);
    layout.spread_key(smithay::input::keyboard::Keysym::Return);
    assert!(layout.is_overview_open());
    assert!(!layout.are_animations_ongoing(None));
    assert!(!layout.are_transitions_ongoing(None));
    layout.spread_key(smithay::input::keyboard::Keysym::Escape);
    assert!(!layout.is_overview_open());
    layout.verify_invariants();
}

#[test]
#[ignore = "requires a DRM render node, EGL and Vulkan; never acquires display master"]
fn spread_static_decorations_keep_damage_counters() {
    use smithay::backend::renderer::element::{Element, Id};
    use smithay::backend::renderer::utils::CommitCounter;

    use crate::render_helpers::renderer::DecorationRenderer;

    fn commits(
        layout: &Layout<TestWindow>,
        renderer: &mut impl DecorationRenderer,
    ) -> Vec<(Id, CommitCounter)> {
        let mut result = Vec::new();
        for (_, _, ws) in layout.workspaces() {
            for tile in ws.tiles() {
                for ring in [tile.border(), tile.focus_ring()] {
                    ring.render(renderer, Point::default(), &mut |elem| {
                        result.push((elem.id().clone(), elem.current_commit()));
                    });
                }
            }
        }
        result
    }

    let (_vk, mut renderer, _allocator) = crate::render_helpers::vulkan::hardware_tests::setup();
    crate::render_helpers::shaders::init(&mut renderer);
    let mut options = Options::default();
    options.layout.auto_stack_new_windows = false;
    options.layout.border.off = false;
    options.layout.focus_ring.off = false;
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
            Op::CompleteAnimations,
        ],
    );
    layout.toggle_window_spread();
    Op::CompleteAnimations.apply(&mut layout);
    for _ in 0..2 {
        layout.update_render_elements(None);
    }
    let before = commits(&layout, &mut renderer);
    assert!(!before.is_empty());
    layout.update_render_elements(None);
    assert_eq!(commits(&layout, &mut renderer), before);
    layout.spread_key(smithay::input::keyboard::Keysym::Left);
    layout.update_render_elements(None);
    let selected = commits(&layout, &mut renderer);
    assert_ne!(
        selected, before,
        "selection changes must still damage the highlight"
    );
    layout.update_render_elements(None);
    assert_eq!(commits(&layout, &mut renderer), selected);
}

#[test]
#[ignore = "CPU-only microbenchmark; excludes GPU rendering and presentation"]
fn spread_update_cost_microbenchmark() {
    let mut options = Options::default();
    options.layout.auto_stack_new_windows = false;
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=128 {
        ops.push(Op::AddWindow {
            params: TestWindowParams::new(id),
        });
    }
    ops.push(Op::CompleteAnimations);
    let mut layout = check_ops_with_options(options, ops);
    layout.toggle_window_spread();
    Op::CompleteAnimations.apply(&mut layout);
    for query in ["", "app128", "no-match"] {
        let monitor = layout.active_monitor().unwrap();
        monitor.reset_spread();
        for ch in query.chars() {
            monitor.spread_key(smithay::input::keyboard::Keysym::from_char(ch));
        }
        for _ in 0..32 {
            layout.update_render_elements(None);
        }
        let mut samples = Vec::new();
        for _ in 0..5 {
            let start = std::time::Instant::now();
            for _ in 0..200 {
                std::hint::black_box(&mut layout).update_render_elements(None);
            }
            samples.push(start.elapsed().as_secs_f64() * 1_000_000. / 200.);
        }
        samples.sort_by(f64::total_cmp);
        eprintln!(
            "spread-update windows=128 query={query:?} median_us={:.3}",
            samples[2]
        );
    }
}

#[test]
fn spread_initial_highlight_follows_focus_on_fractional_monitors() {
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
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
                Op::FocusWorkspaceDown,
                Op::AddWindow {
                    params: TestWindowParams::new(2),
                },
                Op::AddScaledOutput {
                    id: 2,
                    scale: 1.5,
                    layout_config: None,
                },
                Op::FocusOutput(2),
                Op::AddWindow {
                    params: TestWindowParams::new(3),
                },
                Op::AddWindow {
                    params: TestWindowParams {
                        is_floating: true,
                        ..TestWindowParams::new(4)
                    },
                },
                Op::FocusOutput(1),
                Op::CompleteAnimations,
            ],
        );
        assert_eq!(layout.active_window().unwrap().id(), &2);
        layout.toggle_window_spread();
        Op::CompleteAnimations.apply(&mut layout);
        layout.update_render_elements(None);
        let MonitorSet::Normal { monitors, .. } = &layout.monitor_set else {
            unreachable!()
        };
        for monitor in monitors {
            let focused = monitor.active_window().unwrap().id();
            assert_eq!(monitor.spread_selected().as_ref(), Some(focused));
            let (_, geometry, _) = monitor
                .spread_tiles()
                .into_iter()
                .find(|(tile, _, _)| tile.window().id() == focused)
                .unwrap();
            assert_eq!(
                monitor
                    .window_under(geometry.loc + geometry.size.to_point().downscale(2.))
                    .unwrap()
                    .0
                    .id(),
                focused,
                "highlight and pointer hit must identify the same preview"
            );
        }
        layout.spread_key(smithay::input::keyboard::Keysym::Return);
        assert_eq!(
            layout.active_window().unwrap().id(),
            &2,
            "Enter keeps the initially focused window"
        );
        Op::CompleteAnimations.apply(&mut layout);
        layout.toggle_window_spread();
        assert_eq!(
            layout.active_monitor_ref().unwrap().spread_selected(),
            Some(2)
        );
        layout.verify_invariants();
    }
}

#[test]
fn spread_arrow_keys_follow_visible_grid_without_crossing_row_edges() {
    use smithay::input::keyboard::Keysym;
    for scale in [1., 1.25, 1.5, 2.] {
        let mut options = Options::default();
        options.layout.auto_stack_new_windows = false;
        let mut ops = vec![Op::AddScaledOutput {
            id: 1,
            scale,
            layout_config: None,
        }];
        for id in 1..=5 {
            ops.push(Op::AddWindow {
                params: TestWindowParams::new(id),
            });
        }
        ops.push(Op::CompleteAnimations);
        let mut layout = check_ops_with_options(options, ops);
        layout.toggle_window_spread();
        Op::CompleteAnimations.apply(&mut layout);
        let monitor = layout.active_monitor().unwrap();
        let previews = monitor.spread_tiles();
        // The visible grid has three previews above two: [1, 2, 3] / [4, 5].
        assert_eq!(previews[0].1.loc.y, previews[2].1.loc.y);
        assert!(previews[3].1.loc.y > previews[2].1.loc.y);
        let cases = [
            (2, Keysym::Right, 3), // Right edge must not jump down-left.
            (3, Keysym::Left, 4),  // Left edge must not jump up-right.
            (1, Keysym::Up, 2),    // Top edge must not slide left.
            (3, Keysym::Down, 4),  // Bottom edge must not slide right.
            (2, Keysym::Down, 3),  // Missing cell below keeps its column and selection.
            (1, Keysym::Down, 5),
            (4, Keysym::Up, 2),
            (2, Keysym::Tab, 4), // Tab remains the sequential traversal key.
        ];
        for (index, key, selected) in cases {
            let preferred = monitor
                .spread_matching_tiles()
                .nth(index)
                .map(|tile| tile.window().core_id());
            monitor
                .core
                .borrow_mut()
                .spread_action(clarion_core::spread::SpreadAction::Reset {
                    connector: monitor.output_name().clone(),
                    preferred,
                });
            monitor.spread_key(key);
            assert_eq!(
                monitor.spread_selected(),
                Some(selected),
                "scale={scale}, key={key:?}, index={index}"
            );
        }
        layout.verify_invariants();
    }
}

#[test]
#[ignore = "requires real DRM render node, EGL and Vulkan"]
fn spread_highlight_pixels_match_gles_on_fractional_outputs() {
    use crate::render_helpers::vulkan::{hardware_tests::setup, native::NativeRenderer};
    use crate::render_helpers::{render_to_vec, shaders, RenderTarget};
    use smithay::backend::allocator::Fourcc;
    use smithay::input::keyboard::Keysym;

    let (vk, mut gles, _) = setup();
    shaders::init(&mut gles);
    let mut native = NativeRenderer::new(vk).unwrap();
    for scale in [1., 1.25, 1.5, 2.] {
        let mut options = Options::default();
        options.layout.auto_stack_new_windows = false;
        options.layout.border.off = true;
        options.layout.focus_ring.off = false;
        options.layout.shadow.on = false;
        options.animations.window_wobble.off = true;
        let mut ops = vec![Op::AddScaledOutput {
            id: 1,
            scale,
            layout_config: None,
        }];
        for id in 1..=3 {
            ops.push(Op::AddWindow {
                params: TestWindowParams::new(id),
            });
        }
        ops.push(Op::CompleteAnimations);
        let mut layout = check_ops_with_options(options, ops);
        for (_, _, workspace) in layout.workspaces() {
            for window in workspace.windows() {
                let color = match *window.id() {
                    1 => [0.65, 0.2, 0.2, 1.],
                    2 => [0.2, 0.65, 0.2, 1.],
                    _ => [0.2, 0.2, 0.65, 1.],
                };
                window.0.render_color.set(Some(color));
            }
        }
        layout.toggle_window_spread();
        Op::CompleteAnimations.apply(&mut layout);
        let mut previous = None;
        for selected in [3, 2] {
            layout.update_render_elements(None);
            let monitor = layout.active_monitor_ref().unwrap();
            assert_eq!(monitor.spread_selected(), Some(selected));
            macro_rules! draw {
                ($renderer:expr) => {{
                    let renderer = &mut $renderer;
                    let mut elements = Vec::new();
                    monitor.render_workspaces(
                        RenderCtx {
                            renderer,
                            target: RenderTarget::Output,
                            xray: None,
                        },
                        true,
                        &mut |element| elements.push(element),
                    );
                    render_to_vec(
                        renderer,
                        (1280, 720).into(),
                        scale.into(),
                        Transform::Normal,
                        Fourcc::Abgr8888,
                        elements.iter(),
                    )
                    .unwrap()
                }};
            }
            let actual = draw!(native);
            let expected = draw!(gles);
            let worst = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(
                worst <= 2,
                "scale={scale}, selected={selected}, worst channel difference={worst}"
            );
            assert!(actual.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 100));
            if let Some(previous) = &previous {
                assert_ne!(
                    &actual, previous,
                    "changing selection must paint a different visible highlight"
                );
            }
            previous = Some(actual);
            layout.spread_key(Keysym::Left);
        }
    }
    eprintln!("PASS: focused Spread highlight and spatial selection pixels match GLES/Vulkan at 1/1.25/1.5/2 scales");
}

#[test]
fn spread_selection_tracks_identity_when_an_earlier_window_is_removed() {
    use smithay::input::keyboard::Keysym;
    let mut layout = moving_spread(1.);
    layout.toggle_window_spread();
    Op::CompleteAnimations.apply(&mut layout);
    layout.spread_key(Keysym::Tab);
    let selected = layout
        .active_monitor_ref()
        .unwrap()
        .spread_selected()
        .unwrap();
    let earlier = layout
        .active_monitor_ref()
        .unwrap()
        .spread_matching_tiles()
        .map(|tile| *tile.window().id())
        .take_while(|id| *id != selected)
        .next()
        .unwrap();
    layout.remove_window(&earlier, Transaction::new());
    assert_eq!(
        layout.active_monitor_ref().unwrap().spread_selected(),
        Some(selected)
    );
    let owner_selected = layout
        .core
        .borrow()
        .spread()
        .output("output1")
        .unwrap()
        .selected()
        .copied();
    assert_eq!(owner_selected, Some(selected as u64));
    layout.verify_invariants();
}
