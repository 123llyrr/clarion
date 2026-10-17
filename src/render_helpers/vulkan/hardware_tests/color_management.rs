//! Numerical HDR acceptance uses the real renderer and GPU readback, without
//! acquiring DRM master or changing the running display's connector state.
use super::*;
use crate::color::{ColorDescription, HdrOutputSettings, RenderingIntent, SurfaceColor};
use crate::render_helpers::vulkan::native::{NativeBusy, NativeError, NativeRenderer};
use smithay::backend::renderer::{ImportMem, Texture};

fn half(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 255) as i32 - 127 + 15;
    let mantissa = bits & 0x7fffff;
    if exponent >= 31 {
        return sign | 0x7c00;
    }
    if exponent <= 0 {
        if exponent < -10 {
            return sign;
        }
        return sign | (((mantissa | 0x800000) + (1 << (13 - exponent))) >> (14 - exponent)) as u16;
    }
    sign | (((exponent as u32) << 10) + ((mantissa + 0x1000) >> 13)) as u16
}

fn source(
    renderer: &mut NativeRenderer,
    encoded: [f32; 3],
    alpha: f32,
    color: SurfaceColor,
) -> (VulkanTexture, [f32; 3]) {
    let rgba = [
        encoded[0] * alpha,
        encoded[1] * alpha,
        encoded[2] * alpha,
        alpha,
    ];
    let words = rgba.map(half);
    let bytes: Vec<_> = words.into_iter().flat_map(u16::to_le_bytes).collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut texture = loop {
        match renderer.import_memory(&bytes, Fourcc::Abgr16161616f, (1, 1).into(), false) {
            Ok(texture) => break texture,
            Err(error @ NativeError::Busy(NativeBusy::Upload | NativeBusy::Completion)) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "numerical fixture upload admission did not retire: {error}"
                );
                // Numerical samples can outnumber the bounded upload queue.
                // Retry this owned source after asynchronous GPU retirement.
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error) => panic!("numerical fixture source upload failed: {error}"),
        }
    };
    texture.color = color;
    let alpha = super::super::color::half_to_f32(words[3]);
    let sampled = std::array::from_fn(|i| super::super::color::half_to_f32(words[i]) / alpha);
    (texture, sampled)
}

fn read(renderer: &mut NativeRenderer, target: &VulkanTexture) -> Vec<[f32; 4]> {
    let mapping = renderer
        .copy_texture(
            target,
            Rectangle::from_size(target.size()),
            Fourcc::Abgr2101010,
        )
        .unwrap();
    renderer
        .map_texture(&mapping)
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| {
            let word = u32::from_le_bytes(*bytes);
            [
                (word & 1023) as f32 / 1023.,
                ((word >> 10) & 1023) as f32 / 1023.,
                ((word >> 20) & 1023) as f32 / 1023.,
                (word >> 30) as f32 / 3.,
            ]
        })
        .collect()
}

fn draw(
    renderer: &mut NativeRenderer,
    target: &mut VulkanTexture,
    textures: &[VulkanTexture],
    output: ColorDescription,
    background: [f32; 4],
) {
    let _color_scope = renderer.output_color_scope(Some(output));
    let size = (target.width() as i32, target.height() as i32).into();
    let mut framebuffer = renderer.bind(target).unwrap();
    let mut frame = renderer
        .render(&mut framebuffer, size, Transform::Normal)
        .unwrap();
    frame
        .clear(
            Color32F::new(background[0], background[1], background[2], background[3]),
            &[Rectangle::from_size(size)],
        )
        .unwrap();
    for (i, texture) in textures.iter().enumerate() {
        let rect = Rectangle::new((i as i32, 0).into(), (1, 1).into());
        frame
            .render_texture_from_to(
                texture,
                Rectangle::from_size((1., 1.).into()),
                rect,
                &[Rectangle::from_size((1, 1).into())],
                &[],
                Transform::Normal,
                1.,
            )
            .unwrap();
    }
    frame.finish().unwrap().wait().unwrap();
}

fn renderer() -> NativeRenderer {
    let (vulkan, _, _) = setup();
    let mut renderer = NativeRenderer::new(vulkan).unwrap();
    renderer
        .prepare_output_format_for_test(Fourcc::Abgr2101010)
        .unwrap();
    renderer
}

