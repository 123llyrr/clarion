//! Coalescing of compositor-owned motion. Discrete and client protocol events
//! stay outside this accumulator and retain their original ordering.

#[derive(Debug, Clone)]
pub struct MotionBatch<P> {
    pub delta: (f64, f64),
    pub latest: P,
    pub samples: usize,
}

#[derive(Debug, Clone)]
pub struct MotionAccumulator<P> {
    pending: Option<MotionBatch<P>>,
}

impl<P> Default for MotionAccumulator<P> {
    fn default() -> Self {
        Self { pending: None }
    }
}

impl<P> MotionAccumulator<P> {
    /// Returns false for a non-finite sample without poisoning pending motion.
    pub fn push(&mut self, delta: (f64, f64), latest: P) -> bool {
        let old = self.pending.as_ref().map_or((0., 0.), |p| p.delta);
        let sum = (old.0 + delta.0, old.1 + delta.1);
        if !delta.0.is_finite() || !delta.1.is_finite() || !sum.0.is_finite() || !sum.1.is_finite()
        {
            return false;
        }
        let samples = self
            .pending
            .as_ref()
            .map_or(1, |p| p.samples.saturating_add(1));
        self.pending = Some(MotionBatch {
            delta: sum,
            latest,
            samples,
        });
        true
    }

    pub fn take(&mut self) -> Option<MotionBatch<P>> {
        self.pending.take()
    }
    pub fn latest(&self) -> Option<&P> {
        self.pending.as_ref().map(|p| &p.latest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dense_motion_keeps_total_delta_and_latest_output_once() {
        let mut motion = MotionAccumulator::default();
        for n in 0..1000 {
            assert!(motion.push((0.25, -0.125), n));
        }
        let frame = motion.take().unwrap();
        assert_eq!(frame.delta, (250., -125.));
        assert_eq!((frame.latest, frame.samples), (999, 1000));
        assert!(motion.take().is_none());
    }
    #[test]
    fn invalid_motion_does_not_replace_a_valid_destination() {
        let mut motion = MotionAccumulator::default();
        motion.push((2., 3.), "first");
        assert!(!motion.push((f64::NAN, 0.), "invalid"));
        assert_eq!(motion.take().unwrap().latest, "first");
    }
    #[test]
    fn flushing_at_a_discrete_event_preserves_the_following_batch() {
        let mut motion = MotionAccumulator::default();
        motion.push((5., 1.), 0);
        assert_eq!(motion.take().unwrap().delta, (5., 1.));
        motion.push((-2., 4.), 1);
        assert_eq!(motion.take().unwrap().delta, (-2., 4.));
    }
}
