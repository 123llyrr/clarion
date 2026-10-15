//! Completion-driven ownership of asynchronous GPU work.
//!
//! One sleeping worker serves a renderer and all its outputs. A submitted fence
//! wakes it in the driver; an idle compositor needs neither another redraw nor
//! a periodic timer to release client buffers and upload storage.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};

use ash::vk;
use smithay::backend::renderer::sync::{Fence, Interrupted, SyncPoint};
use smithay::reexports::calloop::ping::Ping;

use super::{frame::GpuCompletion, memory::PendingUpload};

/// A shared retirement slot, separate from the independent native sync file.
/// Both the renderer and idle worker can retire it. An empty slot means GPU
/// references have actually been dropped, not merely handed to another thread.
#[derive(Debug)]
pub(super) struct FrameCompletion {
    resources: Mutex<Result<Option<Arc<GpuCompletion>>, vk::Result>>,
    sync: SyncPoint,
    fd: Option<Arc<std::os::fd::OwnedFd>>,
}

impl FrameCompletion {
    fn new(frame: Arc<GpuCompletion>) -> Self {
        Self {
            sync: frame.sync_point(),
            fd: frame.exported_fence(),
            resources: Mutex::new(Ok(Some(frame))),
        }
    }

    pub(super) fn sync_point(self: &Arc<Self>) -> SyncPoint {
        SyncPoint::from(FrameFence(self.clone()))
    }

    pub(super) fn retire_completed(&self) -> Result<bool, vk::Result> {
        self.retire(false)
    }

    fn retire(&self, wait: bool) -> Result<bool, vk::Result> {
        let resources = if wait {
            self.resources.lock().unwrap()
        } else {
            match self.resources.try_lock() {
                Ok(resources) => resources,
                Err(std::sync::TryLockError::WouldBlock) => return Ok(false),
                Err(error) => panic!("poisoned Vulkan retirement slot: {error}"),
            }
        };
        let Some(frame) = resources.as_ref().map_err(|error| *error)? else {
            return Ok(true);
        };
        // Ordinary rendering only queries the GPU and never waits for either
        // the worker or a VkFence tail following native semaphore readiness.
        let result = match frame.status() {
            Ok(true) => frame.confirm_retirement(),
            Ok(false) if !wait => return Ok(false),
            _ if wait => frame.prepare_retirement(),
            Err(error) => return Err(error),
            Ok(false) => unreachable!(),
        };
        Self::release(resources, result)
    }

    fn release(
        mut resources: std::sync::MutexGuard<'_, Result<Option<Arc<GpuCompletion>>, vk::Result>>,
        result: Result<(), vk::Result>,
    ) -> Result<bool, vk::Result> {
        let frame = resources.as_ref().unwrap().as_ref().unwrap();
        let clients = if result.is_ok() {
            frame.publish_gpu_timing();
            frame.take_retired_client_buffers()
        } else {
            Vec::new()
        };
        // Keep this short CPU cleanup serialized with cache-sharing checks.
        // GpuCompletion::drop reuses the confirmed result and cannot wait here.
        // All Wayland owners are dropped outside this mutex to avoid reversing
        // the main thread's surface-state/Wayland lock order.
        drop(std::mem::replace(&mut *resources, result.map(|_| None)));
        drop(resources);
        drop(clients);
        result.map(|_| true)
    }

    fn wait_and_retire(&self) -> Result<(), Interrupted> {
        let resources = self.resources.lock().unwrap();
        let Some(frame) = resources.as_ref().map_err(|_| Interrupted)? else {
            return Ok(());
        };
        // Preserve the original native renderer's explicit VkFence wait. A
        // timeout remains retryable and does not commit the frame to retirement.
        frame.wait()?;
        let result = frame.confirm_retirement();
        Self::release(resources, result)
            .map(|_| ())
            .map_err(|_| Interrupted)
    }

