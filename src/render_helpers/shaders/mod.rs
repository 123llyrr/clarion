use std::cell::RefCell;

use glam::Mat3;
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform, UniformName, UniformType,
    UniformValue,
};

use super::shader_element::ShaderProgram;
use crate::render_helpers::blur::BlurProgram;
use crate::render_helpers::mesh::MeshProgram;
use crate::render_helpers::particles::ParticleProgram;

pub struct Shaders {
    pub border: Option<ShaderProgram>,
    pub shadow: Option<ShaderProgram>,
    pub clipped_surface: Option<GlesTexProgram>,
    pub postprocess_and_clip: Option<GlesTexProgram>,
    pub resize: Option<ShaderProgram>,
    pub gradient_fade: Option<GlesTexProgram>,
    pub blur: Option<BlurProgram>,
    pub close: Option<ShaderProgram>,
    pub open: Option<ShaderProgram>,
    pub mesh: Option<MeshProgram>,
    pub particles: Option<ParticleProgram>,
    pub custom_resize: RefCell<Option<ShaderProgram>>,
    pub custom_close: RefCell<Option<ShaderProgram>>,
    pub custom_open: RefCell<Option<ShaderProgram>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramType {
    Border,
    Shadow,
    Resize,
    Close,
    Open,
}

impl Shaders {
    fn compile(renderer: &mut GlesRenderer) -> Self {
        let _span = clarion_tracy_span!("Shaders::compile");

        let border = ShaderProgram::compile(
            renderer,
            concat!(
                include_str!("border.frag"),
                include_str!("rounding_alpha.frag")
            ),
            &[
                UniformName::new("colorspace", UniformType::_1f),
                UniformName::new("hue_interpolation", UniformType::_1f),
                UniformName::new("color_from", UniformType::_4f),
                UniformName::new("color_to", UniformType::_4f),
                UniformName::new("grad_offset", UniformType::_2f),
                UniformName::new("grad_width", UniformType::_1f),
                UniformName::new("grad_vec", UniformType::_2f),
                UniformName::new("input_to_geo", UniformType::Matrix3x3),
                UniformName::new("geo_size", UniformType::_2f),
                UniformName::new("outer_radius", UniformType::_4f),
                UniformName::new("border_width", UniformType::_1f),
                UniformName::new("rounding_power", UniformType::_1f),
            ],
            &[],
        )
        .map_err(|err| {
            warn!("error compiling border shader: {err:?}");
        })
        .ok();

        let shadow = ShaderProgram::compile(
            renderer,
            concat!(
                include_str!("shadow.frag"),
                include_str!("rounding_alpha.frag")
            ),
            &[
                UniformName::new("shadow_color", UniformType::_4f),
                UniformName::new("sigma", UniformType::_1f),
                UniformName::new("input_to_geo", UniformType::Matrix3x3),
                UniformName::new("geo_size", UniformType::_2f),
                UniformName::new("corner_radius", UniformType::_4f),
                UniformName::new("window_input_to_geo", UniformType::Matrix3x3),
                UniformName::new("window_geo_size", UniformType::_2f),
                UniformName::new("window_corner_radius", UniformType::_4f),
            ],
            &[],
        )
        .map_err(|err| {
            warn!("error compiling shadow shader: {err:?}");
        })
        .ok();

        let clipped_surface = renderer
            .compile_custom_texture_shader(
                concat!(
                    include_str!("clipped_surface.frag"),
                    include_str!("rounding_alpha.frag"),
                    "\nvec4 postprocess(vec4 color) { return color; }",
                    "\nvec4 clarion_sample_background(vec2 uv) { return texture2D(tex, uv); }",
                ),
                &[
                    UniformName::new("clarion_scale", UniformType::_1f),
                    UniformName::new("geo_size", UniformType::_2f),
                    UniformName::new("corner_radius", UniformType::_4f),
                    UniformName::new("input_to_geo", UniformType::Matrix3x3),
                    UniformName::new("rounding_power", UniformType::_1f),
                ],
            )
            .map_err(|err| {
                warn!("error compiling clipped surface shader: {err:?}");
            })
            .ok();

        let postprocess_and_clip = renderer
            .compile_custom_texture_shader(
                concat!(
                    include_str!("clipped_surface.frag"),
                    include_str!("rounding_alpha.frag"),
                    include_str!("postprocess.frag"),
                    include_str!("liquid_glass_common.glsl"),
                    include_str!("liquid_glass.frag"),
                ),
                &[
                    UniformName::new("clarion_scale", UniformType::_1f),
                    UniformName::new("geo_size", UniformType::_2f),
                    UniformName::new("corner_radius", UniformType::_4f),
                    UniformName::new("input_to_geo", UniformType::Matrix3x3),
                    UniformName::new("rounding_power", UniformType::_1f),
                    UniformName::new("noise", UniformType::_1f),
                    UniformName::new("saturation", UniformType::_1f),
                    UniformName::new("bg_color", UniformType::_4f),
                    UniformName::new("liquid_glass", UniformType::_2f),
                ],
            )
            .map_err(|err| {
                warn!("error compiling postprocess_and_clip shader: {err:?}");
            })
            .ok();

        let resize = compile_resize_program(renderer, include_str!("resize.frag"))
            .map_err(|err| {
                warn!("error compiling resize shader: {err:?}");
            })
            .ok();

        let gradient_fade = renderer
            .compile_custom_texture_shader(
                include_str!("gradient_fade.frag"),
                &[UniformName::new("cutoff", UniformType::_2f)],
            )
            .map_err(|err| {
                warn!("error compiling gradient fade shader: {err:?}");
            })
            .ok();

        let blur = BlurProgram::compile(renderer)
            .map_err(|err| {
                warn!("error compiling blur shaders: {err:?}");
            })
            .ok();

        let close = compile_close_program(renderer, include_str!("close_default.frag"))
            .map_err(|err| warn!("error compiling default close shader: {err:?}"))
            .ok();
        let open = compile_open_program(renderer, include_str!("open_default.frag"))
            .map_err(|err| warn!("error compiling default open shader: {err:?}"))
            .ok();
        let mesh = MeshProgram::compile(renderer)
            .map_err(|err| {
                warn!("error compiling texture mesh shader; 3D effects disabled: {err:?}")
            })
            .ok();
        let particles = ParticleProgram::compile(renderer)
            .map_err(|err| {
                warn!("error compiling Denial particle shader; close particles disabled: {err:?}")
            })
            .ok();

        Self {
            border,
            shadow,
            clipped_surface,
            postprocess_and_clip,
            resize,
            gradient_fade,
            blur,
            close,
            open,
            mesh,
            particles,
            custom_resize: RefCell::new(None),
            custom_close: RefCell::new(None),
            custom_open: RefCell::new(None),
        }
    }

