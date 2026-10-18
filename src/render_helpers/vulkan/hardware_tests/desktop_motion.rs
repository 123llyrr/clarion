//! Opt-in native desktop motion probe. Uses the production offscreen, opening,
//! mesh and output-color paths; never needs display master or changes an output.
use std::rc::Rc;
use std::time::{Duration, Instant};

use smithay::backend::renderer::element::{Element, Kind, RenderElement};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Bind, Frame, Offscreen, Renderer, Texture};
use smithay::utils::{Rectangle, Scale, Transform};

use super::{setup, Fourcc};
use crate::animation::{Animation, Clock};
use crate::color::{ColorDescription, HdrOutputSettings};
use crate::layout::opening_window::OpenAnimation;
use crate::render_helpers::mesh::MeshRenderElement;
use crate::render_helpers::offscreen::NativeOffscreenBuffer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::vulkan::native::NativeRenderer;
use crate::render_helpers::vulkan::timing::GpuTimingTag;
use crate::scene::opening_window::{render_opening_window_native, OpeningWindowRenderElement};
use crate::scene::window_resources::AnimationBuffers;

fn distribution(label: &str, values: &mut [f64]) {
    assert!(!values.is_empty(), "missing {label} measurements");
    values.sort_by(f64::total_cmp);
    eprintln!(
        "{label}: n={} p50_ms={:.6} p95_ms={:.6} max_ms={:.6}",
        values.len(),
        values[(values.len() - 1) / 2],
        values[(values.len() as f64 * 0.95).ceil() as usize - 1],
        values[values.len() - 1],
    );
}

