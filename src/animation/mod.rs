use std::time::Duration;

use clarion_motion::trajectory::Trajectory;
use clarion_motion::workspace_feedback::FeedbackParams;

mod bezier;
use bezier::CubicBezier;

mod spring;
pub use spring::{Spring, SpringParams};

mod clock;
pub use clock::Clock;

/// Platform configuration and rate-clock adapter for the pure core trajectory.
#[derive(Debug, Clone)]
pub struct Animation {
    clock: Clock,
    trajectory: Trajectory,
}

#[derive(Debug, Clone, Copy)]
pub enum Curve {
    Linear,
    EaseOutQuad,
    EaseOutCubic,
    EaseOutExpo,
    CubicBezier(CubicBezier),
}

impl Animation {
    /// Transfer numerical ownership to a pure controller. The platform clock
    /// and configuration adapter are consumed, never retained as a second target.
    pub(crate) fn into_trajectory(self) -> Trajectory {
        self.trajectory
    }

    pub fn new(
        clock: Clock,
        from: f64,
        to: f64,
        initial_velocity: f64,
        config: clarion_config::Animation,
    ) -> Self {
        // Translate real-second gesture velocity into the shared clock domain.
        let initial_velocity = initial_velocity / clock.rate().max(0.001);
        let mut animation = Self::ease(clock, from, to, initial_velocity, 0, Curve::EaseOutCubic);
        animation.replace_config(config);
        animation
    }

    /// Workspace indices use one unit per displayed desktop. Supply the actual
    /// physical-pixel distance instead of scaling completion by the remaining
    /// gesture distance. Core gesture policy already handles range rubber bands.
    pub fn workspace(
        clock: Clock,
        from: f64,
        to: f64,
        initial_velocity: f64,
        config: clarion_config::Animation,
        single_pixel: f64,
    ) -> Self {
        if config.kind != clarion_config::animations::Kind::WorkspaceFeedback {
            return Self::new(clock, from, to, initial_velocity, config);
        }
        let initial_velocity = initial_velocity / clock.rate().max(0.001);
        let mut trajectory = Trajectory::workspace_feedback_with_params_at(
            clock.now(),
            from,
            to,
            initial_velocity,
            FeedbackParams {
                single_pixel,
                ..FeedbackParams::default()
            },
        );
        if config.off {
            trajectory.disable();
        }
        Self { clock, trajectory }
    }

    pub fn replace_config(&mut self, config: clarion_config::Animation) {
        if config.off {
            self.trajectory.disable();
            return;
        }
        let now = self.clock.now();
        let active = now > self.trajectory.start_time()
            && !self
                .trajectory
                .is_done_at(now, self.clock.should_complete_instantly());
        let start_time = if active {
            now
        } else {
            self.trajectory.start_time()
        };
        let from = if active {
            self.trajectory.value_at(now, false)
        } else {
            self.trajectory.from()
        };
        let to = self.trajectory.to();
        // Sampling the core's clock-domain velocity avoids scaling real-time
        // momentum again when replacing an active animation's configuration.
        let initial_velocity = if active {
            self.trajectory.velocity_at(now, false)
        } else {
            self.trajectory.initial_velocity()
        };
        self.trajectory = match config.kind {
            clarion_config::animations::Kind::WorkspaceFeedback => {
                // This configuration also follows windows moving with their
                // workspace, where coordinates may be logical pixels. Retain
                // explicit display parameters when present; otherwise use a
                // relative tolerance for these coordinate-agnostic followers.
                let params = self
                    .trajectory
                    .workspace_feedback_params()
                    .unwrap_or_else(|| {
                        let extent = (to - from).abs().max(1.);
                        FeedbackParams {
                            single_pixel: extent / 1080.,
                            velocity_epsilon: extent * 0.01,
                            ..FeedbackParams::default()
                        }
                    });
                Trajectory::workspace_feedback_with_params_at(
                    start_time,
                    from,
                    to,
                    initial_velocity,
                    params,
                )
            }
            clarion_config::animations::Kind::Spring(p) => {
                let _span = clarion_tracy_span!("Animation::spring");
                Trajectory::spring_at(
                    start_time,
                    Spring {
                        from,
                        to,
                        initial_velocity,
                        params: SpringParams::new(
                            p.damping_ratio,
                            f64::from(p.stiffness),
                            p.epsilon,
                        ),
                    },
                )
            }
            clarion_config::animations::Kind::Easing(p) => {
                let duration = Duration::from_millis(u64::from(p.duration_ms));
                if active || initial_velocity != 0. {
                    Trajectory::continuous_ease_at(start_time, from, to, initial_velocity, duration)
                } else {
                    Trajectory::ease_at(
                        start_time,
                        from,
                        to,
                        initial_velocity,
                        duration,
                        Curve::from(p.curve).into_core(),
                    )
                }
            }
        };
    }

