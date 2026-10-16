//! Numeric canvas camera kinetics. The platform supplies accepted input/event
//! time and consumes displacement; presentation forecasts never own state.
//!
//! Clarion's retained parameters, adapted from Nourish, are not verified Apple
//! timing curves. See docs/nourish-port.md for provenance and licensing.

use std::time::Duration;

const INERTIA_DELAY: Duration = Duration::from_millis(80);
const VISCOUS_FRICTION: f64 = 3.6;
const CONSTANT_DECELERATION: f64 = 400.;
const STOP_SPEED: f64 = 8.;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CanvasPan {
    offset: [f64; 2],
    velocity: [f64; 2],
    last_input: Option<Duration>,
    last_tick: Option<Duration>,
}

impl CanvasPan {
    pub fn offset(&self) -> [f64; 2] {
        self.offset
    }
    pub fn velocity(&self) -> [f64; 2] {
        self.velocity
    }

    pub fn pan(&mut self, delta: [f64; 2], now: Duration, kinetic: bool) -> [f64; 2] {
        if !delta[0].is_finite() || !delta[1].is_finite() {
            return [0.; 2];
        }
        if kinetic {
            if let Some(last) = self.last_input {
                let dt = now.saturating_sub(last).as_secs_f64();
                if dt > 0. && dt < 0.15 {
                    self.velocity = [
                        (delta[0] / dt).clamp(-10000., 10000.),
                        (delta[1] / dt).clamp(-10000., 10000.),
                    ];
                } else {
                    self.velocity = [0.; 2];
                }
            } else {
                // A new first sample takes ownership from coasting. Its own
                // interval is unknown until the next accepted input sample.
                self.velocity = [0.; 2];
            }
            self.last_input = Some(now);
        } else {
            self.stop();
        }
        self.last_tick = Some(now);
        self.translate(delta)
    }

    fn translate(&mut self, delta: [f64; 2]) -> [f64; 2] {
        let next = [
            (self.offset[0] + delta[0]).clamp(-10_000_000., 10_000_000.),
            (self.offset[1] + delta[1]).clamp(-10_000_000., 10_000_000.),
        ];
        let applied = [next[0] - self.offset[0], next[1] - self.offset[1]];
        self.offset = next;
        applied
    }

    pub fn stop(&mut self) {
        self.velocity = [0.; 2];
        self.last_input = None;
    }
    pub fn ongoing(&self) -> bool {
        self.velocity != [0.; 2]
    }

    /// Resetting returns the inverse of the accepted camera displacement once.
    /// The platform applies it to its permanent pure placement policy.
    pub fn reset(&mut self) -> [f64; 2] {
        let applied = [-self.offset[0], -self.offset[1]];
        *self = Self::default();
        applied
    }

    /// Sample a presentation time without changing accepted offset, input
    /// grace, or the next canonical tick. No renderer or output handle enters.
    pub fn forecast_delta(&self, now: Duration) -> [f64; 2] {
        self.clone().tick(now)
    }

