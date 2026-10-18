//! Real queue/async-preparation regression for reused opening snapshot targets.

use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::Texture;
use smithay::utils::Scale;

use super::{setup, Fourcc};
use crate::color::{ColorDescription, HdrOutputSettings};
use crate::render_helpers::offscreen::NativeOffscreenBuffer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::vulkan::native::{NativeError, NativeRenderer};
use crate::render_helpers::vulkan::VulkanTexture;

fn capture_ready(
    renderer: &mut NativeRenderer,
    cache: &NativeOffscreenBuffer,
    color: [f32; 4],
) -> VulkanTexture {
    let source = SolidColorBuffer::new((83., 47.), color);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        renderer.poll_preparation_for_test().unwrap();
        renderer.begin_scene();
        let element =
            SolidColorRenderElement::from_buffer(&source, (0., 0.), 1., Kind::Unspecified);
        match cache.render(renderer, Scale::from(1.), &[element]) {
            Ok((texture, sync, geometry, _, _)) => {
                assert_eq!(geometry.size, (83, 47).into());
                sync.wait().unwrap();
                renderer.retire_completed().unwrap();
                return texture;
            }
            Err(error) => {
                assert!(NativeError::is_backpressure(error.as_ref()), "{error:#}");
                assert!(
                    Instant::now() < deadline,
                    "opening allocation never became ready"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

#[test]
#[ignore = "requires a real DRM render node and Vulkan; no display master"]
fn native_opening_cache_reuses_ready_targets_without_overwriting_held_snapshots() {
    // Some(sRGB), like HDR, uses the production FP16 working scene.
    for hdr in [false, true] {
        let (vk, reference, allocator) = setup();
        drop(reference);
        drop(allocator);
        let mut renderer = NativeRenderer::new(vk).unwrap();
        renderer.set_completion_export_required(false);
        renderer.set_output_color(Some(if hdr {
            HdrOutputSettings::default().description()
        } else {
            ColorDescription::srgb()
        }));
        renderer
            .prepare_output_format_for_test(Fourcc::Abgr16161616f)
            .unwrap();
        let initial_allocations = renderer.compatibility_work().cold_allocations;
        let _async_scope = renderer.async_offscreen_scope();
        let first = NativeOffscreenBuffer::default();
        let source = SolidColorBuffer::new((83., 47.), [1., 0., 0., 1.]);
        let gate = renderer.gate_gpu_preparation_for_test().unwrap();
        renderer.begin_scene();
        let cold = first.render(
            &mut renderer,
            1.0.into(),
            &[SolidColorRenderElement::from_buffer(
                &source,
                (0., 0.),
                1.,
                Kind::Unspecified,
            )],
        );
        assert!(cold
            .as_ref()
            .is_err_and(|error| NativeError::is_backpressure(error.as_ref())));
        assert!(renderer.scene_was_deferred());
        drop(gate);
        let texture = capture_ready(&mut renderer, &first, [1., 0., 0., 1.]);
        assert_eq!(texture.format(), Some(Fourcc::Abgr16161616f));
        assert_eq!(
            texture.color.description,
            renderer.scene_color_description()
        );
        let original_image = Arc::as_ptr(&texture.inner);
        drop(texture);
        first.clear();
        renderer.retire_completed().unwrap();

        // A later window must hit the pool in its first production async scene,
        // rather than queue another allocation or mark the output deferred.
        let second = NativeOffscreenBuffer::default();
        renderer.begin_scene();
        let element =
            SolidColorRenderElement::from_buffer(&source, (0., 0.), 1., Kind::Unspecified);
        let (held_snapshot, sync, _, _, _) = second
            .render(&mut renderer, 1.0.into(), &[element])
            .expect("a completed cleared opening must be reused immediately");
        assert_eq!(Arc::as_ptr(&held_snapshot.inner), original_image);
        assert!(!renderer.scene_was_deferred());
        sync.wait().unwrap();
        renderer.retire_completed().unwrap();
        second.clear();

        // Retain that snapshot after its scene is cleared. A concurrent opening
        // cannot draw over it, even though the pool also holds a reference.
        let third = NativeOffscreenBuffer::default();
        let gate = renderer.gate_gpu_preparation_for_test().unwrap();
        renderer.begin_scene();
        let element =
            SolidColorRenderElement::from_buffer(&source, (0., 0.), 1., Kind::Unspecified);
        let shared = third.render(&mut renderer, 1.0.into(), &[element]);
        assert!(shared
            .as_ref()
            .is_err_and(|error| NativeError::is_backpressure(error.as_ref())));
        drop(gate);
        let other = capture_ready(&mut renderer, &third, [0., 0., 1., 1.]);
        assert_ne!(Arc::as_ptr(&other.inner), original_image);

        // Once the snapshot releases the original, it becomes reusable again;
        // the other image remains live in the third scene and this `other` lease.
        drop(held_snapshot);
        renderer.retire_completed().unwrap();
        let fourth = NativeOffscreenBuffer::default();
        renderer.begin_scene();
        let element =
            SolidColorRenderElement::from_buffer(&source, (0., 0.), 1., Kind::Unspecified);
        let (reused, sync, _, _, _) = fourth
            .render(&mut renderer, 1.0.into(), &[element])
            .unwrap();
        assert_eq!(Arc::as_ptr(&reused.inner), original_image);
        assert_eq!(reused.color.description, renderer.scene_color_description());
        assert!(!renderer.scene_was_deferred());
        sync.wait().unwrap();
        assert_eq!(
            renderer.compatibility_work().cold_allocations,
            initial_allocations
        );
        drop(other);
    }
}
