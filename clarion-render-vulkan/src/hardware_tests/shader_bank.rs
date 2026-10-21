//! Shader-bank ownership checks on a real GLES context. These cover all three
//! animation setters, their shared replacement transaction and old draw owners.
use std::collections::HashMap;
use std::rc::Rc;

use super::*;
use crate::render_helpers::shader_element::{ShaderProgram, ShaderRenderElement};
use crate::render_helpers::{render_to_vec, resources, shaders};
use shaders::{ProgramType, Shaders};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;

fn set(renderer: &mut GlesRenderer, kind: ProgramType, source: Option<&str>) {
    match kind {
        ProgramType::Resize => shaders::set_custom_resize_program(renderer, source),
        ProgramType::Open => shaders::set_custom_open_program(renderer, source),
        ProgramType::Close => shaders::set_custom_close_program(renderer, source),
        _ => unreachable!(),
    }
}

fn exchange(
    bank: &Shaders,
    kind: ProgramType,
    program: Option<ShaderProgram>,
) -> Option<ShaderProgram> {
    match kind {
        ProgramType::Resize => bank.replace_custom_resize_program(program),
        ProgramType::Open => bank.replace_custom_open_program(program),
        ProgramType::Close => bank.replace_custom_close_program(program),
        _ => unreachable!(),
    }
}

#[test]
#[ignore = "requires selected DRM node, Vulkan and GLES"]
fn animation_override_failure_reset_and_retired_clone_remain_valid() {
    let (_, mut gles, _) = setup();
    resources::init(&mut gles);
    shaders::init(&mut gles);
    for (kind, entry) in [
        (ProgramType::Resize, "resize_color"),
        (ProgramType::Open, "open_color"),
        (ProgramType::Close, "close_color"),
    ] {
        let fallback = Shaders::get(&mut gles).program(kind).unwrap();
        set(&mut gles, kind, Some("invalid shader source"));
        assert_eq!(
            Shaders::get(&mut gles).program(kind),
            Some(fallback.clone())
        );
        let valid =
            format!("vec4 {entry}(vec3 p, vec3 s) {{ return vec4(0.25, 0.125, 0.0, 0.5); }}");
        set(&mut gles, kind, Some(&valid));
        let saved = Shaders::get(&mut gles).program(kind).unwrap();
        assert_ne!(saved, fallback);
        set(&mut gles, kind, Some("invalid replacement"));
        assert_eq!(Shaders::get(&mut gles).program(kind), Some(saved.clone()));
        shaders::init(&mut gles);
        assert_eq!(Shaders::get(&mut gles).program(kind), Some(saved.clone()));
        set(&mut gles, kind, None);
        assert_eq!(Shaders::get(&mut gles).program(kind), Some(fallback));

        // The setter retired its bank reference, but an outstanding draw owner
        // must retain both linked variants. Reinstall that owner and draw it.
        assert!(exchange(Shaders::get(&mut gles), kind, Some(saved)).is_none());
        let element = ShaderRenderElement::new(
            kind,
            (8.0, 8.0).into(),
            None,
            1.0,
            1.0,
            Rc::new([]),
            HashMap::new(),
            Kind::Unspecified,
        );
        let pixels = render_to_vec(
            &mut gles,
            (8, 8).into(),
            1.0.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            [&element].into_iter(),
        )
        .unwrap();
        for pixel in pixels.as_chunks::<4>().0 {
            for (actual, expected) in pixel.iter().zip([64u8, 32, 0, 128]) {
                assert!(actual.abs_diff(expected) <= 1, "{kind:?}: {pixel:?}");
            }
        }
        set(&mut gles, kind, None);
    }
}
