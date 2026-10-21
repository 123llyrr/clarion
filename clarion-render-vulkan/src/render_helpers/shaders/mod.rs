//! Shader interfaces and animations for Clarion's Vulkan / Native renderer.

use glam::Mat3;
use smithay::backend::renderer::gles::{Uniform, UniformName, UniformType, UniformValue};
use tracing::warn;

pub mod registry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramType {
    Border,
    Shadow,
    Resize,
    Close,
    Open,
}

impl ProgramType {
    pub(crate) fn animation_kind(self) -> Option<registry::AnimationKind> {
        match self {
            Self::Border | Self::Shadow => None,
            Self::Resize => Some(registry::AnimationKind::Resize),
            Self::Close => Some(registry::AnimationKind::Close),
            Self::Open => Some(registry::AnimationKind::Open),
        }
    }
}

const _: [(); 1] =
    [(); (ProgramType::Resize as u8 == registry::AnimationKind::Resize.key()) as usize];
const _: [(); 1] =
    [(); (ProgramType::Close as u8 == registry::AnimationKind::Close.key()) as usize];
const _: [(); 1] = [(); (ProgramType::Open as u8 == registry::AnimationKind::Open.key()) as usize];

fn source_part(part: registry::SourcePart) -> &'static str {
    match part {
        registry::SourcePart::ResizePrelude => include_str!("resize_prelude.frag"),
        registry::SourcePart::ResizeDefault => include_str!("resize.frag"),
        registry::SourcePart::ResizeEpilogue => include_str!("resize_epilogue.frag"),
        registry::SourcePart::RoundingAlpha => include_str!("rounding_alpha.frag"),
        registry::SourcePart::OpenPrelude => include_str!("open_prelude.frag"),
        registry::SourcePart::OpenDefault => include_str!("open_default.frag"),
        registry::SourcePart::OpenEpilogue => include_str!("open_epilogue.frag"),
        registry::SourcePart::ClosePrelude => include_str!("close_prelude.frag"),
        registry::SourcePart::CloseDefault => include_str!("close_default.frag"),
        registry::SourcePart::CloseEpilogue => include_str!("close_epilogue.frag"),
    }
}

pub fn animation_source_for(kind: registry::AnimationKind, custom: Option<&str>) -> String {
    registry::assemble(registry::spec(kind).layout, custom, source_part)
}

pub fn animation_source(program: ProgramType, custom: Option<&str>) -> String {
    let kind = program.animation_kind().expect("not an animation shader");
    animation_source_for(kind, custom)
}

pub const DENIAL_EXPLOSION_SHADER: &str = include_str!("denial_explosion.frag");
pub const DENIAL_IMPLODE_SHADER: &str = include_str!("denial_implode.frag");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCloseEffect {
    Explosion,
    Implode,
    Fade,
}

pub fn close_shader_for_effect(effect: WindowCloseEffect) -> Option<&'static str> {
    match effect {
        WindowCloseEffect::Explosion => Some(DENIAL_EXPLOSION_SHADER),
        WindowCloseEffect::Implode => Some(DENIAL_IMPLODE_SHADER),
        WindowCloseEffect::Fade => None,
    }
}

pub fn resolve_close_shader(
    custom_shader: Option<&str>,
    effect: WindowCloseEffect,
) -> Option<&str> {
    registry::close_body(custom_shader, close_shader_for_effect(effect))
}

pub fn mat3_uniform(name: &str, mat: Mat3) -> Uniform<'_> {
    Uniform::new(
        name,
        UniformValue::Matrix3x3 {
            matrices: vec![mat.to_cols_array()],
            transpose: false,
        },
    )
}

pub fn set_custom_animation_native(
    native: &mut crate::native::NativeRenderer,
    program_type: ProgramType,
    src: Option<&str>,
) {
    let kind = program_type
        .animation_kind()
        .expect("animation setter requires an animation");
    if let Err(error) =
        native.configure_animation_program(program_type, &animation_source_for(kind, src))
    {
        warn!("error compiling native {} shader: {error:#}", kind.label());
    }
}
