use super::*;
use crate::scene::monitor_resources::{CubeFaceRenderElement, CubeFaceState};
use crate::scene::workspace_composition::Item;

fn cube_layout(scale: f64, mode: clarion_ipc::TilingMode) -> Layout<TestWindow> {
    cube_layout_with_privacy(scale, mode, false)
}

fn cube_layout_with_privacy(
    scale: f64,
    mode: clarion_ipc::TilingMode,
    private: bool,
) -> Layout<TestWindow> {
    let mut options = Options::default();
    options.animations.workspace_switch.0 = clarion_config::Animation {
        off: false,
        kind: clarion_config::animations::Kind::Easing(clarion_config::animations::EasingParams {
            duration_ms: 1000,
            curve: clarion_config::animations::Curve::Linear,
        }),
    };
    options.animations.workspace_switch.1.off = false;
    let mut ops = vec![Op::AddScaledOutput {
        id: 1,
        scale,
        layout_config: None,
    }];
    for workspace in (1..=3).rev() {
        ops.push(Op::AddNamedWorkspace {
            ws_name: workspace,
            output_name: Some(1),
            layout_config: None,
        });
        for offset in 0..2 {
            ops.push(Op::AddWindowToNamedWorkspace {
                params: TestWindowParams {
                    is_floating: offset == 1,
                    rules: private.then(|| ResolvedWindowRules {
                        block_out_from: Some(clarion_config::BlockOutFrom::ScreenCapture),
                        ..Default::default()
                    }),
                    ..TestWindowParams::new(workspace * 10 + offset)
                },
                ws_name: workspace,
            });
        }
    }
    ops.extend([Op::FocusWorkspace(0), Op::CompleteAnimations]);
    let mut layout = check_ops_with_options(options, ops);
    for workspace in layout.workspaces_mut() {
        workspace.set_tiling_mode(mode);
    }
    Op::CompleteAnimations.apply(&mut layout);
    layout
}

fn fingerprint() -> CubeFaceState {
    CubeFaceState {
        workspace_idx: 0,
        source_commit: Default::default(),
        source_texture: 1,
        angle_bits: 0,
        perspective_bits: 0,
        width_bits: 0,
        height_bits: 0,
        scale_bits: 0,
        focus_ring: true,
    }
}

