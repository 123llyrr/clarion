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
use crate::render_helpers::vulkan::native::{NativeError, NativeRenderer};
use crate::render_helpers::vulkan::timing::GpuTimingTag;
use crate::render_helpers::vulkan::VulkanTexture;
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

fn motion_thread_cpu() -> Option<Duration> {
    super::super::custom_shader::gpu_preparation::thread_cpu()
}

fn motion_cpu_elapsed(start: Option<Duration>) -> Option<f64> {
    motion_thread_cpu()
        .zip(start)
        .and_then(|(end, start)| end.checked_sub(start))
        .map(|duration| duration.as_secs_f64() * 1000.)
}

/// Read only a private offscreen pixel, after the measured submission has
/// completed. Dominant-channel checks survive the SDR/HDR scene encoding while
/// detecting an unchanged or incorrectly reused target after source damage.
fn assert_motion_source_pixel(
    renderer: &NativeRenderer,
    texture: &VulkanTexture,
    dominant: usize,
) -> [f32; 4] {
    let bytes = renderer
        .vulkan()
        .read_memory_texture(texture, Rectangle::new((16, 16).into(), (1, 1).into()))
        .unwrap();
    assert_eq!(bytes.len(), 8);
    let rgba: [f32; 4] = std::array::from_fn(|channel| {
        super::super::color::half_to_f32(u16::from_le_bytes([
            bytes[channel * 2],
            bytes[channel * 2 + 1],
        ]))
    });
    assert!(rgba.iter().all(|value| value.is_finite()), "{rgba:?}");
    assert!(rgba[dominant] > 0.01 && rgba[3] > 0.99, "{rgba:?}");
    for (channel, value) in rgba.iter().enumerate().take(3) {
        if channel != dominant {
            assert!(value.abs() < rgba[dominant] * 0.01, "{rgba:?}");
        }
    }
    rgba
}

