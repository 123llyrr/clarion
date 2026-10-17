//! Release privileged output state before acknowledging a seat handoff.
//!
//! This helper has no seat or DRM dependency so unwind/error ordering can be
//! tested without a real VT or DRM master. The acknowledgement closure must
//! handle its own I/O errors without panicking, including during unwinding.

pub(super) fn pause_before_acknowledge(pause: impl FnOnce(), acknowledge: impl FnOnce()) {
    struct AcknowledgeOnDrop<A: FnOnce()>(Option<A>);
    impl<A: FnOnce()> Drop for AcknowledgeOnDrop<A> {
        fn drop(&mut self) {
            if let Some(acknowledge) = self.0.take() {
                acknowledge();
            }
        }
    }
    let acknowledge = AcknowledgeOnDrop(Some(acknowledge));
    pause();
    drop(acknowledge);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    #[test]
    fn cleanup_finishes_before_disable_acknowledgement() {
        let events = RefCell::new(Vec::new());
        pause_before_acknowledge(
            || {
                events.borrow_mut().push("clear HDR and disable CRTC");
                events.borrow_mut().push("pause DRM and suspend leases");
            },
            || events.borrow_mut().push("acknowledge disable seat"),
        );
        assert_eq!(
            *events.borrow(),
            [
                "clear HDR and disable CRTC",
                "pause DRM and suspend leases",
                "acknowledge disable seat",
            ]
        );
    }

    #[test]
    fn failed_cleanup_still_acknowledges_disable() {
        let events = RefCell::new(Vec::new());
        pause_before_acknowledge(
            || {
                let cleanup: Result<(), &str> = Err("master revoked / driver deadline");
                if cleanup.is_err() {
                    events
                        .borrow_mut()
                        .push("cleanup failed; signal state unknown");
                    return;
                }
                events.borrow_mut().push("cleanup succeeded");
            },
            || events.borrow_mut().push("acknowledge disable seat"),
        );
        assert_eq!(
            *events.borrow(),
            [
                "cleanup failed; signal state unknown",
                "acknowledge disable seat"
            ]
        );
    }

    #[test]
    fn unwinding_cleanup_acknowledges_disable_exactly_once() {
        let events = RefCell::new(Vec::new());
        let panic = catch_unwind(AssertUnwindSafe(|| {
            pause_before_acknowledge(
                || {
                    events.borrow_mut().push("cleanup began");
                    panic!("cleanup panicked");
                },
                || events.borrow_mut().push("acknowledge disable seat"),
            );
        }));
        assert!(panic.is_err());
        assert_eq!(
            *events.borrow(),
            ["cleanup began", "acknowledge disable seat"]
        );
    }
}
