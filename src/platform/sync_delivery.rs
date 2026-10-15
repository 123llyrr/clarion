//! Completion delivery for a renderer fence that could not export a sync fd.
//! A failed export is not a completed GPU operation. Keep the original fence
//! and its caller-owned buffer on the event loop until completion is observed.

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};
use smithay::backend::renderer::sync::SyncPoint;

const FAST_POLL: Duration = Duration::from_millis(2);
const FAST_POLL_LIMIT: Duration = Duration::from_secs(5);
const SLOW_POLL: Duration = Duration::from_millis(100);

struct SyncDelivery {
    sync: SyncPoint,
    start: Instant,
    warned: bool,
}

impl SyncDelivery {
    fn poll_at(&mut self, now: Instant) -> TimeoutAction {
        if self.sync.is_reached() {
            return TimeoutAction::Drop;
        }
        if now.saturating_duration_since(self.start) < FAST_POLL_LIMIT {
            return TimeoutAction::ToDuration(FAST_POLL);
        }
        if !self.warned {
            tracing::warn!("unexported GPU fence still pending after five seconds; retaining capture buffer and polling slowly");
            self.warned = true;
        }
        // A timeout cannot release a DMA-BUF that the GPU may still be writing.
        // Back off without publishing ready/failed or returning a reusable buffer.
        TimeoutAction::ToDuration(SLOW_POLL)
    }
}

pub(crate) fn defer_unexported_sync<'l, Data>(
    event_loop: &LoopHandle<'l, Data>,
    sync: SyncPoint,
    on_ready: impl FnOnce(&mut Data) + 'l,
) -> RegistrationToken {
    let mut delivery = SyncDelivery {
        sync,
        start: Instant::now(),
        warned: false,
    };
    let mut on_ready = Some(on_ready);
    event_loop
        .insert_source(Timer::from_duration(Duration::ZERO), move |_, _, data| {
            let action = delivery.poll_at(Instant::now());
            if matches!(action, TimeoutAction::Drop) {
                on_ready.take().unwrap()(data);
            }
            action
        })
        // The pinned calloop Timer::register only inserts into its timer wheel
        // and returns Ok; there is no fallible fd registration on this path.
        .expect("registering capture completion timer")
}

#[cfg(test)]
mod tests {
    use super::*;
    use calloop::EventLoop;
    use smithay::backend::renderer::sync::{Fence, Interrupted};
    use std::os::fd::OwnedFd;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Debug, Clone)]
    struct FailedExportFence(Arc<AtomicBool>);

    impl Fence for FailedExportFence {
        fn is_signaled(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn wait(&self) -> Result<(), Interrupted> {
            panic!("capture delivery must not block")
        }
        fn is_exportable(&self) -> bool {
            true
        }
        fn export(&self) -> Option<OwnedFd> {
            None
        }
    }

    struct BufferLease(Arc<AtomicUsize>);
    impl Drop for BufferLease {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn failed_export_retains_buffer_and_delivers_once_only_after_original_fence_signals() {
        let reached = Arc::new(AtomicBool::new(false));
        let sync = SyncPoint::from(FailedExportFence(reached.clone()));
        assert!(sync.export().is_none());
        assert!(!sync.is_reached());
        let drops = Arc::new(AtomicUsize::new(0));
        let lease = BufferLease(drops.clone());
        let mut event_loop: EventLoop<usize> = EventLoop::try_new().unwrap();
        let token = defer_unexported_sync(&event_loop.handle(), sync, move |delivered| {
            *delivered += 1;
            drop(lease);
        });
        let mut delivered = 0;
        event_loop.dispatch(Duration::ZERO, &mut delivered).unwrap();
        assert_eq!(delivered, 0);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        reached.store(true, Ordering::SeqCst);
        event_loop
            .dispatch(Duration::from_millis(20), &mut delivered)
            .unwrap();
        assert_eq!(delivered, 1);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        event_loop.dispatch(Duration::ZERO, &mut delivered).unwrap();
        assert_eq!(delivered, 1);
        event_loop.handle().remove(token);
    }

    #[test]
    fn reached_fence_without_fd_delivers_without_blocking_or_repeat() {
        let sync = SyncPoint::signaled();
        assert!(sync.export().is_none());
        let mut event_loop: EventLoop<usize> = EventLoop::try_new().unwrap();
        defer_unexported_sync(&event_loop.handle(), sync, |count| *count += 1);
        let mut delivered = 0;
        event_loop.dispatch(Duration::ZERO, &mut delivered).unwrap();
        assert_eq!(delivered, 1);
        event_loop.dispatch(Duration::ZERO, &mut delivered).unwrap();
        assert_eq!(delivered, 1);
    }

    #[test]
    fn timed_out_fence_backs_off_and_retains_ownership_until_it_reaches() {
        let reached = Arc::new(AtomicBool::new(false));
        let start = Instant::now();
        let mut delivery = SyncDelivery {
            sync: FailedExportFence(reached.clone()).into(),
            start,
            warned: false,
        };
        assert!(matches!(
            delivery.poll_at(start),
            TimeoutAction::ToDuration(FAST_POLL)
        ));
        assert!(matches!(
            delivery.poll_at(start + FAST_POLL_LIMIT),
            TimeoutAction::ToDuration(SLOW_POLL)
        ));
        assert!(delivery.warned);
        assert!(matches!(
            delivery.poll_at(start + Duration::from_secs(30)),
            TimeoutAction::ToDuration(SLOW_POLL)
        ));
        reached.store(true, Ordering::SeqCst);
        assert!(matches!(
            delivery.poll_at(start + Duration::from_secs(31)),
            TimeoutAction::Drop
        ));
    }

    #[test]
    fn removing_completion_source_releases_lease_without_publishing_frame() {
        let sync = FailedExportFence(Arc::new(AtomicBool::new(false))).into();
        let drops = Arc::new(AtomicUsize::new(0));
        let lease = BufferLease(drops.clone());
        let mut event_loop: EventLoop<usize> = EventLoop::try_new().unwrap();
        let token = defer_unexported_sync(&event_loop.handle(), sync, move |count| {
            *count += 1;
            drop(lease);
        });
        event_loop.handle().remove(token);
        let mut delivered = 0;
        event_loop.dispatch(Duration::ZERO, &mut delivered).unwrap();
        assert_eq!(delivered, 0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