#[test]
#[ignore = "requires real DRM render node, Vulkan timestamps; opt-in 4K motion probe"]
fn native_4k_open_and_genie_hdr_sdr_probe() {
    let (vk, reference, allocator) = setup();
    drop(reference);
    drop(allocator);
    let mut renderer = NativeRenderer::new(vk).unwrap();
    for format in [Fourcc::Abgr8888, Fourcc::Abgr16161616f, Fourcc::Abgr2101010] {
        renderer.prepare_output_format_for_test(format).unwrap();
    }
    renderer.set_completion_export_required(false);
    let output_size = smithay::utils::Size::<i32, smithay::utils::Physical>::from((3840, 2400));
    let mut output = renderer
        .create_buffer(Fourcc::Abgr2101010, (3840, 2400).into())
        .unwrap();
    let scale = Scale::from(2.);
    let window_size = (1600., 1000.).into();
    let source = SolidColorBuffer::new(window_size, [0.25, 0.5, 0.75, 1.]);
    let elements = [SolidColorRenderElement::from_buffer(
        &source,
        (0., 0.),
        1.,
        Kind::Unspecified,
    )];
    let config = clarion_config::animations::WindowOpenAnim::default();
    let mut sequence = 0;
    for hdr in [false, true, false] {
        renderer.set_output_color(Some(if hdr {
            HdrOutputSettings::default().description()
        } else {
            ColorDescription::srgb()
        }));
        let buffers = AnimationBuffers::default();
        let snapshot = NativeOffscreenBuffer::default();
        let (texture, ready, geometry, _, _) =
            snapshot.render(&mut renderer, scale, &elements).unwrap();
        ready.wait().unwrap();
        assert_eq!(
            texture.color.description,
            renderer.scene_color_description()
        );
        assert_eq!(texture.format(), Some(Fourcc::Abgr16161616f));
        for effect in ["open", "genie"] {
            let mut clock = Clock::with_time(Duration::ZERO);
            let open = OpenAnimation::new(
                Animation::new(clock.clone(), 0., 1., 0., config.anim),
                config.popin_ratio,
                config.glide,
                true,
                config.effect,
            );
            let mut submission = Vec::new();
            let mut gpu = Vec::new();
            // Warm the exact path, then sample a complete opening or a complete
            // minimize/restore cycle. No readback or fence wait enters CPU work.
            for frame_index in 0..72 {
                let measured = frame_index >= 8;
                let progress = if measured {
                    (frame_index - 8) as f64 / 63.
                } else {
                    0.5
                };
                let started = Instant::now();
                let element = if effect == "open" {
                    clock.set_unadjusted(Duration::from_secs_f64(progress * 0.220));
                    Some(
                        render_opening_window_native(
                            &open.pose(),
                            &mut renderer,
                            &buffers,
                            None,
                            &elements,
                            window_size,
                            (40., 40.).into(),
                            scale,
                            1.,
                        )
                        .unwrap()
                        .0,
                    )
                } else {
                    None
                };
                let mesh = if effect == "genie" {
                    let progress = if progress < 0.5 {
                        progress * 2.
                    } else {
                        (1. - progress) * 2.
                    };
                    let (area, vertices) = clarion_core::deformation::minimize_vertices_with_scale(
                        clarion_core::Rect::new(40., 40., 1600., 1000.),
                        clarion_core::Rect::new(920., 1110., 48., 48.),
                        progress,
                        clarion_core::deformation::Edge::Bottom,
                        clarion_core::deformation::Effect::Genie,
                        2.,
                    );
                    Some(MeshRenderElement::new_vulkan(
                        smithay::backend::renderer::element::Id::new(),
                        CommitCounter::default(),
                        texture.clone(),
                        (f64::from(geometry.size.w), f64::from(geometry.size.h)).into(),
                        Rectangle::new((area.x, area.y).into(), (area.width, area.height).into()),
                        Rc::from(vertices),
                        1.,
                        Kind::Unspecified,
                    ))
                } else {
                    None
                };
                sequence += 1;
                renderer.arm_output_timing(
                    GpuTimingTag {
                        output_generation: 1,
                        redraw_sequence: sequence,
                    },
                    (3840, 2400),
                );
                let completion = {
                    let mut target = renderer.bind(&mut output).unwrap();
                    let mut frame = renderer
                        .render(&mut target, output_size, Transform::Normal)
                        .unwrap();
                    frame
                        .clear(
                            [0., 0., 0., 1.].into(),
                            &[Rectangle::from_size(output_size)],
                        )
                        .unwrap();
                    if let Some(element) = &element {
                        let dst = element.geometry(scale);
                        <OpeningWindowRenderElement as RenderElement<NativeRenderer>>::draw(
                            element,
                            &mut frame,
                            element.src(),
                            dst,
                            &[Rectangle::from_size(dst.size)],
                            &[],
                            None,
                        )
                        .unwrap();
                    }
                    if let Some(mesh) = &mesh {
                        let dst = mesh.geometry(scale);
                        <MeshRenderElement as RenderElement<NativeRenderer>>::draw(
                            mesh,
                            &mut frame,
                            mesh.src(),
                            dst,
                            &[Rectangle::from_size(dst.size)],
                            &[],
                            None,
                        )
                        .unwrap();
                    }
                    frame.finish().unwrap()
                };
                if measured {
                    submission.push(started.elapsed().as_secs_f64() * 1000.);
                }
                completion.wait().unwrap();
                renderer.retire_completed().unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut completed = false;
                while !completed {
                    renderer.drain_gpu_timings(|sample| {
                        assert_eq!(sample.tag.redraw_sequence, sequence);
                        completed = true;
                        if measured {
                            gpu.push(sample.gpu_time.as_secs_f64() * 1000.);
                        }
                    });
                    assert!(
                        Instant::now() < deadline,
                        "GPU timestamp delivery timed out"
                    );
                    if !completed {
                        std::thread::yield_now();
                    }
                }
            }
            assert_eq!(submission.len(), 64);
            assert_eq!(gpu.len(), 64);
            distribution(
                &format!("MOTION_PROBE hdr={hdr} effect={effect} cpu_submission_debug"),
                &mut submission,
            );
            distribution(
                &format!("MOTION_PROBE hdr={hdr} effect={effect} gpu_output_only"),
                &mut gpu,
            );
        }
    }
}
