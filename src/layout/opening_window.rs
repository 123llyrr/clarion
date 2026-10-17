//! Denial-derived reveal geometry and timing: Copyright (C) 2026 Doctor Logix.
//! GPL-3.0-or-later; see resources/licenses/denial-GPL-3.0-or-later.txt.

//! Renderer-independent opening trajectory and per-presentation numeric pose.

use crate::animation::Animation;

#[derive(Debug)]
pub struct OpenAnimation {
    anim: Animation,
    popin_ratio: Option<f64>,
    glide: clarion_config::Glide,
    denial_reveal: bool,
    lightweight: bool,
    random_seed: f32,
}

/// Immutable values captured for one output presentation. Contains neither a
/// clock nor a renderer resource, so rendering cannot advance animation state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OpeningWindowPose {
    pub(crate) progress: f64,
    pub(crate) clamped_progress: f64,
    /// Geometry for the built-in fade and renderer-independent fallback.
    pub(crate) scale: f64,
    /// Legacy GLES texture fallback has a distinct custom-shader scale policy.
    pub(crate) texture_scale: f64,
    pub(crate) shader_popin_ratio: f32,
    pub(crate) denial_reveal: bool,
    pub(crate) lightweight: bool,
    pub(crate) random_seed: f32,
    pub(crate) glide_enabled: bool,
    pub(crate) glide_tilt: f32,
    pub(crate) glide_depth: f32,
}

