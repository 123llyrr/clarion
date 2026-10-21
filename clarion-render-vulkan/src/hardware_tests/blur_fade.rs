//! Independent convolution/coverage oracles and GL resource/ownership contracts.
use super::*;
use crate::render_helpers::blur::{Blur, BlurOptions};
use crate::render_helpers::gradient_fade_texture::{
    GradientFadeTextureRenderElement, VulkanGradientFadeTextureRenderElement,
};
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::{render_to_vec, resources, shaders};
use smithay::backend::renderer::element::{Element, Kind};
use smithay::backend::renderer::gles::{ffi, GlesError};
use smithay::backend::renderer::{DebugFlags, ImportMem, Texture};

fn bilinear(bytes: &[u8], size: [usize; 2], uv: [f64; 2]) -> [f64; 4] {
    let at = [uv[0] * size[0] as f64 - 0.5, uv[1] * size[1] as f64 - 0.5];
    let base = [at[0].floor(), at[1].floor()];
    let phase = [at[0] - base[0], at[1] - base[1]];
    let mut color = [0.; 4];
    for y in 0..2 {
        for x in 0..2 {
            let ix = (base[0] + x as f64).clamp(0., (size[0] - 1) as f64) as usize;
            let iy = (base[1] + y as f64).clamp(0., (size[1] - 1) as f64) as usize;
            let weight = if x == 0 { 1. - phase[0] } else { phase[0] }
                * if y == 0 { 1. - phase[1] } else { phase[1] };
            for channel in 0..4 {
                color[channel] += f64::from(bytes[(iy * size[0] + ix) * 4 + channel]) * weight;
            }
        }
    }
    color
}

// Discrete probability masses, evaluated at pixel centers with an independent
// f64 bilinear sampler. Every render target is quantized back to UNORM8.
fn convolution(
    bytes: &[u8],
    input: [usize; 2],
    output: [usize; 2],
    up: bool,
    offset: f64,
) -> Vec<u8> {
    let down = [
        (0., 0., 4.),
        (-1., -1., 1.),
        (1., -1., 1.),
        (-1., 1., 1.),
        (1., 1., 1.),
    ];
    let upward = [
        (-2., 0., 1.),
        (2., 0., 1.),
        (0., -2., 1.),
        (0., 2., 1.),
        (-1., 1., 2.),
        (1., 1., 2.),
        (-1., -1., 2.),
        (1., -1., 2.),
    ];
    let masses = if up { &upward[..] } else { &down[..] };
    let footprint = if up { input } else { output };
    let mut result = Vec::with_capacity(output[0] * output[1] * 4);
    for y in 0..output[1] {
        for x in 0..output[0] {
            let mut value = [0.; 4];
            for &(dx, dy, mass) in masses {
                let uv = [
                    (x as f64 + 0.5) / output[0] as f64 + dx * 0.5 * offset / footprint[0] as f64,
                    (y as f64 + 0.5) / output[1] as f64 + dy * 0.5 * offset / footprint[1] as f64,
                ];
                for (channel, sample) in value.iter_mut().zip(bilinear(bytes, input, uv)) {
                    *channel += sample * mass;
                }
            }
            result.extend(
                value.map(|v| (v / if up { 12. } else { 8. }).round().clamp(0., 255.) as u8),
            );
        }
    }
    result
}
fn oracle(source: &[u8], size: [usize; 2], options: BlurOptions) -> Vec<u8> {
    let mut levels = vec![size];
    for _ in 0..options.passes.clamp(1, 31) {
        let [w, h] = *levels.last().unwrap();
        levels.push([(w / 2).max(1), (h / 2).max(1)]);
    }
    let mut image = source.to_vec();
    for pair in levels.windows(2) {
        image = convolution(
            &image,
            pair[0],
            pair[1],
            false,
            f64::from(options.offset as f32),
        );
    }
    for pair in levels.windows(2).rev() {
        image = convolution(
            &image,
            pair[1],
            pair[0],
            true,
            f64::from(options.offset as f32),
        );
    }
    image
}