#[test]
fn cube_projection_forces_two_complete_face_sources_without_changing_2d_culling() {
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        for scale in [1., 1.25, 1.5, 2.] {
            let mut layout = cube_layout(scale, mode);
            layout.switch_workspace(1);
            let now = layout.clock.now_unadjusted();
            layout.clock.set_unadjusted(now + Duration::from_millis(1));
            layout.advance_animations();
            layout.update_render_elements(None);
            let monitor = layout.active_monitor_ref().unwrap();
            assert!(monitor.is_cube_scene());
            let captured = monitor.cube_composition();
            let ordinary = monitor.standard_composition();
            assert_eq!(captured.faces.iter().flatten().count(), 2);
            let mut forced = 0;
            for face in captured.faces.iter().flatten() {
                let placement = &captured.fallback.geometry.placements[face.workspace_index];
                let sources = &captured.fallback.workspaces[face.workspace_index];
                let workspace = &monitor.workspaces[face.workspace_index];
                assert_eq!(face.workspace_id, workspace.id());
                assert_eq!(placement.id, face.workspace_id);
                assert_eq!(
                    placement.normal_visible,
                    ordinary.geometry.placements[face.workspace_index].normal_visible
                );
                let actual = sources
                    .items
                    .iter()
                    .filter_map(|item| match &item.item {
                        Item::Tile { source, .. } => Some(*source.window().id()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let expected = workspace
                    .tiles_with_render_positions()
                    .filter(|(_, _, visible)| *visible)
                    .map(|(tile, _, _)| *tile.window().id())
                    .collect::<Vec<_>>();
                assert_eq!(actual, expected);
                assert!(!actual.is_empty());
                if !placement.normal_visible {
                    forced += 1;
                    assert!(ordinary.workspaces[face.workspace_index].items.is_empty());
                }
            }
            assert_eq!(
                forced, 1,
                "the entering cube face is still below the 2D output"
            );
            let layers = captured.workspace_scenes().collect::<Vec<_>>();
            assert_eq!(
                layers.len(),
                ordinary
                    .geometry
                    .placements
                    .iter()
                    .filter(|p| p.normal_visible)
                    .count()
            );
            for layer in layers {
                let placement = ordinary
                    .geometry
                    .placements
                    .iter()
                    .find(|p| p.id.get() as usize == layer.namespace)
                    .unwrap();
                assert_eq!(layer.geometry, placement.geometry);
                assert_eq!(layer.zoom, placement.zoom);
            }
            layout.verify_invariants();
        }
    }
}

#[test]
fn cube_exit_clears_feedback_and_targets_once_while_ordinary_frames_leave_caches_untouched() {
    let layout = cube_layout(1., clarion_ipc::TilingMode::Scrolling);
    let monitor = layout.active_monitor_ref().unwrap();
    let workspace = &monitor.workspaces[0];
    let id = workspace.id();
    let lease = monitor.cube_lifecycle().resources;
    *lease.cube_visible_workspaces.borrow_mut() = [Some(id), None];
    lease.cube_face_states.borrow_mut()[0] = Some(fingerprint());
    lease.cube_elements[0]
        .borrow_mut()
        .push(CubeFaceRenderElement::SolidColor(
            workspace.render_background(),
        ));
    for window in workspace.windows() {
        *window.0.offscreen_writes.borrow_mut() = Some(Vec::new());
    }
    let lifecycle = monitor.cube_lifecycle();
    let commits = *lease.cube_commits.borrow();
    lifecycle.leave_cube();
    assert!(lease
        .cube_visible_workspaces
        .borrow()
        .iter()
        .all(Option::is_none));
    assert!(lease.cube_face_states.borrow().iter().all(Option::is_none));
    assert!(lease.cube_elements[0].borrow().is_empty());
    assert!(lease.cube_feedback.borrow().iter().all(Option::is_none));
    for window in workspace.windows() {
        let writes = window.0.offscreen_writes.borrow();
        let writes = writes.as_ref().unwrap();
        assert_eq!(writes.len(), 1);
        assert!(writes[0].is_none());
    }
    // A borrowed scratch buffer makes accidental per-frame clearing fail. The
    // marker fingerprint likewise detects cleanup beyond the first transition.
    lease.cube_face_states.borrow_mut()[0] = Some(fingerprint());
    let mut scratch = lease.cube_elements[0].borrow_mut();
    scratch.push(CubeFaceRenderElement::SolidColor(
        workspace.render_background(),
    ));
    lifecycle.leave_cube();
    lifecycle.leave_cube();
    assert_eq!(scratch.len(), 1);
    assert_eq!(lease.cube_face_states.borrow()[0], Some(fingerprint()));
    assert_eq!(*lease.cube_commits.borrow(), commits);
    for window in workspace.windows() {
        assert_eq!(
            window.0.offscreen_writes.borrow().as_ref().unwrap().len(),
            1
        );
    }
}

#[test]
fn cube_busy_restores_cached_feedback_only_for_visible_faces_without_retiring_state() {
    use smithay::backend::renderer::element::Id;
    let layout = cube_layout(1., clarion_ipc::TilingMode::Scrolling);
    let monitor = layout.active_monitor_ref().unwrap();
    let workspace = &monitor.workspaces[0];
    let id = workspace.id();
    let lease = monitor.cube_lifecycle().resources;
    *lease.cube_visible_workspaces.borrow_mut() = [Some(id), None];
    lease.cube_face_states.borrow_mut()[0] = Some(fingerprint());
    let data = OffscreenData {
        id: Id::new(),
        states: Default::default(),
    };
    *lease.cube_feedback.borrow_mut() = [
        Some((id, data.clone())),
        Some((monitor.workspaces[1].id(), data.clone())),
    ];
    for window in workspace.windows() {
        *window.0.offscreen_writes.borrow_mut() = Some(Vec::new());
    }
    for window in monitor.workspaces[1].windows() {
        *window.0.offscreen_writes.borrow_mut() = Some(Vec::new());
    }
    let lifecycle = monitor.cube_lifecycle();
    let commits = *lease.cube_commits.borrow();
    let scratch = lease.cube_elements[0].borrow_mut();
    lifecycle.restore_feedback();
    drop(scratch);
    assert_eq!(*lease.cube_visible_workspaces.borrow(), [Some(id), None]);
    assert_eq!(lease.cube_face_states.borrow()[0], Some(fingerprint()));
    assert_eq!(*lease.cube_commits.borrow(), commits);
    assert!(lease.cube_feedback.borrow()[0].is_some());
    for window in workspace.windows() {
        let writes = window.0.offscreen_writes.borrow();
        let writes = writes.as_ref().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].as_ref().unwrap().id, data.id);
    }
    for window in monitor.workspaces[1].windows() {
        assert!(window
            .0
            .offscreen_writes
            .borrow()
            .as_ref()
            .unwrap()
            .is_empty());
    }
}