impl OpenAnimation {
    pub fn new(
        anim: Animation,
        popin_ratio: Option<f64>,
        glide: clarion_config::Glide,
        denial_reveal: bool,
        effect: clarion_config::animations::WindowOpenEffect,
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
                && effect == clarion_config::animations::WindowOpenEffect::Fade,
            random_seed: fastrand::f32(),
        }
    }

    pub fn is_done(&self) -> bool {
        if self.denial_reveal && !self.keeps_scale_tail() {
            // Reveal and physical springs finish at first arrival. In particular, an
            // undamped spring must not retain its snapshot and redraw indefinitely.
            self.anim.is_clamped_done()
        } else {
            // Timed fades keep their small scale rebound through the final resting pose.
            // Custom shaders retain their complete trajectory as before.
            self.anim.is_done()
        }
    }

    pub fn progress(&self) -> f32 {
        self.anim.clamped_value().clamp(0., 1.) as f32
    }

    pub fn popin_ratio(&self) -> f64 {
        self.popin_ratio
            .unwrap_or(if self.lightweight {
                0.90
            } else if self.denial_reveal {
                1.
            } else {
                0.8
            })
            .clamp(0.01, 1.)
    }

    fn keeps_scale_tail(&self) -> bool {
        // A bounded trajectory (fixed curve or damped spring) settles, so its
        // clamped rebound stays visible instead of being cut at first arrival.
        // An undamped spring never settles and must stop at arrival.
        self.lightweight
            && self.popin_ratio() < 1.
            && self.anim.duration() != std::time::Duration::MAX
    }

    /// Capture the output's already-projected animation time once. The scene
    /// receives only this value and never reaches back into the shared clock.
    pub(crate) fn pose(&self) -> OpeningWindowPose {
        let progress = self.anim.value();
        let clamped_progress = self.anim.clamped_value().clamp(0., 1.);
        let popin_ratio = self.popin_ratio();
        let scale_progress = if self.keeps_scale_tail() {
            // Opacity stops at first arrival, while geometry can briefly pass its
            // resting size. Bound user curves without extending the texture canvas.
            progress.clamp(0., 1.12)
        } else {
            f64::from(clamped_progress as f32)
        };
        let scale = if self.is_done() {
            1.
        } else {
            popin_ratio + (1. - popin_ratio) * scale_progress
        };
        let texture_scale = if self.lightweight {
            scale
        } else {
            // Custom shaders historically use the unbounded configured ratio
            // when GLES falls back to a texture. Keep that separate from the
            // shader ratio's clamp and the layout fallback's bounded geometry.
            let start_ratio = self.popin_ratio.unwrap_or(0.8);
            let progress = if self.denial_reveal {
                clamped_progress
            } else {
                progress
            };
            (start_ratio + (1. - start_ratio) * progress).max(0.)
        };
        OpeningWindowPose {
            progress,
            clamped_progress,
            scale,
            texture_scale,
            shader_popin_ratio: self
                .popin_ratio
                .unwrap_or(if self.denial_reveal { 1. } else { 0.8 })
                .clamp(0.01, 1.) as f32,
            denial_reveal: self.denial_reveal,
            lightweight: self.lightweight,
            random_seed: self.random_seed,
            glide_enabled: !self.glide.off,
            glide_tilt: self.glide.tilt_deg.to_radians() as f32,
            glide_depth: self.glide.depth as f32,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::animation::{Clock, Spring, SpringParams};

    #[test]
    fn builtin_reveal_keeps_a_damped_spring_rebound_and_stops_undamped_at_arrival() {
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
            if damping_ratio == 0. {
                // An undamped spring never settles: the reveal must stop at
                // first arrival instead of redrawing its snapshot forever.
                assert!(builtin.is_done());
                assert_eq!(anim.duration(), Duration::MAX);
                clock.set_unadjusted(Duration::from_secs(10));
                assert!(!custom.is_done());
            } else {
                // A damped spring settles, so its bounded rebound renders.
                assert!(
                    !builtin.is_done(),
                    "the bounded rebound still has to settle"
                );
                let peak = (0..=400)
                    .map(|millis| {
                        clock.set_unadjusted(Duration::from_millis(millis));
                        builtin.pose().scale
                    })
                    .fold(1.0_f64, f64::max);
                assert!(peak > 1.0005, "the rebound never renders: {peak}");
                assert!(peak < 1.12, "the rebound must stay bounded: {peak}");
                clock.set_unadjusted(anim.end_time());
                assert!(builtin.is_done());
                assert!(custom.is_done(), "custom shaders retain their overshoot");
            }
        }
    }

    #[test]
    fn builtin_reveal_stops_at_the_first_bouncy_easing_endpoint() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let config = clarion_config::Animation {
            off: false,
            kind: clarion_config::animations::Kind::Easing(
                clarion_config::animations::EasingParams {
                    duration_ms: 1000,
                    curve: clarion_config::animations::Curve::CubicBezier(0.68, -0.55, 0.265, 1.55),
                },
            ),
        };
        let animation = Animation::new(clock.clone(), 0., 1., 0., config);
        let builtin = OpenAnimation::new(
            animation.clone(),
            None,
            Default::default(),
            true,
            clarion_config::animations::WindowOpenEffect::Reveal,
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
            let config = clarion_config::animations::WindowOpenAnim::default();
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
            clock.set_unadjusted(Duration::from_millis(50));
            assert!(
                open.progress() < 1.,
                "hz={hz}: the reveal skipped its motion"
            );
            // The default spring owns its schedule: no refresh may skip ahead of
            // it, and the reveal must finish on the spring's own arrival.
            let mut settled_at = None;
            for millis in 51..2000 {
                clock.set_unadjusted(Duration::from_millis(millis));
                if open.is_done() {
                    settled_at = Some(millis);
                    break;
                }
            }
            let settled_at = settled_at.expect("the spring settles within a second");
            assert!(
                (200..=800).contains(&settled_at),
                "hz={hz}: a soft open settles in {settled_at}ms"
            );
            assert_eq!(open.progress(), 1.);
            assert_eq!(open.pose().scale, 1.);
        }
    }

    #[test]
    fn timed_fade_keeps_the_rebound_while_opacity_stays_at_full_strength() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let config = clarion_config::animations::WindowOpenAnim::default();
        let open = OpenAnimation::new(
            Animation::new(clock.clone(), 0., 1., 0., config.anim),
            config.popin_ratio,
            config.glide,
            true,
            config.effect,
        );
        assert_eq!(open.pose().scale, 0.90);
        let mut previous_opacity = 0.;
        let mut peak_scale = 1.;
        let mut settled_at = None;
        for millis in 0..=2000 {
            clock.set_unadjusted(Duration::from_millis(millis));
            let opacity = open.progress();
            assert!(
                opacity >= previous_opacity,
                "opacity reversed at {millis}ms"
            );
            previous_opacity = opacity;
            peak_scale = f64::max(peak_scale, open.pose().scale);
            if open.is_done() {
                settled_at = Some(millis);
                break;
            }
        }
        let settled_at = settled_at.expect("the spring settles");
        // The reveal rebounds past its resting size before settling.
        assert!(
            peak_scale > 1.0005,
            "no visible rebound: peak {peak_scale} at {settled_at}ms"
        );
        assert!(peak_scale < 1.02, "rebound too large: {peak_scale}");
        assert_eq!(open.pose().scale, 1.);
        assert!(open.anim.velocity().abs() < 0.01);
        // Opacity reaches full strength before the geometry finishes settling.
        let arrival = (0..=settled_at)
            .find(|millis| {
                clock.set_unadjusted(Duration::from_millis(*millis));
                open.progress() >= 1.
            })
            .expect("opacity arrives");
        assert!(
            arrival < settled_at,
            "opacity still rising after the geometry settled"
        );
    }

    #[test]
    fn opacity_only_fade_does_not_retain_an_invisible_scale_tail() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let config = clarion_config::animations::WindowOpenAnim::default();
        let open = OpenAnimation::new(
            Animation::new(clock.clone(), 0., 1., 0., config.anim),
            Some(1.),
            config.glide,
            true,
            config.effect,
        );
        clock.set_unadjusted(Duration::from_millis(20));
        assert!(open.progress() > 0.);
        assert!(!open.is_done());
        clock.set_unadjusted(Duration::from_millis(1000));
        assert_eq!(open.progress(), 1.);
        assert_eq!(open.pose().scale, 1.);
        assert!(open.is_done());
    }

    #[test]
    fn timed_fade_obeys_off_and_reduced_motion_without_a_residual_transform() {
        for (disabled, millis) in [(false, 0), (false, 16), (true, 0), (true, 16)] {
            let mut clock = Clock::with_time(Duration::ZERO);
            let mut config = clarion_config::animations::WindowOpenAnim::default();
            config.anim.off = disabled;
            let open = OpenAnimation::new(
                Animation::new(clock.clone(), 0., 1., 0., config.anim),
                config.popin_ratio,
                config.glide,
                true,
                config.effect,
            );
            clock.set_unadjusted(Duration::from_millis(millis));
            if !disabled {
                assert!(!open.is_done());
                clock.set_complete_instantly(true);
            }
            assert!(open.is_done());
            assert_eq!(open.progress(), 1.);
            assert_eq!(open.pose().scale, 1.);
        }
    }

    #[test]
    fn opening_pose_freezes_projected_time_and_restores_event_time() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OpeningWindowPose>();
        let mut clock = Clock::with_time(Duration::ZERO);
        let config = clarion_config::animations::WindowOpenAnim::default();
        let open = OpenAnimation::new(
            Animation::new(clock.clone(), 0., 1., 0., config.anim),
            config.popin_ratio,
            config.glide,
            true,
            config.effect,
        );
        clock.set_unadjusted(Duration::from_millis(16));
        let event_pose = open.pose();
        // Find the spring's own overshoot instead of assuming a fixed-time peak.
        let overshoot_ms = (17..900)
            .find(|millis| {
                let _projection = clock.project_to(Duration::from_millis(*millis));
                open.pose().scale > 1.
            })
            .expect("the default spring passes its resting size");
        let presented_pose = {
            let _projection = clock.project_to(Duration::from_millis(overshoot_ms));
            let projected = open.pose();
            assert!(projected.clamped_progress > event_pose.clamped_progress);
            assert!(projected.scale > 1.);
            projected
        };
        assert_eq!(open.pose(), event_pose);
        assert_eq!(presented_pose.random_seed, event_pose.random_seed);
        assert_eq!(event_pose.clamped_progress as f32, open.progress());
        clock.set_complete_instantly(true);
        assert_eq!(open.pose().clamped_progress, 1.);
        assert_eq!(open.pose().scale, 1.);
        // No shared clock or model reference is retained by a captured pose.
        drop(open);
        drop(clock);
        assert!(event_pose.clamped_progress < 1.);
        assert!(presented_pose.scale > 1.);
    }

    #[test]
    fn custom_shader_pose_retains_separate_fallback_scale_and_shader_ratio() {
        for (ratio, scale, texture_scale, shader_ratio) in [
            (None, 0.8, 0.8, 0.8),
            (Some(-0.5), 0.01, 0., 0.01),
            (Some(1.5), 1., 1.5, 1.),
        ] {
            let clock = Clock::with_time(Duration::ZERO);
            let config = clarion_config::animations::WindowOpenAnim::default();
            let open = OpenAnimation::new(
                Animation::new(clock, 0., 1., 0., config.anim),
                ratio,
                clarion_config::Glide {
                    off: false,
                    tilt_deg: -17.,
                    depth: 0.12,
                },
                false,
                config.effect,
            );
            let pose = open.pose();
            assert!(!pose.lightweight);
            assert!(!pose.denial_reveal);
            assert_eq!(pose.scale, scale);
            assert_eq!(pose.texture_scale, texture_scale);
            assert_eq!(pose.shader_popin_ratio, shader_ratio);
            assert!(pose.glide_enabled);
            assert_eq!(pose.glide_tilt, (-17_f64).to_radians() as f32);
            assert_eq!(pose.glide_depth, 0.12_f32);
        }
    }
}
