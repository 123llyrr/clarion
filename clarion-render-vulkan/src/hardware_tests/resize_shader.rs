//! Scalar pixel oracle for the resize shader's admitted source domains.
use super::*;
use crate::render_helpers::shader_element::ShaderRenderElement;
use crate::render_helpers::shaders::{self, animation_source, mat3_uniform, ProgramType};
use crate::render_helpers::{render_to_vec, resources};
use smithay::backend::renderer::element::{Kind, RenderElement};
use smithay::backend::renderer::gles::Uniform;
use smithay::backend::renderer::{ImportMem, RendererSuper, Texture};
use std::collections::HashMap;
use std::rc::Rc;

struct Case {
    progress: f32,
    clip: bool,
    offset: f32,
    alpha: f32,
    nan_source: bool,
}
type Bindings<T> = HashMap<String, T>;
type Uniforms = Rc<[Uniform<'static>]>;
fn uniforms(case: &Case) -> Uniforms {
    let input = glam::Mat3::from_cols_array(&[1., 0., 0., 0., 1., 0., case.offset, 0., 1.]);
    let previous = if case.nan_source {
        glam::Mat3::from_cols_array(&[f32::NAN; 9])
    } else {
        glam::Mat3::IDENTITY
    };
    Rc::new([
        mat3_uniform("clarion_input_to_curr_geo", input),
        mat3_uniform("clarion_curr_geo_to_prev_geo", glam::Mat3::IDENTITY),
        mat3_uniform("clarion_curr_geo_to_next_geo", glam::Mat3::IDENTITY),
        Uniform::new("clarion_curr_geo_size", [8f32, 8.]),
        mat3_uniform("clarion_geo_to_tex_prev", previous),
        mat3_uniform("clarion_geo_to_tex_next", glam::Mat3::IDENTITY),
        Uniform::new("clarion_progress", case.progress),
        Uniform::new("clarion_clamped_progress", case.progress),
        Uniform::new("clarion_corner_radius", [0f32; 4]),
        Uniform::new(
            "clarion_clip_to_geometry",
            if case.clip { 1f32 } else { 0f32 },
        ),
    ])
}
fn exercise<R, F>(renderer: &mut R, mut make: F)
where
    R: Renderer
        + ImportMem
        + ExportMem
        + Offscreen<<R as RendererSuper>::TextureId>
        + Bind<<R as RendererSuper>::TextureId>,
    R::Error: Send + Sync + 'static,
    R::TextureId: Clone,
    ShaderRenderElement: RenderElement<R>,
    for<'a> R::Framebuffer<'a>: Texture,
    F: FnMut(&mut R, &Case, Uniforms, Bindings<R::TextureId>) -> ShaderRenderElement,
{
    let previous = renderer
        .import_memory(
            &[128, 0, 0, 128].repeat(64),
            Fourcc::Abgr8888,
            (8, 8).into(),
            false,
        )
        .unwrap();
    let next = renderer
        .import_memory(
            &[0, 0, 255, 255].repeat(64),
            Fourcc::Abgr8888,
            (8, 8).into(),
            false,
        )
        .unwrap();
    for progress in [0., 0.25, 1.] {
        for clip in [false, true] {
            for offset in [0., -0.25, 0.25] {
                for alpha in [0., 0.5, 1.] {
                    for nan_source in [false, true] {
                        let case = Case {
                            progress,
                            clip,
                            offset,
                            alpha,
                            nan_source,
                        };
                        let inputs = HashMap::from([
                            ("clarion_tex_prev".to_owned(), previous.clone()),
                            ("clarion_tex_next".to_owned(), next.clone()),
                        ]);
                        let element = make(renderer, &case, uniforms(&case), inputs);
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
                            // All coordinates are binary fractions at texel centers.
                            // This scalar oracle does not use production matrices or GLSL.
                            let u = (pixel % 8) as f64 / 8. + 1. / 16. + f64::from(offset);
                            let inside = (0. ..=1.).contains(&u);
                            let old = if inside && !nan_source {
                                [128., 0., 0., 128.]
                            } else {
                                [0.; 4]
                            };
                            let new = if inside {
                                [0., 0., 255., 255.]
                            } else {
                                [0.; 4]
                            };
                            let expected: [u8; 4] = std::array::from_fn(|channel| {
                                ((old[channel] * (1. - f64::from(progress))
                                    + new[channel] * f64::from(progress))
                                    * f64::from(alpha))
                                .round() as u8
                            });
                            for (channel, (&actual, expected)) in
                                actual.iter().zip(expected).enumerate()
                            {
                                assert!(actual.abs_diff(expected)<=1, "p={progress} clip={clip} offset={offset} alpha={alpha} nan={nan_source} pixel={pixel} channel={channel}: {actual} vs {expected}");
                            }
                        }
                    }
                }
            }
        }
    }
}
#[test]
#[ignore = "requires selected DRM render node, GLES and Vulkan; no display master"]
fn resize_endpoints_crop_and_nan_source_match_scalar_premultiplied_oracle() {
    let (vk, mut gles, _allocator) = setup();
    resources::init(&mut gles);
    shaders::init(&mut gles);
    exercise(&mut gles, |_, case, uniforms, bindings| {
        ShaderRenderElement::new(
            ProgramType::Resize,
            (8., 8.).into(),
            None,
            1.,
            case.alpha,
            uniforms,
            bindings,
            Kind::Unspecified,
        )
    });
    let mut native = native::NativeRenderer::new(vk).unwrap();
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    let source = animation_source(ProgramType::Resize, None);
    native
        .wait_for_animation_program_for_test(ProgramType::Resize, &source)
        .unwrap();
    exercise(&mut native, |renderer, case, uniforms, bindings| {
        ShaderRenderElement::new_native(
            renderer,
            ProgramType::Resize,
            &source,
            (8., 8.).into(),
            1.,
            case.alpha,
            uniforms,
            bindings,
            Kind::Unspecified,
        )
        .unwrap()
    });
}