#[test]
#[ignore = "requires real DRM render node and Vulkan timestamps; cold 4K opening target probe"]
fn native_4k_cold_open_target_hdr_sdr_projected_refresh_probe() {
    let config = clarion_config::animations::WindowOpenAnim::default();
    let scale = Scale::from(2.);
    let output_size = smithay::utils::Size::<i32, smithay::utils::Physical>::from((3840, 2400));
    let window_size = smithay::utils::Size::<f64, smithay::utils::Logical>::from((1600., 1000.));
    let colors: [[f32; 4]; 3] = [[1., 0., 0., 1.], [0., 1., 0., 1.], [0., 0., 1., 1.]];

    for projected_refresh_hz in [60u32, 120, 144] {
        let (vk, reference, allocator) = setup();
        drop(reference);
        drop(allocator);
        let mut renderer = NativeRenderer::new(vk).unwrap();
        renderer.set_completion_export_required(false);
        // Device/program/attachment bootstrap is deliberately outside the
        // workload. This measures a cold retained target, not cold shader JIT.
        for format in [Fourcc::Abgr8888, Fourcc::Abgr16161616f, Fourcc::Abgr2101010] {
            renderer.prepare_output_format_for_test(format).unwrap();
        }
        let mut output = renderer
            .create_buffer(
                Fourcc::Abgr2101010,
                output_size.to_logical(1).to_buffer(1, Transform::Normal),
            )
            .unwrap();
        let mut retained = Vec::new();
        let mut retained_images = Vec::new();
        let mut sequence = 0;

        for (phase_index, (phase, hdr)) in [("sdr", false), ("hdr", true), ("sdr-return", false)]
            .into_iter()
            .enumerate()
        {
            renderer.set_output_color(Some(if hdr {
                HdrOutputSettings::default().description()
            } else {
                ColorDescription::srgb()
            }));
            // The 10-bit output owns a separate FP16 working scene. Prepare it
            // with the same whole-scene async scope as TTY/winit, before the
            // cold-window clock starts. Otherwise the first measured output
            // silently takes the synchronous compatibility allocation path.
            let attachment_started = Instant::now();
            let attachment_deadline = attachment_started + Duration::from_secs(15);
            let mut attachment_attempts = 0;
            let mut attachment_deferrals = 0;
            let blank_completion = loop {
                renderer.poll_preparation_for_test().unwrap();
                renderer.begin_scene();
                attachment_attempts += 1;
                let result = {
                    let _async_scope = renderer.async_offscreen_scope();
                    (|| {
                        let mut target = renderer.bind(&mut output)?;
                        let mut frame =
                            renderer.render(&mut target, output_size, Transform::Normal)?;
                        frame.clear(
                            [0., 0., 0., 1.].into(),
                            &[Rectangle::from_size(output_size)],
                        )?;
                        frame.finish()
                    })()
                };
                match result {
                    Ok(completion) => break completion,
                    Err(error) => {
                        assert!(NativeError::is_backpressure(&error), "{error:#}");
                        attachment_deferrals += 1;
                        assert!(
                            Instant::now() < attachment_deadline,
                            "blank output working scene never became ready"
                        );
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            };
            blank_completion.wait().unwrap();
            renderer.retire_completed().unwrap();
            let attachment_bootstrap_wall_ms = attachment_started.elapsed().as_secs_f64() * 1000.;
            let buffers = AnimationBuffers::default();
            let mut source = SolidColorBuffer::new(window_size, colors[phase_index]);
            let mut clock = Clock::with_time(Duration::ZERO);
            let animation = Animation::new(clock.clone(), 0., 1., 0., config.anim);
            let duration = animation.duration();
            assert!(!duration.is_zero() && duration < Duration::from_secs(2));
            let open = OpenAnimation::new(
                animation,
                config.popin_ratio,
                config.glide,
                true,
                config.effect,
            );
            let cold_started = Instant::now();
            let deadline = cold_started + Duration::from_secs(15);
            let owner_allocations = renderer.compatibility_work().cold_allocations;
            let mut attempts = Vec::new();
            let mut deferrals = 0;
            let (texture, initial_sync, geometry, _, initial_commit) = loop {
                // Deliberately leave completion installation until after two
                // owner calls. The second call exercises the real pending job
                // path without introducing an artificial worker gate or delay.
                if attempts.len() >= 2 {
                    renderer.poll_preparation_for_test().unwrap();
                }
                let elements = [SolidColorRenderElement::from_buffer(
                    &source,
                    (0., 0.),
                    1.,
                    Kind::Unspecified,
                )];
                renderer.begin_scene();
                let started = Instant::now();
                let cpu_started = motion_thread_cpu();
                let result = {
                    let _async_scope = renderer.async_offscreen_scope();
                    buffers.native.render(&mut renderer, scale, &elements)
                };
                let wall_ms = started.elapsed().as_secs_f64() * 1000.;
                let thread_cpu_ms = motion_cpu_elapsed(cpu_started);
                attempts.push(serde_json::json!({
                    "attempt": attempts.len() + 1,
                    "at_ms_since_cold_request": started.duration_since(cold_started).as_secs_f64() * 1000.,
                    "owner_call_wall_ms": wall_ms,
                    "owner_thread_cpu_ms": thread_cpu_ms,
                    "deferred": result.is_err(),
                }));
                match result {
                    Ok(ready) => break ready,
                    Err(error) => {
                        assert!(NativeError::is_backpressure(error.as_ref()), "{error:#}");
                        assert!(renderer.scene_was_deferred());
                        deferrals += 1;
                        assert!(Instant::now() < deadline, "cold target never became ready");
                        // Waiting for preparation is not owner CPU work. It is
                        // included in first_ready_return_wall_ms below.
                        if attempts.len() >= 2 {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                    }
                }
            };
            let first_ready_return_wall_ms = cold_started.elapsed().as_secs_f64() * 1000.;
            assert!(
                deferrals >= 2,
                "admission and pending paths were not observed"
            );
            assert!(!renderer.scene_was_deferred());
            assert_eq!(geometry.size, (3200, 2000).into());
            assert_eq!(texture.format(), Some(Fourcc::Abgr16161616f));
            assert_eq!(
                texture.color.description,
                renderer.scene_color_description()
            );
            assert_eq!(
                renderer.compatibility_work().cold_allocations,
                owner_allocations
            );
            let cold_target_owner_allocations =
                renderer.compatibility_work().cold_allocations - owner_allocations;
            let image = std::sync::Arc::as_ptr(&texture.inner) as usize;
            assert!(
                !retained_images.contains(&image),
                "a held phase target was overwritten"
            );
            retained_images.push(image);

            let expected =
                (duration.as_secs_f64() * f64::from(projected_refresh_hz)).ceil() as usize + 1;
            let mut frames = Vec::new();
            let mut submission = Vec::new();
            let mut thread_cpu = Vec::new();
            let mut gpu = Vec::new();
            let mut first_output_submit_return_wall_ms = None;
            let mut initial_pixel = None;
            for frame_index in 0..expected {
                if frame_index == expected / 2 {
                    // Real element damage/commit, not a fabricated timestamp.
                    // This re-renders the same retained target during opening.
                    source.set_color(colors[(phase_index + 1) % colors.len()]);
                }
                let projected_time = Duration::from_secs_f64(
                    (frame_index as f64 / f64::from(projected_refresh_hz))
                        .min(duration.as_secs_f64()),
                );
                clock.set_unadjusted(projected_time);
                renderer.begin_scene();
                let started = Instant::now();
                let cpu_started = motion_thread_cpu();
                // Production holds this guard through scene construction AND
                // the final output submission, not just the opening element.
                let async_scope = renderer.async_offscreen_scope();
                let pose = open.pose();
                let elements = [SolidColorRenderElement::from_buffer(
                    &source,
                    (0., 0.),
                    1.,
                    Kind::Unspecified,
                )];
                let element = render_opening_window_native(
                    &pose,
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
                .0;
                assert!(!renderer.scene_was_deferred());
                sequence += 1;
                renderer.arm_output_timing(
                    GpuTimingTag {
                        output_generation: u64::from(projected_refresh_hz),
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
                    let dst = element.geometry(scale);
                    <OpeningWindowRenderElement as RenderElement<NativeRenderer>>::draw(
                        &element,
                        &mut frame,
                        element.src(),
                        dst,
                        &[Rectangle::from_size(dst.size)],
                        &[],
                        None,
                    )
                    .unwrap();
                    frame.finish().unwrap()
                };
                drop(async_scope);
                let wall_ms = started.elapsed().as_secs_f64() * 1000.;
                let thread_cpu_ms = motion_cpu_elapsed(cpu_started);
                if frame_index == 0 {
                    first_output_submit_return_wall_ms =
                        Some(cold_started.elapsed().as_secs_f64() * 1000.);
                }
                submission.push(wall_ms);
                if let Some(value) = thread_cpu_ms {
                    thread_cpu.push(value);
                }
                // Fence retirement, timestamp delivery, and pixel readback are
                // outside BOTH measured owner-call wall/CPU spans.
                completion.wait().unwrap();
                renderer.retire_completed().unwrap();
                let timestamp_deadline = Instant::now() + Duration::from_secs(5);
                let mut sample_for_frame = None;
                while sample_for_frame.is_none() {
                    renderer.drain_gpu_timings(|sample| {
                        assert_eq!(
                            sample.tag.output_generation,
                            u64::from(projected_refresh_hz)
                        );
                        assert_eq!(sample.tag.redraw_sequence, sequence);
                        assert!(sample_for_frame.replace(sample).is_none());
                    });
                    assert!(
                        Instant::now() < timestamp_deadline,
                        "missing output-command timestamp"
                    );
                    if sample_for_frame.is_none() {
                        std::thread::yield_now();
                    }
                }
                let sample = sample_for_frame.unwrap();
                gpu.push(sample.gpu_time.as_secs_f64() * 1000.);
                frames.push(serde_json::json!({
                    "frame_index": frame_index, "redraw_sequence": sequence,
                    "projected_time_ms": projected_time.as_secs_f64() * 1000.,
                    "cpu_submission_wall_ms": wall_ms, "owner_thread_cpu_ms": thread_cpu_ms,
                    "gpu_output_command_time_ns": sample.gpu_time.as_nanos(),
                    "pose_progress": pose.progress, "pose_scale": pose.scale,
                    "pose_opacity": pose.opacity, "pose_offset_y": pose.offset_y,
                    "source_changed": frame_index >= expected / 2,
                }));
                if frame_index == 0 {
                    initial_sync.wait().unwrap();
                    initial_pixel =
                        Some(assert_motion_source_pixel(&renderer, &texture, phase_index));
                }
            }
            assert_eq!(submission.len(), expected);
            assert_eq!(gpu.len(), expected);
            assert!(open.is_done(), "final projected pose did not complete");
            let elements = [SolidColorRenderElement::from_buffer(
                &source,
                (0., 0.),
                1.,
                Kind::Unspecified,
            )];
            let (current, sync, _, _, commit) = buffers
                .native
                .render(&mut renderer, scale, &elements)
                .unwrap();
            sync.wait().unwrap();
            assert_eq!(std::sync::Arc::as_ptr(&current.inner) as usize, image);
            assert_ne!(
                commit, initial_commit,
                "source damage did not redraw the retained target"
            );
            let changed_pixel =
                assert_motion_source_pixel(&renderer, &current, (phase_index + 1) % colors.len());
            eprintln!(
                "COLD_OPEN_TARGET {}",
                serde_json::json!({
                    "phase": phase, "hdr": hdr, "projected_refresh_hz": projected_refresh_hz,
                    "output_pixels": [3840, 2400], "scale": 2,
                    "opening_effect": format!("{:?}", config.effect),
                    "cold_scope": "retained_target_only; renderer/program/formats/10bit output attachment and its blank FP16 working scene bootstrapped",
                    "blank_output_working_scene_bootstrapped": true,
                    "blank_output_bootstrap_attempts": attachment_attempts,
                    "blank_output_bootstrap_deferrals": attachment_deferrals,
                    "blank_output_bootstrap_wall_ms": attachment_bootstrap_wall_ms,
                    "async_scope": "whole opening scene and output submission, matching TTY/winit",
                    "attempts_n": attempts.len(), "deferrals": deferrals,
                    "first_ready_return_wall_ms": first_ready_return_wall_ms,
                    "first_output_submit_return_wall_ms": first_output_submit_return_wall_ms,
                    "first_ready_definition": "target exists and offscreen render submitted; not GPU completion or visible presentation",
                    "retry_poll_interval_ms": 1,
                    "cold_target_owner_allocations_delta": cold_target_owner_allocations,
                    "whole_scenario_owner_allocations_delta": renderer.compatibility_work().cold_allocations-owner_allocations,
                    "frames_expected": expected, "cpu_wall_n": submission.len(),
                    "owner_thread_cpu_n": thread_cpu.len(), "gpu_timestamp_n": gpu.len(),
                    "cpu_wall_coverage_complete": submission.len() == expected,
                    "owner_thread_cpu_coverage_complete": thread_cpu.len() == expected,
                    "gpu_timestamp_coverage_complete": gpu.len() == expected,
                    "attempt_thread_cpu_n": attempts.iter().filter(|attempt| !attempt["owner_thread_cpu_ms"].is_null()).count(),
                    "gpu_scope": "native_output_command_buffer_device_timestamp_pair; excludes prior snapshot/upload commands and KMS",
                    "sampling_scope": "projected timeline replay, first output included, no discarded warm frames; serialized completion outside measured spans",
                    "initial_source_pixel": initial_pixel, "changed_source_pixel": changed_pixel,
                    "attempts": attempts, "frames": frames,
                })
            );
            distribution(
                &format!(
                    "COLD_OPEN phase={phase} projected_hz={projected_refresh_hz} cpu_submission"
                ),
                &mut submission,
            );
            if !thread_cpu.is_empty() {
                distribution(&format!("COLD_OPEN phase={phase} projected_hz={projected_refresh_hz} owner_thread_cpu"), &mut thread_cpu);
            }
            distribution(
                &format!(
                    "COLD_OPEN phase={phase} projected_hz={projected_refresh_hz} gpu_output_only"
                ),
                &mut gpu,
            );
            // Retention forces the next color phase to obtain a different cold
            // target even though all three phases use an FP16 scene format.
            retained.push(buffers);
        }
        assert_eq!(retained.len(), 3);
    }
}