fn render_sdr_output_frame(
    renderer: &NativeRenderer,
    scene: &VulkanTexture,
    target: &VulkanTexture,
    force_full_transform: bool,
    timing: Option<super::super::timing::QueryLease>,
) {
    let vk = renderer.vulkan();
    let mut frame = super::super::frame::VulkanFrame::begin(
        vk.ctx.as_ref().unwrap().clone(),
        vk.pipeline_mgr.as_ref().unwrap().clone(),
        vk.scratch_textures.clone(),
        vk.importer.as_ref().unwrap().clone(),
        scene.image(),
        scene.image_view(),
        vk::Format::R16G16B16A16_SFLOAT,
        true,
        scene.width,
        scene.height,
        None,
        scene.current_layout().get(),
        false,
    )
    .unwrap();
    frame.retain_target(scene);
    frame.set_color_output(
        target,
        ColorDescription::srgb(),
        &[Rectangle::from_size(
            (scene.width as i32, scene.height as i32).into(),
        )],
    );
    if force_full_transform {
        frame.disable_output_identity_for_test();
    }
    frame.measure_gpu_work(timing);
    frame.finish_and_wait().unwrap();
}

#[test]
#[ignore = "requires real DRM render node and Vulkan validation"]
fn sdr_identity_output_matches_full_transform_with_extended_premultiplied_alpha() {
    let mut renderer = renderer();
    let samples = [
        [0.7, -0.3, 1.2, 0.],
        [0.2, 0.3, 0.4, -0.5],
        [0.00025, 0.002, -0.001, 0.001],
        [0.1, 0.25, 0.75, 0.5],
        [0.1, 0.5, 1.3, 1.],
        [0.3, 1.4, -0.3, 2.],
        [0., 0., 0., 1.],
        [1., 1., 1., 1.],
        [0.0001, 0.004, 0.04, 1.],
    ];
    let words: Vec<_> = samples.into_iter().flatten().map(half).collect();
    let bytes: Vec<_> = words.iter().copied().flat_map(u16::to_le_bytes).collect();
    let vk = renderer.vulkan();
    let scene = allocate_texture(
        vk.ctx.as_ref().unwrap(),
        samples.len() as u32,
        1,
        vk::Format::R16G16B16A16_SFLOAT,
        vk::ImageTiling::OPTIMAL,
        vk::ImageUsageFlags::COLOR_ATTACHMENT
            | vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_SRC
            | vk::ImageUsageFlags::TRANSFER_DST,
        vk::ImageLayout::UNDEFINED,
        true,
    )
    .unwrap();
    vk.update_memory_texture(
        &scene,
        &bytes,
        samples.len() * 8,
        Rectangle::from_size((samples.len() as i32, 1).into()),
    )
    .unwrap();
    let size = (samples.len() as i32, 1).into();
    let mut results = Vec::new();
    for force_full_transform in [false, true] {
        let target = renderer.create_buffer(Fourcc::Abgr8888, size).unwrap();
        render_sdr_output_frame(&renderer, &scene, &target, force_full_transform, None);
        let mapping = renderer
            .copy_texture(&target, Rectangle::from_size(size), Fourcc::Abgr8888)
            .unwrap();
        results.push(renderer.map_texture(&mapping).unwrap().to_vec());
    }
    for (index, (fast, full)) in results[0].iter().zip(&results[1]).enumerate() {
        assert!(
            fast.abs_diff(*full) <= 1,
            "SDR identity byte {index}: fast {fast}, full transform {full}"
        );
    }
    for (pixel, words) in results[0]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(words.as_chunks::<4>().0)
    {
        let decoded = words.map(super::super::color::half_to_f32);
        let alpha = decoded[3].clamp(0., 1.);
        let expected: [u8; 4] = std::array::from_fn(|channel| {
            let value = if channel == 3 {
                alpha
            } else {
                decoded[channel].clamp(0., alpha)
            };
            (value * 255.).round() as u8
        });
        assert!(pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1));
    }
}

#[test]
#[ignore = "optional 4K final-output GPU timing; run serially outside desktop diagnosis"]
fn sdr_identity_4k_output_gpu_timing() {
    let mut renderer = renderer();
    let size = (3840, 2400).into();
    let scene = renderer.create_buffer(Fourcc::Abgr16161616f, size).unwrap();
    let target = renderer.create_buffer(Fourcc::Abgr8888, size).unwrap();
    let vk = renderer.vulkan();
    let mut clear = vk
        .render_frame(
            scene.image(),
            scene.image_view(),
            vk::Format::R16G16B16A16_SFLOAT,
            true,
            3840,
            2400,
            Some([0.1, 0.4, 0.8, 1.]),
        )
        .unwrap();
    clear.retain_target(&scene);
    clear.finish_and_wait().unwrap();
    let timings = super::super::timing::GpuTimings::new(vk.ctx.as_ref().unwrap().clone())
        .unwrap()
        .expect("4K microbenchmark requires real GPU timestamps");
    let mut samples: [Vec<f64>; 2] = std::array::from_fn(|_| Vec::new());
    for iteration in 0..40 {
        for (variant, force_full_transform) in [false, true].into_iter().enumerate() {
            let tag = super::super::timing::GpuTimingTag {
                output_generation: 1,
                redraw_sequence: (iteration * 2 + variant) as u64,
            };
            let timing = timings.acquire(tag).unwrap();
            render_sdr_output_frame(
                &renderer,
                &scene,
                &target,
                force_full_transform,
                Some(timing),
            );
            let mut measured = None;
            timings.drain(|sample| {
                assert_eq!(sample.tag, tag);
                measured = Some(sample.gpu_time.as_secs_f64() * 1000.);
            });
            let measured =
                measured.expect("completed output frame must publish its GPU timestamps");
            if iteration >= 8 {
                samples[variant].push(measured);
            }
        }
    }
    for (variant, mode) in ["identity", "full_color_transform"].into_iter().enumerate() {
        samples[variant].sort_by(f64::total_cmp);
        assert_eq!(samples[variant].len(), 32);
        eprintln!(
            "SDR_OUTPUT_GPU_TIMING {}",
            serde_json::json!({
                "mode": mode, "size": [3840,2400], "samples": 32,
                "gpu_p50_ms": samples[variant][16], "gpu_p95_ms": samples[variant][30],
                "scope": "GPU timestamp of identical FP16 attachment load and final SDR pass, excluding CPU/fence wait",
                "setup_and_warmup_excluded": true, "variants_interleaved": true,
            })
        );
    }
}

