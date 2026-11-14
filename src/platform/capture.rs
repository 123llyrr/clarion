//! Bounded screenshot tasks. Only owned pixels, file options and device jobs
//! cross worker threads; desktop and protocol state stay on the event loop.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

const MAX_SCREENSHOTS: usize = 2;

/// Only the event-loop owner invalidates a generation. Workers receive a read
/// token, allowing cancellation without access to desktop or protocol state.
#[derive(Default)]
pub(crate) struct CaptureEpoch(Arc<AtomicU64>);

#[derive(Clone)]
pub(crate) struct CaptureStamp {
    epoch: Arc<AtomicU64>,
    value: u64,
}

impl CaptureEpoch {
    pub(crate) fn invalidate(&mut self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
    pub(crate) fn stamp(&self) -> CaptureStamp {
        CaptureStamp {
            epoch: self.0.clone(),
            value: self.0.load(Ordering::Acquire),
        }
    }
}
impl CaptureStamp {
    pub(crate) fn is_current(&self) -> bool {
        self.epoch.load(Ordering::Acquire) == self.value
    }
}

#[derive(Default)]
pub(crate) struct CaptureSlots(Arc<AtomicUsize>);

pub(crate) struct CapturePermit(Arc<AtomicUsize>);

impl CaptureSlots {
    pub(crate) fn acquire(&self) -> anyhow::Result<CapturePermit> {
        self.0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_SCREENSHOTS).then_some(count + 1)
            })
            .map_err(|_| anyhow::anyhow!("two screenshots are still being processed"))?;
        Ok(CapturePermit(self.0.clone()))
    }
}

impl Drop for CapturePermit {
    fn drop(&mut self) {
        let previous = self.0.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
    }
}

pub(crate) struct ScreenshotSaveOptions {
    pub(crate) destination: Option<(PathBuf, bool)>,
    pub(crate) publish_clipboard: bool,
    /// Internal previews complete without changing the clipboard or alerting the user.
    pub(crate) is_preview: bool,
    pub(crate) completion: ScreenshotCompletion,
}

/// The portal must receive an answer even if a transfer is cancelled, the
/// device disappears, or encoding fails. Ordinary captures use an empty reply.
#[derive(Default)]
pub(crate) struct ScreenshotCompletion(Option<Box<dyn FnOnce(Option<PathBuf>) + Send>>);

impl ScreenshotCompletion {
    #[cfg(feature = "dbus")]
    pub(crate) fn new(reply: impl FnOnce(Option<PathBuf>) + Send + 'static) -> Self {
        Self(Some(Box::new(reply)))
    }

    pub(crate) fn complete(mut self, path: Option<PathBuf>) {
        if let Some(reply) = self.0.take() {
            reply(path);
        }
    }
}

impl Drop for ScreenshotCompletion {
    fn drop(&mut self) {
        if let Some(reply) = self.0.take() {
            reply(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_slots_remain_bounded_until_worker_completion_or_cancellation() {
        let slots = CaptureSlots::default();
        let first = slots.acquire().unwrap();
        let second = slots.acquire().unwrap();
        assert!(slots.acquire().is_err());
        std::thread::spawn(move || drop(first)).join().unwrap();
        let third = slots.acquire().unwrap();
        assert!(slots.acquire().is_err());
        drop(second);
        drop(third);
        assert_eq!(slots.0.load(Ordering::Acquire), 0);
    }

    #[test]
    fn cancellation_token_stays_invalid_after_a_later_scene_returns() {
        let mut epoch = CaptureEpoch::default();
        let old = epoch.stamp();
        let worker = old.clone();
        assert!(worker.is_current());
        epoch.invalidate();
        let locked = epoch.stamp();
        epoch.invalidate();
        let current = epoch.stamp();
        assert!(!old.is_current() && !locked.is_current());
        assert!(!std::thread::spawn(move || worker.is_current())
            .join()
            .unwrap());
        assert!(current.is_current());
    }

    #[cfg(feature = "dbus")]
    #[test]
    fn portal_gets_exactly_one_result_including_cancellation_and_worker_errors() {
        let (tx, rx) = std::sync::mpsc::channel();
        let reply = ScreenshotCompletion::new(move |path| tx.send(path).unwrap());
        let path = PathBuf::from("capture.png");
        reply.complete(Some(path.clone()));
        assert_eq!(rx.recv().unwrap(), Some(path));
        assert!(rx.recv().is_err());

        let (tx, rx) = std::sync::mpsc::channel();
        let reply = ScreenshotCompletion::new(move |path| tx.send(path).unwrap());
        std::thread::spawn(move || drop(reply)).join().unwrap();
        assert_eq!(rx.recv().unwrap(), None);
        assert!(rx.recv().is_err());
    }
}
