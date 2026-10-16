use super::*;

fn core_records(layout: &Layout<TestWindow>) -> Vec<clarion_core::WindowRecord<u64>> {
    layout.core.borrow().windows().cloned().collect()
}

#[test]
fn renderer_replacement_preserves_layout_identity_minimization_and_drag() {
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        for floating in [false, true] {
            let mut layout = check_ops([
                Op::AddOutput(1),
                Op::AddOutput(2),
                Op::FocusOutput(1),
                Op::SwitchTilingMode(Some(mode)),
                Op::AddWindow {
                    params: TestWindowParams::new(0),
                },
                Op::AddWindow {
                    params: TestWindowParams {
                        is_floating: floating,
                        ..TestWindowParams::new(1)
                    },
                },
                Op::CompleteAnimations,
            ]);
            assert!(layout.minimize_window(&0));
            let output = layout.active_output().unwrap().clone();
            assert!(layout.interactive_move_begin(1, &output, (100., 100.).into()));
            assert!(layout.interactive_move_update(
                &1,
                (500., 100.).into(),
                output,
                (600., 200.).into()
            ));
            let before = core_records(&layout);
            layout.on_native_renderer_replaced();
            assert_eq!(before, core_records(&layout));
            assert!(layout.is_minimized(&0));
            assert!(matches!(
                layout.interactive_move,
                Some(InteractiveMoveState::Moving(_))
            ));
            layout.verify_invariants();
            layout.interactive_move_end(&1);
            assert!(layout.restore_window(&0));
            layout.verify_invariants();
            // A recovery completion may race output removal. The same cleanup
            // must also work for detached workspaces without losing membership.
            Op::RemoveOutput(1).apply(&mut layout);
            Op::RemoveOutput(2).apply(&mut layout);
            let detached = core_records(&layout);
            layout.on_native_renderer_replaced();
            assert_eq!(detached, core_records(&layout));
            layout.verify_invariants();
        }
    }
}

#[test]
#[ignore = "requires real DRM render node and two native Vulkan contexts"]
fn native_renderer_replacement_discards_stale_tile_snapshots() {
    use crate::render_helpers::vulkan::{hardware_tests::setup, native::NativeRenderer};
    use smithay::backend::allocator::Fourcc;
    use smithay::backend::renderer::{Bind, ExportMem, Renderer as _, Texture};

    let (vk, reference, allocator) = setup();
    drop(reference);
    drop(allocator);
    let mut previous = NativeRenderer::new(vk).unwrap();
    previous
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    let mut options = Options::default();
    options.animations.off = true;
    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams {
                    is_floating: true,
                    bbox: Rectangle::from_size((48, 32).into()),
                    ..TestWindowParams::new(1)
                },
            },
            Op::CompleteAnimations,
        ],
    );
    layout.update_render_elements(None);
    {
        let tile = layout
            .active_workspace_mut()
            .unwrap()
            .tiles_mut()
            .next()
            .unwrap();
        tile.window().0.render_color.set(Some([1., 0., 0., 1.]));
        assert!(
            tile.store_unmap_snapshot_native_if_empty(
                &mut previous,
                None,
                false,
                XrayPos::default(),
            ),
            "the regression must start with an actual cached image from the old context"
        );
        // The cached unmap image is red; a regenerated image must be green.
        tile.window().0.render_color.set(Some([0., 1., 0., 1.]));
    }
    let (vk, reference, allocator) = setup();
    drop(reference);
    drop(allocator);
    let mut replacement = NativeRenderer::new(vk).unwrap();
    replacement
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    assert_ne!(previous.context_id(), replacement.context_id());
    let before = core_records(&layout);
    layout.on_native_renderer_replaced();
    assert_eq!(before, core_records(&layout));
    let snapshot = layout
        .active_workspace_mut()
        .unwrap()
        .tiles_mut()
        .next()
        .unwrap()
        .snapshot_native(&mut replacement)
        .unwrap();
    for (mut texture, _) in snapshot.targets {
        let size = texture.size();
        // Without invalidation, snapshot_native returns the previous context's
        // red texture and binding it to replacement fails as a foreign image.
        let framebuffer = replacement.bind(&mut texture).unwrap();
        let mapping = replacement
            .copy_framebuffer(&framebuffer, Rectangle::from_size(size), Fourcc::Abgr8888)
            .unwrap();
        let pixels = replacement.map_texture(&mapping).unwrap();
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| { pixel[0] < 10 && pixel[1] > 240 && pixel[2] < 10 }),
            "all capture targets must contain fresh green pixels from the replacement context"
        );
    }
    layout.verify_invariants();
}