#[test]
fn cube_busy_after_front_order_swap_keeps_both_workspace_feedback_generations() {
    use smithay::backend::renderer::element::Id;
    let layout = cube_layout(1., clarion_ipc::TilingMode::Scrolling);
    let monitor = layout.active_monitor_ref().unwrap();
    let first = monitor.workspaces[0].id();
    let second = monitor.workspaces[1].id();
    let data = || OffscreenData {
        id: Id::new(),
        states: Default::default(),
    };
    let first_old = data();
    let second_old = data();
    let second_new = data();
    let lease = monitor.cube_lifecycle().resources;
    *lease.cube_visible_workspaces.borrow_mut() = [Some(first), Some(second)];
    *lease.cube_feedback.borrow_mut() =
        [Some((first, first_old.clone())), Some((second, second_old))];
    for workspace in &monitor.workspaces[..2] {
        for window in workspace.windows() {
            *window.0.offscreen_writes.borrow_mut() = Some(Vec::new());
        }
    }
    let lifecycle = monitor.cube_lifecycle();
    // The new front face (second workspace) finishes in target slot zero;
    // rendering the first workspace in target slot one then becomes Busy.
    *lease.cube_visible_workspaces.borrow_mut() = [Some(second), Some(first)];
    lifecycle.record_feedback(second, second_new.clone());
    lifecycle.restore_feedback();
    for (workspace, expected) in [
        (&monitor.workspaces[0], &first_old),
        (&monitor.workspaces[1], &second_new),
    ] {
        for window in workspace.windows() {
            let writes = window.0.offscreen_writes.borrow();
            let writes = writes.as_ref().unwrap();
            assert_eq!(writes.last().unwrap().as_ref().unwrap().id, expected.id);
        }
    }
    let identities = lease
        .cube_feedback
        .borrow()
        .iter()
        .flatten()
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    assert_eq!(identities.iter().filter(|id| **id == first).count(), 1);
    assert_eq!(identities.iter().filter(|id| **id == second).count(), 1);
}