#[test]
#[ignore = "requires real DRM render node and Vulkan validation"]
fn hdr_mixed_pq_scrgb_sdr_and_wide_gamut_match_cpu_output() {
    let mut renderer = renderer();
    let output = HdrOutputSettings::default().description();
    let pq = SurfaceColor {
        description: ColorDescription::bt2020_pq(),
        intent: RenderingIntent::Perceptual,
    };
    let scrgb = SurfaceColor {
        description: ColorDescription::windows_scrgb(),
        intent: RenderingIntent::Perceptual,
    };
    let samples = [
        ([1., 1., 1.], SurfaceColor::default()),
        ([0.5, 0.25, 0.125], SurfaceColor::default()),
        ([0.1, 0.1, 0.1].map(crate::color::pq_encode), pq),
        ([203., 203., 203.].map(crate::color::pq_encode), pq),
        ([1000., 1000., 1000.].map(crate::color::pq_encode), pq),
        ([1000., 0., 0.].map(crate::color::pq_encode), pq),
        ([0., 1000., 0.].map(crate::color::pq_encode), pq),
        ([0., 0., 1000.].map(crate::color::pq_encode), pq),
        ([2.5375, 2.5375, 2.5375], scrgb),
        ([12.5, 12.5, 12.5], scrgb),
        ([12.5, 0.5, -0.1], scrgb),
    ];
    let (textures, expected): (Vec<_>, Vec<_>) = samples
        .into_iter()
        .map(|(rgb, color)| {
            let (texture, sampled) = source(&mut renderer, rgb, 1., color);
            (texture, crate::color::convert_color(sampled, color, output))
        })
        .unzip();
    let mut target = renderer
        .create_buffer(Fourcc::Abgr2101010, (textures.len() as i32, 1).into())
        .unwrap();
    draw(&mut renderer, &mut target, &textures, output, [0.; 4]);
    let actual = read(&mut renderer, &target);
    let mut maximum_near_black_error = 0.0_f32;
    for (i, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        for channel in 0..3 {
            // Saturated BT.2020 needs signed extended-sRGB intermediates.
            // FP16 quantization before the inverse gamut matrix can leave a
            // small residual in near-black components of saturated highlights.
            // Keep their absolute error bounded without weakening the code
            // tolerance for primary colors or any mid/high luminance channel.
            let code_error = (actual[channel] - expected[channel].clamp(0., 1.)).abs();
            let actual_nits = crate::color::pq_decode(actual[channel]);
            let expected_nits = crate::color::pq_decode(expected[channel]);
            let nits_error = (actual_nits - expected_nits).abs();
            if expected_nits <= 0.01 {
                maximum_near_black_error = maximum_near_black_error.max(nits_error);
            }
            if code_error > 3. / 1023. {
                eprintln!("FP16_NEAR_BLACK sample={i} channel={channel} actual_nits={actual_nits} expected_nits={expected_nits} error_nits={nits_error}");
            }
            assert!(
                code_error <= 3. / 1023. || expected_nits <= 0.01 && nits_error <= 0.1,
                "sample {i} channel {channel}: actual {actual:?}, expected {expected:?}"
            );
        }
    }
    eprintln!("FP16_NEAR_BLACK_MAX error_nits={maximum_near_black_error}");
    let white_nits = crate::color::pq_decode(actual[0][0]);
    let highlight_nits = crate::color::pq_decode(actual[4][0]);
    assert!((white_nits - 203.).abs() < 2.);
    assert!(
        highlight_nits > white_nits * 4.5,
        "HDR highlight must retain headroom above SDR white"
    );
    assert!(
        crate::color::pq_decode(actual[5][0]) > 970.,
        "BT.2020 red must survive the negative extended-sRGB working components"
    );
}

