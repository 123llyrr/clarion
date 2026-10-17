//! Completion checks and explicit compatibility waits for renderer fences.
use smithay::backend::renderer::sync::{Interrupted, SyncPoint};

/// The producer still owns unfinished work. This is retryable backpressure,
/// never successful acquisition or a device failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProducerNotReady {
    Pending,
    Failed(ash::vk::Result),
}

impl std::fmt::Display for ProducerNotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => f.write_str("producer fence is not ready"),
            Self::Failed(error) => write!(f, "producer fence failed: {error}"),
        }
    }
}
impl std::error::Error for ProducerNotReady {}

/// Observe readiness without calling the fence's potentially blocking wait.
/// Callers must retain their previous frame and retry when this returns pending.
pub(crate) fn require_ready(sync: &SyncPoint) -> Result<(), ProducerNotReady> {
    if let Some(result) = super::vulkan::completion::poll_native_sync(sync) {
        return match result {
            Ok(true) => Ok(()),
            Ok(false) => Err(ProducerNotReady::Pending),
            Err(error) => Err(ProducerNotReady::Failed(error)),
        };
    }
    if sync.is_reached() {
        Ok(())
    } else {
        Err(ProducerNotReady::Pending)
    }
}

/// One prepared frame and its leases, retained until GPU completion is observed.
/// Backends keep this in an Option and do not render into the same target while
/// occupied. Feedback/state is delivered only with the ready payload.
pub(crate) struct DeferredFrame<T> {
    sync: SyncPoint,
    payload: T,
}

impl<T> DeferredFrame<T> {
    pub(crate) fn new(sync: SyncPoint, payload: T) -> Self {
        Self { sync, payload }
    }

    pub(crate) fn take_ready(frame: &mut Option<Self>) -> Result<Option<T>, ash::vk::Result> {
        let Some(candidate) = frame.as_ref() else {
            return Ok(None);
        };
        match require_ready(&candidate.sync) {
            Ok(()) => Ok(frame.take().map(|frame| frame.payload)),
            Err(ProducerNotReady::Pending) => Ok(None),
            Err(ProducerNotReady::Failed(error)) => Err(error),
        }
    }

    /// Cancel protocol-only state while retaining the GPU lease itself. This
    /// does not return the payload or declare its storage safe to reuse.
    pub(crate) fn update_retained(&mut self, update: impl FnOnce(&mut T)) {
        update(&mut self.payload);
    }
}

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
    use std::os::fd::OwnedFd;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    use smithay::backend::renderer::sync::Fence;

    use super::*;

    #[derive(Debug)]
    struct PollOnlyFence(Arc<AtomicBool>);

    impl Fence for PollOnlyFence {
        fn is_signaled(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }

        fn wait(&self) -> Result<(), Interrupted> {
            panic!("a nonblocking readiness check must never wait for its producer")
        }

        fn is_exportable(&self) -> bool {
            false
        }

        fn export(&self) -> Option<OwnedFd> {
            None
        }
    }

    #[test]
    fn pending_producer_is_retryable_without_cpu_wait_or_false_completion() {
        let ready = Arc::new(AtomicBool::new(false));
        let sync = SyncPoint::from(PollOnlyFence(ready.clone()));
        for _ in 0..8 {
            assert_eq!(require_ready(&sync), Err(ProducerNotReady::Pending));
            assert!(!sync.is_reached());
        }
        ready.store(true, Ordering::Release);
        assert_eq!(require_ready(&sync), Ok(()));
        assert_eq!(require_ready(&sync), Ok(()));
    }

    #[test]
    fn a_sync_point_without_a_fence_is_ready_without_a_wait() {
        assert_eq!(require_ready(&SyncPoint::signaled()), Ok(()));
    }

    #[test]
    fn deferred_frame_retains_its_lease_and_feedback_until_ready_exactly_once() {
        struct Lease(Arc<AtomicUsize>);
        impl Drop for Lease {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let ready = Arc::new(AtomicBool::new(false));
        let releases = Arc::new(AtomicUsize::new(0));
        let mut pending = Some(DeferredFrame::new(
            SyncPoint::from(PollOnlyFence(ready.clone())),
            (Lease(releases.clone()), "unpublished feedback"),
        ));
        for _ in 0..120 {
            assert!(DeferredFrame::take_ready(&mut pending).unwrap().is_none());
            assert!(pending.is_some());
            assert_eq!(releases.load(Ordering::SeqCst), 0);
        }
        ready.store(true, Ordering::Release);
        let payload = DeferredFrame::take_ready(&mut pending).unwrap().unwrap();
        assert!(pending.is_none());
        assert_eq!(payload.1, "unpublished feedback");
        assert_eq!(releases.load(Ordering::SeqCst), 0);
        assert!(DeferredFrame::take_ready(&mut pending).unwrap().is_none());
        drop(payload);
        assert_eq!(releases.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_native_fence_is_not_pending_or_ready_and_retains_the_frame() {
        let error = ash::vk::Result::ERROR_DEVICE_LOST;
        let sync = super::super::vulkan::completion::failed_sync_for_test(error);
        assert_eq!(require_ready(&sync), Err(ProducerNotReady::Failed(error)));
        let mut pending = Some(DeferredFrame::new(sync, "still-owned swapchain lease"));
        assert_eq!(DeferredFrame::take_ready(&mut pending), Err(error));
        assert!(pending.is_some());
        assert_eq!(DeferredFrame::take_ready(&mut pending), Err(error));
        assert!(pending.is_some());
    }

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
