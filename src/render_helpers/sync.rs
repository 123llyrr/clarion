//! CPU fence waits shared by the GLES and Vulkan presentation paths.
use smithay::backend::renderer::sync::{Interrupted, SyncPoint};

/// A signal can interrupt a fence wait without completing the producer's work.
/// Retry briefly; persistent interruption must return to the event loop without
/// presenting the frame or treating the producer interruption as a Vulkan fault.
pub(crate) fn wait_for_completion(sync: &SyncPoint) -> Result<(), Interrupted> {
    let _latency =
        crate::utils::latency::Span::new(crate::utils::latency::Operation::ProducerFence);
    for _ in 0..3 {
        if sync.wait().is_ok() {
            return Ok(());
        }
    }
    Err(Interrupted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::backend::renderer::sync::Fence;
    use std::os::fd::OwnedFd;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct InterruptibleFence {
        calls: AtomicUsize,
        interruptions: usize,
    }

    impl Fence for InterruptibleFence {
        fn is_signaled(&self) -> bool {
            self.calls.load(Ordering::SeqCst) > self.interruptions
        }

        fn wait(&self) -> Result<(), Interrupted> {
            if self.calls.fetch_add(1, Ordering::SeqCst) < self.interruptions {
                Err(Interrupted)
            } else {
                Ok(())
            }
        }

        fn is_exportable(&self) -> bool {
            false
        }

        fn export(&self) -> Option<OwnedFd> {
            None
        }
    }

    #[test]
    fn interrupted_wait_only_succeeds_after_producer_completion() {
        let sync = SyncPoint::from(InterruptibleFence {
            calls: AtomicUsize::new(0),
            interruptions: 2,
        });
        wait_for_completion(&sync).unwrap();
        assert!(sync.is_reached());
    }

    #[test]
    fn persistent_interruption_is_bounded_and_can_be_retried() {
        let sync = SyncPoint::from(InterruptibleFence {
            calls: AtomicUsize::new(0),
            interruptions: 3,
        });
        assert_eq!(wait_for_completion(&sync), Err(Interrupted));
        assert!(!sync.is_reached());
        wait_for_completion(&sync).unwrap();
        assert!(sync.is_reached());
    }
}
