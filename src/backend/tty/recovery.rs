//! Bounded device preparation, independent of desktop and renderer ownership.
//!
//! One worker owns driver initialization and stale-result destruction. Only a
//! matching generation can cross back to the event owner. Retired device anchors
//! keep final VulkanContext::drop (including device_wait_idle) off that owner.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use smithay::reexports::calloop::ping::Ping;

use crate::render_helpers::vulkan::lifecycle;

const RETIREMENT_CAPACITY: usize = 4;

pub(crate) struct Completion<T> {
    pub generation: u64,
    pub result: Result<T, String>,
}

struct Mailbox<J, T, R> {
    generation: u64,
    requested: Option<(u64, J)>,
    active: Option<u64>,
    completed: Option<Completion<T>>,
    retired: Vec<R>,
    closed: bool,
}

/// There is at most one executing request, one replacement request and one result.
/// Retirement capacity is bounded separately; refusal leaves ownership with the
/// caller so even repeated GPU loss cannot create unbounded detached devices.
type SharedMailbox<J, T, R> = Arc<(Mutex<Mailbox<J, T, R>>, Condvar)>;

pub(crate) struct RecoveryWorker<J, T, R> {
    shared: SharedMailbox<J, T, R>,
    wakeup: Ping,
}

impl<J: Send + 'static, T: Send + 'static, R: Send + 'static> RecoveryWorker<J, T, R> {
    pub fn new(
        wakeup: Ping,
        mut prepare: impl FnMut(J) -> Result<T, String> + Send + 'static,
        ready_to_retire: impl Fn(&R) -> bool + Send + 'static,
    ) -> std::io::Result<Self> {
        let shared = Arc::new((
            Mutex::new(Mailbox {
                generation: 0,
                requested: None,
                active: None,
                completed: None,
                retired: Vec::new(),
                closed: false,
            }),
            Condvar::new(),
        ));
        let worker = shared.clone();
        let stop_wakeup = shared.clone();
        let owner_wakeup = wakeup.clone();
        lifecycle::spawn_worker(
            "clarion-vk-recovery",
            move || {
                // Synchronize with wait's unlock-and-sleep transition. A notify
                // without this lock could be lost during final process shutdown.
                let _mailbox = stop_wakeup
                    .0
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                stop_wakeup.1.notify_all();
            },
            move |stop| loop {
                let (lock, changed) = &*worker;
                let mut mailbox = lock.lock().unwrap();
                mailbox.closed |= stop.is_closing();
                let cancelled = if mailbox.closed {
                    mailbox.requested.take()
                } else {
                    None
                };
                let mut retire = Vec::new();
                for index in (0..mailbox.retired.len()).rev() {
                    if mailbox.requested.is_none() && ready_to_retire(&mailbox.retired[index]) {
                        retire.push(mailbox.retired.swap_remove(index));
                    }
                }
                let stale = if mailbox.closed
                    || mailbox
                        .completed
                        .as_ref()
                        .is_some_and(|done| done.generation != mailbox.generation)
                {
                    mailbox.completed.take()
                } else {
                    None
                };
                // Driver object destruction is outside the mailbox mutex.
                if !retire.is_empty() || stale.is_some() || cancelled.is_some() {
                    drop(mailbox);
                    drop(cancelled);
                    drop(stale);
                    drop(retire);
                    continue;
                }
                if mailbox.closed && mailbox.retired.is_empty() {
                    return;
                }
                if let Some((generation, job)) = mailbox.requested.take() {
                    mailbox.active = Some(generation);
                    drop(mailbox);
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prepare(job)))
                            .unwrap_or_else(|_| Err("Vulkan recovery worker panicked".into()));
                    let mut mailbox = lock.lock().unwrap();
                    mailbox.active = None;
                    if !mailbox.closed && generation == mailbox.generation {
                        debug_assert!(mailbox.completed.is_none());
                        mailbox.completed = Some(Completion { generation, result });
                        drop(mailbox);
                        wakeup.ping();
                    } else {
                        drop(mailbox);
                        drop(result);
                    }
                    continue;
                }
                if mailbox.retired.is_empty() {
                    drop(changed.wait(mailbox).unwrap());
                } else {
                    // Only disconnected/retired devices need a low-frequency lifetime check.
                    // This timer never requests an output repaint or holds an owner lock.
                    drop(
                        changed
                            .wait_timeout(mailbox, Duration::from_millis(50))
                            .unwrap(),
                    );
                }
            },
        )?;
        Ok(Self {
            shared,
            wakeup: owner_wakeup,
        })
    }

    /// Supersede a request after hotplug without ever cancelling an executing driver call.
    /// The old result is discarded on the worker before the replacement is published.
    pub fn request(&mut self, job: J, retired: Option<R>) -> Result<u64, Option<R>> {
        let (lock, changed) = &*self.shared;
        let mut mailbox = lock.lock().unwrap();
        if mailbox.closed || lifecycle::is_closing() {
            return Err(retired);
        }
        // Keep one emergency retirement slot for owner-side registration failure.
        if retired.is_some() && mailbox.retired.len() >= RETIREMENT_CAPACITY - 1 {
            return Err(retired);
        }
        mailbox.generation = mailbox
            .generation
            .checked_add(1)
            .expect("GPU generation exhausted");
        let generation = mailbox.generation;
        mailbox.requested = Some((generation, job));
        if let Some(retired) = retired {
            mailbox.retired.push(retired);
        }
        changed.notify_one();
        self.wakeup.ping();
        Ok(generation)
    }

    pub fn retire(&mut self, retired: R) -> Result<(), R> {
        let (lock, changed) = &*self.shared;
        let mut mailbox = lock.lock().unwrap();
        if mailbox.retired.len() >= RETIREMENT_CAPACITY {
            return Err(retired);
        }
        mailbox.retired.push(retired);
        changed.notify_one();
        Ok(())
    }

    pub fn take_completed(&mut self) -> Option<Completion<T>> {
        let mut mailbox = self.shared.0.lock().unwrap();
        if mailbox
            .completed
            .as_ref()
            .is_some_and(|done| done.generation == mailbox.generation)
        {
            mailbox.completed.take()
        } else {
            None
        }
    }

    pub fn invalidate(&mut self) {
        let (lock, changed) = &*self.shared;
        let mut mailbox = lock.lock().unwrap();
        mailbox.generation = mailbox
            .generation
            .checked_add(1)
            .expect("GPU generation exhausted");
        mailbox.requested = None;
        changed.notify_one();
    }
}

