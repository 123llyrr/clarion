//! Compile the complete wiki resource, then check every example policy on GPUs.
use super::*;
use crate::render_helpers::shader_element::ShaderRenderElement;
use crate::render_helpers::shaders::{self, animation_source, mat3_uniform, ProgramType};
use crate::render_helpers::{render_to_vec, resources};
use smithay::backend::renderer::element::{Kind, RenderElement};
use smithay::backend::renderer::gles::Uniform;
use smithay::backend::renderer::{ImportMem, RendererSuper, Texture};
use std::collections::HashMap;
use std::rc::Rc;
mod oracle;
use oracle::Policy;
const EXAMPLE: &str = include_str!("../../../../docs/wiki/examples/resize_custom_shader.frag");
fn body(policy: Policy) -> String {
    EXAMPLE.replace(
        "return stretch_or_crop_next(point, geometry_size);",
        &format!("return {}(point, geometry_size);", policy.entry()),
    )
}
struct Case {
    ratio: [f32; 2],
    offset: f32,
    progress: f32,
}
type Uniforms = Rc<[Uniform<'static>]>;
type Bindings<T> = HashMap<String, T>;
fn exercise<R, F>(renderer: &mut R, policy: Policy, mut make: F)
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
    F: FnMut(&mut R, Uniforms, Bindings<R::TextureId>) -> ShaderRenderElement,
{
    let previous = oracle::source(true);
    let next = oracle::source(false);
    let old = renderer
        .import_memory(&previous, Fourcc::Abgr8888, (8, 8).into(), false)
        .unwrap();
    let new = renderer
        .import_memory(&next, Fourcc::Abgr8888, (8, 8).into(), false)
        .unwrap();
    for ratio in [[0.5, 0.5], [0.5, 1.5], [1.5, 0.5], [1.5, 1.5]] {
        for offset in [-0.25, 0., 0.25] {
            for progress in [0., 0.375, 1.] {
                let case = Case {
                    ratio,
                    offset,
                    progress,
                };
                let input =
                    glam::Mat3::from_cols_array(&[1., 0., 0., 0., 1., 0., case.offset, 0., 1.]);
                let natural = glam::Mat3::from_cols_array(&[
                    case.ratio[0],
                    0.,
                    0.,
                    0.,
                    case.ratio[1],
                    0.,
                    0.,
                    0.,
                    1.,
                ]);
                let uniforms: Uniforms = Rc::new([
                    mat3_uniform("clarion_input_to_curr_geo", input),
                    mat3_uniform("clarion_curr_geo_to_prev_geo", glam::Mat3::IDENTITY),
                    mat3_uniform("clarion_curr_geo_to_next_geo", natural),
                    Uniform::new("clarion_curr_geo_size", [8f32, 8.]),
                    mat3_uniform("clarion_geo_to_tex_prev", glam::Mat3::IDENTITY),
                    mat3_uniform("clarion_geo_to_tex_next", glam::Mat3::IDENTITY),
                    Uniform::new("clarion_progress", case.progress),
                    Uniform::new("clarion_clamped_progress", case.progress),
                    Uniform::new("clarion_corner_radius", [0f32; 4]),
                    Uniform::new("clarion_clip_to_geometry", 0f32),
                ]);
                let bindings = HashMap::from([
                    ("clarion_tex_prev".to_owned(), old.clone()),
                    ("clarion_tex_next".to_owned(), new.clone()),
                ]);
                let element = make(renderer, uniforms, bindings);
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
                        policy,
                        point,
                        ratio.map(f64::from),
                        f64::from(progress),
                        &previous,
                        &next,
                    );
                    for (channel, (&actual, expected)) in actual.iter().zip(expected).enumerate() {
                        assert!(actual.abs_diff(expected)<=1,"{policy:?} ratio={ratio:?} offset={offset} p={progress} pixel={pixel} channel={channel}: {actual} vs {expected}");
                    }
                }
            }
        }
    }
}
#[test]
#[ignore = "requires isolated DRM render node, GLES, Vulkan and shader compiler; no display master"]
fn complete_wiki_resize_example_policies_match_independent_pixel_oracle() {
    let (vk, mut gles, _allocator) = setup();
    resources::init(&mut gles);
    shaders::init(&mut gles);
    for policy in Policy::ALL {
        let body = body(policy);
        shaders::set_custom_resize_program(&mut gles, Some(&body));
        exercise(&mut gles, policy, |_, uniforms, bindings| {
            ShaderRenderElement::new(
                ProgramType::Resize,
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
    shaders::set_custom_resize_program(&mut gles, None);
    let mut native = native::NativeRenderer::new(vk).unwrap();
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    for policy in Policy::ALL {
        let source = animation_source(ProgramType::Resize, Some(&body(policy)));
        native
            .wait_for_animation_program_for_test(ProgramType::Resize, &source)
            .unwrap();
        exercise(&mut native, policy, |renderer, uniforms, bindings| {
            ShaderRenderElement::new_native(
                renderer,
                ProgramType::Resize,
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
