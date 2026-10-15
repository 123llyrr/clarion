//! Denial-derived reveal geometry and timing: Copyright (C) 2026 Doctor Logix.
//! GPL-3.0-or-later; see resources/licenses/denial-GPL-3.0-or-later.txt.

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

use crate::animation::Animation;
use crate::realand_render_elements;
use crate::render_helpers::offscreen::{
    NativeOffscreenBuffer, OffscreenBuffer, OffscreenData, OffscreenRenderElement,
};
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::shader_element::ShaderRenderElement;
use crate::render_helpers::shaders::{mat3_uniform, ProgramType, Shaders};
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::vulkan::native::NativeRenderer;

#[derive(Debug)]
pub struct OpenAnimation {
    anim: Animation,
    popin_ratio: Option<f64>,
    glide: realand_config::Glide,
    denial_reveal: bool,
    lightweight: bool,
    random_seed: f32,
    buffer: OffscreenBuffer,
    /// Vulkan counterpart of `buffer`: the snapshot target and its damage history
    /// are reused across every reveal frame, so a still window is composited to the
    /// offscreen once and only client commits redraw the changed parts.
    native_buffer: NativeOffscreenBuffer,
}

realand_render_elements! {
    OpeningWindowRenderElement => {
        Offscreen = RelocateRenderElement<RescaleRenderElement<OffscreenRenderElement>>,
        Texture = RelocateRenderElement<RescaleRenderElement<PrimaryGpuTextureRenderElement>>,
        Shader = ShaderRenderElement,
    }
}

impl OpenAnimation {
    pub fn new(
        anim: Animation,
        popin_ratio: Option<f64>,
        glide: realand_config::Glide,
        denial_reveal: bool,
        effect: realand_config::animations::WindowOpenEffect,
    ) -> Self {
        // Mapping already waits for the first committed buffer. Texture/fence
        // readiness belongs to the renderer; an extra fixed delay only hides
        // feedback for several refreshes without making that buffer safer.
        Self {
            anim,
            popin_ratio,
            glide,
            denial_reveal,
            lightweight: denial_reveal
                && effect == realand_config::animations::WindowOpenEffect::Fade,
            random_seed: fastrand::f32(),
            buffer: OffscreenBuffer::default(),
            native_buffer: NativeOffscreenBuffer::default(),
        }
    }

    pub fn is_done(&self) -> bool {
        if self.denial_reveal {
            // The reveal, Glide and pop-in all stop at clamped progress 1. Waiting for an
            // underdamped spring to settle would keep redrawing the completed window and retain
            // its offscreen buffer even though none of the built-in effects can change anymore.
            self.anim.is_clamped_done()
        } else {
            // A custom shader may use the full spring trajectory, including its overshoot.
            self.anim.is_done()
        }
    }

    pub fn progress(&self) -> f32 {
        self.anim.clamped_value().clamp(0., 1.) as f32
    }

    pub fn popin_ratio(&self) -> f64 {
        self.popin_ratio
            .unwrap_or(if self.lightweight {
                0.98
            } else if self.denial_reveal {
                1.
            } else {
                0.8
            })
            .clamp(0.01, 1.)
    }

    fn scale(&self) -> f64 {
        self.popin_ratio() + (1. - self.popin_ratio()) * f64::from(self.progress())
    }

