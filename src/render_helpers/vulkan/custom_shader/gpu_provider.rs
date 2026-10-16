//! Bounded device-only preparation. GPU payloads discarded by an owner or closed receiver
//! retire on this same fixed worker, with their slot retained until destruction completes.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, Weak};

use calloop::channel::{Channel, SyncSender};

const CAPACITY: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    pub renderer: u64,
    pub kind: u8,
    pub generation: u64,
    pub source: String,
}

struct Listener<W, R> {
    reply: SyncSender<Event<W, R>>,
    wake_queued: AtomicBool,
}
pub(crate) struct CapacityWake<W, R>(Arc<Listener<W, R>>);
impl<W, R> Drop for CapacityWake<W, R> {
    fn drop(&mut self) {
        self.0.wake_queued.store(false, Ordering::Release);
    }
}
struct Shared<W, R> {
    jobs: Arc<mpsc::SyncSender<Job<W, R>>>,
    slots: AtomicUsize,
    listeners: Mutex<Vec<Weak<Listener<W, R>>>>,
}
impl<W, R> Shared<W, R> {
    fn notify_capacity(&self) {
        let mut listeners = self.listeners.lock().unwrap();
        listeners.retain(|listener| {
            let Some(listener) = listener.upgrade() else {
                return false;
            };
            if !listener.wake_queued.swap(true, Ordering::AcqRel) {
                // The endpoint holds at most two results and one coalesced capacity wake.
                let _ = listener
                    .reply
                    .try_send(Event::CapacityAvailable(CapacityWake(listener.clone())));
            }
            true
        });
    }
}
struct Slot<W, R>(Arc<Shared<W, R>>);
impl<W, R> Drop for Slot<W, R> {
    fn drop(&mut self) {
        self.0.slots.fetch_sub(1, Ordering::AcqRel);
        self.0.notify_capacity();
    }
}
struct Request<W, R> {
    identity: Identity,
    work: W,
    cancelled: Arc<AtomicBool>,
    slot: Slot<W, R>,
    listener: Arc<Listener<W, R>>,
}
enum Job<W, R> {
    Wake(super::super::lifecycle::Wake),
    Work(Request<W, R>),
    Retire {
        result: Result<R, String>,
        slot: Slot<W, R>,
    },
}
pub(crate) enum Event<W, R> {
    Ready(Completion<W, R>),
    CapacityAvailable(CapacityWake<W, R>),
}
pub(crate) struct Completion<W, R> {
    pub identity: Identity,
    pub result: Option<Result<R, String>>,
    cancelled: Arc<AtomicBool>,
    slot: Option<Slot<W, R>>,
    listener: Arc<Listener<W, R>>,
}
impl<W, R> Completion<W, R> {
    /// Only a validated current owner may consume GPU resources into its active caches.
    pub fn consume(mut self) -> Result<R, String> {
        if super::super::lifecycle::is_closing() {
            return Err("GPU preparation is shutting down".into());
        }
        let result = self.result.take().unwrap();
        drop(self.slot.take());
        result
    }
    /// Reuses the existing slot when a new format appeared during preparation.
    pub fn retry(mut self, transform: impl FnOnce(R) -> W) -> Result<(), String> {
        if super::super::lifecycle::is_closing() {
            return Err("GPU preparation is shutting down".into());
        }
        let result = self.result.take().unwrap()?;
        let slot = self.slot.take().unwrap();
        let shared = slot.0.clone();
        let request = Request {
            identity: self.identity.clone(),
            work: transform(result),
            cancelled: self.cancelled.clone(),
            slot,
            listener: self.listener.clone(),
        };
        enqueue_retained(&shared, Job::Work(request))
    }
}
impl<W, R> Drop for Completion<W, R> {
    fn drop(&mut self) {
        if let (Some(result), Some(slot)) = (self.result.take(), self.slot.take()) {
            let shared = slot.0.clone();
            let _ = enqueue_retained(&shared, Job::Retire { result, slot });
        }
    }
}
fn enqueue_retained<W, R>(shared: &Shared<W, R>, job: Job<W, R>) -> Result<(), String> {
    match shared.jobs.try_send(job) {
        Ok(()) => Ok(()),
        Err(error) => {
            // A retained slot guarantees queue space: at most one other slot exists.
            // If the fixed worker has failed, leak only these at-most-two retained jobs
            // rather than run a last-context GPU idle wait on the owner. Their occupied
            // slots stop all further preparation; no replacement worker is spawned.
            let (mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job)) = error;
            super::super::lifecycle::record_quarantine();
            std::mem::forget(job);
            Err("GPU preparation worker unavailable; bounded retained resources quarantined".into())
        }
    }
}
pub(crate) struct Service<W, R> {
    shared: Arc<Shared<W, R>>,
}
pub(crate) struct Endpoint<W, R> {
    shared: Arc<Shared<W, R>>,
    listener: Arc<Listener<W, R>>,
}
pub(crate) struct Rejected<W> {
    pub work: W,
    pub unavailable: bool,
}
impl<W: Send + 'static, R: Send + 'static> Service<W, R> {
    pub fn new(prepare: fn(W, &AtomicBool) -> Result<R, String>) -> std::io::Result<Self> {
        let (jobs, receiver) = mpsc::sync_channel(CAPACITY + 1);
        let jobs = Arc::new(jobs);
        let shared = Arc::new(Shared {
            jobs,
            slots: AtomicUsize::new(0),
            listeners: Mutex::new(Vec::new()),
        });
        let wake = super::super::lifecycle::queue_waker(&shared.jobs, Job::Wake);
        super::super::lifecycle::spawn_worker("vulkan-program-preparation", wake, move |stop| {
            while let Ok(job) = receiver.recv() {
                match job {
                    Job::Wake(wake) => {
                        drop(wake);
                        if stop.should_exit() {
                            for job in receiver.try_iter() {
                                drop(job);
                            }
                            break;
                        }
                    }
                    Job::Retire { result, slot } => {
                        let _ =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(result)));
                        drop(slot);
                    }
                    Job::Work(request) => {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            if stop.is_closing() || request.cancelled.load(Ordering::Acquire) {
                                drop(request.work);
                                return Err("GPU preparation cancelled".into());
                            }
                            prepare(request.work, &request.cancelled)
                        }))
                        .unwrap_or_else(|_| Err("GPU preparation worker panicked".into()));
                        if stop.is_closing() || request.cancelled.load(Ordering::Acquire) {
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                drop(result)
                            }));
                            drop(request.slot);
                            continue;
                        }
                        let listener = request.listener;
                        let completion = Completion {
                            identity: request.identity,
                            result: Some(result),
                            cancelled: request.cancelled,
                            slot: Some(request.slot),
                            listener: listener.clone(),
                        };
                        // Closed receivers drop Completion on this worker and enqueue bounded retirement.
                        let _ = listener.reply.send(Event::Ready(completion));
                    }
                }
            }
        })?;
        Ok(Self { shared })
    }
    pub fn endpoint(&self) -> (Endpoint<W, R>, Channel<Event<W, R>>) {
        let (reply, receiver) = calloop::channel::sync_channel(CAPACITY + 1);
        let listener = Arc::new(Listener {
            reply,
            wake_queued: AtomicBool::new(false),
        });
        self.shared
            .listeners
            .lock()
            .unwrap()
            .push(Arc::downgrade(&listener));
        (
            Endpoint {
                shared: self.shared.clone(),
                listener,
            },
            receiver,
        )
    }
}
impl<W: Send + 'static, R: Send + 'static> Endpoint<W, R> {
    pub fn request(
        &self,
        identity: Identity,
        work: W,
        cancelled: Arc<AtomicBool>,
    ) -> Result<(), Rejected<W>> {
        if super::super::lifecycle::is_closing() {
            return Err(Rejected {
                work,
                unavailable: true,
            });
        }
        if self
            .shared
            .slots
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |slots| {
                (slots < CAPACITY).then_some(slots + 1)
            })
            .is_err()
        {
            return Err(Rejected {
                work,
                unavailable: false,
            });
        }
        let request = Request {
            identity,
            work,
            cancelled,
            slot: Slot(self.shared.clone()),
            listener: self.listener.clone(),
        };
        match self.shared.jobs.try_send(Job::Work(request)) {
            Ok(()) => Ok(()),
            Err(error) => {
                let (mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job)) = error;
                let Job::Work(request) = job else {
                    unreachable!()
                };
                Err(Rejected {
                    work: request.work,
                    unavailable: true,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;
    struct Payload {
        thread: std::thread::ThreadId,
        retired: mpsc::Sender<std::thread::ThreadId>,
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            let _ = self.retired.send(std::thread::current().id());
        }
    }
    fn prepare(
        retired: mpsc::Sender<std::thread::ThreadId>,
        _: &AtomicBool,
    ) -> Result<Payload, String> {
        Ok(Payload {
            thread: std::thread::current().id(),
            retired,
        })
    }
    fn identity(generation: u64) -> Identity {
        Identity {
            renderer: 17,
            kind: 2,
            generation,
            source: "shader".into(),
        }
    }
    fn ready<W, R>(channel: &Channel<Event<W, R>>) -> Completion<W, R> {
        loop {
            match channel.recv().unwrap() {
                Event::Ready(result) => return result,
                Event::CapacityAvailable(wake) => drop(wake),
            }
        }
    }
    #[test]
    fn discarded_gpu_result_retires_off_owner_and_capacity_wakes_new_idle_endpoint() {
        let service = Service::new(prepare).unwrap();
        let (old, channel) = service.endpoint();
        let (new, new_channel) = service.endpoint();
        let (retired, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        assert!(old
            .request(identity(1), retired.clone(), cancelled.clone())
            .is_ok());
        assert!(old
            .request(identity(2), retired.clone(), cancelled.clone())
            .is_ok());
        let first = ready(&channel);
        let second = ready(&channel);
        let worker = first.result.as_ref().unwrap().as_ref().unwrap().thread;
        assert_ne!(worker, std::thread::current().id());
        assert!(new
            .request(identity(3), retired.clone(), cancelled.clone())
            .is_err());
        drop(first);
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), worker);
        loop {
            if let Event::CapacityAvailable(wake) = new_channel.recv().unwrap() {
                drop(wake);
                break;
            }
        }
        assert!(new.request(identity(3), retired, cancelled).is_ok());
        drop(second);
        drop(ready(&new_channel));
    }
    #[test]
    fn renderer_receiver_drop_retires_bounded_results_and_shared_worker_recovers() {
        let service = Service::new(prepare).unwrap();
        let (endpoint, channel) = service.endpoint();
        let (retired, rx) = mpsc::channel();
        assert!(endpoint
            .request(
                identity(1),
                retired.clone(),
                Arc::new(AtomicBool::new(false))
            )
            .is_ok());
        let result = ready(&channel);
        let worker = result.result.as_ref().unwrap().as_ref().unwrap().thread;
        drop(channel);
        drop(endpoint);
        drop(result);
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), worker);
        let (endpoint, channel) = service.endpoint();
        assert!(endpoint
            .request(identity(2), retired, Arc::new(AtomicBool::new(false)))
            .is_ok());
        drop(ready(&channel));
    }
    #[test]
    fn format_growth_retry_keeps_slot_and_gpu_payload_on_worker_until_ready() {
        enum Work {
            Create(mpsc::Sender<std::thread::ThreadId>),
            MoreFormats(Payload),
        }
        fn prepare_formats(work: Work, _: &AtomicBool) -> Result<Payload, String> {
            match work {
                Work::Create(retired) => prepare(retired, &AtomicBool::new(false)),
                Work::MoreFormats(payload) => {
                    assert_eq!(payload.thread, std::thread::current().id());
                    Ok(payload)
                }
            }
        }
        let service = Service::new(prepare_formats).unwrap();
        let (endpoint, channel) = service.endpoint();
        let (retired, rx) = mpsc::channel();
        assert!(endpoint
            .request(
                identity(1),
                Work::Create(retired.clone()),
                Arc::new(AtomicBool::new(false))
            )
            .is_ok());
        let ready_formats = |channel: &Channel<Event<Work, Payload>>| loop {
            match channel.recv().unwrap() {
                Event::Ready(result) => break result,
                Event::CapacityAvailable(wake) => drop(wake),
            }
        };
        let first = ready_formats(&channel);
        let worker = first.result.as_ref().unwrap().as_ref().unwrap().thread;
        assert!(endpoint
            .request(
                identity(2),
                Work::Create(retired.clone()),
                Arc::new(AtomicBool::new(false))
            )
            .is_ok());
        first.retry(Work::MoreFormats).unwrap();
        assert!(endpoint
            .request(
                identity(3),
                Work::Create(retired),
                Arc::new(AtomicBool::new(false))
            )
            .is_err());
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(10)),
            Err(RecvTimeoutError::Timeout)
        );
        let other = ready_formats(&channel);
        let second = ready_formats(&channel);
        assert_eq!(second.identity.generation, 1);
        assert_eq!(
            second.result.as_ref().unwrap().as_ref().unwrap().thread,
            worker
        );
        drop(other);
        drop(second);
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), worker);
    }
    #[test]
    fn executing_cancellation_keeps_capacity_until_device_work_returns() {
        struct Work {
            started: mpsc::Sender<()>,
            gate: mpsc::Receiver<()>,
            retired: mpsc::Sender<std::thread::ThreadId>,
        }
        fn blocked(work: Work, _: &AtomicBool) -> Result<Payload, String> {
            work.started.send(()).unwrap();
            work.gate.recv_timeout(Duration::from_secs(2)).unwrap();
            prepare(work.retired, &AtomicBool::new(false))
        }
        let service = Service::new(blocked).unwrap();
        let (endpoint, channel) = service.endpoint();
        let (retired, rx) = mpsc::channel();
        let (started, signal) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(endpoint
            .request(
                identity(1),
                Work {
                    started: started.clone(),
                    gate,
                    retired: retired.clone()
                },
                cancel.clone()
            )
            .is_ok());
        signal.recv_timeout(Duration::from_secs(1)).unwrap();
        let (_, queued_gate) = mpsc::channel();
        assert!(endpoint
            .request(
                identity(2),
                Work {
                    started: started.clone(),
                    gate: queued_gate,
                    retired: retired.clone()
                },
                Arc::new(AtomicBool::new(true))
            )
            .is_ok());
        cancel.store(true, Ordering::Release);
        let (_, third_gate) = mpsc::channel();
        assert!(endpoint
            .request(
                identity(3),
                Work {
                    started,
                    gate: third_gate,
                    retired
                },
                Arc::new(AtomicBool::new(false))
            )
            .is_err());
        assert!(
            rx.recv_timeout(Duration::from_millis(10)).is_err(),
            "cancellation destroyed an executing payload early"
        );
        release.send(()).unwrap();
        let worker = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_ne!(worker, std::thread::current().id());
        match channel.recv().unwrap() {
            Event::CapacityAvailable(wake) => drop(wake),
            Event::Ready(_) => panic!("cancelled result was published"),
        }
    }
    #[test]
    fn failed_and_panicking_preparation_do_not_replace_or_spawn_worker() {
        fn run(source: &'static str, _: &AtomicBool) -> Result<&'static str, String> {
            match source {
                "bad" => Err("driver error".into()),
                "panic" => panic!("test preparation failure"),
                _ => Ok(source),
            }
        }
        let service = Service::new(run).unwrap();
        let (endpoint, channel) = service.endpoint();
        for (generation, source) in [(1, "bad"), (2, "panic"), (3, "ready")] {
            assert!(endpoint
                .request(
                    identity(generation),
                    source,
                    Arc::new(AtomicBool::new(false))
                )
                .is_ok());
            let result = ready(&channel).consume();
            if source == "ready" {
                assert_eq!(result.unwrap(), source);
            } else {
                assert!(result.is_err());
            }
        }
    }
}