    /// Restart the retained configuration, preserving real-time momentum.
    pub fn restarted(&self, from: f64, to: f64, initial_velocity: f64) -> Self {
        let initial_velocity = initial_velocity / self.clock.rate().max(0.001);
        Self {
            trajectory: self
                .trajectory
                .restarted_at(self.clock.now(), from, to, initial_velocity),
            clock: self.clock.clone(),
        }
    }

    /// Begin a fresh retained preset after a deferred capture becomes ready.
    pub fn restarted_fresh(&self, from: f64, to: f64) -> Self {
        Self {
            trajectory: self
                .trajectory
                .restarted_fresh_at(self.clock.now(), from, to),
            clock: self.clock.clone(),
        }
    }

    pub fn ease(
        clock: Clock,
        from: f64,
        to: f64,
        initial_velocity: f64,
        duration_ms: u64,
        curve: Curve,
    ) -> Self {
        Self {
            trajectory: Trajectory::ease_at(
                clock.now(),
                from,
                to,
                initial_velocity,
                Duration::from_millis(duration_ms),
                curve.into_core(),
            ),
            clock,
        }
    }

    pub fn continuous_ease(
        clock: Clock,
        from: f64,
        to: f64,
        initial_velocity: f64,
        duration_ms: u64,
    ) -> Self {
        let initial_velocity = initial_velocity / clock.rate().max(0.001);
        Self {
            trajectory: Trajectory::continuous_ease_at(
                clock.now(),
                from,
                to,
                initial_velocity,
                Duration::from_millis(duration_ms),
            ),
            clock,
        }
    }

    pub fn spring(clock: Clock, spring: Spring) -> Self {
        let _span = clarion_tracy_span!("Animation::spring");
        Self {
            trajectory: Trajectory::spring_at(clock.now(), spring),
            clock,
        }
    }

    pub fn decelerate(
        clock: Clock,
        from: f64,
        initial_velocity: f64,
        deceleration_rate: f64,
        threshold: f64,
    ) -> Self {
        Self {
            trajectory: Trajectory::decelerate_at(
                clock.now(),
                from,
                initial_velocity,
                deceleration_rate,
                threshold,
            ),
            clock,
        }
    }

    pub fn is_done(&self) -> bool {
        self.trajectory
            .is_done_at(self.clock.now(), self.clock.should_complete_instantly())
    }

    pub fn is_clamped_done(&self) -> bool {
        self.trajectory
            .is_clamped_done_at(self.clock.now(), self.clock.should_complete_instantly())
    }

    pub fn value_at(&self, at: Duration) -> f64 {
        self.trajectory
            .value_at(at, self.clock.should_complete_instantly())
    }

    pub fn value(&self) -> f64 {
        self.value_at(self.clock.now())
    }

    /// Current velocity in units per real second, suitable for `new`/`restarted`.
    pub fn velocity(&self) -> f64 {
        self.trajectory
            .velocity_at(self.clock.now(), self.clock.should_complete_instantly())
            * self.clock.rate()
    }

    pub fn clamped_value(&self) -> f64 {
        self.trajectory
            .clamped_value_at(self.clock.now(), self.clock.should_complete_instantly())
    }

    pub fn to(&self) -> f64 {
        self.trajectory.to()
    }
    pub fn from(&self) -> f64 {
        self.trajectory.from()
    }
    pub fn start_time(&self) -> Duration {
        self.trajectory.start_time()
    }
    pub fn end_time(&self) -> Duration {
        self.trajectory.end_time()
    }
    pub fn duration(&self) -> Duration {
        self.trajectory.duration()
    }

    /// Delay the start without shortening the retained motion.
    pub fn delay_by(&mut self, delay: Duration) {
        self.trajectory.delay_by(delay);
    }
    pub fn offset(&mut self, offset: f64) {
        self.trajectory.offset(offset);
    }
}