    fn wait_in_worker(&self) -> Result<(), vk::Result> {
        use std::os::fd::AsRawFd;

        if let Some(fd) = &self.fd {
            // Only this independent fd is used outside the retirement lock.
            // The render thread is free to recycle the original VkFence and
            // command buffer while this worker is blocked or unscheduled.
            loop {
                let mut poll = libc::pollfd {
                    fd: fd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut poll, 1, -1) };
                if ready == 1 && poll.revents & libc::POLLIN != 0 {
                    break;
                }
                if ready < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                {
                    continue;
                }
                return Err(vk::Result::ERROR_UNKNOWN);
            }
        } else {
            // Internal targets do not require SYNC_FD export. Retain an
            // independent GPU owner while waiting outside the retirement lock,
            // so ordinary renderer status queries remain nonblocking. A slow
            // healthy frame must not permanently cache a two-second TIMEOUT.
            let frame = {
                let resources = self.resources.lock().unwrap();
                resources.as_ref().map_err(|error| *error)?.clone()
            };
            if let Some(frame) = frame {
                let result = frame.wait_in_worker();
                if result.is_err() {
                    // Commit a real device failure through the established
                    // safe-retention path, then propagate the driver's error.
                    let _ = self.retire(true);
                    return result;
                }
                // The fence has now reached: this retirement can only contend
                // with short CPU cleanup, never a pending GPU wait.
                return self.retire(true).map(|_| ());
            }
        }
        self.retire(true).map(|_| ())
    }
}

#[derive(Debug)]
struct FrameFence(Arc<FrameCompletion>);

impl Fence for FrameFence {
    fn is_signaled(&self) -> bool {
        let resources = match self.0.resources.try_lock() {
            Ok(resources) => resources,
            Err(std::sync::TryLockError::WouldBlock) => return false,
            Err(error) => panic!("poisoned Vulkan retirement slot: {error}"),
        };
        match resources.as_ref() {
            Ok(Some(frame)) => frame.status().unwrap_or(false),
            Ok(None) => true,
            Err(_) => false,
        }
    }

    fn wait(&self) -> Result<(), Interrupted> {
        self.0.wait_and_retire()
    }

    fn is_exportable(&self) -> bool {
        self.0.sync.is_exportable()
    }

    fn export(&self) -> Option<std::os::fd::OwnedFd> {
        self.0.sync.export()
    }
}

enum Work {
    Frame(Arc<FrameCompletion>),
    Upload(Arc<PendingUpload>),
}

impl Work {
    fn wait(&self) -> Result<(), vk::Result> {
        match self {
            Self::Frame(frame) => frame.wait_in_worker(),
            Self::Upload(upload) => upload.wait_in_worker(),
        }
    }
}

struct Submission {
    work: Work,
    wakeup: Option<Ping>,
    _permit: CompletionPermit,
}

/// Reserve before recording/submitting GPU work. A permit covers queued work,
/// the running worker and resource retirement, not just channel occupancy.
#[derive(Debug)]
pub(super) struct CompletionPermit(Arc<AtomicUsize>);

impl Drop for CompletionPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug)]
pub(super) struct CompletionQueue {
    sender: mpsc::SyncSender<Submission>,
    outstanding: Arc<AtomicUsize>,
    capacity: usize,
}

impl CompletionQueue {
    #[cfg(test)]
    fn paused(sender: mpsc::SyncSender<Submission>, capacity: usize) -> Self {
        Self {
            sender,
            outstanding: Arc::new(AtomicUsize::new(0)),
            capacity,
        }
    }
    pub(super) fn new() -> std::io::Result<Self> {
        // Bound completed-but-not-yet-destroyed work as well as pending GPU
        // work. A busy teardown worker must not turn fast GPU submissions into
        // an unbounded queue of client buffers and textures.
        let (sender, receiver) = mpsc::sync_channel::<Submission>(16);
        std::thread::Builder::new()
            .name("realand-vk-completion".into())
            .spawn(move || {
                while let Ok(Submission {
                    work,
                    wakeup,
                    _permit,
                }) = receiver.recv()
                {
                    // Never hold the queue/command-pool lock while waiting. A
                    // busy GPU may legitimately take longer than the bounded
                    // synchronous renderer wait. Renderer teardown disconnects
                    // this channel without joining a possibly stalled device;
                    // each owner keeps its context alive until completion.
                    if let Err(err) = work.wait() {
                        warn!(?err, "Vulkan completion worker fence wait failed");
                    }
                    // The owners' existing Drop path handles failed waits by
                    // retaining in-flight resources instead of freeing them.
                    drop(work);
                    // Capacity must be visible before the wakeup retries a frame.
                    drop(_permit);
                    if let Some(wakeup) = wakeup {
                        // Buffer::drop queues wl_buffer.release and signals an
                        // explicit release point. Wake only after those drops,
                        // so the event loop's ordinary client flush sees them.
                        wakeup.ping();
                    }
                }
            })?;
        Ok(Self {
            sender,
            outstanding: Arc::new(AtomicUsize::new(0)),
            capacity: 16,
        })
    }