#[test]
#[ignore = "requires selected DRM node, GLES, and Vulkan discovery"]
fn gles_blur_matches_independent_bilinear_oracle_and_reuses_borrowed_output_scratch() {
    let (_, mut gles, _) = setup();
    resources::init(&mut gles);
    shaders::init(&mut gles);
    // Destroying a shared bank handle must not delete names still held by it.
    shaders::Shaders::get(&mut gles)
        .blur
        .clone()
        .unwrap()
        .destroy(&mut gles)
        .unwrap();
    let mut blur = Blur::new(&mut gles).unwrap();
    for (w, h) in [(37, 23), (1, 9), (1, 1)] {
        let bytes: Vec<_> = (0..w * h)
            .flat_map(|i| {
                let a = 128 + (i * 37 % 128) as u8;
                [a / 2, (i * 17 % u32::from(a)) as u8, a / 4, a]
            })
            .collect();
        let source = gles
            .import_memory(&bytes, Fourcc::Abgr8888, (w as i32, h as i32).into(), false)
            .unwrap();
        for passes in [1, 3, 31] {
            for offset in [0., 1.25, 10.] {
                let options = BlurOptions { passes, offset };
                blur.prepare_textures(
                    |format, size| gles.create_buffer(format, size),
                    &source,
                    options,
                )
                .unwrap();
                let output = blur.render(&mut gles, &source, options).unwrap();
                let mapping = gles
                    .copy_texture(
                        &output,
                        Rectangle::from_size(output.size()),
                        Fourcc::Abgr8888,
                    )
                    .unwrap();
                let actual = gles.map_texture(&mapping).unwrap().to_vec();
                let expected = oracle(&bytes, [w as usize, h as usize], options);
                let worst = actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                assert!(
                    worst <= 3,
                    "{w}x{h} passes={passes} offset={offset}: oracle difference {worst}"
                );
                let mut allocations = 0;
                blur.prepare_textures(
                    |format, size| {
                        allocations += 1;
                        gles.create_buffer(format, size)
                    },
                    &source,
                    options,
                )
                .unwrap();
                assert_eq!(allocations, 1, "only borrowed output may be replaced");
                let next = blur.render(&mut gles, &source, options).unwrap();
                let mapping = gles
                    .copy_texture(
                        &output,
                        Rectangle::from_size(output.size()),
                        Fourcc::Abgr8888,
                    )
                    .unwrap();
                assert_eq!(
                    gles.map_texture(&mapping).unwrap(),
                    &actual,
                    "published output was overwritten"
                );
                drop(next);
            }
        }
    }
}

#[test]
#[ignore = "requires selected DRM node, GLES, and Vulkan discovery"]
fn gles_blur_failure_invalidates_preparation_and_restores_gl_bindings() {
    let (_, mut gles, _) = setup();
    shaders::init(&mut gles);
    let source = gles
        .import_memory(
            &[128; 64 * 32 * 4],
            Fourcc::Abgr8888,
            (64, 32).into(),
            false,
        )
        .unwrap();
    let mut blur = Blur::new(&mut gles).unwrap();
    let options = BlurOptions {
        passes: 3,
        offset: 1.25,
    };
    blur.prepare_textures(
        |format, size| gles.create_buffer(format, size),
        &source,
        options,
    )
    .unwrap();
    assert!(blur
        .prepare_textures(
            |_, _| Err(GlesError::FramebufferBindingError),
            &source,
            BlurOptions {
                passes: 4,
                ..options
            }
        )
        .is_err());
    assert!(blur.render(&mut gles, &source, options).is_err());
    blur.prepare_textures(
        |format, size| gles.create_buffer(format, size),
        &source,
        options,
    )
    .unwrap();
    let fbo = gles
        .with_context(|gl| unsafe {
            let mut name = 0;
            gl.GenFramebuffers(1, &mut name);
            gl.BindFramebuffer(ffi::FRAMEBUFFER, name);
            gl.Viewport(2, 3, 17, 19);
            gl.Enable(ffi::BLEND);
            gl.Enable(ffi::SCISSOR_TEST);
            gl.ActiveTexture(ffi::TEXTURE7);
            name
        })
        .unwrap();
    drop(blur.render(&mut gles, &source, options).unwrap());
    gles.with_context(|gl| unsafe {
        let mut binding = 0;
        let mut viewport = [0; 4];
        let mut active = 0;
        gl.GetIntegerv(ffi::FRAMEBUFFER_BINDING, &mut binding);
        gl.GetIntegerv(ffi::VIEWPORT, viewport.as_mut_ptr());
        gl.GetIntegerv(ffi::ACTIVE_TEXTURE, &mut active);
        assert_eq!(binding as u32, fbo);
        assert_eq!(viewport, [2, 3, 17, 19]);
        assert_eq!(active as u32, ffi::TEXTURE7);
        assert_eq!(gl.IsEnabled(ffi::BLEND), ffi::TRUE);
        assert_eq!(gl.IsEnabled(ffi::SCISSOR_TEST), ffi::TRUE);
        gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
        gl.DeleteFramebuffers(1, &fbo);
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.Disable(ffi::SCISSOR_TEST);
    })
    .unwrap();
}

