use std::collections::VecDeque;
use std::time::Duration;

const HISTORY_LIMIT: Duration = Duration::from_millis(150);
const DECELERATION_TOUCHPAD: f64 = 0.997;

/// Retained Realand filtering parameters. Previous project notes associate
/// them with Catalina, but that complete native mechanism has not been
/// verified in this migration's hash/address/instruction evidence ledger.
const MIN_DT: Duration = Duration::from_millis(1);
const MAX_DT_RESET: Duration = Duration::from_millis(200);
const EPSILON_THRESHOLD: f64 = 0.5;

#[derive(Debug, Clone)]
pub struct SwipeTracker {
    history: VecDeque<Event>,
    pos: f64,
}

#[derive(Debug, Clone, Copy)]
struct Event {
    delta: f64,
    timestamp: Duration,
}

impl SwipeTracker {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            history: VecDeque::new(),
            pos: 0.,
        }
    }

    /// Pushes a new reading into the tracker.
    pub fn push(&mut self, delta: f64, timestamp: Duration) {
        if !delta.is_finite() || !(self.pos + delta).is_finite() {
            return;
        }
        // For the events that we care about, timestamps should always increase
        // monotonically.
        if let Some(last) = self.history.back_mut() {
            if timestamp < last.timestamp {
                return;
            }

            // If the finger was stationary/paused for longer than MAX_DT_RESET (200ms),
            // reset previous history so that old momentum does not bleed into the new motion.
            if timestamp > last.timestamp.saturating_add(MAX_DT_RESET) {
                self.history.clear();
            } else if timestamp == last.timestamp || timestamp - last.timestamp < MIN_DT {
                // Multiple readings within MIN_DT (1ms) form a single velocity sample,
                // eliminating high-frequency timestamp jitter from trackpad polling.
                if !(last.delta + delta).is_finite() {
                    return;
                }
                last.delta += delta;
                self.pos += delta;
                return;
            }
        }

        self.history.push_back(Event { delta, timestamp });
        self.pos += delta;

        self.trim_history();
    }

    /// Returns the current gesture position.
    pub fn pos(&self) -> f64 {
        self.pos
    }

    /// Computes the current gesture velocity.
    pub fn velocity(&self) -> f64 {
        let Some(last) = self.history.back() else {
            return 0.;
        };

        // Switch velocity immediately on reversal, using the final combined
        // delta at each timestamp. Keep the history intact: another reading at
        // the same timestamp can cancel or reverse that sample again.
        let direction = self.history.iter().rev().find(|event| event.delta != 0.);
        let anchor = direction
            .and_then(|direction| {
                self.history.iter().rposition(|event| {
                    (direction.delta > 0. && event.delta < 0.)
                        || (direction.delta < 0. && event.delta > 0.)
                })
            })
            .unwrap_or(0);
        let first = &self.history[anchor];

        let total_time = (last.timestamp - first.timestamp).as_secs_f64();
        if total_time == 0. {
            return 0.;
        }

        // The first delta ends at the start of this interval; its preceding time
        // is unknown (or was trimmed), so it contributes only to position.
        let total_delta = self
            .history
            .iter()
            .skip(anchor + 1)
            .map(|event| event.delta)
            .sum::<f64>();

        if total_delta.abs() < EPSILON_THRESHOLD {
            return 0.;
        }

        let velocity = total_delta / total_time;
        if velocity.is_finite() {
            velocity
        } else {
            0.
        }
    }

    /// Computes the gesture end position after decelerating to a halt.
    pub fn projected_end_pos(&self) -> f64 {
        let vel = self.velocity();
        let target = self.pos - vel / (1000. * DECELERATION_TOUCHPAD.ln());
        if target.is_finite() {
            target
        } else {
            self.pos
        }
    }

    fn trim_history(&mut self) {
        let Some(&Event { timestamp, .. }) = self.history.back() else {
            return;
        };

        while let Some(first) = self.history.front() {
            if timestamp <= first.timestamp.saturating_add(HISTORY_LIMIT) {
                break;
            }

            let _ = self.history.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Duration, SwipeTracker};

    fn push(tracker: &mut SwipeTracker, delta: f64, millis: u64) {
        tracker.push(delta, Duration::from_millis(millis));
    }

    #[test]
    fn velocity_uses_only_movement_between_sample_times() {
        for initial_delta in [0., 10., 1000.] {
            let mut tracker = SwipeTracker::new();
            push(&mut tracker, initial_delta, 10);
            assert_eq!(tracker.pos(), initial_delta);
            assert_eq!(tracker.velocity(), 0.);
            assert_eq!(tracker.projected_end_pos(), initial_delta);

            push(&mut tracker, 10., 20);
            assert_eq!(tracker.pos(), initial_delta + 10.);
            assert_eq!(tracker.velocity(), 1000.);
        }
    }

    #[test]
    fn splitting_same_timestamp_motion_does_not_change_velocity() {
        for direction in [-1., 1.] {
            let mut combined = SwipeTracker::new();
            let mut split = SwipeTracker::new();
            for (first, second, time) in [(4., 6., 10), (7., 13., 20), (4., 6., 30)] {
                push(&mut combined, direction * (first + second), time);
                push(&mut split, direction * first, time);
                push(&mut split, direction * second, time);
                assert_eq!(combined.pos(), split.pos());
                assert_eq!(combined.velocity(), split.velocity());
                assert_eq!(combined.projected_end_pos(), split.projected_end_pos());
            }
            assert_eq!(combined.velocity(), direction * 1500.);
            assert_eq!(split.history.len(), 3);
        }
    }

    #[test]
    fn coalesced_reversals_use_the_total_delta_at_each_timestamp() {
        for direction in [-1., 1.] {
            for (first, second) in [(5., -15.), (-15., 5.), (-20., 10.)] {
                let mut combined = SwipeTracker::new();
                let mut split = SwipeTracker::new();
                for time in [0, 10] {
                    push(&mut combined, direction * 10., time);
                    push(&mut split, direction * 10., time);
                }
                push(&mut combined, direction * (first + second), 20);
                push(&mut split, direction * first, 20);
                push(&mut split, direction * second, 20);
                assert_eq!(split.pos(), combined.pos());
                assert_eq!(split.velocity(), -direction * 1000.);
                assert_eq!(split.velocity(), combined.velocity());
                assert_eq!(split.projected_end_pos(), combined.projected_end_pos());
            }
        }
    }

    #[test]
    fn cancelled_same_timestamp_reversal_preserves_previous_motion() {
        for direction in [-1., 1.] {
            let mut combined = SwipeTracker::new();
            let mut split = SwipeTracker::new();
            for time in [0, 10, 20] {
                push(&mut combined, direction * 10., time);
                push(&mut split, direction * 10., time);
            }
            push(&mut combined, 0., 30);
            push(&mut split, -direction * 10., 30);
            push(&mut split, direction * 10., 30);
            assert_eq!(split.pos(), combined.pos());
            assert_eq!(split.velocity(), combined.velocity());
            assert_eq!(split.projected_end_pos(), combined.projected_end_pos());
            assert!(split.velocity() * direction > 0.);
        }
    }

    #[test]
    fn history_trimming_keeps_constant_motion_velocity() {
        let mut tracker = SwipeTracker::new();
        for time in (0..=150).step_by(50) {
            push(&mut tracker, 10., time);
        }
        assert_eq!(tracker.velocity(), 200.);
        assert_eq!(tracker.pos(), 40.);

        for time in (200..=500).step_by(50) {
            push(&mut tracker, 10., time);
            assert_eq!(tracker.velocity(), 200.);
            assert_eq!(
                tracker.history.front().unwrap().timestamp,
                Duration::from_millis(time - 150)
            );
        }
        assert_eq!(tracker.pos(), 110.);
    }

    #[test]
    fn releasing_after_a_stationary_history_window_has_no_inertia() {
        for release_time in [160, 161, 250] {
            let mut tracker = SwipeTracker::new();
            push(&mut tracker, 10., 0);
            push(&mut tracker, 10., 10);
            push(&mut tracker, 0., release_time);
            assert_eq!(tracker.pos(), 20.);
            assert_eq!(tracker.velocity(), 0.);
            assert_eq!(tracker.projected_end_pos(), 20.);
        }
    }

    #[test]
    fn releasing_at_the_last_motion_time_keeps_velocity() {
        let mut tracker = SwipeTracker::new();
        push(&mut tracker, 10., 0);
        push(&mut tracker, 10., 10);
        let velocity = tracker.velocity();
        let projected = tracker.projected_end_pos();
        push(&mut tracker, 0., 10);
        assert_eq!(tracker.pos(), 20.);
        assert_eq!(tracker.velocity(), velocity);
        assert_eq!(tracker.projected_end_pos(), projected);
        assert_eq!(tracker.velocity(), 1000.);
    }

    #[test]
    fn out_of_order_motion_is_ignored_completely() {
        let mut tracker = SwipeTracker::new();
        push(&mut tracker, 10., 10);
        push(&mut tracker, 10., 20);
        let velocity = tracker.velocity();
        let projected = tracker.projected_end_pos();
        push(&mut tracker, 100., 19);
        assert_eq!(tracker.pos(), 20.);
        assert_eq!(tracker.velocity(), velocity);
        assert_eq!(tracker.projected_end_pos(), projected);
        assert_eq!(tracker.history.len(), 2);
    }

    #[test]
    fn reversing_direction_instantly_snaps_velocity_without_old_momentum_lag() {
        let mut tracker = SwipeTracker::new();
        // Moving right at +1000 px/s
        for t in (0..=50).step_by(10) {
            push(&mut tracker, 10., t);
        }
        assert_eq!(tracker.velocity(), 1000.);
        assert_eq!(tracker.pos(), 60.);

        // Immediately reverse left at -1000 px/s at t=60ms
        push(&mut tracker, -10., 60);
        assert_eq!(tracker.pos(), 50.);
        // Velocity should immediately be -1000 px/s instead of dragging old positive momentum!
        assert_eq!(tracker.velocity(), -1000.);
    }

    #[test]
    fn sub_millisecond_readings_are_coalesced() {
        let mut tracker = SwipeTracker::new();
        // Two events at t=10.000ms and t=10.500ms (diff < 1ms) coalesce into one sample
        tracker.push(5., Duration::from_micros(10_000));
        tracker.push(5., Duration::from_micros(10_500));
        assert_eq!(tracker.pos(), 10.);
        assert_eq!(tracker.history.len(), 1);
        assert_eq!(tracker.history[0].delta, 10.);

        // Third event at t=20.000ms
        tracker.push(10., Duration::from_micros(20_000));
        assert_eq!(tracker.pos(), 20.);
        assert_eq!(tracker.history.len(), 2);
        assert_eq!(tracker.velocity(), 1000.);
    }

    #[test]
    fn pause_longer_than_max_dt_resets_history_momentum() {
        let mut tracker = SwipeTracker::new();
        // Moving at 1000 px/s
        push(&mut tracker, 10., 0);
        push(&mut tracker, 10., 10);
        assert_eq!(tracker.velocity(), 1000.);

        // User pauses finger for 250ms (> 200ms MAX_DT_RESET) and then nudges 2px at t=260ms
        push(&mut tracker, 2., 260);
        assert_eq!(tracker.pos(), 22.);
        // Previous 1000 px/s momentum was cleared upon pause!
        assert_eq!(tracker.velocity(), 0.);
    }

    #[test]
    fn sub_pixel_noise_below_epsilon_has_zero_velocity() {
        let mut tracker = SwipeTracker::new();
        // 0.3px displacement (< 0.5px EPSILON_THRESHOLD)
        push(&mut tracker, 0.1, 0);
        push(&mut tracker, 0.2, 10);
        assert!((tracker.pos() - 0.3).abs() < 1e-6);
        // Electrical noise below 0.5px deadzone returns 0 velocity
        assert_eq!(tracker.velocity(), 0.);

        // Once displacement reaches 0.7px (> 0.5px), velocity is recognized
        push(&mut tracker, 0.4, 20);
        assert!((tracker.pos() - 0.7).abs() < 1e-6);
        assert!(tracker.velocity() > 0.);
    }
}

#[cfg(test)]
mod finite_input_tests {
    use super::*;
    #[test]
    fn invalid_delta_and_extreme_timestamp_cannot_poison_or_panic() {
        let mut tracker = SwipeTracker::new();
        tracker.push(1., Duration::MAX - Duration::from_millis(10));
        tracker.push(1., Duration::MAX);
        let position = tracker.pos();
        for delta in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            tracker.push(delta, Duration::MAX);
        }
        assert_eq!(tracker.pos(), position);
        assert!(tracker.velocity().is_finite());
        assert!(tracker.projected_end_pos().is_finite());
    }
}