    pub fn get_from_frame<'a>(frame: &'a mut GlesFrame<'_, '_>) -> &'a Self {
        let data = frame.egl_context().user_data();
        data.get()
            .expect("shaders::init() must be called when creating the renderer")
    }

    pub fn get(renderer: &mut GlesRenderer) -> &Self {
        let data = renderer.egl_context().user_data();
        data.get()
            .expect("shaders::init() must be called when creating the renderer")
    }

    pub fn replace_custom_resize_program(
        &self,
        program: Option<ShaderProgram>,
    ) -> Option<ShaderProgram> {
        self.custom_resize.replace(program)
    }

    pub fn replace_custom_close_program(
        &self,
        program: Option<ShaderProgram>,
    ) -> Option<ShaderProgram> {
        self.custom_close.replace(program)
    }

    pub fn replace_custom_open_program(
        &self,
        program: Option<ShaderProgram>,
    ) -> Option<ShaderProgram> {
        self.custom_open.replace(program)
    }

    pub fn program(&self, program: ProgramType) -> Option<ShaderProgram> {
        match program {
            ProgramType::Border => self.border.clone(),
            ProgramType::Shadow => self.shadow.clone(),
            ProgramType::Resize => self
                .custom_resize
                .borrow()
                .clone()
                .or_else(|| self.resize.clone()),
            ProgramType::Close => self
                .custom_close
                .borrow()
                .clone()
                .or_else(|| self.close.clone()),
            ProgramType::Open => self
                .custom_open
                .borrow()
                .clone()
                .or_else(|| self.open.clone()),
        }
    }
}

pub fn init(renderer: &mut GlesRenderer) {
    let shaders = Shaders::compile(renderer);
    let data = renderer.egl_context().user_data();
    if !data.insert_if_missing(|| shaders) {
        error!("shaders were already compiled");
    }
}