#[test]
#[ignore = "requires real DRM render node and Vulkan validation"]
fn hdr_content_on_sdr_is_tone_mapped_instead_of_clipped() {
    let mut renderer = renderer();
    let color = SurfaceColor {
        description: ColorDescription::windows_scrgb(),
        intent: RenderingIntent::Perceptual,
    };
    let levels = [0., 0.1, 0.5, 1., 2.5375, 5., 8., 12.5];
    let mut expected = Vec::new();
    let textures: Vec<_> = levels
        .into_iter()
        .map(|level| {
            let (texture, sampled) = source(&mut renderer, [level; 3], 1., color);
            expected.push(crate::color::convert_color(
                sampled,
                color,
                ColorDescription::srgb(),
            ));
            texture
        })
        .collect();
    let mut target = renderer
        .create_buffer(Fourcc::Abgr8888, (textures.len() as i32, 1).into())
        .unwrap();
    draw(
        &mut renderer,
        &mut target,
        &textures,
        ColorDescription::srgb(),
        [0.; 4],
    );
    let actual = read(&mut renderer, &target);
    for (actual, expected) in actual.iter().zip(expected) {
        for channel in 0..3 {
            assert!(
                (actual[channel] - expected[channel].clamp(0., 1.)).abs() <= 2. / 255.,
                "actual {actual:?}, expected {expected:?}"
            );
        }
    }
    assert!(
        actual[5][0] < actual[6][0] && actual[6][0] < actual[7][0],
        "different HDR highlights must not collapse to a clipped SDR white"
    );
}

#[test]
#[ignore = "requires real DRM render node and Vulkan validation"]
fn hdr_premultiplied_alpha_composes_in_extended_srgb_before_output_encoding() {
    let mut renderer = renderer();
    let output = HdrOutputSettings::default().description();
    let color = SurfaceColor {
        description: ColorDescription::windows_scrgb(),
        intent: RenderingIntent::Perceptual,
    };
    let (texture, sampled) = source(&mut renderer, [8., 3., 1.], 0.5, color);
    let scene = super::super::color::scene_description(Some(output));
    let foreground = crate::color::convert_color(sampled, color, scene);
    let mixed: [f32; 3] = std::array::from_fn(|i| foreground[i] * 0.5 + [0.2, 0.3, 0.4][i] * 0.5);
    let expected = crate::color::convert_color(
        mixed,
        SurfaceColor {
            description: scene,
            intent: RenderingIntent::Relative,
        },
        output,
    );
    let mut target = renderer
        .create_buffer(Fourcc::Abgr2101010, (1, 1).into())
        .unwrap();
    draw(
        &mut renderer,
        &mut target,
        &[texture],
        output,
        [0.2, 0.3, 0.4, 1.],
    );
    let actual = read(&mut renderer, &target)[0];
    for channel in 0..3 {
        assert!(
            (actual[channel] - expected[channel]).abs() <= 3. / 1023.,
            "actual {actual:?}, expected {expected:?}"
        );
    }
    assert_eq!(actual[3], 1.);
}