    // We can't depend on view_rect here, because the result of window opening can be snapshot and
    // then rendered elsewhere.
    pub fn render(
        &self,
        renderer: &mut GlesRenderer,
        elements: &[impl RenderElement<GlesRenderer>],
        geo_size: Size<f64, Logical>,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> anyhow::Result<(OpeningWindowRenderElement, OffscreenData)> {
        let progress = self.anim.value();
        let clamped_progress = self.anim.clamped_value().clamp(0., 1.);

        let (elem, _sync_point, mut data) = self
            .buffer
            .render(renderer, scale, elements)
            .context("error rendering to offscreen buffer")?;

        if !self.lightweight && Shaders::get(renderer).program(ProgramType::Open).is_some() {
            // OffscreenBuffer renders with Transform::Normal and the scale that we passed, so we
            // can assume that below.
            let offset = elem.offset();
            let texture = elem.texture();
            let texture_size = elem.logical_size();

            let effect_rect = if self.denial_reveal {
                // Denial clips the complete child RepaintBoundary, including decorations and
                // shadow, rather than only the client's logical geometry.
                Rectangle::new(location + offset, texture_size)
            } else {
                Rectangle::new(location, geo_size)
            };
            let mut area = Rectangle::new(location + offset, texture_size);

            if self.denial_reveal {
                // The squircle never leaves the window. Only optional Glide can extend a corner a
                // little, so reserve a small, bounded margin instead of shading most of a 4K
                // output for every opening frame.
                if !self.glide.off {
                    let padding = (area.size.w.max(area.size.h) * 0.08).max(2.);
                    let diff = Point::from((padding, padding))
                        .to_physical_precise_round(scale)
                        .to_logical(scale);
                    area.loc -= diff;
                    area.size += diff.upscale(2.).to_size();
                }
            } else {
                // Preserve the historical generous canvas for user-supplied shaders.
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
            let tex_size = Vec2::new(texture.width() as f32, texture.height() as f32) / tex_scale;

            let geo_to_tex = if self.denial_reveal {
                // OffscreenBuffer may reuse a larger allocation after the snapshot shrinks; only
                // the current used prefix belongs to this window.
                Mat3::from_scale(effect_size / tex_size)
            } else {
                Mat3::from_translation(-tex_loc / tex_size)
                    * Mat3::from_scale(effect_size / tex_size)
            };

            let elem = ShaderRenderElement::new(
                ProgramType::Open,
                area.size,
                None,
                scale.x as f32,
                alpha,
                Rc::new([
                    mat3_uniform("realand_input_to_geo", input_to_geo),
                    Uniform::new("realand_geo_size", effect_size.to_array()),
                    mat3_uniform("realand_geo_to_tex", geo_to_tex),
                    Uniform::new("realand_progress", progress as f32),
                    Uniform::new("realand_clamped_progress", clamped_progress as f32),
                    Uniform::new("realand_random_seed", self.random_seed),
                    Uniform::new(
                        "realand_glide_enabled",
                        if self.glide.off { 0. } else { 1. },
                    ),
                    Uniform::new(
                        "realand_glide_tilt",
                        (self.glide.tilt_deg.to_radians()) as f32,
                    ),
                    Uniform::new("realand_glide_depth", self.glide.depth as f32),
                    Uniform::new(
                        "realand_popin_ratio",
                        self.popin_ratio
                            .unwrap_or(if self.denial_reveal { 1. } else { 0.8 })
                            .clamp(0.01, 1.) as f32,
                    ),
                ]),
                HashMap::from([(String::from("realand_tex"), texture.clone())]),
                Kind::Unspecified,
            )
            .with_location(area.loc);

            // We're drawing the shader, not the offscreen itself.
            data.id = elem.id().clone();

            return Ok((elem.into(), data));
        }

        let elem = elem.with_alpha(clamped_progress as f32 * alpha);

        let start_ratio = if self.lightweight {
            self.popin_ratio()
        } else {
            self.popin_ratio.unwrap_or(0.8)
        };
        let scale_progress = if self.denial_reveal {
            clamped_progress
        } else {
            progress
        };
        let current_scale = (start_ratio + (1.0 - start_ratio) * scale_progress).max(0.0);

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
    pub fn render_native(
        &self,
        renderer: &mut NativeRenderer,
        custom: Option<&str>,
        elements: &[impl RenderElement<NativeRenderer>],
        geo_size: Size<f64, Logical>,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> anyhow::Result<(OpeningWindowRenderElement, OffscreenData)> {
        let progress = self.anim.value();
        let clamped_progress = self.anim.clamped_value().clamp(0., 1.);
        let (texture, _sync, geo, states, _commit) = self
            .native_buffer
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
        if self.lightweight {
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
                self.progress() * alpha,
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
                self.scale(),
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
        let effect_rect = if self.denial_reveal {
            Rectangle::new(location + offset, texture_size)
        } else {
            Rectangle::new(location, geo_size)
        };
        let mut area = Rectangle::new(location + offset, texture_size);
        if self.denial_reveal {
            if !self.glide.off {
                let padding = (area.size.w.max(area.size.h) * 0.08).max(2.);
                let diff = Point::from((padding, padding))
                    .to_physical_precise_round(scale)
                    .to_logical(scale);
                area.loc -= diff;
                area.size += diff.upscale(2.).to_size();
            }
        } else {
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
        let tex_size = Vec2::new(texture.width() as f32, texture.height() as f32) / tex_scale;

        let geo_to_tex = if self.denial_reveal {
            // OffscreenBuffer may reuse a larger allocation after the snapshot shrinks; only
            // the current used prefix belongs to this window.
            Mat3::from_scale(effect_size / tex_size)
        } else {
            Mat3::from_translation(-tex_loc / tex_size) * Mat3::from_scale(effect_size / tex_size)
        };

        let elem = ShaderRenderElement::new_native(
            renderer,
            ProgramType::Open,
            &crate::render_helpers::shaders::animation_source(ProgramType::Open, custom),
            area.size,
            scale.x as f32,
            alpha,
            Rc::new([
                mat3_uniform("realand_input_to_geo", input_to_geo),
                Uniform::new("realand_geo_size", effect_size.to_array()),
                mat3_uniform("realand_geo_to_tex", geo_to_tex),
                Uniform::new("realand_progress", progress as f32),
                Uniform::new("realand_clamped_progress", clamped_progress as f32),
                Uniform::new("realand_random_seed", self.random_seed),
                Uniform::new(
                    "realand_glide_enabled",
                    if self.glide.off { 0. } else { 1. },
                ),
                Uniform::new(
                    "realand_glide_tilt",
                    (self.glide.tilt_deg.to_radians()) as f32,
                ),
                Uniform::new("realand_glide_depth", self.glide.depth as f32),
                Uniform::new(
                    "realand_popin_ratio",
                    self.popin_ratio
                        .unwrap_or(if self.denial_reveal { 1. } else { 0.8 })
                        .clamp(0.01, 1.) as f32,
                ),
            ]),
            HashMap::from([(String::from("realand_tex"), texture.clone())]),
            Kind::Unspecified,
        )?
        .with_location(area.loc);

        let data = OffscreenData {
            id: elem.id().clone(),
            states,
        };
        Ok((elem.into(), data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{Clock, Spring, SpringParams};
    use std::time::Duration;

    #[test]
    fn builtin_reveal_stops_when_visible_motion_finishes_with_a_spring() {
        for damping_ratio in [0., 0.4] {
            let mut clock = Clock::with_time(Duration::ZERO);
            let spring = Spring {
                from: 0.,
                to: 1.,
                initial_velocity: 0.,
                params: SpringParams::new(damping_ratio, 400., 0.0001),
            };
            let visible_end = spring.clamped_duration().unwrap();
            let anim = Animation::spring(clock.clone(), spring);
            assert!(visible_end < anim.duration());
            let builtin = OpenAnimation::new(
                anim.clone(),
                None,
                Default::default(),
                true,
                Default::default(),
            );
            let custom = OpenAnimation::new(
                anim.clone(),
                None,
                Default::default(),
                false,
                Default::default(),
            );

            clock.set_unadjusted(visible_end - Duration::from_millis(1));
            assert!(!builtin.is_done());
            assert!(!custom.is_done());

            clock.set_unadjusted(visible_end);
            assert_eq!(builtin.progress(), 1.);
            assert!(builtin.is_done());
            assert!(
                !custom.is_done(),
                "custom shaders retain their spring overshoot"
            );

            // Undamped custom shaders deliberately keep their full, unbounded trajectory.
            if damping_ratio == 0. {
                assert_eq!(anim.duration(), Duration::MAX);
                clock.set_unadjusted(Duration::from_secs(10));
                assert!(!custom.is_done());
            } else {
                clock.set_unadjusted(anim.end_time());
                assert!(custom.is_done());
            }
        }
    }

    #[test]
    fn builtin_reveal_stops_at_the_first_bouncy_easing_endpoint() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let config = realand_config::Animation {
            off: false,
            kind: realand_config::animations::Kind::Easing(
                realand_config::animations::EasingParams {
                    duration_ms: 1000,
                    curve: realand_config::animations::Curve::CubicBezier(0.68, -0.55, 0.265, 1.55),
                },
            ),
        };
        let animation = Animation::new(clock.clone(), 0., 1., 0., config);
        let builtin = OpenAnimation::new(
            animation.clone(),
            None,
            Default::default(),
            true,
            Default::default(),
        );
        let custom = OpenAnimation::new(
            animation.clone(),
            None,
            Default::default(),
            false,
            Default::default(),
        );
        let first_visible_end = (1..1000)
            .find(|millis| animation.value_at(Duration::from_millis(*millis)) >= 1.)
            .unwrap();

        clock.set_unadjusted(Duration::from_millis(first_visible_end - 1));
        assert!(!builtin.is_done());
        clock.set_unadjusted(Duration::from_millis(first_visible_end));
        assert_eq!(builtin.progress(), 1.);
        assert!(builtin.is_done());
        assert!(!custom.is_done());
        assert!(animation.value() > 1.);

        clock.set_unadjusted(Duration::from_millis(1000));
        assert!(custom.is_done());
    }

    #[test]
    fn builtin_reveal_obeys_reduced_motion_during_an_active_spring() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let anim = Animation::spring(
            clock.clone(),
            Spring {
                from: 0.,
                to: 1.,
                initial_velocity: 0.,
                params: SpringParams::new(0.4, 400., 0.0001),
            },
        );
        let open = OpenAnimation::new(anim, None, Default::default(), true, Default::default());
        clock.set_unadjusted(Duration::from_millis(16));
        assert!(!open.is_done());
        assert!(open.progress() < 1.);

        clock.set_complete_instantly(true);
        assert!(open.is_done());
        assert_eq!(open.progress(), 1.);
    }

    #[test]
    fn mapped_window_reveals_on_the_first_refresh_without_shortening_the_animation() {
        for hz in [60., 120., 144., 240., 360.] {
            let mut clock = Clock::with_time(Duration::ZERO);
            let config = realand_config::animations::WindowOpenAnim::default();
            let anim = Animation::new(clock.clone(), 0., 1., 0., config.anim);
            let open =
                OpenAnimation::new(anim, config.popin_ratio, config.glide, true, config.effect);
            assert_eq!(open.progress(), 0.);
            clock.set_unadjusted(Duration::from_secs_f64(1. / hz));
            assert!(
                open.progress() > 0.,
                "hz={hz}: first refresh has no feedback"
            );
            assert!(!open.is_done());
            clock.set_unadjusted(Duration::from_millis(179));
            assert!(!open.is_done());
            clock.set_unadjusted(Duration::from_millis(180));
            assert!(open.is_done());
            assert_eq!(open.progress(), 1.);
        }
    }
}