impl Curve {
    /// Native macOS Core Animation default curve: kCAMediaTimingFunctionDefault (0.25, 0.1, 0.25,
    /// 1.0)
    pub const MACOS_DEFAULT: Self = Self::CubicBezier(CubicBezier::MACOS_DEFAULT);
    pub const CA_DEFAULT: Self = Self::MACOS_DEFAULT;
    /// Native macOS Core Animation linear curve: kCAMediaTimingFunctionLinear (0.0, 0.0, 1.0, 1.0)
    pub const MACOS_LINEAR: Self = Self::Linear;
    pub const CA_LINEAR: Self = Self::Linear;
    /// Native macOS Core Animation ease-in curve: kCAMediaTimingFunctionEaseIn (0.42, 0.0, 1.0,
    /// 1.0)
    pub const MACOS_EASE_IN: Self = Self::CubicBezier(CubicBezier::MACOS_EASE_IN);
    pub const CA_EASE_IN: Self = Self::MACOS_EASE_IN;
    /// Native macOS Core Animation ease-out curve: kCAMediaTimingFunctionEaseOut (0.0, 0.0, 0.58,
    /// 1.0)
    pub const MACOS_EASE_OUT: Self = Self::CubicBezier(CubicBezier::MACOS_EASE_OUT);
    pub const CA_EASE_OUT: Self = Self::MACOS_EASE_OUT;
    /// Native macOS Core Animation ease-in-ease-out curve: kCAMediaTimingFunctionEaseInEaseOut
    /// (0.42, 0.0, 0.58, 1.0)
    pub const MACOS_EASE_IN_EASE_OUT: Self = Self::CubicBezier(CubicBezier::MACOS_EASE_IN_EASE_OUT);
    pub const CA_EASE_IN_EASE_OUT: Self = Self::MACOS_EASE_IN_EASE_OUT;

    pub fn y(self, x: f64) -> f64 {
        self.into_core().sample(x)
    }

    fn into_core(self) -> clarion_motion::trajectory::Curve {
        use clarion_motion::trajectory::Curve as CoreCurve;
        match self {
            Self::Linear => CoreCurve::Linear,
            Self::EaseOutQuad => CoreCurve::EaseOutQuad,
            Self::EaseOutCubic => CoreCurve::EaseOutCubic,
            Self::EaseOutExpo => CoreCurve::EaseOutExpo,
            Self::CubicBezier(bezier) => CoreCurve::CubicBezier(bezier.into_core()),
        }
    }
}

