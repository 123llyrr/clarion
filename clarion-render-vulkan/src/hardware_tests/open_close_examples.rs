//! Complete drop-in wiki examples, sampled against an independent scalar oracle.
use super::*;
use crate::render_helpers::shader_element::ShaderRenderElement;
use crate::render_helpers::shaders::{animation_source, mat3_uniform, ProgramType};
use crate::render_helpers::{render_to_vec, resources, shaders};
use smithay::backend::renderer::element::{Kind, RenderElement};
use smithay::backend::renderer::gles::Uniform;
use smithay::backend::renderer::{ImportMem, RendererSuper, Texture};
use std::{collections::HashMap, rc::Rc};
mod oracle;
use oracle::Effect;
const OPEN: &str = include_str!("../../../../docs/wiki/examples/open_custom_shader.frag");
const CLOSE: &str = include_str!("../../../../docs/wiki/examples/close_custom_shader.frag");
fn kind(effect: Effect) -> ProgramType {
    if effect.opening() {
        ProgramType::Open
    } else {
        ProgramType::Close
    }
}
fn body(effect: Effect) -> String {
    if effect.opening() {
        OPEN.replace(
            "return expanding_circle(point, geometry_size);",
            &format!("return {}(point, geometry_size);", effect.entry()),
        )
    } else {
        CLOSE.replace(
            "return fall_and_rotate(point, geometry_size);",
            &format!("return {}(point, geometry_size);", effect.entry()),
        )
    }
}
type Uniforms = Rc<[Uniform<'static>]>;
type Bindings<T> = HashMap<String, T>;
fn exercise<R, F>(renderer: &mut R, effect: Effect, mut make: F)
where
    R: Renderer
        + ImportMem
        + ExportMem
        + Offscreen<<R as RendererSuper>::TextureId>
        + Bind<<R as RendererSuper>::TextureId>,
    R::TextureId: Clone,
    R::Error: Send + Sync + 'static,
    ShaderRenderElement: RenderElement<R>,
    for<'a> R::Framebuffer<'a>: Texture,
    F: FnMut(&mut R, Uniforms, Bindings<R::TextureId>) -> ShaderRenderElement,
{
    let image = oracle::image();
    let texture = renderer
        .import_memory(&image, Fourcc::Abgr8888, (8, 8).into(), false)
        .unwrap();
    for (progress, clamped) in [(-1., 0.5), (0., 0.), (0.5, 0.5), (1., 1.), (1.25, 1.)] {
        for seed in [0.1, 0.5, 0.9] {
            for offset in [-0.25, 0., 0.25] {
                let matrix = glam::Mat3::from_cols_array(&[1., 0., 0., 0., 1., 0., offset, 0., 1.]);
                let mut values = vec![
                    mat3_uniform("clarion_input_to_geo", matrix),
                    mat3_uniform("clarion_geo_to_tex", glam::Mat3::IDENTITY),
                    Uniform::new("clarion_geo_size", [256f32, 192.]),
                    Uniform::new("clarion_progress", progress),
                    Uniform::new("clarion_clamped_progress", clamped),
                    Uniform::new("clarion_random_seed", seed),
                    Uniform::new("clarion_glide_enabled", 0f32),
                    Uniform::new("clarion_glide_tilt", 0f32),
                    Uniform::new("clarion_glide_depth", 0f32),
                ];
                if effect.opening() {
                    values.push(Uniform::new("clarion_popin_ratio", 1f32));
                }
                let element = make(
                    renderer,
                    Rc::from(values),
                    HashMap::from([("clarion_tex".to_owned(), texture.clone())]),
                );
                let pixels = render_to_vec(
                    renderer,
                    (8, 8).into(),
                    1.0.into(),
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    std::iter::once(&element),
                )
                .unwrap();
                for (pixel, actual) in pixels.as_chunks::<4>().0.iter().enumerate() {
                    let point = [
                        (pixel % 8) as f64 / 8. + 1. / 16. + f64::from(offset),
                        (pixel / 8) as f64 / 8. + 1. / 16.,
                    ];
                    let expected = oracle::pixel(
                        effect,
                        point,
                        [256., 192.],
                        f64::from(progress),
                        f64::from(clamped),
                        f64::from(seed),
                        &image,
                    );
                    for (channel, (&actual, expected)) in actual.iter().zip(expected).enumerate() {
                        assert!(actual.abs_diff(expected)<=1,"{effect:?} p={progress}/{clamped} seed={seed} offset={offset} pixel={pixel} channel={channel}: {actual} vs {expected}");
                    }
                }
            }
        }
    }
}
#[test]
#[ignore = "requires selected DRM render node, GLES, Vulkan and shader compiler; no display master"]
fn complete_open_and_close_wiki_policies_match_motion_and_sampling_oracle() {
    let (vk, mut gles, _allocator) = setup();
    resources::init(&mut gles);
    shaders::init(&mut gles);
    for effect in Effect::ALL {
        let source = body(effect);
        if effect.opening() {
            shaders::set_custom_open_program(&mut gles, Some(&source));
        } else {
            shaders::set_custom_close_program(&mut gles, Some(&source));
        }
        exercise(&mut gles, effect, |_, uniforms, bindings| {
            ShaderRenderElement::new(
                kind(effect),
                (8., 8.).into(),
                None,
                1.,
                0.5,
                uniforms,
                bindings,
                Kind::Unspecified,
            )
        });
    }
    shaders::set_custom_open_program(&mut gles, None);
    shaders::set_custom_close_program(&mut gles, None);
    let mut native = native::NativeRenderer::new(vk).unwrap();
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    for effect in Effect::ALL {
        let kind = kind(effect);
        let source = animation_source(kind, Some(&body(effect)));
        native
            .wait_for_animation_program_for_test(kind, &source)
            .unwrap();
        exercise(&mut native, effect, |renderer, uniforms, bindings| {
            ShaderRenderElement::new_native(
                renderer,
                kind,
                &source,
                (8., 8.).into(),
                1.,
                0.5,
                uniforms,
                bindings,
                Kind::Unspecified,
            )
            .unwrap()
        });
    }
}