#[test]
#[ignore = "requires real DRM render node and Vulkan validation"]
fn hdr_snapshot_keeps_fp16_headroom_and_exports_tone_mapped_sdr() {
    let mut renderer = renderer();
    let output = HdrOutputSettings::default().description();
    let color = SurfaceColor {
        description: ColorDescription::windows_scrgb(),
        intent: RenderingIntent::Perceptual,
    };
    let (texture, _) = source(&mut renderer, [12.5; 3], 1., color);
    let mut target = renderer
        .create_buffer(Fourcc::Abgr2101010, (1, 1).into())
        .unwrap();
    let scope = renderer.output_color_scope(Some(output));
    let mut framebuffer = renderer.bind(&mut target).unwrap();
    let mut frame = renderer
        .render(&mut framebuffer, (1, 1).into(), Transform::Normal)
        .unwrap();
    let full = Rectangle::from_size((1, 1).into());
    frame.clear(Color32F::TRANSPARENT, &[full]).unwrap();
    frame
        .render_texture_from_to(
            &texture,
            Rectangle::from_size((1., 1.).into()),
            full,
            &[full],
            &[],
            Transform::Normal,
            1.,
        )
        .unwrap();
    let snapshot = frame
        .create_buffer(Fourcc::Abgr8888, (1, 1).into())
        .unwrap();
    assert_eq!(snapshot.format(), Some(Fourcc::Abgr16161616f));
    frame.capture_into(&snapshot, (0, 0).into()).unwrap();
    frame.finish().unwrap().wait().unwrap();
    drop(framebuffer);
    drop(scope);
    let raw = renderer
        .copy_texture(
            &snapshot,
            Rectangle::from_size((1, 1).into()),
            Fourcc::Abgr16161616f,
        )
        .unwrap();
    let raw = renderer.map_texture(&raw).unwrap();
    let encoded = super::super::color::half_to_f32(u16::from_le_bytes([raw[0], raw[1]]));
    assert!(
        encoded > 1.8,
        "HDR snapshot must retain floating-point headroom, got {encoded}"
    );
    let captured = renderer
        .copy_texture(
            &snapshot,
            Rectangle::from_size((1, 1).into()),
            Fourcc::Abgr8888,
        )
        .unwrap();
    let captured = renderer.map_texture(&captured).unwrap().to_vec();
    let mut sdr = renderer
        .create_buffer(Fourcc::Abgr8888, (1, 1).into())
        .unwrap();
    let mut framebuffer = renderer.bind(&mut sdr).unwrap();
    let mut frame = renderer
        .render(&mut framebuffer, (1, 1).into(), Transform::Normal)
        .unwrap();
    frame.clear(Color32F::TRANSPARENT, &[full]).unwrap();
    frame
        .render_texture_from_to(
            &snapshot,
            Rectangle::from_size((1., 1.).into()),
            full,
            &[full],
            &[],
            Transform::Normal,
            1.,
        )
        .unwrap();
    frame.finish().unwrap().wait().unwrap();
    drop(framebuffer);
    let rendered = renderer
        .copy_texture(&sdr, Rectangle::from_size((1, 1).into()), Fourcc::Abgr8888)
        .unwrap();
    let rendered = renderer.map_texture(&rendered).unwrap();
    assert!(
        rendered
            .iter()
            .zip(&captured)
            .all(|(a, b)| a.abs_diff(*b) <= 1),
        "GPU and screenshot CPU tone-map differ: GPU {rendered:?}, CPU {captured:?}"
    );
}

#[test]
#[ignore = "requires real DRM render node and Vulkan validation"]
fn live_swapchain_scenes_preserve_partial_alpha_damage_past_sixteen_buffers_and_resize() {
    let mut renderer = renderer();
    let output = HdrOutputSettings::default().description();
    let (red, _) = source(&mut renderer, [1., 0., 0.], 1., SurfaceColor::default());
    let (green, _) = source(&mut renderer, [0., 1., 0.], 1., SurfaceColor::default());
    let (blue, _) = source(&mut renderer, [0., 0., 1.], 0.5, SurfaceColor::default());
    let mut targets = Vec::new();
    for _ in 0..17 {
        let mut target = renderer
            .create_buffer(Fourcc::Abgr2101010, (2, 1).into())
            .unwrap();
        draw(
            &mut renderer,
            &mut target,
            &[red.clone(), green.clone()],
            output,
            [0.; 4],
        );
        targets.push(target);
    }
    assert_eq!(renderer.output_scene_count_for_test(), 17);
    let before = read(&mut renderer, &targets[0]);
    let scope = renderer.output_color_scope(Some(output));
    let mut framebuffer = renderer.bind(&mut targets[0]).unwrap();
    let mut frame = renderer
        .render(&mut framebuffer, (2, 1).into(), Transform::Normal)
        .unwrap();
    frame
        .render_texture_from_to(
            &blue,
            Rectangle::from_size((1., 1.).into()),
            Rectangle::from_size((1, 1).into()),
            &[Rectangle::from_size((1, 1).into())],
            &[],
            Transform::Normal,
            1.,
        )
        .unwrap();
    frame.finish().unwrap().wait().unwrap();
    drop(framebuffer);
    drop(scope);
    let actual = read(&mut renderer, &targets[0]);
    assert_eq!(
        actual[1], before[1],
        "the untouched pixel belongs to the exact retained framebuffer"
    );
    let expected = crate::color::convert_color([0.5, 0., 0.5], SurfaceColor::default(), output);
    for c in 0..3 {
        assert!(
            (actual[0][c] - expected[c]).abs() <= 3. / 1023.,
            "retained alpha blend lost history: actual {:?}, expected {expected:?}",
            actual[0]
        );
    }
    drop(targets);
    renderer.retire_completed().unwrap();
    let mut resized = renderer
        .create_buffer(Fourcc::Abgr2101010, (3, 1).into())
        .unwrap();
    draw(
        &mut renderer,
        &mut resized,
        &[red, green, blue],
        output,
        [0.; 4],
    );
    renderer.retire_completed().unwrap();
    assert_eq!(
        renderer.output_scene_count_for_test(),
        1,
        "dead resized framebuffers must release FP16 scenes"
    );
}