impl<J, T, R> Drop for RecoveryWorker<J, T, R> {
    fn drop(&mut self) {
        let (lock, changed) = &*self.shared;
        let mut mailbox = lock.lock().unwrap();
        mailbox.closed = true;
        // Queued preparation and result payloads retire on the tracked worker.
        // In particular, final-context destruction is never an owner Drop job.
        changed.notify_one();
        // The process-level deadline/barrier owns waiting; runtime Drop stays nonblocking.
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread::ThreadId;
    use std::time::Instant;

    use super::*;

    #[derive(Debug)]
    struct DroppedOn(mpsc::Sender<ThreadId>);
    impl Drop for DroppedOn {
        fn drop(&mut self) {
            let _ = self.0.send(std::thread::current().id());
        }
    }

    fn completion<T>(worker: &mut RecoveryWorker<u32, T, Arc<DroppedOn>>) -> Completion<T>
    where
        T: Send + 'static,
    {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(done) = worker.take_completed() {
                return done;
            }
            assert!(Instant::now() < deadline, "worker failed to complete");
            std::thread::yield_now();
        }
    }

    #[test]
    fn blocked_initialization_keeps_owner_free_and_replacement_wins() {
        let (ping, _source) = smithay::reexports::calloop::ping::make_ping().unwrap();
        let (started, started_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (dropped, dropped_rx) = mpsc::channel();
        let mut worker = RecoveryWorker::new(
            ping,
            move |job| {
                started.send(job).unwrap();
                if job == 1 {
                    release_rx.recv().unwrap();
                }
                Ok((job, DroppedOn(dropped.clone())))
            },
            |anchor: &Arc<DroppedOn>| Arc::strong_count(anchor) == 1,
        )
        .unwrap();
        let old = worker.request(1, None).unwrap();
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(3)).unwrap(), 1);
        // No release has occurred: invalidation/request must still return to the owner.
        worker.invalidate();
        let current = worker.request(2, None).unwrap();
        assert!(current > old);
        release.send(()).unwrap();
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(3)).unwrap(), 2);
        assert_ne!(
            dropped_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            std::thread::current().id()
        );
        let done = completion(&mut worker);
        assert_eq!(done.generation, current);
        assert_eq!(done.result.unwrap().0, 2);
    }

    #[test]
    fn failure_can_retry_and_invalidation_cannot_publish_old_success() {
        let (ping, _source) = smithay::reexports::calloop::ping::make_ping().unwrap();
        let mut worker = RecoveryWorker::new(
            ping,
            |job| {
                if job == 1 {
                    Err("device unavailable".into())
                } else {
                    Ok(job)
                }
            },
            |anchor: &Arc<DroppedOn>| Arc::strong_count(anchor) == 1,
        )
        .unwrap();
        worker.request(1, None).unwrap();
        assert_eq!(
            completion(&mut worker).result.unwrap_err(),
            "device unavailable"
        );
        let generation = worker.request(2, None).unwrap();
        assert_eq!(completion(&mut worker).generation, generation);
        worker.request(3, None).unwrap();
        worker.invalidate();
        assert!(worker.take_completed().is_none());
    }

    #[test]
    fn retained_devices_are_bounded_and_final_destruction_stays_on_worker() {
        let (ping, _source) = smithay::reexports::calloop::ping::make_ping().unwrap();
        let mut worker: RecoveryWorker<u32, u32, Arc<DroppedOn>> =
            RecoveryWorker::new(ping, Ok::<_, String>, |anchor: &Arc<DroppedOn>| {
                Arc::strong_count(anchor) == 1
            })
            .unwrap();
        let (dropped, dropped_rx) = mpsc::channel();
        let anchors: Vec<_> = (0..RETIREMENT_CAPACITY)
            .map(|_| Arc::new(DroppedOn(dropped.clone())))
            .collect();
        for anchor in &anchors {
            assert!(worker.retire(anchor.clone()).is_ok());
        }
        let extra = Arc::new(DroppedOn(dropped.clone()));
        assert!(worker.retire(extra.clone()).is_err());
        drop(anchors);
        for _ in 0..RETIREMENT_CAPACITY {
            assert_ne!(
                dropped_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
                std::thread::current().id()
            );
        }
        // Only the test's extra anchor remains; it never entered the worker mailbox.
        assert_eq!(Arc::strong_count(&extra), 1);
    }

    #[test]
    fn capacity_refusal_never_enqueues_driver_work_and_preserves_retirement_ownership() {
        let (ping, _source) = smithay::reexports::calloop::ping::make_ping().unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_calls = calls.clone();
        let mut worker = RecoveryWorker::new(
            ping,
            move |job: u32| {
                worker_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(job)
            },
            |anchor: &Arc<DroppedOn>| Arc::strong_count(anchor) == 1,
        )
        .unwrap();
        let (dropped, dropped_rx) = mpsc::channel();
        let mut anchors: Vec<_> = (0..RETIREMENT_CAPACITY - 1)
            .map(|_| Arc::new(DroppedOn(dropped.clone())))
            .collect();
        for anchor in &anchors {
            assert!(worker.retire(anchor.clone()).is_ok());
        }
        let current = Arc::new(DroppedOn(dropped));
        for _ in 0..100 {
            assert!(worker.request(7, Some(current.clone())).is_err());
            assert_eq!(Arc::strong_count(&current), 1);
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(worker.take_completed().is_none());
        // Only actual lease retirement admits another driver initialization.
        drop(anchors.remove(0));
        dropped_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        worker.request(8, Some(current.clone())).unwrap();
        assert_eq!(completion(&mut worker).result.unwrap(), 8);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn teardown_does_not_join_stalled_driver_and_discards_its_result_off_owner() {
        let (ping, _source) = smithay::reexports::calloop::ping::make_ping().unwrap();
        let (started, started_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (dropped, dropped_rx) = mpsc::channel();
        let mut worker = RecoveryWorker::new(
            ping,
            move |_job: u32| {
                started.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(DroppedOn(dropped.clone()))
            },
            |anchor: &Arc<DroppedOn>| Arc::strong_count(anchor) == 1,
        )
        .unwrap();
        worker.request(1, None).unwrap();
        started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        drop(worker);
        release.send(()).unwrap();
        assert_ne!(
            dropped_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            std::thread::current().id()
        );
    }

    #[test]
    fn teardown_retires_the_queued_request_on_worker_after_active_call_returns() {
        struct Job(u32, mpsc::Sender<(u32, ThreadId)>);
        impl Drop for Job {
            fn drop(&mut self) {
                let _ = self.1.send((self.0, std::thread::current().id()));
            }
        }
        let (ping, _source) = smithay::reexports::calloop::ping::make_ping().unwrap();
        let (started, started_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (dropped, dropped_rx) = mpsc::channel();
        let mut worker = RecoveryWorker::new(
            ping,
            move |job: Job| {
                started.send(job.0).unwrap();
                release_rx.recv().unwrap();
                Ok(job.0)
            },
            |_: &()| true,
        )
        .unwrap();
        worker.request(Job(1, dropped.clone()), None).unwrap();
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(3)).unwrap(), 1);
        worker.request(Job(2, dropped), None).unwrap();
        drop(worker);
        // Neither the active call nor the cancelled queued request is destroyed
        // by owner teardown; the first driver call has not returned yet.
        assert!(matches!(
            dropped_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        let owner = std::thread::current().id();
        for id in [1, 2] {
            let (actual, thread) = dropped_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            assert_eq!(actual, id);
            assert_ne!(thread, owner);
        }
        assert!(started_rx.try_recv().is_err());
    }
}
