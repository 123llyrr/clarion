//! VBlank throttling.
//!
//! Some buggy drivers deliver VBlanks way earlier than necessary. This helper throttles the VBlank
//! in such cases to avoid tearing and to get more consistent timings.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};

use crate::compositor::State;
use crate::utils::get_monotonic_time;

/// Only wait until the intended vblank time. The DRM event's timestamp can be older than
/// the time at which the main loop handles it, so using `refresh - (timestamp - last)` as a
/// relative timer would add event-delivery latency to the frame delay. Return the effective
/// timestamp as well: subsequent events must be paced from when this callback was delivered,
/// rather than from the original, early DRM timestamp.
fn throttle_timing(
    last: Duration,
    refresh: Duration,
    timestamp: Duration,
    now: Duration,
) -> (Option<Duration>, Duration) {
    if timestamp.saturating_sub(last) >= refresh / 2 {
        return (None, timestamp);
    }

    let expected = last.saturating_add(refresh);
    let delay = expected.saturating_sub(now);
    let effective_timestamp = expected.max(now);
    ((!delay.is_zero()).then_some(delay), effective_timestamp)
}

#[derive(Debug)]
pub struct VBlankThrottle {
    event_loop: LoopHandle<'static, State>,
    last_vblank_timestamp: Option<Duration>,
    throttle_timer_token: Rc<Cell<Option<RegistrationToken>>>,
    /// The delayed callback re-enters `throttle` with a synthetic timestamp. It must pass
    /// through once instead of being classified as another early hardware event.
    in_synthetic_callback: Rc<Cell<bool>>,
    printed_warning: bool,
    output_name: String,
}

impl VBlankThrottle {
    pub fn new(event_loop: LoopHandle<'static, State>, output_name: String) -> Self {
        Self {
            event_loop,
            last_vblank_timestamp: None,
            throttle_timer_token: Rc::new(Cell::new(None)),
            in_synthetic_callback: Rc::new(Cell::new(false)),
            printed_warning: false,
            output_name,
        }
    }

    pub fn throttle(
        &mut self,
        refresh_interval: Option<Duration>,
        timestamp: Duration,
        mut call_vblank: impl FnMut(&mut State) + 'static,
    ) -> bool {
        if self.in_synthetic_callback.get() {
            self.last_vblank_timestamp = Some(timestamp);
            return false;
        }
        if self.throttle_timer_token.get().is_some() {
            // KMS cannot receive another frame until the delayed completion is
            // delivered. Keep that frame's callback and original deadline when
            // duplicate/rogue events arrive: replacing it would add a refresh to
            // the wait for every event and can starve the output indefinitely.
            return true;
        }

        let mut effective_timestamp = timestamp;
        if let Some((last, refresh)) = Option::zip(self.last_vblank_timestamp, refresh_interval) {
            let (remaining, effective) =
                throttle_timing(last, refresh, timestamp, get_monotonic_time());
            effective_timestamp = effective;
            if let Some(remaining) = remaining {
                if !self.printed_warning {
                    self.printed_warning = true;
                    warn!(
                        "output {} running faster than expected, throttling vblanks: \
                         expected refresh {refresh:?}, got vblank after {:?}",
                        self.output_name,
                        timestamp.saturating_sub(last)
                    );
                }

                let in_synthetic_callback = Rc::clone(&self.in_synthetic_callback);
                let throttle_timer_token = Rc::clone(&self.throttle_timer_token);
                let token = self
                    .event_loop
                    .insert_source(Timer::from_duration(remaining), move |_, _, state| {
                        // Clear before re-entry or output removal from the callback.
                        // The source will remove itself after the callback returns.
                        throttle_timer_token.set(None);
                        in_synthetic_callback.set(true);
                        call_vblank(state);
                        in_synthetic_callback.set(false);
                        TimeoutAction::Drop
                    })
                    .unwrap();
                self.throttle_timer_token.set(Some(token));
                self.last_vblank_timestamp = Some(effective_timestamp);
                return true;
            }
        }

        self.last_vblank_timestamp = Some(effective_timestamp);
        false
    }
}

impl Drop for VBlankThrottle {
    fn drop(&mut self) {
        if let Some(token) = self.throttle_timer_token.take() {
            // The output may disappear before its delayed callback. Calloop
            // also permits removing this source from inside that callback.
            self.event_loop.remove(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_vblank_wait_excludes_event_delivery_latency() {
        let last = Duration::from_secs(1);
        let refresh = Duration::from_millis(16);
        let timestamp = last + Duration::from_millis(2);
        let now = last + Duration::from_millis(5);

        assert_eq!(
            throttle_timing(last, refresh, timestamp, now),
            (Some(Duration::from_millis(11)), last + refresh)
        );
        assert_eq!(
            throttle_timing(last, refresh, timestamp, last + refresh),
            (None, last + refresh)
        );
        assert_eq!(
            throttle_timing(
                last,
                refresh,
                timestamp,
                last + refresh + Duration::from_millis(4)
            ),
            (None, last + refresh + Duration::from_millis(4))
        );
    }

    #[test]
    fn normal_and_invalid_refreshes_do_not_throttle() {
        let last = Duration::from_secs(1);
        let refresh = Duration::from_millis(16);
        assert_eq!(
            throttle_timing(last, refresh, last + refresh / 2, last + refresh / 2),
            (None, last + refresh / 2)
        );
        assert_eq!(
            throttle_timing(last, Duration::ZERO, last, last),
            (None, last)
        );
    }

    #[test]
    fn consecutive_early_vblanks_remain_one_refresh_apart() {
        let last = Duration::from_secs(1);
        let refresh = Duration::from_millis(16);
        let first_event = last + Duration::from_millis(2);
        let (first_wait, first_delivery) = throttle_timing(last, refresh, first_event, first_event);
        assert_eq!(first_wait, Some(Duration::from_millis(14)));
        assert_eq!(first_delivery, last + refresh);

        // The next page flip arrives two milliseconds after the delayed callback. Pacing from
        // the original timestamp would accept it immediately and produce a short frame.
        let second_event = last + Duration::from_millis(18);
        let (second_wait, second_delivery) =
            throttle_timing(first_delivery, refresh, second_event, second_event);
        assert_eq!(second_wait, Some(Duration::from_millis(14)));
        assert_eq!(second_delivery, last + refresh * 2);
    }
}
