//! Exercise the actual shared GLSL kernel on both renderers, without a display.
use super::*;
use crate::render_helpers::shader_element::ShaderRenderElement;
use crate::render_helpers::shaders::{animation_source, mat3_uniform, ProgramType};
use crate::render_helpers::{render_to_vec, resources, shaders};
use smithay::backend::renderer::element::{Kind, RenderElement};
use smithay::backend::renderer::gles::Uniform;
use smithay::backend::renderer::{ImportMem, RendererSuper, Texture};
use std::{collections::HashMap, rc::Rc};
mod oracle;
const KERNEL: &str = include_str!("../../shaders/postprocess_kernel.glsl");
fn literal(values: &[f32]) -> String {
    values
        .iter()
        .map(|v| format!("{v:.9}"))
        .collect::<Vec<_>>()
        .join(",")
}
fn source(rgb: [f32; 3], background: [f32; 4], count: bool) -> String {
    let kernel = if count {
        KERNEL.replace(
            "clarion_pixel_grain(gl_FragCoord.xy)",
            "counted_grain(gl_FragCoord.xy)",
        )
    } else {
        KERNEL.to_owned()
    };
    let instrumentation = if count {
        "float hash_calls=0.0; float counted_grain(vec2 p);"
    } else {
        ""
    };
    let hook = if count {
        "float counted_grain(vec2 p) { hash_calls+=1.0; return clarion_pixel_grain(p); }"
    } else {
        ""
    };
    let output = if count {
        "vec4(hash_calls*0.25,0.0,0.0,1.0)"
    } else {
        "(result+vec4(16.0))/32.0"
    };
    format!("#define CLARION_POST_NOISE clarion_random_seed\n#define CLARION_POST_SATURATION clarion_clamped_progress\n#define CLARION_POST_BACKGROUND vec4({})\n{instrumentation}\n{kernel}\n{hook}\nvec4 close_color(vec3 point,vec3 size) {{ vec4 result=postprocess(vec4(vec3({}),clarion_progress)); return {output}; }}", literal(&background), literal(&rgb))
}
type Uniforms = Rc<[Uniform<'static>]>;
fn exercise<R, F>(renderer: &mut R, rgb: [f32; 3], background: [f32; 4], count: bool, mut make: F)
where
    R: Renderer
        + ExportMem
        + Offscreen<<R as RendererSuper>::TextureId>
        + Bind<<R as RendererSuper>::TextureId>,
    R::Error: Send + Sync + 'static,
    ShaderRenderElement: RenderElement<R>,
    for<'a> R::Framebuffer<'a>: Texture,
    F: FnMut(&mut R, Uniforms) -> ShaderRenderElement,
{
    for alpha in oracle::ALPHAS {
        for saturation in oracle::SATURATIONS {
            for noise in oracle::NOISES {
                let uniforms = Rc::from(vec![
                    mat3_uniform("clarion_input_to_geo", glam::Mat3::IDENTITY),
                    mat3_uniform("clarion_geo_to_tex", glam::Mat3::IDENTITY),
                    Uniform::new("clarion_geo_size", [1f32, 1.]),
                    Uniform::new("clarion_progress", alpha),
                    Uniform::new("clarion_clamped_progress", saturation),
                    Uniform::new("clarion_random_seed", noise),
                ]);
                let element = make(renderer, uniforms);
                let pixels = render_to_vec(
                    renderer,
                    (1, 1).into(),
                    1.0.into(),
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    std::iter::once(&element),
                )
                .unwrap();
                let expected = if count {
                    let calls = noise > 0. && alpha != 0.;
                    [if calls { 64 } else { 0 }, 0, 0, 255]
                } else {
                    oracle::bytes(oracle::evaluate(rgb, alpha, saturation, noise, background))
                };
                for (channel, (&actual, expected)) in pixels.iter().zip(expected).enumerate() {
                    assert!(actual.abs_diff(expected)<=1,"rgb={rgb:?} bg={background:?} alpha={alpha} saturation={saturation} noise={noise} counter={count} channel={channel}: {actual} vs {expected}");
                }
            }
        }
    }
}
#[test]
#[ignore = "requires selected DRM render node, GLES, Vulkan and shader compiler; no display master"]
fn shared_postprocess_preserves_extended_domains_and_skips_only_zero_noise_terms() {
    let (vk, mut gles, _allocator) = setup();
    resources::init(&mut gles);
    shaders::init(&mut gles);
    let mut native = native::NativeRenderer::new(vk).unwrap();
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    // The public Close ABI declares a texture even for a constant-color hook.
    // Native descriptor admission requires an actual binding, independently
    // of whether the compiler optimizes its sample away.
    let gles_abi_texture = gles
        .import_memory(&[0; 4], Fourcc::Abgr8888, (1, 1).into(), false)
        .unwrap();
    let native_abi_texture = native
        .import_memory(&[0; 4], Fourcc::Abgr8888, (1, 1).into(), false)
        .unwrap();
    for background in oracle::BACKGROUNDS {
        for rgb in oracle::COLORS {
            for count in [false, true] {
                let source = source(rgb, background, count);
                shaders::set_custom_close_program(&mut gles, Some(&source));
                exercise(&mut gles, rgb, background, count, |_, uniforms| {
                    ShaderRenderElement::new(
                        ProgramType::Close,
                        (1., 1.).into(),
                        None,
                        1.,
                        1.,
                        uniforms,
                        HashMap::from([("clarion_tex".to_owned(), gles_abi_texture.clone())]),
                        Kind::Unspecified,
                    )
                });
                let source = animation_source(ProgramType::Close, Some(&source));
                native
                    .wait_for_animation_program_for_test(ProgramType::Close, &source)
                    .unwrap();
                exercise(&mut native, rgb, background, count, |renderer, uniforms| {
                    ShaderRenderElement::new_native(
                        renderer,
                        ProgramType::Close,
                        &source,
                        (1., 1.).into(),
                        1.,
                        1.,
                        uniforms,
                        HashMap::from([("clarion_tex".to_owned(), native_abi_texture.clone())]),
                        Kind::Unspecified,
                    )
                    .unwrap()
                });
            }
        }
    }
    shaders::set_custom_close_program(&mut gles, None);
}

#[test]
#[ignore = "requires selected DRM render node, GLES, Vulkan and shader compiler; no display master"]
fn shared_pixel_grain_matches_every_integer_oracle_level_on_both_backends() {
    let (vk, mut gles, _allocator) = setup();
    resources::init(&mut gles);
    shaders::init(&mut gles);
    let mut native = native::NativeRenderer::new(vk).unwrap();
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    let source = format!(
        "#define CLARION_POST_NOISE 0.0\n#define CLARION_POST_SATURATION 1.0\n#define CLARION_POST_BACKGROUND vec4(0.0)\n{}\nvec4 close_color(vec3 point, vec3 size) {{ return vec4(vec3(clarion_pixel_grain(gl_FragCoord.xy + vec2(clarion_progress, clarion_clamped_progress))), 1.0); }}",
        KERNEL
    );
    let gles_texture = gles
        .import_memory(&[0; 4], Fourcc::Abgr8888, (1, 1).into(), false)
        .unwrap();
    let native_texture = native
        .import_memory(&[0; 4], Fourcc::Abgr8888, (1, 1).into(), false)
        .unwrap();
    let uniforms = |offset: [i32; 2]| -> Uniforms {
        Rc::from(vec![
            mat3_uniform("clarion_input_to_geo", glam::Mat3::IDENTITY),
            mat3_uniform("clarion_geo_to_tex", glam::Mat3::IDENTITY),
            Uniform::new("clarion_geo_size", [251f32; 2]),
            Uniform::new("clarion_progress", offset[0] as f32),
            Uniform::new("clarion_clamped_progress", offset[1] as f32),
            Uniform::new("clarion_random_seed", 0f32),
        ])
    };
    shaders::set_custom_close_program(&mut gles, Some(&source));
    let native_source = animation_source(ProgramType::Close, Some(&source));
    native
        .wait_for_animation_program_for_test(ProgramType::Close, &native_source)
        .unwrap();
    for offset in [[0, 0], [-252, -1], [251, 251], [32768, 16384]] {
        let element = ShaderRenderElement::new(
            ProgramType::Close,
            (251., 251.).into(),
            None,
            1.,
            1.,
            uniforms(offset),
            HashMap::from([("clarion_tex".to_owned(), gles_texture.clone())]),
            Kind::Unspecified,
        );
        let gles_pixels = render_to_vec(
            &mut gles,
            (251, 251).into(),
            1.0.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            std::iter::once(&element),
        )
        .unwrap();
        let element = ShaderRenderElement::new_native(
            &mut native,
            ProgramType::Close,
            &native_source,
            (251., 251.).into(),
            1.,
            1.,
            uniforms(offset),
            HashMap::from([("clarion_tex".to_owned(), native_texture.clone())]),
            Kind::Unspecified,
        )
        .unwrap();
        let native_pixels = render_to_vec(
            &mut native,
            (251, 251).into(),
            1.0.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            std::iter::once(&element),
        )
        .unwrap();
        for (backend, pixels) in [("GLES", gles_pixels), ("Native", native_pixels)] {
            assert_eq!(pixels.len(), 251 * 251 * 4);
            for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
                let x = (index % 251) as i32 + offset[0];
                let y = (index / 251) as i32 + offset[1];
                let expected = (oracle::grain(x, y) * 255.).round() as u8;
                for (channel, actual) in pixel[..3].iter().enumerate() {
                    assert!(
                        actual.abs_diff(expected) <= 1,
                        "{backend} grain ({x},{y}) channel {channel}: {actual} vs {expected}"
                    );
                }
                assert_eq!(pixel[3], 255, "{backend} grain ({x},{y}) changed alpha");
            }
        }
    }
    shaders::set_custom_close_program(&mut gles, None);
}