fn compile_resize_program(
    renderer: &mut GlesRenderer,
    src: &str,
) -> Result<ShaderProgram, GlesError> {
    let mut program = include_str!("resize_prelude.frag").to_string();
    program.push_str(src);
    program.push_str(include_str!("resize_epilogue.frag"));
    program.push_str(include_str!("rounding_alpha.frag"));

    ShaderProgram::compile(
        renderer,
        &program,
        &[
            UniformName::new("clarion_input_to_curr_geo", UniformType::Matrix3x3),
            UniformName::new("clarion_curr_geo_to_prev_geo", UniformType::Matrix3x3),
            UniformName::new("clarion_curr_geo_to_next_geo", UniformType::Matrix3x3),
            UniformName::new("clarion_curr_geo_size", UniformType::_2f),
            UniformName::new("clarion_geo_to_tex_prev", UniformType::Matrix3x3),
            UniformName::new("clarion_geo_to_tex_next", UniformType::Matrix3x3),
            UniformName::new("clarion_progress", UniformType::_1f),
            UniformName::new("clarion_clamped_progress", UniformType::_1f),
            UniformName::new("clarion_corner_radius", UniformType::_4f),
            UniformName::new("clarion_clip_to_geometry", UniformType::_1f),
        ],
        &["clarion_tex_prev", "clarion_tex_next"],
    )
}

pub fn set_custom_resize_program(
    renderer: &mut impl super::renderer::ClarionRenderer,
    src: Option<&str>,
) {
    if let Some(native) = renderer.as_native_renderer() {
        if let Err(err) = native.configure_animation_program(
            ProgramType::Resize,
            &animation_source(ProgramType::Resize, src),
        ) {
            warn!("error compiling native resize shader: {err:#}");
        }
        return;
    }
    let Some(renderer) = renderer.as_gles_renderer() else {
        return;
    };
    let program = if let Some(src) = src {
        match compile_resize_program(renderer, src) {
            Ok(program) => Some(program),
            Err(err) => {
                warn!("error compiling custom resize shader: {err:?}");
                return;
            }
        }
    } else {
        None
    };

    if let Some(prev) = Shaders::get(renderer).replace_custom_resize_program(program) {
        if let Err(err) = prev.destroy(renderer) {
            warn!("error destroying previous custom resize shader: {err:?}");
        }
    }
}

fn compile_close_program(
    renderer: &mut GlesRenderer,
    src: &str,
) -> Result<ShaderProgram, GlesError> {
    let mut program = include_str!("close_prelude.frag").to_string();
    program.push_str(src);
    program.push_str(include_str!("close_epilogue.frag"));

    ShaderProgram::compile(
        renderer,
        &program,
        &[
            UniformName::new("clarion_input_to_geo", UniformType::Matrix3x3),
            UniformName::new("clarion_geo_size", UniformType::_2f),
            UniformName::new("clarion_geo_to_tex", UniformType::Matrix3x3),
            UniformName::new("clarion_progress", UniformType::_1f),
            UniformName::new("clarion_clamped_progress", UniformType::_1f),
            UniformName::new("clarion_random_seed", UniformType::_1f),
            UniformName::new("clarion_glide_enabled", UniformType::_1f),
            UniformName::new("clarion_glide_tilt", UniformType::_1f),
            UniformName::new("clarion_glide_depth", UniformType::_1f),
        ],
        &["clarion_tex"],
    )
}

pub const DENIAL_EXPLOSION_SHADER: &str = include_str!("denial_explosion.frag");
pub const DENIAL_IMPLODE_SHADER: &str = include_str!("denial_implode.frag");

pub fn close_shader_for_effect(effect: clarion_config::WindowCloseEffect) -> Option<&'static str> {
    match effect {
        clarion_config::WindowCloseEffect::Explosion => Some(DENIAL_EXPLOSION_SHADER),
        clarion_config::WindowCloseEffect::Implode => Some(DENIAL_IMPLODE_SHADER),
        clarion_config::WindowCloseEffect::Fade => None,
    }
}

pub fn resolve_close_shader(
    custom_shader: Option<&str>,
    effect: clarion_config::WindowCloseEffect,
) -> Option<&str> {
    if let Some(custom) = custom_shader {
        Some(custom)
    } else {
        close_shader_for_effect(effect)
    }
}