#[test]
#[ignore = "requires real DRM render node and Vulkan validation"]
fn first_managed_partial_repaint_recovers_initialized_sdr_framebuffer() {
    let mut renderer = renderer();
    let mut target = renderer
        .create_buffer(Fourcc::Abgr2101010, (2, 1).into())
        .unwrap();
    let full = Rectangle::from_size((2, 1).into());
    let mut framebuffer = renderer.bind(&mut target).unwrap();
    let mut frame = renderer
        .render(&mut framebuffer, (2, 1).into(), Transform::Normal)
        .unwrap();
    frame
        .clear(Color32F::new(0.6, 0.2, 0.1, 1.), &[full])
        .unwrap();
    frame.finish().unwrap().wait().unwrap();
    drop(framebuffer);
    let previous = read(&mut renderer, &target);
    let output = HdrOutputSettings::default().description();
    let (blue, _) = source(&mut renderer, [0., 0., 1.], 0.5, SurfaceColor::default());
    let scope = renderer.output_color_scope(Some(output));
    let mut framebuffer = renderer.bind(&mut target).unwrap();
    let mut frame = renderer
        .render(&mut framebuffer, (2, 1).into(), Transform::Normal)
        .unwrap();
    frame
        .render_texture_from_to(
            &blue,
            Rectangle::from_size((1., 1.).into()),
            Rectangle::from_size((1, 1).into()),
            &[Rectangle::from_size((1, 1).into())],
            &[],
            Transform::Normal,
            1.,
        )
        .unwrap();
    frame.finish().unwrap().wait().unwrap();
    drop(framebuffer);
    drop(scope);
    let actual = read(&mut renderer, &target);
    for i in 0..2 {
        let rgb: [f32; 3] = std::array::from_fn(|c| {
            if i == 0 {
                previous[i][c] * 0.5 + [0., 0., 0.5][c]
            } else {
                previous[i][c]
            }
        });
        let expected = crate::color::convert_color(rgb, SurfaceColor::default(), output);
        for c in 0..3 {
            assert!((actual[i][c]-expected[c]).abs() <= 3./1023.,"initial partial frame lost previous pixels: pixel{i} actual {:?}, expected {expected:?}",actual[i]);
        }
    }
}

#[test]
#[ignore = "requires real DRM render node and Vulkan validation"]
fn recycled_effect_and_blur_targets_follow_new_output_headroom_without_double_tonemapping() {
    let mut renderer = renderer();
    let old_output = HdrOutputSettings::default().description();
    let new_output = HdrOutputSettings {
        sdr_white: 250.,
        peak_luminance: 500.,
        max_fall: 250.,
        ..HdrOutputSettings::default()
    }
    .description();
    let old_scope = renderer.output_color_scope(Some(old_output));
    let old = renderer
        .create_buffer(Fourcc::Abgr8888, (2, 1).into())
        .unwrap();
    let old_image = old.image();
    let old_color = old.color;
    renderer.recycle_offscreen(old);
    drop(old_scope);
    let new_scope = renderer.output_color_scope(Some(new_output));
    let mut effect = renderer
        .create_buffer(Fourcc::Abgr8888, (2, 1).into())
        .unwrap();
    assert_eq!(
        effect.image(),
        old_image,
        "exercise the real cache checkout"
    );
    assert_ne!(effect.color, old_color);
    let mut framebuffer = renderer.bind(&mut effect).unwrap();
    let mut frame = renderer
        .render(&mut framebuffer, (2, 1).into(), Transform::Normal)
        .unwrap();
    frame
        .clear(
            Color32F::new(0.5, 0.5, 0.5, 1.),
            &[Rectangle::from_size((2, 1).into())],
        )
        .unwrap();
    frame.finish().unwrap().wait().unwrap();
    drop(framebuffer);
    drop(new_scope);
    let mut blur = super::super::blur::NativeBlur::default();
    let scope = renderer.output_color_scope(Some(old_output));
    let (old_blur, old_done) = blur
        .render(
            &mut renderer,
            &effect,
            crate::render_helpers::blur::BlurOptions {
                passes: 1,
                offset: 1.,
            },
        )
        .unwrap();
    old_done.wait().unwrap();
    let retained_color = old_blur.color;
    drop(scope);
    let scope = renderer.output_color_scope(Some(new_output));
    let (new_blur, done) = blur
        .render(
            &mut renderer,
            &effect,
            crate::render_helpers::blur::BlurOptions {
                passes: 1,
                offset: 1.,
            },
        )
        .unwrap();
    done.wait().unwrap();
    assert_eq!(
        old_blur.color, retained_color,
        "retained old snapshots keep their original description"
    );
    assert_eq!(
        new_blur.color.description,
        renderer.scene_color_description()
    );
    drop(scope);
    let mut target = renderer
        .create_buffer(Fourcc::Abgr2101010, (1, 1).into())
        .unwrap();
    // Use a one-pixel view of the constant blur output.
    let scope = renderer.output_color_scope(Some(new_output));
    let mut framebuffer = renderer.bind(&mut target).unwrap();
    let mut frame = renderer
        .render(&mut framebuffer, (1, 1).into(), Transform::Normal)
        .unwrap();
    frame
        .clear(
            Color32F::TRANSPARENT,
            &[Rectangle::from_size((1, 1).into())],
        )
        .unwrap();
    frame
        .render_texture_from_to(
            &new_blur,
            Rectangle::from_size((2., 1.).into()),
            Rectangle::from_size((1, 1).into()),
            &[Rectangle::from_size((1, 1).into())],
            &[],
            Transform::Normal,
            1.,
        )
        .unwrap();
    frame.finish().unwrap().wait().unwrap();
    drop(framebuffer);
    drop(scope);
    let actual = read(&mut renderer, &target)[0];
    let expected = crate::color::convert_color([0.5; 3], SurfaceColor::default(), new_output);
    for c in 0..3 {
        assert!((actual[c]-expected[c]).abs() <= 3./1023.,"reused effects must not be tone-mapped twice: actual {actual:?}, expected {expected:?}");
    }
}