    pub fn tick(&mut self, now: Duration) -> [f64; 2] {
        if !self.ongoing() {
            return [0.; 2];
        }
        let previous = self.last_tick.unwrap_or(now);
        if now <= previous {
            return [0.; 2];
        }
        self.last_tick = Some(now);
        let start = if let Some(last) = self.last_input {
            let start = last.saturating_add(INERTIA_DELAY);
            if now < start {
                return [0.; 2];
            }
            self.last_input = None;
            previous.max(start)
        } else {
            previous
        };
        let speed = self.velocity[0].hypot(self.velocity[1]);
        if speed <= STOP_SPEED {
            self.stop();
            return [0.; 2];
        }
        let elapsed = now.saturating_sub(start).as_secs_f64();
        // Exact integral of dv/dt = -k*v - a, including the final partial
        // interval. Its result is independent of how output refreshes divide it.
        let terminal = CONSTANT_DECELERATION / VISCOUS_FRICTION;
        let stop_time = ((speed + terminal) / (STOP_SPEED + terminal)).ln() / VISCOUS_FRICTION;
        let dt = elapsed.min(stop_time);
        let decay_delta = (-VISCOUS_FRICTION * dt).exp_m1();
        let distance =
            (-(speed + terminal) * decay_delta / VISCOUS_FRICTION - terminal * dt).max(0.);
        let delta = [
            self.velocity[0] / speed * distance,
            self.velocity[1] / speed * distance,
        ];
        if elapsed >= stop_time {
            self.stop();
        } else {
            let next_speed = ((speed + terminal) * (1. + decay_delta) - terminal).max(0.);
            let factor = next_speed / speed;
            self.velocity[0] *= factor;
            self.velocity[1] *= factor;
        }
        self.translate(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inertia_moves_both_axes_and_stops() {
        let mut pan = CanvasPan::default();
        pan.pan([10., -20.], Duration::ZERO, true);
        pan.pan([10., -20.], Duration::from_millis(16), true);
        let start = pan.offset;
        for frame in 1..600 {
            pan.tick(Duration::from_millis(16 + frame * 16));
        }
        assert!(pan.offset[0] > start[0]);
        assert!(pan.offset[1] < start[1]);
        assert!(!pan.ongoing());
    }
    #[test]
    fn direct_drag_cancels_inertia_and_invalid_input_is_ignored() {
        let mut pan = CanvasPan::default();
        pan.pan([10., 10.], Duration::ZERO, true);
        pan.pan([10., 10.], Duration::from_millis(16), true);
        pan.pan([1., 1.], Duration::from_millis(20), false);
        let offset = pan.offset;
        assert!(!pan.ongoing());
        pan.pan((f64::NAN, 0.).into(), Duration::from_millis(21), true);
        assert_eq!(pan.offset, offset);
    }

    fn kinetic_pan() -> CanvasPan {
        let mut pan = CanvasPan::default();
        pan.pan([10., -20.], Duration::ZERO, true);
        pan.pan([10., -20.], Duration::from_millis(16), true);
        pan
    }

    #[test]
    fn inertia_distance_is_independent_of_frame_rate() {
        let mut endpoints = Vec::new();
        for rate in [30, 60, 120, 144, 240, 360] {
            let mut pan = kinetic_pan();
            for frame in 1..=rate * 3 {
                pan.tick(
                    Duration::from_millis(16)
                        + Duration::from_secs_f64(f64::from(frame) / f64::from(rate)),
                );
            }
            assert!(!pan.ongoing());
            endpoints.push(pan.offset);
        }
        for offset in &endpoints[1..] {
            assert!((offset[0] - endpoints[0][0]).abs() < 1e-8);
            assert!((offset[1] - endpoints[0][1]).abs() < 1e-8);
        }
    }

    #[test]
    fn inertia_start_ignores_time_before_the_grace_period() {
        let mut sampled = kinetic_pan();
        let mut delayed = kinetic_pan();
        sampled.tick(Duration::from_millis(80));
        sampled.tick(Duration::from_millis(96));
        let before = sampled.offset;
        sampled.tick(Duration::from_millis(100));
        delayed.tick(Duration::from_millis(100));
        assert_eq!(sampled.offset, delayed.offset);
        assert!(sampled.offset[0] > before[0]);
        assert!(sampled.offset[0] - before[0] < 625. * 0.004);
    }

    #[test]
    fn late_or_repeated_ticks_do_not_extend_inertia() {
        let mut first = kinetic_pan();
        let mut second = kinetic_pan();
        first.tick(Duration::from_millis(100));
        first.tick(Duration::from_millis(100));
        first.tick(Duration::from_millis(80));
        first.tick(Duration::from_millis(140));
        second.tick(Duration::from_millis(140));
        assert!((first.offset[0] - second.offset[0]).abs() < 1e-10);
        assert!((first.offset[1] - second.offset[1]).abs() < 1e-10);
        // A long pause finishes at the same endpoint without hundreds of catch-up ticks.
        first.tick(Duration::from_secs(10));
        for milliseconds in 150..2000 {
            second.tick(Duration::from_millis(milliseconds));
        }
        assert!(!first.ongoing() && !second.ongoing());
        assert!((first.offset[0] - second.offset[0]).abs() < 1e-8);
    }

    #[test]
    fn idle_ticks_do_not_move_canvas_or_delay_new_inertia() {
        let mut pan = CanvasPan::default();
        pan.pan([10., 0.], Duration::ZERO, true);
        pan.pan([10., 0.], Duration::from_millis(16), true);
        pan.stop();
        let stopped_at = pan.offset;
        assert_eq!(pan.tick(Duration::from_secs(10)), [0.; 2]);
        assert_eq!(pan.offset, stopped_at);

        pan.pan([10., 0.], Duration::from_secs(10), true);
        pan.pan([10., 0.], Duration::from_millis(10_016), true);
        let after_input = pan.offset;
        pan.tick(Duration::from_millis(10_116));
        assert!(pan.offset[0] > after_input[0]);
    }

    #[test]
    fn new_gesture_first_sample_cancels_previous_inertia() {
        for refresh_rate in [30, 60, 120, 144, 240, 360] {
            let mut pan = kinetic_pan();
            for frame in 1..=refresh_rate / 4 {
                pan.tick(Duration::from_secs_f64(
                    f64::from(frame) / f64::from(refresh_rate),
                ));
            }
            assert!(pan.ongoing());
            let before_input = pan.offset;
            let now = Duration::from_millis(260);
            let delta = [-1., 2.];
            assert_eq!(pan.pan(delta, now, true), delta);
            assert_eq!(pan.offset, [before_input[0] - 1., before_input[1] + 2.]);
            assert!(!pan.ongoing(), "refresh_rate={refresh_rate}");

            let after_input = pan.offset;
            for frame in 1..=refresh_rate * 2 {
                assert_eq!(
                    pan.tick(
                        now + Duration::from_secs_f64(f64::from(frame) / f64::from(refresh_rate),)
                    ),
                    [0.; 2],
                    "refresh_rate={refresh_rate}, frame={frame}",
                );
            }
            assert_eq!(pan.offset, after_input);
        }
    }

    #[test]
    fn new_gesture_second_sample_establishes_its_own_inertia() {
        let mut endpoints = Vec::new();
        for refresh_rate in [30, 60, 120, 144, 240, 360] {
            let mut pan = kinetic_pan();
            pan.tick(Duration::from_millis(250));
            assert!(pan.ongoing());
            pan.pan([-1., 2.], Duration::from_millis(260), true);
            assert!(!pan.ongoing());
            pan.pan([-8., 16.], Duration::from_millis(276), true);
            assert!(pan.ongoing());
            assert_eq!(pan.velocity, [-500., 1000.]);

            let after_input = pan.offset;
            // The new gesture gets its own grace period, independent of the old one.
            assert_eq!(pan.tick(Duration::from_millis(356)), [0.; 2]);
            assert_eq!(pan.offset, after_input);
            for frame in 1..=refresh_rate * 3 {
                pan.tick(
                    Duration::from_millis(356)
                        + Duration::from_secs_f64(f64::from(frame) / f64::from(refresh_rate)),
                );
            }
            assert!(pan.offset[0] < after_input[0]);
            assert!(pan.offset[1] > after_input[1]);
            assert!(!pan.ongoing());
            endpoints.push(pan.offset);
        }
        for endpoint in &endpoints[1..] {
            assert!((endpoint[0] - endpoints[0][0]).abs() < 1e-8);
            assert!((endpoint[1] - endpoints[0][1]).abs() < 1e-8);
        }
    }

    #[test]
    fn forecasts_for_mixed_output_times_never_advance_the_canonical_camera() {
        let mut pan = kinetic_pan();
        pan.tick(Duration::from_millis(120));
        let accepted = pan.clone();
        for timestamp in [300, 125, 5000, 80, 300] {
            let now = Duration::from_millis(timestamp);
            let mut independently_advanced = accepted.clone();
            assert_eq!(pan.forecast_delta(now), independently_advanced.tick(now));
            assert_eq!(pan, accepted);
        }
        let expected_delta = pan.forecast_delta(Duration::from_millis(140));
        assert_eq!(pan.tick(Duration::from_millis(140)), expected_delta);
    }

    #[test]
    fn canonical_tick_and_presentation_forecast_share_coasting_pose_and_velocity() {
        let mut pan = kinetic_pan();
        pan.tick(Duration::from_millis(120));
        let incoming_velocity = pan.velocity();
        let following = Duration::from_micros(120001);
        let forecast = pan.forecast_delta(following);
        for axis in 0..2 {
            let derivative = forecast[axis] / 0.000001;
            assert!((derivative - incoming_velocity[axis]).abs() < 0.01);
        }
        let accepted = pan.offset();
        assert_eq!(pan.tick(following), forecast);
        for axis in 0..2 {
            assert_eq!(pan.offset()[axis], accepted[axis] + forecast[axis]);
            assert!((pan.velocity()[axis] - incoming_velocity[axis]).abs() < 0.01);
        }
    }

    #[test]
    fn reset_reverses_only_the_accepted_displacement_and_stops_forecast_once() {
        let mut pan = kinetic_pan();
        pan.tick(Duration::from_millis(120));
        let accepted = pan.offset();
        assert_ne!(pan.forecast_delta(Duration::from_millis(300)), [0.; 2]);
        assert_eq!(pan.reset(), [-accepted[0], -accepted[1]]);
        assert_eq!(pan, CanvasPan::default());
        assert_eq!(pan.forecast_delta(Duration::from_secs(10)), [0.; 2]);
        assert_eq!(pan.reset(), [0.; 2]);
    }
}
