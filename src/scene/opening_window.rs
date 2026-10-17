//! Denial-derived reveal geometry and timing: Copyright (C) 2026 Doctor Logix.
//! GPL-3.0-or-later; see resources/licenses/denial-GPL-3.0-or-later.txt.

//! Frozen opening-pose composition. Animation clocks and trajectories stay in the
//! layout model; retained pixels belong to the scene's window-resource registry.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Context as _;
use glam::{Mat3, Vec2};
use smithay::backend::renderer::element::utils::{
    Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::{Element, Kind, RenderElement};
use smithay::backend::renderer::gles::{GlesRenderer, Uniform};
use smithay::backend::renderer::Texture;
use smithay::utils::{Logical, Point, Rectangle, Scale, Size};

use crate::clarion_render_elements;
use crate::layout::opening_window::OpeningWindowPose;
use crate::render_helpers::offscreen::{OffscreenData, OffscreenRenderElement};
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::shader_element::ShaderRenderElement;
use crate::render_helpers::shaders::{mat3_uniform, ProgramType, Shaders};
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::vulkan::native::NativeRenderer;
use crate::scene::window_resources::AnimationBuffers;

clarion_render_elements! {
    OpeningWindowRenderElement => {
        Offscreen = RelocateRenderElement<RescaleRenderElement<OffscreenRenderElement>>,
        Texture = RelocateRenderElement<RescaleRenderElement<PrimaryGpuTextureRenderElement>>,
        Shader = ShaderRenderElement,
    }
}

// We can't depend on view_rect here, because the result of window opening can be snapshot and
// then rendered elsewhere.
// Retained buffers have a separate scene owner; geometry stays call-local.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_opening_window_gles(
    pose: &OpeningWindowPose,
    renderer: &mut GlesRenderer,
    buffers: &AnimationBuffers,
    elements: &[impl RenderElement<GlesRenderer>],
    geo_size: Size<f64, Logical>,
    location: Point<f64, Logical>,
    scale: Scale<f64>,
    alpha: f32,
) -> anyhow::Result<(OpeningWindowRenderElement, OffscreenData)> {
    let clamped_progress = pose.clamped_progress;

    let (elem, _sync_point, mut data) = buffers
        .gles
        .render(renderer, scale, elements)
        .context("error rendering to offscreen buffer")?;

    if !pose.lightweight && Shaders::get(renderer).program(ProgramType::Open).is_some() {
        // OffscreenBuffer renders with Transform::Normal and the scale that we passed, so we
        // can assume that below.
        let offset = elem.offset();
        let texture = elem.texture();
        let texture_size = elem.logical_size();

        let geometry = OpeningShaderGeometry::new(
            pose,
            geo_size,
            location,
            scale,
            offset,
            texture_size,
            Vec2::new(texture.width() as f32, texture.height() as f32),
        );

        let elem = ShaderRenderElement::new(
            ProgramType::Open,
            geometry.area.size,
            None,
            scale.x as f32,
            alpha,
            geometry.uniforms(pose),
            HashMap::from([(String::from("clarion_tex"), texture.clone())]),
            Kind::Unspecified,
        )
        .with_location(geometry.area.loc);

        // We're drawing the shader, not the offscreen itself.
        data.id = elem.id().clone();

        return Ok((elem.into(), data));
    }

    let elem = elem.with_alpha(clamped_progress as f32 * alpha);

    let current_scale = pose.texture_scale;

    let center = geo_size.to_point().downscale(2.);
    let elem = RescaleRenderElement::from_element(
        elem,
        center.to_physical_precise_round(scale),
        current_scale,
    );

    let elem = RelocateRenderElement::from_element(
        elem,
        location.to_physical_precise_round(scale),
        Relocate::Relative,
    );

    Ok((elem.into(), data))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_opening_window_native(
    pose: &OpeningWindowPose,
    renderer: &mut NativeRenderer,
    buffers: &AnimationBuffers,
    custom: Option<&str>,
    elements: &[impl RenderElement<NativeRenderer>],
    geo_size: Size<f64, Logical>,
    location: Point<f64, Logical>,
    scale: Scale<f64>,
    alpha: f32,
) -> anyhow::Result<(OpeningWindowRenderElement, OffscreenData)> {
    let (texture, _sync, geo, states, _commit) =
        buffers
            .native
            .render(renderer, scale, elements)
            .context("error rendering opening window to Vulkan texture")?;
    // The persistent target stays across frames; the native queue and sampling
    // barriers order each re-render against the previous frame's sampling.

    let offset = geo.loc.to_f64().to_logical(scale);
    // The used region is the current encompassing geometry: the pooled allocation
    // may keep a larger size after a shrink, and only its [0, geo.size) prefix
    // belongs to this window.
    let texture_size = Size::from((
        f64::from(geo.size.w) / scale.x,
        f64::from(geo.size.h) / scale.y,
    ));
    if pose.lightweight {
        // Keep group opacity correct for overlapping decorations/subsurfaces.
        // The persistent snapshot only redraws on client damage; each animation
        // frame then uses the ordinary texture pipeline, without a reveal shader.
        let buffer = TextureBuffer::from_texture(
            renderer,
            texture,
            scale,
            smithay::utils::Transform::Normal,
            Vec::new(),
        );
        let elem = TextureRenderElement::from_texture_buffer(
            buffer,
            offset,
            pose.clamped_progress as f32 * alpha,
            Some(Rectangle::from_size(texture_size)),
            Some(texture_size),
            Kind::Unspecified,
        );
        let elem = RescaleRenderElement::from_element(
            PrimaryGpuTextureRenderElement::from(elem),
            geo_size
                .to_point()
                .downscale(2.)
                .to_physical_precise_round(scale),
            pose.scale,
        );
        let elem = RelocateRenderElement::from_element(
            elem,
            location.to_physical_precise_round(scale),
            Relocate::Relative,
        );
        let data = OffscreenData {
            id: elem.id().clone(),
            states,
        };
        return Ok((elem.into(), data));
    }
    let geometry = OpeningShaderGeometry::new(
        pose,
        geo_size,
        location,
        scale,
        offset,
        texture_size,
        Vec2::new(texture.width() as f32, texture.height() as f32),
    );

    let elem = ShaderRenderElement::new_native(
        renderer,
        ProgramType::Open,
        &crate::render_helpers::shaders::animation_source(ProgramType::Open, custom),
        geometry.area.size,
        scale.x as f32,
        alpha,
        geometry.uniforms(pose),
        HashMap::from([(String::from("clarion_tex"), texture.clone())]),
        Kind::Unspecified,
    )?
    .with_location(geometry.area.loc);

    let data = OffscreenData {
        id: elem.id().clone(),
        states,
    };
    Ok((elem.into(), data))
}

/// One geometry path for both backends. The current used snapshot extent and
/// the allocated texture extent are deliberately distinct: pooled targets can
/// outlive a larger previous window buffer.
struct OpeningShaderGeometry {
    area: Rectangle<f64, Logical>,
    effect_size: Vec2,
    input_to_geo: Mat3,
    geo_to_tex: Mat3,
}

impl OpeningShaderGeometry {
    #[allow(clippy::too_many_arguments)]
    fn new(
        pose: &OpeningWindowPose,
        geo_size: Size<f64, Logical>,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
        offset: Point<f64, Logical>,
        used_size: Size<f64, Logical>,
        texture_pixel_size: Vec2,
    ) -> Self {
        let effect_rect = if pose.denial_reveal {
            // The built-in reveal includes decorations and shadow, rather than
            // clipping to the client's logical geometry.
            Rectangle::new(location + offset, used_size)
        } else {
            Rectangle::new(location, geo_size)
        };
        let mut area = Rectangle::new(location + offset, used_size);
        if pose.denial_reveal {
            // The squircle stays in the snapshot. Only Glide needs a small,
            // pixel-aligned margin instead of a mostly empty full-output pass.
            if pose.glide_enabled {
                let padding = (area.size.w.max(area.size.h) * 0.08).max(2.);
                let diff = Point::from((padding, padding))
                    .to_physical_precise_round(scale)
                    .to_logical(scale);
                area.loc -= diff;
                area.size += diff.upscale(2.).to_size();
            }
        } else {
            // User shaders retain the historical generous effect canvas.
            let mut target_size = area.size.upscale(1.5);
            target_size.w = f64::max(area.size.w + 1000., target_size.w);
            target_size.h = f64::max(area.size.h + 1000., target_size.h);
            let diff = (target_size.to_point() - area.size.to_point()).downscale(2.);
            let diff = diff.to_physical_precise_round(scale).to_logical(scale);
            area.loc -= diff;
            area.size += diff.upscale(2.).to_size();
        }
        let area_loc = Vec2::new(area.loc.x as f32, area.loc.y as f32);
        let area_size = Vec2::new(area.size.w as f32, area.size.h as f32);
        let effect_loc = Vec2::new(effect_rect.loc.x as f32, effect_rect.loc.y as f32);
        let effect_size = Vec2::new(effect_rect.size.w as f32, effect_rect.size.h as f32);
        let input_to_geo = Mat3::from_scale(area_size / effect_size)
            * Mat3::from_translation((area_loc - effect_loc) / area_size);
        let tex_scale = Vec2::new(scale.x as f32, scale.y as f32);
        let tex_loc = Vec2::new(offset.x as f32, offset.y as f32);
        let tex_size = texture_pixel_size / tex_scale;
        let geo_to_tex = if pose.denial_reveal {
            // The built-in effect maps only the currently used prefix, even
            // when the offscreen allocation is retained after a shrink.
            Mat3::from_scale(effect_size / tex_size)
        } else {
            Mat3::from_translation(-tex_loc / tex_size) * Mat3::from_scale(effect_size / tex_size)
        };
        Self {
            area,
            effect_size,
            input_to_geo,
            geo_to_tex,
        }
    }

    fn uniforms(&self, pose: &OpeningWindowPose) -> Rc<[Uniform<'static>]> {
        Rc::new([
            mat3_uniform("clarion_input_to_geo", self.input_to_geo),
            Uniform::new("clarion_geo_size", self.effect_size.to_array()),
            mat3_uniform("clarion_geo_to_tex", self.geo_to_tex),
            Uniform::new("clarion_progress", pose.progress as f32),
            Uniform::new("clarion_clamped_progress", pose.clamped_progress as f32),
            Uniform::new("clarion_random_seed", pose.random_seed),
            Uniform::new(
                "clarion_glide_enabled",
                if pose.glide_enabled { 1. } else { 0. },
            ),
            Uniform::new("clarion_glide_tilt", pose.glide_tilt),
            Uniform::new("clarion_glide_depth", pose.glide_depth),
            Uniform::new("clarion_popin_ratio", pose.shader_popin_ratio),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(denial_reveal: bool, glide_enabled: bool) -> OpeningWindowPose {
        OpeningWindowPose {
            progress: 0.5,
            clamped_progress: 0.5,
            scale: 1.,
            texture_scale: 1.,
            shader_popin_ratio: 1.,
            denial_reveal,
            lightweight: false,
            random_seed: 0.42,
            glide_enabled,
            glide_tilt: 12_f32.to_radians(),
            glide_depth: 0.08,
        }
    }

    fn geometry(pose: &OpeningWindowPose) -> OpeningShaderGeometry {
        OpeningShaderGeometry::new(
            pose,
            (40., 24.).into(),
            (16., 16.).into(),
            1.25.into(),
            (-4., -6.).into(),
            (48., 32.).into(),
            Vec2::new(128., 96.),
        )
    }

    fn assert_vec_eq(actual: Vec2, expected: Vec2) {
        assert!(
            (actual - expected).abs().max_element() < 1e-6,
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn built_in_reveal_maps_the_used_snapshot_prefix_after_a_pooled_shrink() {
        let geometry = geometry(&pose(true, false));
        assert_eq!(
            geometry.area,
            Rectangle::new((12., 10.).into(), (48., 32.).into())
        );
        assert_vec_eq(geometry.effect_size, Vec2::new(48., 32.));
        assert_eq!(geometry.input_to_geo, Mat3::IDENTITY);
        assert_vec_eq(geometry.geo_to_tex.transform_point2(Vec2::ZERO), Vec2::ZERO);
        assert_vec_eq(
            geometry.geo_to_tex.transform_point2(Vec2::ONE),
            Vec2::new(60. / 128., 40. / 96.),
        );
        // Reading [0, 1] would sample unused pixels from the previous allocation.
        assert!(
            geometry
                .geo_to_tex
                .transform_point2(Vec2::ONE)
                .max_element()
                < 1.
        );
    }

    #[test]
    fn glide_reveal_padding_is_bounded_and_aligned_to_fractional_output_pixels() {
        let geometry = geometry(&pose(true, true));
        // 48 * .08 = 3.84 logical; at 1.25 scale this rounds to 5 pixels.
        assert_eq!(
            geometry.area,
            Rectangle::new((8., 6.).into(), (56., 40.).into())
        );
        assert_vec_eq(
            geometry
                .input_to_geo
                .transform_point2(Vec2::new(4. / 56., 4. / 40.)),
            Vec2::ZERO,
        );
        assert_vec_eq(
            geometry
                .input_to_geo
                .transform_point2(Vec2::new(52. / 56., 36. / 40.)),
            Vec2::ONE,
        );
    }

    #[test]
    fn custom_shader_keeps_its_canvas_and_negative_decoration_offset() {
        let geometry = geometry(&pose(false, true));
        assert_eq!(
            geometry.area,
            Rectangle::new((-488., -490.).into(), (1048., 1032.).into())
        );
        assert_vec_eq(geometry.effect_size, Vec2::new(40., 24.));
        assert_vec_eq(
            geometry.geo_to_tex.transform_point2(Vec2::ZERO),
            Vec2::new(5. / 128., 7.5 / 96.),
        );
        assert_vec_eq(
            geometry.geo_to_tex.transform_point2(Vec2::ONE),
            Vec2::new(55. / 128., 37.5 / 96.),
        );
        // Both backends must transform the effect's logical origin identically.
        assert_vec_eq(
            geometry
                .input_to_geo
                .transform_point2(Vec2::new(504. / 1048., 506. / 1032.)),
            Vec2::ZERO,
        );
    }
}