#[test]
#[ignore = "requires real DRM render node and Vulkan validation"]
fn absolute_intent_and_perceptual_blackpoint_match_direct_cpu_conversion() {
    let mut renderer = renderer();
    for (sdr_white, min_luminance) in [(203., 0.005), (300., 0.005), (203., 0.), (300., 0.)] {
        let output = HdrOutputSettings {
            sdr_white,
            min_luminance,
            ..HdrOutputSettings::default()
        }
        .description();
        let parametric_srgb = ColorDescription {
            min_luminance: 0.2,
            linear_scale: 79.8,
            max_luminance: 400.,
            max_fall: 80.,
            ..ColorDescription::srgb()
        };
        let mut samples = Vec::new();
        for description in [
            parametric_srgb,
            ColorDescription::bt2020_pq(),
            ColorDescription::windows_scrgb(),
        ] {
            for intent in [
                RenderingIntent::Perceptual,
                RenderingIntent::Relative,
                RenderingIntent::Absolute,
            ] {
                samples.push((
                    [description.encode_channel(100.); 3],
                    SurfaceColor {
                        description,
                        intent,
                    },
                ));
            }
        }
        let parametric_sdr = ColorDescription {
            max_luminance: 80.,
            ..parametric_srgb
        };
        samples.push((
            [0.; 3],
            SurfaceColor {
                description: parametric_sdr,
                intent: RenderingIntent::Perceptual,
            },
        ));
        samples.push((
            [1.; 3],
            SurfaceColor {
                description: parametric_sdr,
                intent: RenderingIntent::Perceptual,
            },
        ));
        samples.push((
            [1.; 3],
            SurfaceColor {
                description: ColorDescription::srgb(),
                intent: RenderingIntent::Absolute,
            },
        ));
        let (textures, expected): (Vec<_>, Vec<_>) = samples
            .into_iter()
            .map(|(rgb, color)| {
                let (texture, sampled) = source(&mut renderer, rgb, 1., color);
                (texture, crate::color::convert_color(sampled, color, output))
            })
            .unzip();
        let mut target = renderer
            .create_buffer(Fourcc::Abgr2101010, (textures.len() as i32, 1).into())
            .unwrap();
        draw(&mut renderer, &mut target, &textures, output, [0.; 4]);
        let actual = read(&mut renderer, &target);
        for (i, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            for c in 0..3 {
                assert!(
                    (actual[c] - expected[c]).abs() <= 3. / 1023.,
                    "SDR white{sdr_white}, sample{i}: actual {actual:?}, expected {expected:?}"
                );
            }
        }
        assert!(
            (crate::color::pq_decode(actual[2][0]) - 100.).abs() < 1.5,
            "absolute 100 nits must not be multiplied by SDR white"
        );
        assert!(
            (crate::color::pq_decode(actual[9][0]) - output.min_luminance).abs() < 0.001,
            "perceptual black must map to the output physical black point"
        );
        assert!(
            (crate::color::pq_decode(actual[10][0]) - sdr_white).abs() < 2.,
            "parametric SDR white must map to the output reference"
        );
        assert!(
            (crate::color::pq_decode(actual[11][0]) - 80.).abs() < 1.,
            "absolute sRGB80 must remain 80 nits even when zero black makes its encoding match the working scene"
        );
    }
}