    pub(super) fn reserve(&self) -> Option<CompletionPermit> {
        self.outstanding
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < self.capacity).then_some(count + 1)
            })
            .ok()
            .map(|_| CompletionPermit(self.outstanding.clone()))
    }

    #[cfg(test)]
    pub(super) fn outstanding_count(&self) -> usize {
        self.outstanding.load(Ordering::Acquire)
    }

    pub(super) fn submit_frame(
        &self,
        frame: Arc<GpuCompletion>,
        wakeup: Option<Ping>,
        permit: CompletionPermit,
    ) -> Result<Arc<FrameCompletion>, &'static str> {
        let frame = Arc::new(FrameCompletion::new(frame));
        self.send(Work::Frame(frame.clone()), wakeup, permit)?;
        Ok(frame)
    }

    pub(super) fn submit_upload(
        &self,
        upload: Arc<PendingUpload>,
        wakeup: Option<Ping>,
        permit: CompletionPermit,
    ) -> Result<(), &'static str> {
        self.send(Work::Upload(upload), wakeup, permit)
    }

    fn send(
        &self,
        work: Work,
        wakeup: Option<Ping>,
        permit: CompletionPermit,
    ) -> Result<(), &'static str> {
        match self.sender.try_send(Submission {
            work,
            wakeup,
            _permit: permit,
        }) {
            Ok(()) => Ok(()),
            Err(error) => {
                let (submission, disconnected) = match error {
                    mpsc::TrySendError::Full(submission) => (submission, false),
                    mpsc::TrySendError::Disconnected(submission) => (submission, true),
                };
                // Reservations make Full unreachable in normal operation. On
                // an exceptional worker failure, never run a GPU wait from an
                // owner Drop. Completed resources can retire now; pending ones
                // and their permits remain retained (at most capacity entries).
                let completed = match &submission.work {
                    Work::Frame(frame) => frame.retire_completed().unwrap_or(false),
                    Work::Upload(upload) => upload.status().unwrap_or(false),
                };
                if completed {
                    let Submission {
                        work,
                        wakeup,
                        _permit,
                    } = submission;
                    drop(work);
                    drop(_permit);
                    if let Some(wakeup) = wakeup {
                        wakeup.ping();
                    }
                } else {
                    std::mem::forget(submission);
                }
                Err(if disconnected {
                    "Vulkan completion worker stopped"
                } else {
                    "Vulkan completion reservation mismatch"
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_helpers::vulkan::{allocate_texture, hardware_tests::setup};
    use smithay::backend::renderer::sync::Fence;

    #[test]
    fn admission_stays_bounded_until_every_outstanding_owner_retires() {
        let (sender, _receiver) = mpsc::sync_channel(2);
        let queue = CompletionQueue::paused(sender, 2);
        let first = queue.reserve().unwrap();
        let second = queue.reserve().unwrap();
        assert!(queue.reserve().is_none());
        drop(first);
        let replacement = queue.reserve().unwrap();
        assert!(queue.reserve().is_none());
        drop(second);
        drop(replacement);
        assert_eq!(queue.outstanding.load(Ordering::Acquire), 0);
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan and native fence export"]
    fn render_thread_retires_completed_frame_while_worker_is_paused() {
        let (renderer, _gles, _allocator) = setup();
        let ctx = renderer.ctx.as_ref().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        let queue = CompletionQueue::paused(sender, 1);
        let target = allocate_texture(
            ctx,
            4,
            4,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageTiling::OPTIMAL,
            vk::ImageUsageFlags::COLOR_ATTACHMENT,
            vk::ImageLayout::UNDEFINED,
            true,
        )
        .unwrap();
        let weak = Arc::downgrade(&target.inner);
        let frame = renderer
            .render_frame(
                target.image(),
                target.image_view(),
                vk::Format::R8G8B8A8_UNORM,
                true,
                4,
                4,
                Some([0.; 4]),
            )
            .unwrap();
        let completion = frame.finish_async(target).unwrap();
        completion.wait().unwrap();
        let pending = queue
            .submit_frame(completion, None, queue.reserve().unwrap())
            .unwrap();
        let sync = pending.sync_point();
        assert!(
            weak.upgrade().is_some(),
            "the deliberately paused worker must still own the submitted frame"
        );
        {
            let _retirement_in_progress = pending.resources.lock().unwrap();
            assert!(
                !pending.retire_completed().unwrap(),
                "ordinary rendering must never wait for a busy retirement slot"
            );
        }
        assert!(pending.retire_completed().unwrap());
        assert!(
            weak.upgrade().is_none(),
            "cache reuse must not depend on the worker being scheduled"
        );
        // The queued worker still exists after immediate render-thread cleanup.
        // It must use its independent sync file, not the recycled VkFence.
        receiver.recv().unwrap().work.wait().unwrap();
        assert!(pending.retire_completed().unwrap());
        assert!(sync.is_reached());
        assert!(sync.export().is_some());
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan and native fence export"]
    fn full_queue_refuses_before_submission_and_disconnected_completed_owners_retire() {
        let (renderer, _gles, _allocator) = setup();
        let ctx = renderer.ctx.as_ref().unwrap();
        let mut event_loop = smithay::reexports::calloop::EventLoop::<usize>::try_new().unwrap();
        let (ping, source) = smithay::reexports::calloop::ping::make_ping().unwrap();
        event_loop
            .handle()
            .insert_source(source, |(), &mut (), count| *count += 1)
            .unwrap();
        // Keep a real receiver alive without a worker to deterministically fill
        // the bounded channel; submissions still use actual Vulkan GPU fences.
        let (sender, receiver) = mpsc::sync_channel(1);
        let queue = CompletionQueue::paused(sender, 1);
        let make = || {
            let target = allocate_texture(
                ctx,
                4,
                4,
                vk::Format::R8G8B8A8_UNORM,
                vk::ImageTiling::OPTIMAL,
                vk::ImageUsageFlags::COLOR_ATTACHMENT,
                vk::ImageLayout::UNDEFINED,
                true,
            )
            .unwrap();
            let weak = Arc::downgrade(&target.inner);
            let frame = renderer
                .render_frame(
                    target.image(),
                    target.image_view(),
                    vk::Format::R8G8B8A8_UNORM,
                    true,
                    4,
                    4,
                    Some([0.; 4]),
                )
                .unwrap();
            let completion = frame.finish_async(target).unwrap();
            completion.wait().unwrap();
            let sync = completion.sync_point();
            (completion, weak, sync)
        };
        let (first, first_weak, first_sync) = make();
        queue
            .submit_frame(first, None, queue.reserve().unwrap())
            .unwrap();
        assert!(first_weak.upgrade().is_some());
        assert!(
            queue.reserve().is_none(),
            "full queue must refuse before any GPU submission"
        );
        let mut wakes = 0;
        event_loop
            .dispatch(std::time::Duration::ZERO, &mut wakes)
            .unwrap();
        assert_eq!(wakes, 0);
        drop(receiver);
        assert!(first_weak.upgrade().is_none());
        let (disconnected, disconnected_weak, disconnected_sync) = make();
        assert!(queue
            .submit_frame(disconnected, Some(ping), queue.reserve().unwrap())
            .is_err());
        assert!(
            disconnected_weak.upgrade().is_none(),
            "disconnected-queue fallback retained completed GPU storage"
        );
        event_loop
            .dispatch(std::time::Duration::ZERO, &mut wakes)
            .unwrap();
        assert_eq!(wakes, 1);
        assert_eq!(queue.outstanding.load(Ordering::Acquire), 0);
        for sync in [first_sync, disconnected_sync] {
            assert!(sync.is_reached());
            assert!(sync.export().is_some());
        }
    }
}
