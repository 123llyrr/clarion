//! Independently reconstructed Big Sur workspace feedback arithmetic.
//!
//! `FeedbackState` preserves the original 60 Hz recurrence and boundary tests.
//! `WorkspaceFeedback` is Clarion's adaptation: build a bounded timeline once,
//! then sample it without mutating shared state at any presentation frequency.

use std::time::Duration;

/// Number of reference steps permitted while constructing a trajectory. A long
/// pause therefore cannot create an unbounded catch-up loop in the render path.
const MAX_STEPS: usize = 600;
const HZ: f64 = 60.;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeedbackParams {
    /// Completion distance in caller coordinates. One workspace is one screen,
    /// so the display adapter can supply 1 / physical screen extent.
    pub single_pixel: f64,
    /// Completion speed in caller coordinate units per second.
    pub velocity_epsilon: f64,
    /// Actual coordinate-domain edges, not the current transition's endpoints.
    /// `None` disables edge recovery when the caller has no domain information.
    pub bounds: Option<(f64, f64)>,
    pub allow_bounce: bool,
}

impl Default for FeedbackParams {
    fn default() -> Self {
        // Display-independent fallback for a screen-sized coordinate unit.
        // These adapter choices are not asserted to be Apple runtime defaults.
        Self {
            single_pixel: 1. / 1080.,
            velocity_epsilon: 0.01,
            bounds: None,
            allow_bounce: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FeedbackState {
    pub position: f64,
    pub velocity: f64,
    pub start: f64,
    pub target: f64,
    pub single_pixel: f64,
    pub allow_bounce: bool,
}

impl FeedbackState {
    /// One original 1/60 second integration step. Position and velocity use
    /// normalized transition units and units/second, respectively.
    pub fn step_60hz(&mut self) -> bool {
        self.step_with_limits(Some((0., 1.)), 0.01)
    }

    /// The compositor adapter retains the recurrence but supplies its actual
    /// coordinate domain and completion units, rather than treating each new
    /// `start` as an artificial screen edge.
    fn step_with_limits(&mut self, bounds: Option<(f64, f64)>, velocity_epsilon: f64) -> bool {
        self.velocity = 0.695 * self.velocity + 2. * (self.target - self.position);
        self.position += self.velocity / HZ;
        if !self.allow_bounce
            && ((self.position > self.target && self.start < self.target)
                || (self.position < self.target && self.target < self.start))
        {
            self.position = self.target;
            self.velocity = 0.;
            return true;
        }
        let done = match bounds {
            Some((lower, _)) if self.position < lower => {
                let distance = lower - self.position;
                self.velocity = 0.85 * self.velocity + 0.8 * distance;
                self.velocity.abs() < velocity_epsilon && distance < self.single_pixel
            }
            Some((_, upper)) if self.position > upper => {
                let distance = self.position - upper;
                self.velocity = 0.85 * self.velocity - 0.8 * distance;
                self.velocity.abs() < velocity_epsilon && distance < self.single_pixel
            }
            _ => {
                self.velocity.abs() < velocity_epsilon
                    && (self.position - self.target).abs() < self.single_pixel
            }
        };
        if done {
            self.position = self.target;
            self.velocity = 0.;
        }
        done
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FeedbackSample {
    pub position: f64,
    pub velocity: f64,
}

#[derive(Debug, Clone)]
pub struct WorkspaceFeedback {
    from: f64,
    target: f64,
    params: FeedbackParams,
    samples: Vec<FeedbackSample>,
    duration: Duration,
    clamped_duration: Duration,
}

impl WorkspaceFeedback {
    /// Construct in caller coordinates, including reversed workspace indices.
    /// One workspace always has the same size: the remaining distance never
    /// rescales pixel/speed thresholds or creates an artificial boundary.
    /// Samples are stored relative to `from` for numerical precision only.
    pub fn new(from: f64, to: f64, initial_velocity: f64, params: FeedbackParams) -> Option<Self> {
        let delta = to - from;
        if ![
            from,
            to,
            delta,
            initial_velocity,
            params.single_pixel,
            params.velocity_epsilon,
        ]
        .iter()
        .all(|v| v.is_finite())
            || params.single_pixel <= 0.
            || params.velocity_epsilon <= 0.
        {
            return None;
        }
        let bounds = match params.bounds {
            Some((lower, upper)) => {
                let relative = (lower - from, upper - from);
                if ![lower, upper, relative.0, relative.1]
                    .iter()
                    .all(|v| v.is_finite())
                    || lower > upper
                {
                    return None;
                }
                Some(relative)
            }
            None => None,
        };
        let target = delta;
        let velocity = initial_velocity;
        let mut state = FeedbackState {
            position: 0.,
            velocity,
            start: 0.,
            target,
            single_pixel: params.single_pixel,
            allow_bounce: params.allow_bounce,
        };
        let mut samples = Vec::with_capacity(128);
        samples.push(FeedbackSample {
            position: 0.,
            velocity,
        });
        let mut first_arrival = None;
        if delta != 0. || velocity != 0. {
            for step in 1..=MAX_STEPS {
                let done = state.step_with_limits(bounds, params.velocity_epsilon);
                if !state.position.is_finite() || !state.velocity.is_finite() {
                    return None;
                }
                if delta != 0.
                    && first_arrival.is_none()
                    && ((delta > 0. && state.position >= target)
                        || (delta < 0. && state.position <= target))
                {
                    first_arrival = Some(step);
                }
                // Bounded construction is an adapter policy, not part of the
                // recovered recurrence. The last knot ends at a stable pose.
                let sample = if done || step == MAX_STEPS {
                    FeedbackSample {
                        position: target,
                        velocity: 0.,
                    }
                } else {
                    FeedbackSample {
                        position: state.position,
                        velocity: state.velocity,
                    }
                };
                samples.push(sample);
                if done {
                    break;
                }
            }
        }
        let mut duration = knot_time(samples.len() - 1);
        // Latch the first continuous target contact, including an inter-knot
        // Hermite excursion, so a capped pose can never retreat on a later frame.
        if !params.allow_bounce && delta != 0. {
            for (index, pair) in samples.windows(2).enumerate() {
                let start = knot_time(index);
                let interval = knot_time(index + 1) - start;
                if let Some(t) =
                    first_target_contact(pair[0], pair[1], delta, interval.as_secs_f64())
                {
                    duration = start
                        .saturating_add(interval.mul_f64(t))
                        .saturating_add(Duration::from_nanos(1))
                        .min(duration);
                    break;
                }
            }
        }
        let clamped_duration = first_arrival
            .map(knot_time)
            .unwrap_or(duration)
            .min(duration);
        Some(Self {
            from,
            target: to,
            params,
            samples,
            duration,
            clamped_duration,
        })
    }

    pub fn params(&self) -> FeedbackParams {
        self.params
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }

    pub fn clamped_duration(&self) -> Duration {
        self.clamped_duration
    }

    pub fn offset(&mut self, offset: f64) {
        self.from += offset;
        self.target += offset;
        if let Some((lower, upper)) = &mut self.params.bounds {
            *lower += offset;
            *upper += offset;
        }
    }

    /// Pure absolute-elapsed-time sampling. Cubic Hermite interpolation between
    /// the 60 Hz knots is Clarion's high-refresh adapter; it is not claimed to be
    /// Dock's original inter-frame interpolation. It retains launch velocity and
    /// each knot's position and velocity, including interrupted/reversed motion.
    /// With bounce disabled, the target-side cap also applies between knots.
    pub fn sample(&self, elapsed: Duration) -> FeedbackSample {
        if elapsed >= self.duration {
            return FeedbackSample {
                position: self.target,
                velocity: 0.,
            };
        }
        // Knots are represented by rounded nanosecond timestamps. Evaluate
        // each segment on those actual timestamps, rather than snapping a
        // neighborhood around the mathematical n/60 boundary. A snap window
        // creates a tiny position step that breaks velocity on interruption.
        let mut index = ((elapsed.as_secs_f64() * HZ).floor() as usize).min(self.samples.len() - 2);
        if index + 1 < self.samples.len() - 1 && elapsed >= knot_time(index + 1) {
            index += 1;
        }
        if index > 0 && elapsed < knot_time(index) {
            index -= 1;
        }
        let start = knot_time(index);
        let interval = knot_time(index + 1) - start;
        let seconds = interval.as_secs_f64();
        let a = self.samples[index];
        let b = self.samples[index + 1];
        let mut relative = if elapsed == start {
            a
        } else {
            let t = ((elapsed - start).as_secs_f64() / seconds).clamp(0., 1.);
            let t2 = t * t;
            let t3 = t2 * t;
            FeedbackSample {
                position: (2. * t3 - 3. * t2 + 1.) * a.position
                    + (t3 - 2. * t2 + t) * a.velocity * seconds
                    + (-2. * t3 + 3. * t2) * b.position
                    + (t3 - t2) * b.velocity * seconds,
                velocity: ((6. * t2 - 6. * t) * a.position
                    + (3. * t2 - 4. * t + 1.) * a.velocity * seconds
                    + (-6. * t2 + 6. * t) * b.position
                    + (3. * t2 - 2. * t) * b.velocity * seconds)
                    / seconds,
            }
        };
        // Hermite tangents can cross the target between two non-crossing
        // reference knots. Honor the selected no-bounce policy at render rates
        // above 60 Hz as well. As with the original crossing branch, this cap
        // deliberately stops velocity rather than claiming a C1 target crossing.
        let delta = self.target - self.from;
        if !self.params.allow_bounce
            && ((delta > 0. && relative.position > delta)
                || (delta < 0. && relative.position < delta))
        {
            relative = FeedbackSample {
                position: delta,
                velocity: 0.,
            };
        }
        FeedbackSample {
            position: self.from + relative.position,
            velocity: relative.velocity,
        }
    }
}

fn knot_time(index: usize) -> Duration {
    Duration::from_secs_f64(index as f64 / HZ)
}

/// First target contact on one Hermite segment. Splitting at derivative roots
/// makes each searched interval monotone, including tangencies and short-lived
/// overshoots which a regular frame-rate sampling grid could miss.
fn first_target_contact(
    a: FeedbackSample,
    b: FeedbackSample,
    target: f64,
    seconds: f64,
) -> Option<f64> {
    let direction = target.signum();
    let coefficients = [
        direction * (a.position - target),
        direction * a.velocity * seconds,
        direction
            * (-3. * a.position + 3. * b.position
                - 2. * a.velocity * seconds
                - b.velocity * seconds),
        direction
            * (2. * a.position - 2. * b.position + a.velocity * seconds + b.velocity * seconds),
    ];
    let evaluate = |t: f64| {
        coefficients[0] + t * (coefficients[1] + t * (coefficients[2] + t * coefficients[3]))
    };
    let mut divisions = [0., 1., 0., 0.];
    let mut division_count = 2;
    let qa = 3. * coefficients[3];
    let qb = 2. * coefficients[2];
    let qc = coefficients[1];
    if qa == 0. {
        if qb != 0. {
            let t = -qc / qb;
            if t > 0. && t < 1. {
                divisions[division_count] = t;
                division_count += 1;
            }
        }
    } else {
        let discriminant = qb * qb - 4. * qa * qc;
        if discriminant >= 0. {
            let root = discriminant.sqrt();
            for t in [(-qb - root) / (2. * qa), (-qb + root) / (2. * qa)] {
                if t > 0. && t < 1. {
                    divisions[division_count] = t;
                    division_count += 1;
                }
            }
        }
    }
    divisions[..division_count].sort_unstable_by(f64::total_cmp);
    for pair in divisions[..division_count].windows(2) {
        let (mut lower, mut upper) = (pair[0], pair[1]);
        if evaluate(lower) >= 0. {
            return Some(lower);
        }
        if evaluate(upper) < 0. {
            continue;
        }
        for _ in 0..60 {
            let middle = (lower + upper) * 0.5;
            if evaluate(middle) < 0. {
                lower = middle;
            } else {
                upper = middle;
            }
        }
        return Some(upper);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_indices_reverse_and_equal_endpoint_keep_launch_velocity() {
        for (from, to, velocity) in [(3., 4., 2.), (4., 2., -3.), (4., 4., 3.), (4., 4., -3.)] {
            let motion =
                WorkspaceFeedback::new(from, to, velocity, FeedbackParams::default()).unwrap();
            assert_eq!(motion.sample(Duration::ZERO).position, from);
            assert_eq!(motion.sample(Duration::ZERO).velocity, velocity);
            let dt = Duration::from_nanos(100);
            let measured = (motion.sample(dt).position - from) / dt.as_secs_f64();
            assert!((measured - velocity).abs() < 0.001);
            assert_eq!(motion.sample(Duration::MAX).position, to);
            assert_eq!(motion.sample(Duration::MAX).velocity, 0.);
            assert!(motion.duration() <= Duration::from_secs(10));
        }
    }

    #[test]
    fn multiple_workspace_indices_are_affine_coordinates_not_unit_interval_limits() {
        let params = FeedbackParams::default();
        for velocity in [-3., 0., 4.] {
            let reference = WorkspaceFeedback::new(0., 1., velocity, params).unwrap();
            for (from, to) in [(3., 4.), (-5., -2.), (9., 3.), (1000., 1000.5)] {
                let span: f64 = to - from;
                let scaled = FeedbackParams {
                    single_pixel: params.single_pixel * span.abs(),
                    velocity_epsilon: params.velocity_epsilon * span.abs(),
                    ..params
                };
                let motion = WorkspaceFeedback::new(from, to, span * velocity, scaled).unwrap();
                assert!(
                    motion.duration().abs_diff(reference.duration()) <= Duration::from_nanos(100)
                );
                for millis in [0, 1, 8, 16, 83, 177, 333, 1000] {
                    let at = Duration::from_millis(millis);
                    let expected = reference.sample(at);
                    let actual = motion.sample(at);
                    assert!((actual.position - (from + span * expected.position)).abs() < 1e-12);
                    assert!((actual.velocity - span * expected.velocity).abs() < 1e-12);
                }
                assert_eq!(motion.sample(Duration::MAX).position, to);
            }
        }
    }

    #[test]
    fn completion_thresholds_use_screen_units_for_short_and_long_remainders() {
        let params = FeedbackParams {
            single_pixel: 1. / 1920.,
            ..FeedbackParams::default()
        };
        let short = WorkspaceFeedback::new(3.99, 4., 0., params).unwrap();
        let long = WorkspaceFeedback::new(3., 6., 0., params).unwrap();
        assert!(short.duration() < long.duration());
        // Compare the stored timeline to the same recurrence in caller units.
        // In particular, a 0.01-workspace remainder does not tighten the pixel
        // threshold 100x and a three-workspace transition does not loosen it 3x.
        for (motion, delta) in [(&short, 4. - 3.99), (&long, 3.)] {
            let mut state = FeedbackState {
                position: 0.,
                velocity: 0.,
                start: 0.,
                target: delta,
                single_pixel: params.single_pixel,
                allow_bounce: false,
            };
            for sample in motion.samples.iter().skip(1) {
                state.step_with_limits(None, params.velocity_epsilon);
                assert_eq!(sample.position, state.position);
                assert_eq!(sample.velocity, state.velocity);
            }
        }
    }

    #[test]
    fn interrupted_start_is_not_an_artificial_domain_boundary() {
        let from = 3.5;
        let to = 4.;
        let velocity = -5.;
        let params = FeedbackParams::default();
        let unbounded = WorkspaceFeedback::new(from, to, velocity, params).unwrap();
        let bounded = WorkspaceFeedback::new(
            from,
            to,
            velocity,
            FeedbackParams {
                bounds: Some((0., 8.)),
                ..params
            },
        )
        .unwrap();
        let at = Duration::from_secs_f64(1. / HZ);
        let expected_velocity = 0.695 * velocity + 2. * (to - from);
        for motion in [unbounded, bounded] {
            let sample = motion.sample(at);
            assert_eq!(sample.velocity, expected_velocity);
            assert_eq!(sample.position, from + expected_velocity / HZ);
        }
    }

    #[test]
    fn offsets_translate_actual_bounds_and_preserve_completion_units() {
        let params = FeedbackParams {
            bounds: Some((0., 8.)),
            ..FeedbackParams::default()
        };
        let mut motion = WorkspaceFeedback::new(3., 4., -2., params).unwrap();
        let at = Duration::from_millis(83);
        let before = motion.sample(at);
        motion.offset(10.);
        assert_eq!(
            motion.params(),
            FeedbackParams {
                bounds: Some((10., 18.)),
                ..params
            }
        );
        assert_eq!(motion.sample(at).position, before.position + 10.);
        assert_eq!(motion.sample(at).velocity, before.velocity);
    }

    #[test]
    fn no_bounce_target_contact_latches_at_high_refresh_rates() {
        for bounds in [None, Some((0., 8.))] {
            let params = FeedbackParams {
                bounds,
                ..FeedbackParams::default()
            };
            for (from, to) in [
                (3.99, 4.),
                (3., 6.),
                (4., 3.99),
                (6., 3.),
                (0.01, 0.),
                (0., 0.01),
            ] {
                for velocity in [-100., -3., 0., 3., 100.] {
                    let motion = WorkspaceFeedback::new(from, to, velocity, params).unwrap();
                    let mut arrived = false;
                    for micros in (0..1_500_000).step_by(100) {
                        let sample = motion.sample(Duration::from_micros(micros));
                        if arrived {
                            assert_eq!(
                                sample.position, to,
                                "retreat: {from} {to} {velocity} {bounds:?}"
                            );
                            assert_eq!(sample.velocity, 0.);
                        }
                        arrived |= sample.position == to;
                    }
                    assert!(arrived);
                }
            }
        }
    }

    #[test]
    fn sampling_order_and_output_frequency_do_not_advance_the_timeline() {
        let motion = WorkspaceFeedback::new(2., 3., -2., FeedbackParams::default()).unwrap();
        let at = Duration::from_millis(133);
        let expected = motion.sample(at);
        for hz in [60, 120, 144, 240] {
            for n in 0..hz {
                let _ = motion.sample(Duration::from_secs_f64(f64::from(n) / f64::from(hz)));
            }
            assert_eq!(motion.sample(at).position, expected.position);
            assert_eq!(motion.sample(at).velocity, expected.velocity);
        }
        let _ = motion.sample(Duration::MAX);
        assert_eq!(motion.sample(at).position, expected.position);
    }

    #[test]
    fn reference_knots_and_subframe_derivatives_are_preserved() {
        let params = FeedbackParams {
            bounds: Some((0., 1.)),
            allow_bounce: true,
            ..FeedbackParams::default()
        };
        let motion = WorkspaceFeedback::new(0., 1., 2., params).unwrap();
        let mut state = FeedbackState {
            position: 0.,
            velocity: 2.,
            start: 0.,
            target: 1.,
            single_pixel: params.single_pixel,
            allow_bounce: true,
        };
        for step in 1..20 {
            state.step_60hz();
            let sample = motion.sample(Duration::from_secs_f64(f64::from(step) / HZ));
            assert_eq!(sample.position, state.position);
            assert_eq!(sample.velocity, state.velocity);
        }
        for micros in [100, 8100, 16700, 23400, 42000] {
            let at = Duration::from_micros(micros);
            let epsilon = Duration::from_nanos(10);
            let measured = (motion.sample(at + epsilon).position
                - motion.sample(at - epsilon).position)
                / (2. * epsilon.as_secs_f64());
            assert!((measured - motion.sample(at).velocity).abs() < 0.00001);
        }
    }

    #[test]
    fn no_bounce_policy_caps_the_target_between_reference_steps() {
        for velocity in [-100., -3., 0., 3., 100.] {
            for (from, to) in [(2., 3.), (3., 2.)] {
                let motion =
                    WorkspaceFeedback::new(from, to, velocity, FeedbackParams::default()).unwrap();
                for micros in (0..1_000_000).step_by(100) {
                    let sample = motion.sample(Duration::from_micros(micros));
                    let normalized = (sample.position - from) / (to - from);
                    assert!(normalized <= 1. + f64::EPSILON, "{velocity}: {sample:?}");
                }
            }
        }
    }

    #[test]
    fn malformed_coordinates_are_rejected() {
        for (from, to, velocity) in [
            (f64::NAN, 1., 0.),
            (0., f64::INFINITY, 0.),
            (0., 1., f64::NAN),
            (-f64::MAX, f64::MAX, 0.),
        ] {
            assert!(
                WorkspaceFeedback::new(from, to, velocity, FeedbackParams::default()).is_none()
            );
        }
        assert!(WorkspaceFeedback::new(
            0.,
            1.,
            0.,
            FeedbackParams {
                single_pixel: 0.,
                ..FeedbackParams::default()
            }
        )
        .is_none());
    }
}