#[test]
#[ignore = "requires real DRM render node and Vulkan validation"]
fn live_dmabuf_swapchain_imports_preserve_hdr_history_past_sixteen_buffers() {
    let (vulkan, gles, mut allocator) = setup();
    let code = Fourcc::Argb2101010;
    let modifiers = super::modifiers(&vulkan, &gles, code);
    let mut renderer = NativeRenderer::new(vulkan).unwrap();
    renderer.prepare_output_format_for_test(code).unwrap();
    let output = HdrOutputSettings::default().description();
    let mut buffers = Vec::new();
    let mut dmabufs = Vec::new();
    let mut untouched_reference = None;
    for index in 0..17 {
        let buffer = allocator.create_buffer(2, 1, code, &modifiers).unwrap();
        let mut dmabuf = buffer.export().unwrap();
        let scope = renderer.output_color_scope(Some(output));
        let mut framebuffer = renderer.bind(&mut dmabuf).unwrap();
        let mut frame = renderer
            .render(&mut framebuffer, (2, 1).into(), Transform::Normal)
            .unwrap();
        frame
            .clear(
                Color32F::new(1., 0., 0., 1.),
                &[Rectangle::from_size((2, 1).into())],
            )
            .unwrap();
        frame.finish().unwrap().wait().unwrap();
        if index == 0 {
            let mapping = renderer
                .copy_framebuffer(
                    &framebuffer,
                    Rectangle::from_size((2, 1).into()),
                    Fourcc::Abgr2101010,
                )
                .unwrap();
            untouched_reference = Some(u32::from_le_bytes(
                renderer.map_texture(&mapping).unwrap().as_chunks::<4>().0[1],
            ));
        }
        drop(framebuffer);
        drop(scope);
        buffers.push(buffer);
        dmabufs.push(dmabuf);
    }
    assert_eq!(
        renderer.target_cache_count_for_test(),
        17,
        "live KMS imports cannot be discarded by an LRU count"
    );
    assert_eq!(renderer.output_scene_count_for_test(), 17);
    // An ephemeral export of the same allocation must not replace the weak
    // lifetime of its still-live swapchain slot and discard that slot's scene.
    let mut reexported = buffers[0].export().unwrap();
    drop(renderer.bind(&mut reexported).unwrap());
    drop(reexported);
    drop(renderer.bind(&mut dmabufs[1]).unwrap());
    assert_eq!(
        renderer.target_cache_count_for_test(),
        17,
        "dropping a temporary wrapper must preserve another live owner of the same allocation"
    );
    let (blue, _) = source(&mut renderer, [0., 0., 1.], 0.5, SurfaceColor::default());
    let scope = renderer.output_color_scope(Some(output));
    let mut framebuffer = renderer.bind(&mut dmabufs[0]).unwrap();
    let mut frame = renderer
        .render(&mut framebuffer, (2, 1).into(), Transform::Normal)
        .unwrap();
    frame
        .render_texture_from_to(
            &blue,
            Rectangle::from_size((1., 1.).into()),
            Rectangle::from_size((1, 1).into()),
            &[Rectangle::from_size((1, 1).into())],
            &[],
            Transform::Normal,
            1.,
        )
        .unwrap();
    frame.finish().unwrap().wait().unwrap();
    let mapping = renderer
        .copy_framebuffer(
            &framebuffer,
            Rectangle::from_size((2, 1).into()),
            Fourcc::Abgr2101010,
        )
        .unwrap();
    let words = renderer.map_texture(&mapping).unwrap().as_chunks::<4>().0;
    let first = u32::from_le_bytes(words[0]);
    let actual = [
        (first & 1023) as f32 / 1023.,
        ((first >> 10) & 1023) as f32 / 1023.,
        ((first >> 20) & 1023) as f32 / 1023.,
    ];
    let expected = crate::color::convert_color([0.5, 0., 0.5], SurfaceColor::default(), output);
    for c in 0..3 {
        assert!((actual[c]-expected[c]).abs() <= 3./1023.,"a live DMA-BUF lost its retained FP16 blend history: actual {actual:?}, expected {expected:?}");
    }
    let second = u32::from_le_bytes(words[1]);
    assert_eq!(
        second,
        untouched_reference.unwrap(),
        "partial damage must preserve the undamaged encoded output byte for byte"
    );
    drop(framebuffer);
    drop(scope);
    drop(dmabufs);
    drop(buffers);
    renderer.retire_completed().unwrap();
    let buffer = allocator.create_buffer(3, 1, code, &modifiers).unwrap();
    let mut fresh = buffer.export().unwrap();
    let scope = renderer.output_color_scope(Some(output));
    let mut framebuffer = renderer.bind(&mut fresh).unwrap();
    let mut frame = renderer
        .render(&mut framebuffer, (3, 1).into(), Transform::Normal)
        .unwrap();
    frame
        .clear(
            Color32F::new(0., 1., 0., 1.),
            &[Rectangle::from_size((3, 1).into())],
        )
        .unwrap();
    frame.finish().unwrap().wait().unwrap();
    drop(framebuffer);
    drop(scope);
    assert_eq!(
        renderer.target_cache_count_for_test(),
        1,
        "dead swapchain slots release imported targets"
    );
    assert_eq!(
        renderer.output_scene_count_for_test(),
        1,
        "dead swapchain slots release FP16 scenes"
    );
}