#[test]
#[ignore = "requires selected DRM node, GLES and Vulkan"]
fn gradient_fade_pixels_follow_coverage_and_no_faded_pixel_claims_opacity() {
    let (vk, mut gles, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    resources::init(&mut gles);
    shaders::init(&mut gles);
    for format in [Fourcc::Abgr8888, Fourcc::Xbgr8888] {
        let raw: Vec<_> = (0..64 * 4).flat_map(|_| [200, 100, 50, 255]).collect();
        let gl_tex = gles
            .import_memory(&raw, format, (64, 4).into(), false)
            .unwrap();
        let vk_tex = native
            .import_memory(&raw, format, (64, 4).into(), false)
            .unwrap();
        let gl_buffer = TextureBuffer::from_texture(
            &gles,
            gl_tex,
            1.,
            Transform::Normal,
            vec![Rectangle::from_size((64, 4).into())],
        );
        let vk_buffer = TextureBuffer::from_texture(
            &native,
            vk_tex,
            1.,
            Transform::Normal,
            vec![Rectangle::from_size((64, 4).into())],
        );
        let gl_inner = TextureRenderElement::from_texture_buffer(
            gl_buffer,
            (0., 0.),
            1.,
            Some(Rectangle::from_size((32., 4.).into())),
            None,
            Kind::Unspecified,
        );
        let vk_inner = TextureRenderElement::from_texture_buffer(
            vk_buffer,
            (0., 0.),
            1.,
            Some(Rectangle::from_size((32., 4.).into())),
            None,
            Kind::Unspecified,
        );
        assert!(!gl_inner.opaque_regions(1.0.into()).is_empty());
        let gl_element = GradientFadeTextureRenderElement::new(
            gl_inner,
            GradientFadeTextureRenderElement::shader(&mut gles).unwrap(),
        );
        let vk_element = VulkanGradientFadeTextureRenderElement::new(vk_inner);
        assert!(gl_element.opaque_regions(1.0.into()).is_empty());
        assert!(vk_element.opaque_regions(1.0.into()).is_empty());
        let vk_pixels = render_to_vec(
            &mut native,
            (32, 4).into(),
            1.0.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            [&vk_element].into_iter(),
        )
        .unwrap();
        for debug in [false, true] {
            gles.set_debug_flags(if debug {
                DebugFlags::TINT
            } else {
                DebugFlags::empty()
            });
            let gl_pixels = render_to_vec(
                &mut gles,
                (32, 4).into(),
                1.0.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                [&gl_element].into_iter(),
            )
            .unwrap();
            for (index, pixel) in gl_pixels.as_chunks::<4>().0.iter().enumerate() {
                let x = index % 32;
                let coverage = ((32. - x as f64 - 0.5) / 18.).clamp(0., 1.);
                let mut expected = [200., 100., 50., 255.].map(|value| value * coverage);
                if debug {
                    for channel in &mut expected {
                        *channel *= 0.8;
                    }
                    expected[1] += 51.;
                    expected[3] += 51.;
                }
                assert!(
                    pixel
                        .iter()
                        .zip(expected)
                        .all(|(a, b)| (f64::from(*a) - b).abs() <= 1.1),
                    "fade x={x} debug={debug}: {pixel:?}"
                );
                if !debug {
                    assert!(pixel
                        .iter()
                        .zip(&vk_pixels[index * 4..][..4])
                        .all(|(a, b)| a.abs_diff(*b) <= 1));
                }
            }
        }
        gles.set_debug_flags(DebugFlags::empty());
    }
}

#[test]
#[ignore = "requires selected DRM node and two shared EGL contexts"]
fn blur_shared_program_bank_keeps_framebuffers_local_to_each_egl_context() {
    use smithay::backend::egl::EGLContext;
    let (_, mut first, _) = setup();
    shaders::init(&mut first);
    let source = first
        .import_memory(
            &[64, 32, 16, 128].repeat(16),
            Fourcc::Abgr8888,
            (4, 4).into(),
            false,
        )
        .unwrap();
    let mut blur = Blur::new(&mut first).unwrap();
    let options = BlurOptions {
        passes: 3,
        offset: 1.25,
    };
    blur.prepare_textures(
        |format, size| first.create_buffer(format, size),
        &source,
        options,
    )
    .unwrap();
    drop(blur.render(&mut first, &source, options).unwrap());
    let egl = EGLContext::new_shared(first.egl_context().display(), first.egl_context()).unwrap();
    let mut second = unsafe { GlesRenderer::new(egl).unwrap() };
    assert_eq!(
        first.context_id(),
        second.context_id(),
        "texture/program share identity"
    );
    shaders::init(&mut second); // Shared user_data must reuse only shareable objects.
    let marker: smithay::backend::renderer::gles::GlesTexture = second
        .create_buffer(Fourcc::Abgr8888, (4, 4).into())
        .unwrap();
    let mut sentinels = [0; 16];
    second
        .with_context(|gl| unsafe {
            gl.GenFramebuffers(sentinels.len() as i32, sentinels.as_mut_ptr());
            for name in sentinels {
                gl.BindFramebuffer(ffi::FRAMEBUFFER, name);
                gl.FramebufferTexture2D(
                    ffi::FRAMEBUFFER,
                    ffi::COLOR_ATTACHMENT0,
                    ffi::TEXTURE_2D,
                    marker.tex_id(),
                    0,
                );
            }
        })
        .unwrap();
    // The same Blur can operate in either texture-compatible context. Its
    // scratch textures/programs share, but it may never reuse an FBO name from
    // the first context and overwrite an unrelated object in the second.
    blur.prepare_textures(
        |format, size| second.create_buffer(format, size),
        &source,
        options,
    )
    .unwrap();
    let output = blur.render(&mut second, &source, options).unwrap();
    second
        .with_context(|gl| unsafe {
            let mut current = 0;
            gl.GetIntegerv(ffi::FRAMEBUFFER_BINDING, &mut current);
            assert_eq!(current as u32, *sentinels.last().unwrap());
            for name in sentinels {
                gl.BindFramebuffer(ffi::FRAMEBUFFER, name);
                let mut attached = 0;
                gl.GetFramebufferAttachmentParameteriv(
                    ffi::FRAMEBUFFER,
                    ffi::COLOR_ATTACHMENT0,
                    ffi::FRAMEBUFFER_ATTACHMENT_OBJECT_NAME,
                    &mut attached,
                );
                assert_eq!(
                    attached as u32,
                    marker.tex_id(),
                    "foreign context FBO {name} was changed"
                );
            }
            gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
            gl.DeleteFramebuffers(sentinels.len() as i32, sentinels.as_ptr());
        })
        .unwrap();
    let mapping = second
        .copy_texture(
            &output,
            Rectangle::from_size(output.size()),
            Fourcc::Abgr8888,
        )
        .unwrap();
    assert!(second
        .map_texture(&mapping)
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .all(|pixel| *pixel == [64, 32, 16, 128]));
}