#[test]
#[ignore = "requires real DRM render node, EGL, Vulkan and a second native renderer epoch"]
fn cube_production_scene_pixels_privacy_exit_and_renderer_epoch_match_gles() {
    use smithay::backend::allocator::Fourcc;
    use smithay::backend::renderer::Renderer;

    use crate::render_helpers::vulkan::hardware_tests::setup;
    use crate::render_helpers::vulkan::native::NativeRenderer;
    use crate::render_helpers::{render_to_vec, shaders, RenderTarget};

    const WIDTH: usize = 1280;
    const HEIGHT: usize = 720;
    fn colored(pixels: &[u8]) -> usize {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| {
                let maximum = *pixel[..3].iter().max().unwrap();
                let minimum = *pixel[..3].iter().min().unwrap();
                maximum > 80 && maximum - minimum > 45
            })
            .count()
    }
    fn assert_interior_matches(actual: &[u8], expected: &[u8], label: &str) {
        assert_eq!(actual.len(), WIDTH * HEIGHT * 4);
        assert_eq!(actual.len(), expected.len());
        let pixel = |pixels: &[u8], x: usize, y: usize| {
            let start = (y * WIDTH + x) * 4;
            [
                pixels[start],
                pixels[start + 1],
                pixels[start + 2],
                pixels[start + 3],
            ]
        };
        let mut interior = 0;
        for y in (2..HEIGHT - 2).step_by(3) {
            for x in (2..WIDTH - 2).step_by(3) {
                let reference = pixel(expected, x, y);
                // Flat 5x5 neighborhoods exclude mesh rasterization boundaries,
                // while still checking the actual window and background pixels.
                if (-2isize..=2).all(|dy| {
                    (-2isize..=2).all(|dx| {
                        pixel(
                            expected,
                            x.wrapping_add_signed(dx),
                            y.wrapping_add_signed(dy),
                        ) == reference
                    })
                }) {
                    let actual = pixel(actual, x, y);
                    assert!(
                        actual
                            .iter()
                            .zip(reference)
                            .all(|(a, b)| a.abs_diff(b) <= 3),
                        "{label}: interior ({x}, {y}) actual={actual:?} expected={reference:?}"
                    );
                    interior += 1;
                }
            }
        }
        assert!(
            interior > 1000,
            "{label}: require meaningful interior coverage"
        );
        let actual_colors = colored(actual);
        let expected_colors = colored(expected);
        assert!(
            actual_colors.abs_diff(expected_colors) <= expected_colors / 200 + 32,
            "{label}: colored coverage {actual_colors} vs {expected_colors}"
        );
    }

    let (vk, mut gles, _) = setup();
    crate::render_helpers::resources::init(&mut gles);
    shaders::init(&mut gles);
    let mut native = NativeRenderer::new(vk).unwrap();
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    for (index, scale) in [1., 1.25, 1.5, 2.].into_iter().enumerate() {
        let mode = if index % 2 == 0 {
            clarion_ipc::TilingMode::Scrolling
        } else {
            clarion_ipc::TilingMode::Dwindle
        };
        let mut layout = cube_layout_with_privacy(scale, mode, true);
        let mut options = Options::clone(&layout.options);
        options.layout.border.off = true;
        options.layout.focus_ring.off = true;
        options.layout.shadow.on = false;
        options.animations.window_wobble.off = true;
        layout.update_options(options);
        for (_, window) in layout.windows() {
            window.0.render_color.set(Some(if *window.id() % 2 == 0 {
                [0.85, 0.12, 0.08, 1.]
            } else {
                [0.08, 0.12, 0.85, 1.]
            }));
            *window.0.offscreen_writes.borrow_mut() = Some(Vec::new());
        }
        Op::CompleteAnimations.apply(&mut layout);
        let output = layout.active_output().unwrap().clone();
        macro_rules! draw {
            ($renderer:expr, $target:expr) => {{
                let renderer = &mut $renderer;
                layout.update_render_elements(None);
                let mut elements = Vec::new();
                let source = layout.scene_source(&output);
                source.monitor().unwrap().render_workspaces(
                    RenderCtx {
                        renderer,
                        target: $target,
                        xray: None,
                    },
                    true,
                    &mut |element| elements.push(element),
                );
                render_to_vec(
                    renderer,
                    (WIDTH as i32, HEIGHT as i32).into(),
                    scale.into(),
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    elements.iter().rev(),
                )
                .unwrap()
            }};
        }
        layout.switch_workspace(1);
        let start = layout.clock.now_unadjusted();
        for millis in [1, 400, 600] {
            layout
                .clock
                .set_unadjusted(start + Duration::from_millis(millis));
            layout.advance_animations();
            assert!(layout.active_monitor_ref().unwrap().is_cube_scene());
            let mut outputs = Vec::new();
            for target in [
                RenderTarget::Output,
                RenderTarget::ScreenCapture,
                RenderTarget::Screencast,
            ] {
                let actual = draw!(native, target);
                let reference = draw!(gles, target);
                assert_interior_matches(
                    &actual,
                    &reference,
                    &format!("scale={scale} t={millis} target={target:?}"),
                );
                if target == RenderTarget::Output {
                    assert!(
                        colored(&actual) > 100,
                        "Cube must actually draw window pixels"
                    );
                } else {
                    assert_eq!(
                        colored(&actual),
                        0,
                        "private Cube windows must be blocked for {target:?}"
                    );
                }
                outputs.push(actual);
            }
            assert_ne!(outputs[0], outputs[1]);
            assert_eq!(colored(&outputs[1]), colored(&outputs[2]));
        }
        if index == 0 {
            let before = layout.core.borrow().windows().cloned().collect::<Vec<_>>();
            let old_context = native.context_id();
            let (vk, reference, allocator) = setup();
            drop(reference);
            drop(allocator);
            let mut replacement = NativeRenderer::new(vk).unwrap();
            replacement
                .prepare_output_format_for_test(Fourcc::Abgr8888)
                .unwrap();
            assert_ne!(old_context, replacement.context_id());
            layout.on_native_renderer_replaced();
            assert_eq!(
                before,
                layout.core.borrow().windows().cloned().collect::<Vec<_>>()
            );
            let previous = std::mem::replace(&mut native, replacement);
            drop(previous);
            for (_, window) in layout.windows() {
                window.0.render_color.set(Some([0.05, 0.85, 0.1, 1.]));
            }
            let actual = draw!(native, RenderTarget::Output);
            let reference = draw!(gles, RenderTarget::Output);
            assert_interior_matches(&actual, &reference, "new native renderer epoch");
            assert!(
                actual
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|pixel| pixel[1] > 150 && pixel[0] < 50 && pixel[2] < 50)
                    .count()
                    > 100,
                "replacement must draw fresh green pixels rather than stale old-context faces"
            );
        }
        Op::CompleteAnimations.apply(&mut layout);
        assert!(!layout.active_monitor_ref().unwrap().is_cube_scene());
        let actual = draw!(native, RenderTarget::Output);
        let reference = draw!(gles, RenderTarget::Output);
        assert_interior_matches(&actual, &reference, "return to ordinary desktop");
        let lifecycle = layout.active_monitor_ref().unwrap().cube_lifecycle();
        assert!(lifecycle
            .resources
            .cube_visible_workspaces
            .borrow()
            .iter()
            .all(Option::is_none));
        assert!(lifecycle
            .resources
            .cube_face_states
            .borrow()
            .iter()
            .all(Option::is_none));
        assert!(lifecycle
            .resources
            .cube_feedback
            .borrow()
            .iter()
            .all(Option::is_none));
        for (_, window) in layout.windows() {
            assert!(
                window
                    .0
                    .offscreen_writes
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .last()
                    .is_none_or(Option::is_none),
                "leaving Cube clears each window's old offscreen callback route"
            );
        }
        layout.verify_invariants();
    }
    eprintln!("PASS: production Cube scene draws matching GLES/Vulkan interior pixels at four scales, isolates both capture targets, retires callback feedback on exit and rebuilds after a native renderer epoch change");
}
