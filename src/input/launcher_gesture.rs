//! Recognize the system Apps pinch without changing ordinary two-finger zoom.
#[derive(Debug, Clone, Copy)]
pub struct LauncherPinch {
    scale: f64,
    cancelled: bool,
}

impl LauncherPinch {
    pub fn begin(fingers: u32, allowed: bool) -> Option<Self> {
        (allowed && matches!(fingers, 3 | 4)).then_some(Self {
            scale: 1.,
            cancelled: false,
        })
    }

    pub fn update(&mut self, scale: f64, allowed: bool) {
        if !allowed || !scale.is_finite() || scale <= 0. {
            self.cancel();
        } else {
            // libinput reports an absolute scale relative to gesture begin.
            self.scale = scale;
        }
    }

    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    pub fn finish(self, cancelled: bool, allowed: bool) -> bool {
        allowed && !cancelled && !self.cancelled && self.scale <= 0.75
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_completed_three_or_four_finger_inward_pinches_open_apps() {
        for fingers in [3, 4] {
            let mut gesture = LauncherPinch::begin(fingers, true).unwrap();
            gesture.update(0.9, true);
            gesture.update(0.74, true);
            assert!(gesture.finish(false, true));
        }
        for fingers in [0, 1, 2, 5] {
            assert!(LauncherPinch::begin(fingers, true).is_none());
        }
        assert!(LauncherPinch::begin(3, false).is_none());
        for scale in [0.76, 1., 1.5] {
            let mut gesture = LauncherPinch::begin(3, true).unwrap();
            gesture.update(scale, true);
            assert!(!gesture.finish(false, true));
        }
    }

    #[test]
    fn cancellation_lock_inhibition_and_invalid_motion_cannot_trigger_later() {
        for scale in [f64::NAN, f64::INFINITY, 0., -1.] {
            let mut gesture = LauncherPinch::begin(3, true).unwrap();
            gesture.update(scale, true);
            gesture.update(0.5, true);
            assert!(!gesture.finish(false, true));
        }
        let mut gesture = LauncherPinch::begin(4, true).unwrap();
        gesture.update(0.5, false);
        gesture.update(0.5, true);
        assert!(!gesture.finish(false, true));
        let mut gesture = LauncherPinch::begin(4, true).unwrap();
        gesture.update(0.5, true);
        assert!(!gesture.finish(true, true));
        assert!(!gesture.finish(false, false));
        gesture.cancel();
        assert!(!gesture.finish(false, true));
    }
}