pub fn set_custom_close_program(
    renderer: &mut impl super::renderer::ClarionRenderer,
    src: Option<&str>,
) {
    if let Some(native) = renderer.as_native_renderer() {
        if let Err(err) = native.configure_animation_program(
            ProgramType::Close,
            &animation_source(ProgramType::Close, src),
        ) {
            warn!("error compiling native close shader: {err:#}");
        }
        return;
    }
    let Some(renderer) = renderer.as_gles_renderer() else {
        return;
    };
    let program = if let Some(src) = src {
        match compile_close_program(renderer, src) {
            Ok(program) => {
                info!("successfully compiled window close shader program");
                Some(program)
            }
            Err(err) => {
                error!("error compiling custom close shader: {err:?}");
                return;
            }
        }
    } else {
        None
    };

    if let Some(prev) = Shaders::get(renderer).replace_custom_close_program(program) {
        if let Err(err) = prev.destroy(renderer) {
            warn!("error destroying previous custom close shader: {err:?}");
        }
    }
}

fn compile_open_program(
    renderer: &mut GlesRenderer,
    src: &str,
) -> Result<ShaderProgram, GlesError> {
    let mut program = include_str!("open_prelude.frag").to_string();
    program.push_str(src);
    program.push_str(include_str!("open_epilogue.frag"));

    ShaderProgram::compile(
        renderer,
        &program,
        &[
            UniformName::new("clarion_input_to_geo", UniformType::Matrix3x3),
            UniformName::new("clarion_geo_size", UniformType::_2f),
            UniformName::new("clarion_geo_to_tex", UniformType::Matrix3x3),
            UniformName::new("clarion_progress", UniformType::_1f),
            UniformName::new("clarion_clamped_progress", UniformType::_1f),
            UniformName::new("clarion_random_seed", UniformType::_1f),
            UniformName::new("clarion_glide_enabled", UniformType::_1f),
            UniformName::new("clarion_glide_tilt", UniformType::_1f),
            UniformName::new("clarion_glide_depth", UniformType::_1f),
            UniformName::new("clarion_popin_ratio", UniformType::_1f),
        ],
        &["clarion_tex"],
    )
}

pub fn set_custom_open_program(
    renderer: &mut impl super::renderer::ClarionRenderer,
    src: Option<&str>,
) {
    if let Some(native) = renderer.as_native_renderer() {
        if let Err(err) = native.configure_animation_program(
            ProgramType::Open,
            &animation_source(ProgramType::Open, src),
        ) {
            warn!("error compiling native open shader: {err:#}");
        }
        return;
    }
    let Some(renderer) = renderer.as_gles_renderer() else {
        return;
    };
    let program = if let Some(src) = src {
        match compile_open_program(renderer, src) {
            Ok(program) => Some(program),
            Err(err) => {
                warn!("error compiling custom open shader: {err:?}");
                return;
            }
        }
    } else {
        None
    };

    if let Some(prev) = Shaders::get(renderer).replace_custom_open_program(program) {
        if let Err(err) = prev.destroy(renderer) {
            warn!("error destroying previous custom open shader: {err:?}");
        }
    }
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

/// The same public GLSL interface is used for both renderer backends.
pub fn animation_source(program: ProgramType, custom: Option<&str>) -> String {
    match program {
        ProgramType::Resize => [
            include_str!("resize_prelude.frag"),
            custom.unwrap_or(include_str!("resize.frag")),
            include_str!("resize_epilogue.frag"),
            include_str!("rounding_alpha.frag"),
        ]
        .concat(),
        ProgramType::Open => [
            include_str!("open_prelude.frag"),
            custom.unwrap_or(include_str!("open_default.frag")),
            include_str!("open_epilogue.frag"),
        ]
        .concat(),
        ProgramType::Close => [
            include_str!("close_prelude.frag"),
            custom.unwrap_or(include_str!("close_default.frag")),
            include_str!("close_epilogue.frag"),
        ]
        .concat(),
        _ => unreachable!("not an animation shader"),
    }
}

pub fn init_native(
    renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
    animations: &clarion_config::Animations,
) {
    // Establish valid defaults before trying user-provided programs.
    set_custom_resize_program(renderer, None);
    set_custom_open_program(renderer, None);
    set_custom_close_program(renderer, None);
    set_custom_resize_program(renderer, animations.window_resize.custom_shader.as_deref());
    set_custom_open_program(renderer, animations.window_open.custom_shader.as_deref());
    set_custom_close_program(
        renderer,
        resolve_close_shader(
            animations.window_close.custom_shader.as_deref(),
            animations.window_close.effect,
        ),
    );
}