impl From<clarion_config::animations::Curve> for Curve {
    fn from(value: clarion_config::animations::Curve) -> Self {
        match value {
            clarion_config::animations::Curve::Linear => Curve::Linear,
            clarion_config::animations::Curve::EaseOutQuad => Curve::EaseOutQuad,
            clarion_config::animations::Curve::EaseOutCubic => Curve::EaseOutCubic,
            clarion_config::animations::Curve::EaseOutExpo => Curve::EaseOutExpo,
            clarion_config::animations::Curve::CubicBezier(x1, y1, x2, y2) => {
                Curve::CubicBezier(CubicBezier::new(x1, y1, x2, y2))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_feedback_uses_output_forecasts_without_consuming_owner_time() {
        for rate in [0.5, 1., 2.] {
            let mut clock = Clock::with_time(Duration::ZERO);
            clock.set_rate(rate);
            let config = clarion_config::Animation {
                off: false,
                kind: clarion_config::animations::Kind::WorkspaceFeedback,
            };
            let single_pixel = 1. / 2112.;
            let mut anim = Animation::workspace(clock.clone(), 2., 3., 0.4, config, single_pixel);
            clock.set_unadjusted(Duration::from_millis(50));
            let owner = (anim.value(), anim.velocity());
            for ms in [100, 150, 90, 220] {
                let _projection = clock.project_to(Duration::from_millis(ms));
                assert!(anim.value().is_finite());
                assert!(anim.velocity().is_finite());
            }
            assert_eq!((anim.value(), anim.velocity()), owner);
            anim.replace_config(config);
            assert_eq!(
                anim.trajectory
                    .workspace_feedback_params()
                    .unwrap()
                    .single_pixel,
                single_pixel
            );
            assert!((anim.value() - owner.0).abs() < 1e-12);
            assert!((anim.velocity() - owner.1).abs() < 1e-9);
            let reversed = anim.restarted(anim.value(), 2., anim.velocity());
            assert_eq!(reversed.value(), anim.value());
            assert!((reversed.velocity() - anim.velocity()).abs() < 1e-9);
        }
    }

    #[test]
    fn workspace_adapter_honors_existing_overrides_and_off() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let config = clarion_config::Animation {
            off: true,
            kind: clarion_config::animations::Kind::WorkspaceFeedback,
        };
        let disabled = Animation::workspace(clock.clone(), 2., 3., 0.4, config, 1. / 1440.);
        assert!(disabled.is_done());
        // Existing Animation semantics retain the origin in the same event
        // cycle even when disabled, then expose the destination on the next tick.
        assert_eq!(disabled.value(), 2.);
        clock.set_unadjusted(Duration::from_nanos(1));
        assert_eq!(disabled.value(), 3.);
        let custom = easing_config(clarion_config::animations::Curve::Linear);
        let animation = Animation::workspace(clock, 2., 3., 0., custom, 1. / 1440.);
        assert!(animation.trajectory.workspace_feedback_params().is_none());
        assert!(animation.trajectory.has_fixed_duration());
    }

    fn easing_config(curve: clarion_config::animations::Curve) -> clarion_config::Animation {
        clarion_config::Animation {
            off: false,
            kind: clarion_config::animations::Kind::Easing(
                clarion_config::animations::EasingParams {
                    duration_ms: 400,
                    curve,
                },
            ),
        }
    }

    #[test]
    fn easing_reversal_retains_real_velocity_at_all_clock_rates() {
        for rate in [0.25, 0.73, 1., 2.] {
            for curve in [
                clarion_config::animations::Curve::Linear,
                clarion_config::animations::Curve::EaseOutQuad,
                clarion_config::animations::Curve::EaseOutCubic,
                clarion_config::animations::Curve::EaseOutExpo,
                clarion_config::animations::Curve::CubicBezier(0.25, 0.1, 0.25, 1.),
            ] {
                let mut clock = Clock::with_time(Duration::ZERO);
                clock.set_rate(rate);
                let animation = Animation::new(clock.clone(), 0., 1., 0., easing_config(curve));
                clock.set_unadjusted(Duration::from_millis(80));
                let pose = animation.value();
                let velocity = animation.velocity();
                let reversed = animation.restarted(pose, 0., velocity);
                assert_eq!(reversed.value(), pose);
                assert!((reversed.velocity() - velocity).abs() < 1e-12);
                clock.set_unadjusted(Duration::from_nanos(80_000_100));
                let measured = (reversed.value() - pose) / 1e-7;
                assert!(
                    (measured - velocity).abs() < 0.001,
                    "rate={rate}, curve={curve:?}"
                );
                clock.set_unadjusted(Duration::from_secs(10));
                assert_eq!(reversed.value(), 0.);
                assert_eq!(reversed.velocity(), 0.);
            }
        }
    }

    #[test]
    fn configured_easing_with_incoming_velocity_preserves_momentum() {
        for rate in [0.25, 0.73, 1., 2.] {
            for velocity in [-3., 3.] {
                let mut clock = Clock::with_time(Duration::ZERO);
                clock.set_rate(rate);
                let animation = Animation::new(
                    clock.clone(),
                    0.5,
                    0.5,
                    velocity,
                    easing_config(clarion_config::animations::Curve::EaseOutCubic),
                );
                assert_eq!(animation.value(), 0.5);
                assert!((animation.velocity() - velocity).abs() < 1e-12);
                assert!(!animation.is_done());
                clock.set_unadjusted(Duration::from_nanos(100));
                let measured = (animation.value() - 0.5) / 1e-7;
                assert!((measured - velocity).abs() < 0.00001);
            }
        }
    }

    #[test]
    fn replacing_active_configuration_samples_pose_and_domain_velocity_once() {
        for rate in [0.25, 0.73, 1., 2.] {
            for kind in 0..4 {
                let mut clock = Clock::with_time(Duration::ZERO);
                clock.set_rate(rate);
                let mut animation = match kind {
                    0 => Animation::new(
                        clock.clone(),
                        0.,
                        1.,
                        0.,
                        easing_config(clarion_config::animations::Curve::EaseOutCubic),
                    ),
                    1 => Animation::spring(
                        clock.clone(),
                        Spring {
                            from: 0.,
                            to: 1.,
                            initial_velocity: 0.,
                            params: SpringParams::new(0.92, 300., 0.0001),
                        },
                    ),
                    2 => Animation::decelerate(clock.clone(), 0., 3., 0.997, 0.001),
                    _ => Animation::continuous_ease(clock.clone(), 0., 1., 2., 400),
                };
                clock.set_unadjusted(Duration::from_millis(80));
                let pose = animation.value();
                let velocity = animation.velocity();
                animation.replace_config(easing_config(clarion_config::animations::Curve::Linear));
                assert_eq!(animation.value(), pose);
                assert!(
                    (animation.velocity() - velocity).abs() < 1e-12,
                    "rate={rate}, kind={kind}"
                );
                clock.set_unadjusted(Duration::from_nanos(80_000_100));
                let measured = (animation.value() - pose) / 1e-7;
                assert!(
                    (measured - velocity).abs() < 0.001,
                    "rate={rate}, kind={kind}"
                );
                let pose = animation.value();
                let velocity = animation.velocity();
                animation.replace_config(clarion_config::Animation {
                    off: false,
                    kind: clarion_config::animations::Kind::Spring(
                        clarion_config::animations::SpringParams::MACOS_INTERACTIVE,
                    ),
                });
                assert_eq!(animation.value(), pose);
                assert!((animation.velocity() - velocity).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn fresh_configured_presets_and_waiting_close_restarts_keep_curve_samples() {
        for curve in [
            clarion_config::animations::Curve::Linear,
            clarion_config::animations::Curve::EaseOutQuad,
            clarion_config::animations::Curve::EaseOutCubic,
            clarion_config::animations::Curve::EaseOutExpo,
            clarion_config::animations::Curve::CubicBezier(0.25, 0.1, 0.25, 1.),
        ] {
            let mut clock = Clock::with_time(Duration::ZERO);
            let fresh = Animation::new(clock.clone(), 0., 1., 0., easing_config(curve));
            let oracle = Animation::ease(clock.clone(), 0., 1., 0., 400, curve.into());
            for millis in [0, 1, 16, 80, 200, 399, 400] {
                clock.set_unadjusted(Duration::from_millis(millis));
                assert_eq!(fresh.value().to_bits(), oracle.value().to_bits());
            }
            clock.set_unadjusted(Duration::from_millis(80));
            let restarted = fresh.restarted_fresh(0., 1.);
            let oracle = Animation::new(clock.clone(), 0., 1., 0., easing_config(curve));
            for millis in [80, 81, 96, 160, 280, 479, 480] {
                clock.set_unadjusted(Duration::from_millis(millis));
                assert_eq!(restarted.value().to_bits(), oracle.value().to_bits());
            }
        }
    }

    #[test]
    fn reversal_preserves_position_and_velocity_at_every_clock_rate() {
        for rate in [0.25, 0.73, 1., 2.] {
            let mut clock = Clock::with_time(Duration::ZERO);
            clock.set_rate(rate);
            let anim = Animation::new(
                clock.clone(),
                0.,
                1.,
                0.,
                clarion_config::animations::WorkspaceSwitchAnim::default().0,
            );
            clock.set_unadjusted(Duration::from_millis(80));
            let before = anim.value();
            let velocity = anim.velocity();
            assert!(velocity > 0.);
            let reversed = anim.restarted(before, 0., velocity);
            assert!((reversed.value() - before).abs() < 1e-10);
            assert!((reversed.velocity() - velocity).abs() < 1e-10);
            clock.set_unadjusted(Duration::from_nanos(80_000_100));
            let actual_velocity = (reversed.value() - before) / 0.0000001;
            assert!(
                (actual_velocity - velocity).abs() < 0.001,
                "rate={rate}, expected={velocity}, actual={actual_velocity}"
            );
            clock.set_unadjusted(Duration::from_secs(10));
            assert_eq!(reversed.value(), 0.);
            assert_eq!(reversed.velocity(), 0.);
            assert!(reversed.is_done());
        }
    }

    #[test]
    fn same_position_target_does_not_discard_momentum() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let anim = Animation::new(
            clock.clone(),
            0.,
            0.,
            10.,
            clarion_config::animations::WorkspaceSwitchAnim::default().0,
        );
        assert!(!anim.is_done());
        assert_eq!(anim.velocity(), 10.);
        clock.set_unadjusted(Duration::from_millis(16));
        assert!(anim.value() > 0.);
        clock.set_unadjusted(Duration::from_secs(1));
        assert!(anim.is_done());
        assert_eq!(anim.value(), 0.);
    }

    #[test]
    fn delayed_and_disabled_animations_have_no_velocity() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let mut anim = Animation::ease(clock.clone(), 0., 1., 0., 100, Curve::Linear);
        anim.delay_by(Duration::from_millis(50));
        assert_eq!(anim.velocity(), 0.);
        clock.set_unadjusted(Duration::from_millis(50));
        assert!((anim.velocity() - 10.).abs() < 1e-8);
        clock.set_complete_instantly(true);
        assert_eq!(anim.velocity(), 0.);
        clock.set_complete_instantly(false);
        let mut config = clarion_config::animations::WorkspaceSwitchAnim::default().0;
        config.off = true;
        let anim = Animation::new(clock.clone(), 0., 1., 10., config);
        let restarted = anim.restarted(20., 40., 15.);
        clock.set_unadjusted(Duration::from_millis(51));
        assert!(restarted.is_done());
        assert_eq!(restarted.velocity(), 0.);
        assert_eq!(restarted.value(), 40.);
    }

    #[test]
    fn delayed_animation_holds_first_frame_without_shortening_motion() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let mut animation = Animation::ease(clock.clone(), 0., 1., 0., 100, Curve::Linear);
        animation.delay_by(Duration::from_millis(64));

        clock.set_unadjusted(Duration::from_millis(63));
        assert_eq!(animation.value(), 0.);
        assert!(!animation.is_done());

        clock.set_unadjusted(Duration::from_millis(114));
        assert_eq!(animation.value(), 0.5);

        clock.set_unadjusted(Duration::from_millis(164));
        assert_eq!(animation.value(), 1.);
        assert!(animation.is_done());
    }

    #[test]
    fn restarted_spring_uses_the_new_velocity() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let moving = Animation::spring(
            clock.clone(),
            Spring {
                from: 0.,
                to: 1.,
                initial_velocity: 12.,
                params: SpringParams::new(1., 800., 0.0001),
            },
        );
        let rest = Animation::spring(
            clock.clone(),
            Spring {
                from: 0.4,
                to: 1.,
                initial_velocity: 0.,
                params: SpringParams::new(1., 800., 0.0001),
            },
        );
        let restarted = moving.restarted(0.4, 1., 0.);
        clock.set_unadjusted(Duration::from_millis(16));
        assert!((restarted.value() - rest.value()).abs() < 1e-9);
        assert!((restarted.value() - moving.value()).abs() > 1e-3);
    }

    #[test]
    fn unbounded_spring_does_not_overflow_animation_deadlines() {
        let clock = Clock::with_time(Duration::from_secs(1));
        let animation = Animation::spring(
            clock,
            Spring {
                from: 0.,
                to: 1.,
                initial_velocity: 0.,
                params: SpringParams::new(0., 800., 0.0001),
            },
        );

        assert_eq!(animation.duration(), Duration::MAX);
        assert_eq!(animation.end_time(), Duration::MAX);
        assert!(!animation.is_done());
        assert!(!animation.is_clamped_done());
        assert!(animation.value_at(Duration::from_secs(2)).is_finite());
    }

    #[test]
    fn exponential_easing_reaches_both_endpoints_smoothly() {
        assert_eq!(Curve::EaseOutExpo.y(0.), 0.);
        assert_eq!(Curve::EaseOutExpo.y(1.), 1.);
        assert!(Curve::EaseOutExpo.y(0.999) < 1.);
    }

    #[test]
    fn overshooting_easing_clamps_after_its_first_target_crossing() {
        let curves = [
            Curve::CubicBezier(CubicBezier::new(0.05, 0.9, 0.1, 1.05)),
            Curve::CubicBezier(CubicBezier::new(0.68, -0.55, 0.265, 1.55)),
            // A custom curve can touch the endpoint, then return below it.
            Curve::CubicBezier(CubicBezier::new(0.2, 4., 0.8, -2.)),
        ];
        for curve in curves {
            for duration in [320, 1000] {
                let crossing = (1..duration)
                    .find(|millis| curve.y(*millis as f64 / duration as f64) >= 1.)
                    .unwrap();
                for (from, to) in [(0., 1.), (1., 0.)] {
                    let mut clock = Clock::with_time(Duration::ZERO);
                    let animation = Animation::ease(clock.clone(), from, to, 0., duration, curve);
                    clock.set_unadjusted(Duration::from_millis(crossing - 1));
                    assert!(!animation.is_clamped_done());
                    clock.set_unadjusted(Duration::from_millis(crossing));
                    assert!(
                        animation.is_clamped_done(),
                        "{curve:?}, duration={duration}"
                    );
                    assert_eq!(animation.clamped_value(), to);
                    assert!(
                        !animation.is_done(),
                        "the full trajectory keeps its duration"
                    );
                    assert_ne!(animation.value(), to);
                    for millis in [crossing + (duration - crossing) / 2, duration - 1] {
                        clock.set_unadjusted(Duration::from_millis(millis));
                        let expected =
                            curve.y(millis as f64 / duration as f64) * (to - from) + from;
                        assert!((animation.value() - expected).abs() < 1e-12);
                        assert!(!animation.is_done());
                        assert_eq!(animation.clamped_value(), to);
                    }
                }
            }
        }
    }

    #[test]
    fn spring_clamped_progress_never_overshoots_between_milliseconds() {
        for rate in [0.25, 0.73, 1., 2.] {
            for hz in [60., 120., 144., 240., 360.] {
                for (from, to) in [(0., 1.), (1., 0.)] {
                    let mut clock = Clock::with_time(Duration::ZERO);
                    clock.set_rate(rate);
                    let spring = Spring {
                        from,
                        to,
                        initial_velocity: 0.,
                        params: SpringParams::new(0.2, 300., 0.001),
                    };
                    let animation = Animation::spring(clock.clone(), spring);
                    let mut completed = false;
                    for frame in 1..=(hz * 10.) as usize {
                        clock.set_unadjusted(Duration::from_secs_f64(frame as f64 / hz));
                        let progress = animation.clamped_value();
                        assert!((from.min(to)..=from.max(to)).contains(&progress));
                        if animation.is_clamped_done() {
                            assert_eq!(progress, to);
                            completed = true;
                            break;
                        }
                    }
                    assert!(completed, "rate={rate}, hz={hz}");
                }
            }
        }
    }

    #[test]
    fn clamped_spring_keeps_launch_motion_away_from_the_target() {
        let mut clock = Clock::with_time(Duration::ZERO);
        for (from, to, initial_velocity) in [(0., 1., -100.), (1., 0., 100.)] {
            let animation = Animation::spring(
                clock.clone(),
                Spring {
                    from,
                    to,
                    initial_velocity,
                    params: SpringParams::new(0.92, 300., 0.001),
                },
            );
            clock.set_unadjusted(animation.start_time() + Duration::from_micros(10));
            assert_eq!(animation.clamped_value(), animation.value());
            assert!((animation.value() - from) * initial_velocity > 0.);
        }
    }

    #[test]
    fn near_target_spring_reversal_keeps_visible_pose_and_velocity() {
        for rate in [0.25, 0.73, 1., 2.] {
            for (from, to, velocity) in [(0.9996, 1., -0.1), (0.0004, 0., 0.1)] {
                let mut clock = Clock::with_time(Duration::ZERO);
                clock.set_rate(rate);
                let config = clarion_config::Animation {
                    off: false,
                    kind: clarion_config::animations::Kind::Spring(
                        clarion_config::animations::SpringParams::MACOS_INTERACTIVE,
                    ),
                };
                let animation = Animation::new(clock.clone(), from, to, velocity, config);
                assert_eq!(animation.clamped_value(), from);
                assert!(!animation.is_clamped_done());
                assert!((animation.velocity() - velocity).abs() < 1e-12);
                let delta = Duration::from_nanos(100);
                clock.set_unadjusted(delta);
                assert!((animation.clamped_value() - from) * velocity > 0.);
                let slope = (animation.clamped_value() - from) / delta.as_secs_f64();
                assert!((slope - velocity).abs() < 0.00001, "rate={rate}");
            }
        }
    }

    #[test]
    fn full_spring_completion_also_finishes_the_clamped_view() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let animation = Animation::spring(
            clock.clone(),
            Spring {
                from: 2400.,
                to: 0.,
                initial_velocity: 0.,
                params: SpringParams::new(1., 300., 0.001),
            },
        );
        assert_eq!(
            animation.trajectory.clamped_duration(),
            animation.duration()
        );
        clock.set_unadjusted(animation.end_time());
        assert!(animation.is_done());
        assert!(animation.is_clamped_done());
        assert_eq!(animation.clamped_value(), 0.);
    }

    #[test]
    fn easing_without_overshoot_keeps_the_complete_clamped_timeline() {
        let curves = [
            Curve::Linear,
            Curve::EaseOutQuad,
            Curve::EaseOutCubic,
            Curve::EaseOutExpo,
            Curve::CubicBezier(CubicBezier::new(0., 0., 0.58, 1.)),
            Curve::CubicBezier(CubicBezier::new(1., 0., 0., 1.)),
        ];
        for curve in curves {
            for (from, to) in [(0., 1.), (1., 0.)] {
                let mut clock = Clock::with_time(Duration::ZERO);
                let animation = Animation::ease(clock.clone(), from, to, 0., 1000, curve);
                assert_eq!(
                    animation.trajectory.clamped_duration(),
                    animation.duration()
                );
                for millis in [0, 100, 250, 500, 750, 999] {
                    clock.set_unadjusted(Duration::from_millis(millis));
                    assert!(!animation.is_clamped_done());
                    assert_eq!(animation.clamped_value(), animation.value());
                }
                clock.set_unadjusted(Duration::from_millis(1000));
                assert!(animation.is_clamped_done());
                assert_eq!(animation.clamped_value(), to);
            }
        }
    }

    #[test]
    fn clamped_bezier_deadline_follows_the_first_tangent_endpoint() {
        // x(t)=t and y(t) touches 1 at t=1/4 before returning below it.
        let curve = Curve::CubicBezier(CubicBezier::new(1. / 3., 3., 2. / 3., -2.));
        let mut clock = Clock::with_time(Duration::ZERO);
        let animation = Animation::ease(clock.clone(), 0., 1., 0., 1000, curve);
        let crossing = Duration::from_millis(250);
        assert_eq!(
            animation.trajectory.clamped_duration(),
            crossing + Duration::from_nanos(1)
        );
        clock.set_unadjusted(crossing - Duration::from_nanos(1));
        assert!(!animation.is_clamped_done());
        clock.set_unadjusted(crossing);
        assert!(!animation.is_clamped_done());
        assert_eq!(animation.value(), 1.);
        clock.set_unadjusted(crossing + Duration::from_nanos(1));
        assert!(animation.is_clamped_done());
        assert_eq!(animation.clamped_value(), 1.);
        clock.set_unadjusted(Duration::from_millis(500));
        assert!(animation.value() < 1.);
        assert_eq!(animation.clamped_value(), 1.);
        assert!(!animation.is_done());
    }

    #[test]
    fn deceleration_duration_reaches_the_requested_speed_threshold() {
        let clock = Clock::with_time(Duration::ZERO);
        let animation = Animation::decelerate(clock, 0., 10., 0.99, 0.1);
        let coeff = 1000. * 0.99f64.ln();
        let expected = Duration::from_secs_f64((0.1f64 / 10.).ln() / coeff);

        assert!(animation.duration().abs_diff(expected) <= Duration::from_nanos(1));
        assert!(animation.duration() < Duration::from_secs(10));
    }

    #[test]
    fn deceleration_handles_degenerate_thresholds_and_rates() {
        for threshold in [0., -1., f64::NAN] {
            let animation =
                Animation::decelerate(Clock::with_time(Duration::ZERO), 0., 10., 0.99, threshold);
            assert_eq!(animation.duration(), Duration::MAX);
            assert!(animation.value_at(Duration::from_secs(1)).is_finite());
        }

        let already_slow =
            Animation::decelerate(Clock::with_time(Duration::ZERO), 0., 0.1, 0.99, 1.);
        assert_eq!(already_slow.duration(), Duration::ZERO);

        for rate in [-1., 0., 1., 1.000001, f64::NAN, f64::INFINITY] {
            let animation =
                Animation::decelerate(Clock::with_time(Duration::ZERO), 0., 10., rate, 0.1);
            assert!(animation.duration() > Duration::ZERO);
            assert!(animation.value_at(Duration::from_millis(1)).is_finite());
        }
    }
}
