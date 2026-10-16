//! Process-wide ownership of Vulkan worker termination.
//!
//! Renderer Drop never joins. Before libc teardown, the process boundary closes
//! admission, cancels queued work, and waits for worker *join* (including TLS and
//! captured-resource destruction). A deadline skips loader/layer atexit with
//! _exit if a driver is stuck; the atexit fallback only verifies prior draining.
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
static CLOSING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static REGISTRY: OnceLock<Arc<Registry>> = OnceLock::new();
const MAX_WORKERS: usize = 64;

/// Cache a service only after it actually starts. Admission/thread-creation
/// failure leaves the cell empty so a later capacity recovery can retry.
/// Once published, the service is never replaced: a disconnected live worker
/// still follows its existing quarantine policy instead of spawning a successor.
/// The factory must return Err only when it has not left a service running.
pub(super) struct ServiceCell<T> {
    value: OnceLock<T>,
    initializing: Mutex<()>,
}
impl<T> ServiceCell<T> {
    pub const fn new() -> Self {
        Self {
            value: OnceLock::new(),
            initializing: Mutex::new(()),
        }
    }
    pub fn get_or_try_init<E>(&self, initialize: impl FnOnce() -> Result<T, E>) -> Result<&T, E> {
        if let Some(service) = self.value.get() {
            return Ok(service);
        }
        let _initializing = self
            .initializing
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(service) = self.value.get() {
            return Ok(service);
        }
        let service = initialize()?;
        // This private cell has one writer, serialized by initializing.
        Ok(self.value.get_or_init(|| service))
    }
}
enum ReaperEvent {
    Registered(u64, std::thread::JoinHandle<()>),
    Finished(u64),
}
type WakeFn = Arc<dyn Fn() + Send + Sync>;
struct Worker {
    name: String,
    wake: Option<WakeFn>,
}
#[derive(Default)]
struct State {
    next: u64,
    workers: BTreeMap<u64, Worker>,
    closing: bool,
    external_tasks: usize,
    live_contexts: usize,
    quarantined: usize,
    panics: usize,
    notifications: usize,
    deadline: Option<Instant>,
}
struct Registry {
    state: Mutex<State>,
    changed: Condvar,
    reaper: mpsc::Sender<ReaperEvent>,
}
impl Registry {
    fn new() -> Arc<Self> {
        let (send, receive) = mpsc::channel();
        let registry = Arc::new(Self {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            reaper: send,
        });
        let owner = Arc::downgrade(&registry);
        std::thread::Builder::new()
            .name("clarion-vk-worker-reaper".into())
            .spawn(move || {
                let mut handles = BTreeMap::new();
                let mut finished = BTreeSet::new();
                loop {
                    let event = if finished.is_empty() {
                        match receive.recv() {
                            Ok(event) => Some(event),
                            Err(_) => return,
                        }
                    } else {
                        match receive.recv_timeout(Duration::from_millis(10)) {
                            Ok(event) => Some(event),
                            Err(mpsc::RecvTimeoutError::Timeout) => None,
                            Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        }
                    };
                    match event {
                        Some(ReaperEvent::Registered(id, handle)) => {
                            handles.insert(id, handle);
                        }
                        Some(ReaperEvent::Finished(id)) => {
                            finished.insert(id);
                        }
                        None => (),
                    }
                    let ready: Vec<_> = finished
                        .iter()
                        .copied()
                        .filter(|id| {
                            handles
                                .get(id)
                                .is_some_and(std::thread::JoinHandle::is_finished)
                        })
                        .collect();
                    for id in ready {
                        // Never join a persistent idle worker merely because it was
                        // registered first. A native TLS tail can still delay this
                        // background join; admission remains hard-bounded then.
                        let panicked = handles.remove(&id).unwrap().join().is_err();
                        finished.remove(&id);
                        let Some(owner) = owner.upgrade() else {
                            return;
                        };
                        let wake = owner
                            .lock()
                            .workers
                            .get_mut(&id)
                            .and_then(|worker| worker.wake.take());
                        drop(wake);
                        let mut state = owner.lock();
                        state.panics += usize::from(panicked);
                        state.workers.remove(&id);
                        owner.changed.notify_all();
                    }
                }
            })
            .expect("cannot start Vulkan lifecycle reaper");
        registry
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
    fn notify_workers(&self) {
        let wakes: Vec<_> = {
            let mut state = self.lock();
            state.notifications += 1;
            state
                .workers
                .values()
                .filter_map(|worker| worker.wake.clone())
                .collect()
        };
        for wake in &wakes {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| wake()));
        }
        drop(wakes);
        self.lock().notifications -= 1;
        self.changed.notify_all();
    }
    fn spawn(
        self: &Arc<Self>,
        name: &str,
        wake: WakeFn,
        run: impl FnOnce(WorkerStop) + Send + 'static,
    ) -> io::Result<()> {
        self.spawn_with(name, wake, run, |run| {
            std::thread::Builder::new().name(name.into()).spawn(run)
        })
    }
    fn spawn_with(
        self: &Arc<Self>,
        name: &str,
        wake: WakeFn,
        run: impl FnOnce(WorkerStop) + Send + 'static,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<std::thread::JoinHandle<()>>,
    ) -> io::Result<()> {
        let mut state = self.lock();
        if state.closing {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Vulkan lifecycle is closing",
            ));
        }
        if state.workers.len() >= MAX_WORKERS {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Vulkan worker lifecycle capacity reached",
            ));
        }
        state.next = state
            .next
            .checked_add(1)
            .expect("Vulkan worker identity exhausted");
        let id = state.next;
        state.workers.insert(
            id,
            Worker {
                name: name.into(),
                wake: Some(wake),
            },
        );
        let stop = WorkerStop(self.clone());
        // The reservation is visible to shutdown before spawning. Do not hold
        // the registry mutex while spawn failure drops its captured GPU owners.
        drop(state);
        let finished = self.reaper.clone();
        let result = spawn(Box::new(move || {
            struct Finished {
                id: u64,
                sender: mpsc::Sender<ReaperEvent>,
            }
            impl Drop for Finished {
                fn drop(&mut self) {
                    let _ = self.sender.send(ReaperEvent::Finished(self.id));
                }
            }
            let _finished = Finished {
                id,
                sender: finished,
            };
            run(stop);
        }));
        match result {
            Ok(handle) => {
                self.reaper
                    .send(ReaperEvent::Registered(id, handle))
                    .expect("Vulkan lifecycle reaper stopped");
                Ok(())
            }
            Err(error) => {
                let wake = self
                    .lock()
                    .workers
                    .get_mut(&id)
                    .and_then(|worker| worker.wake.take());
                drop(wake);
                self.lock().workers.remove(&id);
                self.changed.notify_all();
                Err(error)
            }
        }
    }
    fn close_if_drained(&self) -> bool {
        let mut state = self.lock();
        state.closing = true;
        state.workers.is_empty()
            && state.external_tasks == 0
            && state.live_contexts == 0
            && state.quarantined == 0
            && state.notifications == 0
    }
    fn shutdown(&self, timeout: Duration) -> Result<ShutdownReport, ShutdownTimeout> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        self.lock().closing = true;
        self.notify_workers();
        let mut state = self.lock();
        loop {
            if state.workers.is_empty()
                && state.external_tasks == 0
                && state.live_contexts == 0
                && state.quarantined == 0
                && state.notifications == 0
            {
                return Ok(ShutdownReport {
                    worker_panics: state.panics,
                });
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ShutdownTimeout {
                    workers: state
                        .workers
                        .values()
                        .map(|worker| worker.name.clone())
                        .collect(),
                    external_tasks: state.external_tasks,
                    live_contexts: state.live_contexts,
                    quarantined: state.quarantined,
                });
            }
            let result = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|error| error.into_inner());
            state = result.0;
        }
    }
}
fn registry() -> &'static Arc<Registry> {
    REGISTRY.get_or_init(Registry::new)
}
#[cfg(any(clarion_exit_trampoline, test))]
fn close_admission_after_publication(cell: &OnceLock<Arc<Registry>>) -> bool {
    // Join a concurrent first initializer before inspecting admission. A separate
    // atomic CLOSING check cannot synchronize an empty OnceLock snapshot with a
    // producer's publication; both sides could otherwise observe stale values.
    cell.get_or_init(Registry::new).close_if_drained()
}
fn closing_error() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "Vulkan lifecycle is closing")
}

#[derive(Clone)]
pub struct WorkerStop(Arc<Registry>);
impl WorkerStop {
    pub fn is_closing(&self) -> bool {
        self.0.lock().closing
    }
    /// Existing external readbacks may still enqueue retirement after close.
    /// Their lease must finish before a persistent retirement queue can stop.
    pub fn should_exit(&self) -> bool {
        let state = self.0.lock();
        state.closing && state.external_tasks == 0
    }
}

/// No native/desktop/Rc value is made Send by this API. Rust checks every capture.
pub fn spawn_worker(
    name: &str,
    wake: impl Fn() + Send + Sync + 'static,
    run: impl FnOnce(WorkerStop) + Send + 'static,
) -> io::Result<()> {
    let owner = registry();
    // Check after registry initialization: an exit boundary that observed an
    // empty OnceLock has already closed admission before it may return to libc.
    if is_closing() {
        return Err(closing_error());
    }
    owner.spawn(name, Arc::new(wake), run)
}
pub fn spawn_task(name: &str, run: impl FnOnce() + Send + 'static) -> io::Result<()> {
    spawn_worker(name, || {}, move |_| run())
}
pub fn is_closing() -> bool {
    CLOSING.load(std::sync::atomic::Ordering::Acquire)
}

/// A coalesced queue wake occupies its reserved control slot until consumed.
pub(crate) struct Wake(Arc<std::sync::atomic::AtomicBool>);
impl Drop for Wake {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}
pub(crate) fn queue_waker<T: Send + 'static>(
    sender: &Arc<mpsc::SyncSender<T>>,
    wrap: fn(Wake) -> T,
) -> impl Fn() + Send + Sync + 'static {
    let sender = Arc::downgrade(sender);
    let queued = Arc::new(std::sync::atomic::AtomicBool::new(false));
    move || {
        if let Some(sender) = sender.upgrade() {
            if !queued.swap(true, std::sync::atomic::Ordering::AcqRel) {
                let _ = sender.try_send(wrap(Wake(queued.clone())));
            }
        }
    }
}
pub(crate) fn unbounded_queue_waker<T: Send + 'static>(
    sender: &Arc<mpsc::Sender<T>>,
    wrap: fn(Wake) -> T,
) -> impl Fn() + Send + Sync + 'static {
    let sender = Arc::downgrade(sender);
    let queued = Arc::new(std::sync::atomic::AtomicBool::new(false));
    move || {
        if let Some(sender) = sender.upgrade() {
            if !queued.swap(true, std::sync::atomic::Ordering::AcqRel) {
                let _ = sender.send(wrap(Wake(queued.clone())));
            }
        }
    }
}

/// Covers GPU-bearing jobs handed to other executors, including portal readback.
/// Keep this field after the GPU resources it protects so its Drop is last.
pub(crate) struct TaskLease(Arc<Registry>);
impl TaskLease {
    pub fn acquire() -> io::Result<Self> {
        let owner = registry().clone();
        if is_closing() {
            return Err(closing_error());
        }
        Self::acquire_from(owner)
    }
    fn acquire_from(owner: Arc<Registry>) -> io::Result<Self> {
        {
            let mut state = owner.lock();
            if state.closing {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Vulkan lifecycle is closing",
                ));
            }
            state.external_tasks += 1;
        }
        Ok(Self(owner))
    }
}
impl Drop for TaskLease {
    fn drop(&mut self) {
        let closing = {
            let mut state = self.0.lock();
            state.external_tasks -= 1;
            self.0.changed.notify_all();
            state.closing
        };
        if closing {
            self.0.notify_workers();
        }
    }
}

/// Admitted initialization and every context owner remain counted through the
/// last field, after vkDestroyDevice, vkDestroyInstance and Entry/library release.
pub(super) struct ContextLease(Arc<Registry>);
impl ContextLease {
    pub fn acquire() -> io::Result<Self> {
        let owner = registry().clone();
        if is_closing() {
            return Err(closing_error());
        }
        Self::acquire_from(owner)
    }
    fn acquire_from(owner: Arc<Registry>) -> io::Result<Self> {
        {
            let mut state = owner.lock();
            if state.closing {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Vulkan lifecycle is closing",
                ));
            }
            state.live_contexts += 1;
        }
        Ok(Self(owner))
    }
}
impl Drop for ContextLease {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.live_contexts -= 1;
        self.0.changed.notify_all();
    }
}
pub(crate) fn record_quarantine() {
    let mut state = registry().lock();
    state.quarantined += 1;
    registry().changed.notify_all();
}

#[derive(Debug)]
pub struct ShutdownReport {
    pub worker_panics: usize,
}
#[derive(Debug)]
pub struct ShutdownTimeout {
    pub workers: Vec<String>,
    pub external_tasks: usize,
    pub live_contexts: usize,
    pub quarantined: usize,
}

/// Exit-only ownership diagnostics. Release the registry lock before logging;
/// no driver calls or event dispatch are needed to identify a retained owner.
pub fn log_shutdown_state(stage: &'static str) {
    let (workers, external_tasks, live_contexts, quarantined, notifications, worker_panics) = {
        let state = registry().lock();
        (
            state
                .workers
                .values()
                .map(|worker| worker.name.clone())
                .collect::<Vec<_>>(),
            state.external_tasks,
            state.live_contexts,
            state.quarantined,
            state.notifications,
            state.panics,
        )
    };
    info!(
        stage,
        workers = ?workers,
        external_tasks,
        live_contexts,
        quarantined,
        notifications,
        worker_panics,
        "Vulkan shutdown ownership"
    );
}

pub fn shutdown_workers(timeout: Duration) -> Result<ShutdownReport, ShutdownTimeout> {
    CLOSING.store(true, std::sync::atomic::Ordering::Release);
    log_shutdown_state("worker-drain");
    registry().shutdown(timeout)
}

fn terminate() -> ! {
    // No Rust formatting/logger locks and no libc exit/layer atexit on this path.
    let message = b"Clarion: Vulkan shutdown deadline/quarantine; skipping unsafe loader teardown (exit 124)\n";
    let mut descriptor = libc::pollfd {
        fd: libc::STDERR_FILENO,
        events: libc::POLLOUT,
        revents: 0,
    };
    unsafe {
        if libc::poll(&mut descriptor, 1, 0) > 0 {
            let _ = libc::write(libc::STDERR_FILENO, message.as_ptr().cast(), message.len());
        }
        libc::_exit(124)
    }
}
pub fn shutdown_or_terminate(timeout: Duration) {
    arm_exit_deadline(timeout);
    let report = match shutdown_workers(timeout) {
        Ok(report) => report,
        Err(_) => terminate(),
    };
    info!(
        worker_panics = report.worker_panics,
        "Vulkan worker shutdown complete"
    );
    registry().lock().deadline = None;
    registry().changed.notify_all();
}

/// The executable's C startup references main from a separate crt object, and
/// Rust's process::exit references libc exit. GNU ld --wrap redirects both before
/// libc begins any layer/global atexit teardown. This also covers default libtest
/// success (main returns) and failure/explicit exit (main never returns).
#[cfg(clarion_exit_trampoline)]
mod process_boundary {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    static ENTERED: AtomicBool = AtomicBool::new(false);
    unsafe extern "C" {
        fn __real_main(
            argc: libc::c_int,
            argv: *mut *mut libc::c_char,
            envp: *mut *mut libc::c_char,
        ) -> libc::c_int;
        fn __real_exit(status: libc::c_int) -> !;
    }
    fn drain() {
        if ENTERED.swap(true, Ordering::AcqRel) {
            // A second exit during process cleanup must not race or recursively
            // join the first exit's worker. Preserve safety through OS teardown.
            terminate();
        }
        CLOSING.store(true, Ordering::Release);
        // Even an empty registry must be published and closed under the same
        // state mutex used by producer admission. Empty CLI exits need no watchdog.
        if close_admission_after_publication(&REGISTRY) {
            return;
        }
        if std::panic::catch_unwind(|| shutdown_or_terminate(exit_timeout())).is_err() {
            terminate();
        }
    }
    /// # Safety
    /// Called only by the linked C startup object with its original main argv/envp.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_main(
        argc: libc::c_int,
        argv: *mut *mut libc::c_char,
        envp: *mut *mut libc::c_char,
    ) -> libc::c_int {
        let status = unsafe { __real_main(argc, argv, envp) };
        drain();
        status
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn __wrap_exit(status: libc::c_int) -> ! {
        drain();
        unsafe { __real_exit(status) }
    }
}

pub struct ExitDeadline;
impl ExitDeadline {
    pub fn disarm(self) {
        registry().lock().deadline = None;
        registry().changed.notify_all();
    }
}
impl Drop for ExitDeadline {
    fn drop(&mut self) {
        registry().lock().deadline = None;
        registry().changed.notify_all();
    }
}
pub fn begin_shutdown_deadline(timeout: Duration) -> ExitDeadline {
    arm_exit_deadline(timeout);
    ExitDeadline
}
pub fn arm_exit_deadline(timeout: Duration) {
    static WATCHDOG: OnceLock<()> = OnceLock::new();
    WATCHDOG.get_or_init(|| {
        let owner = Arc::downgrade(registry());
        std::thread::Builder::new()
            .name("clarion-vk-exit-deadline".into())
            .spawn(move || {
                let Some(owner) = owner.upgrade() else {
                    return;
                };
                let mut state = owner.lock();
                loop {
                    if let Some(deadline) = state.deadline {
                        let left = deadline.saturating_duration_since(Instant::now());
                        if left.is_zero() {
                            drop(state);
                            terminate();
                        }
                        state = owner
                            .changed
                            .wait_timeout(state, left)
                            .unwrap_or_else(|error| error.into_inner())
                            .0;
                    } else {
                        state = owner
                            .changed
                            .wait(state)
                            .unwrap_or_else(|error| error.into_inner());
                    }
                }
            })
            .expect("cannot start Vulkan exit watchdog");
    });
    let mut state = registry().lock();
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    state.deadline = Some(state.deadline.map_or(deadline, |old| old.min(deadline)));
    registry().changed.notify_all();
}

/// Last-resort validation only: never start GPU destruction from atexit.
/// VVL registers another ApplicationAtExit on every vkCreateInstance, so a
/// once-registered callback cannot precede all future layer cleanup. Main and
/// the linked main/exit trampolines must drain before libc starts its callbacks.
pub(super) fn install_exit_hook_after_driver_initialization() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        extern "C" fn on_exit() {
            let result = std::panic::catch_unwind(|| {
                CLOSING.store(true, std::sync::atomic::Ordering::Release);
                arm_exit_deadline(exit_timeout());
                if registry().close_if_drained() {
                    registry().lock().deadline = None;
                    registry().changed.notify_all();
                } else {
                    // A layer may already have erased its dispatch tables. Waking
                    // a pending worker here would turn safe retention into a crash.
                    terminate();
                }
            });
            if result.is_err() {
                terminate();
            }
        }
        if unsafe { libc::atexit(on_exit) } != 0 {
            terminate();
        }
    });
}
fn exit_timeout() -> Duration {
    #[cfg(test)]
    {
        let value = TEST_EXIT_TIMEOUT_MS.load(std::sync::atomic::Ordering::Acquire);
        if value != 0 {
            return Duration::from_millis(value);
        }
    }
    DEFAULT_SHUTDOWN_TIMEOUT
}
#[cfg(test)]
static TEST_EXIT_TIMEOUT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(test)]
pub(super) fn set_exit_timeout_for_test(timeout: Duration) {
    TEST_EXIT_TIMEOUT_MS.store(
        timeout.as_millis() as u64,
        std::sync::atomic::Ordering::Release,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const WAIT: Duration = Duration::from_secs(3);
    fn wait_workers(owner: &Registry, count: usize) {
        let deadline = Instant::now() + WAIT;
        let mut state = owner.lock();
        while state.workers.len() != count {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "workers not reaped: {}",
                state.workers.len()
            );
            state = owner.changed.wait_timeout(state, remaining).unwrap().0;
        }
    }
    struct CountDrop(Arc<AtomicUsize>);
    impl Drop for CountDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Release);
        }
    }

    #[test]
    fn service_initialization_retries_after_capacity_failure_without_caching_error() {
        let service = ServiceCell::new();
        let capacity_available = std::sync::atomic::AtomicBool::new(false);
        let attempts = AtomicUsize::new(0);
        let initialize = || {
            attempts.fetch_add(1, Ordering::Relaxed);
            if capacity_available.load(Ordering::Acquire) {
                Ok(17usize)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "injected admission capacity",
                ))
            }
        };
        assert_eq!(
            service.get_or_try_init(initialize).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(service.value.get().is_none());
        capacity_available.store(true, Ordering::Release);
        assert_eq!(*service.get_or_try_init(initialize).unwrap(), 17);
        assert_eq!(*service.get_or_try_init(initialize).unwrap(), 17);
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn concurrent_service_initialization_publishes_exactly_one_service() {
        let service = Arc::new(ServiceCell::new());
        let attempts = Arc::new(AtomicUsize::new(0));
        let (entered, observed) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let first = service.clone();
        let first_attempts = attempts.clone();
        let initializer = std::thread::spawn(move || {
            first
                .get_or_try_init(|| {
                    first_attempts.fetch_add(1, Ordering::Relaxed);
                    entered.send(()).unwrap();
                    gate.recv().unwrap();
                    Ok::<_, io::Error>(Arc::new(19usize))
                })
                .unwrap()
                .clone()
        });
        observed.recv_timeout(WAIT).unwrap();
        let simultaneous = Arc::new(std::sync::Barrier::new(17));
        let contenders: Vec<_> = (0..16)
            .map(|_| {
                let service = service.clone();
                let attempts = attempts.clone();
                let simultaneous = simultaneous.clone();
                std::thread::spawn(move || {
                    simultaneous.wait();
                    service
                        .get_or_try_init(|| {
                            attempts.fetch_add(1, Ordering::Relaxed);
                            Ok::<_, io::Error>(Arc::new(23usize))
                        })
                        .unwrap()
                        .clone()
                })
            })
            .collect();
        simultaneous.wait();
        release.send(()).unwrap();
        let first = initializer.join().unwrap();
        for contender in contenders {
            assert!(Arc::ptr_eq(&first, &contender.join().unwrap()));
        }
        assert_eq!(*first, 19);
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn published_service_is_not_replaced_after_its_worker_disconnects() {
        let service = ServiceCell::new();
        let attempts = AtomicUsize::new(0);
        let mut worker = None;
        let endpoint = service
            .get_or_try_init(|| {
                attempts.fetch_add(1, Ordering::Relaxed);
                let (send, receive) = mpsc::channel();
                worker = Some(std::thread::spawn(move || receive.recv().unwrap()));
                Ok::<_, io::Error>(send)
            })
            .unwrap();
        endpoint.send(()).unwrap();
        worker.take().unwrap().join().unwrap();
        assert!(endpoint.send(()).is_err());
        let same = service
            .get_or_try_init(|| {
                attempts.fetch_add(1, Ordering::Relaxed);
                Err::<mpsc::Sender<()>, _>(io::Error::other(
                    "a published worker must never respawn",
                ))
            })
            .unwrap();
        assert!(std::ptr::eq(endpoint, same));
        assert!(same.send(()).is_err());
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn persistent_worker_does_not_block_short_task_reaping_or_capacity_reuse() {
        let owner = Registry::new();
        let (send, receive) = mpsc::channel();
        owner
            .spawn(
                "persistent-first",
                Arc::new(move || {
                    let _ = send.send(());
                }),
                move |_| {
                    receive.recv().unwrap();
                },
            )
            .unwrap();
        let reclaimed = Arc::new(AtomicUsize::new(0));
        for batch in 0..8 {
            for _ in 0..32 {
                let captured = CountDrop(reclaimed.clone());
                owner
                    .spawn(
                        "short",
                        Arc::new(move || {
                            let _ = &captured;
                        }),
                        |_| {},
                    )
                    .unwrap();
            }
            wait_workers(&owner, 1);
            assert_eq!(reclaimed.load(Ordering::Acquire), (batch + 1) * 32);
        }
        assert_eq!(owner.shutdown(WAIT).unwrap().worker_panics, 0);
    }

    struct DropGate {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl Drop for DropGate {
        fn drop(&mut self) {
            self.entered.send(()).unwrap();
            self.release.get_mut().unwrap().recv().unwrap();
        }
    }
    #[test]
    fn shutdown_acknowledges_only_after_wake_capture_destruction() {
        let owner = Registry::new();
        let (entered, observed) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let captured = DropGate {
            entered,
            release: Mutex::new(gate),
        };
        owner
            .spawn(
                "capture",
                Arc::new(move || {
                    let _ = &captured;
                }),
                |_| {},
            )
            .unwrap();
        observed.recv_timeout(WAIT).unwrap();
        let result = owner.shutdown(Duration::ZERO).unwrap_err();
        assert_eq!(result.workers, ["capture"]);
        release.send(()).unwrap();
        owner.shutdown(WAIT).unwrap();
    }

    thread_local! {
        static TLS_GATE: std::cell::RefCell<Option<DropGate>> = const { std::cell::RefCell::new(None) };
    }
    #[test]
    fn worker_function_return_does_not_acknowledge_pending_native_tls_destructor() {
        let owner = Registry::new();
        let (entered, observed) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let (returned, body) = mpsc::channel();
        owner
            .spawn("tls", Arc::new(|| {}), move |_| {
                TLS_GATE.with(|slot| {
                    *slot.borrow_mut() = Some(DropGate {
                        entered,
                        release: Mutex::new(gate),
                    })
                });
                returned.send(()).unwrap();
            })
            .unwrap();
        body.recv_timeout(WAIT).unwrap();
        observed.recv_timeout(WAIT).unwrap();
        assert_eq!(owner.shutdown(Duration::ZERO).unwrap_err().workers, ["tls"]);
        release.send(()).unwrap();
        owner.shutdown(WAIT).unwrap();
    }

    #[test]
    fn exit_waits_for_first_registry_publication_and_closes_stale_producer_admission() {
        let cell = Arc::new(OnceLock::new());
        let (initializing, observed) = mpsc::channel();
        let (publish, publication_gate) = mpsc::channel();
        let (admit, admission_gate) = mpsc::channel();
        let producer_cell = cell.clone();
        let producer = std::thread::spawn(move || {
            let owner = producer_cell.get_or_init(|| {
                initializing.send(()).unwrap();
                publication_gate.recv().unwrap();
                Registry::new()
            });
            // Deliberately bypass any front-door AtomicBool check, modelling
            // a producer that already observed its old false value.
            admission_gate.recv().unwrap();
            ContextLease::acquire_from(owner.clone()).is_err()
        });
        observed.recv_timeout(WAIT).unwrap();
        assert!(cell.get().is_none());
        let (started, boundary_started) = mpsc::channel();
        let (closed, boundary_closed) = mpsc::channel();
        let boundary_cell = cell.clone();
        let boundary = std::thread::spawn(move || {
            started.send(()).unwrap();
            closed
                .send(close_admission_after_publication(&boundary_cell))
                .unwrap();
        });
        boundary_started.recv_timeout(WAIT).unwrap();
        assert_eq!(
            boundary_closed.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout),
            "an unpublished registry cannot be treated as drained while its initializer is gated"
        );
        publish.send(()).unwrap();
        assert!(boundary_closed.recv_timeout(WAIT).unwrap());
        admit.send(()).unwrap();
        assert!(
            producer.join().unwrap(),
            "state-mutex close must reject even a stale atomic-gate reader"
        );
        boundary.join().unwrap();
        cell.get().unwrap().shutdown(WAIT).unwrap();

        // The opposite ordering retains already admitted owners in the barrier.
        let published = OnceLock::new();
        let owner = Registry::new();
        let lease = ContextLease::acquire_from(owner.clone()).unwrap();
        assert!(published.set(owner.clone()).is_ok());
        assert!(!close_admission_after_publication(&published));
        drop(lease);
        owner.shutdown(WAIT).unwrap();
    }

    #[test]
    fn atexit_fallback_closes_admission_without_waking_live_gpu_workers() {
        let owner = Registry::new();
        let wakes = Arc::new(AtomicUsize::new(0));
        let wake_count = wakes.clone();
        let (release, gate) = mpsc::channel();
        owner
            .spawn(
                "retained-at-exit",
                Arc::new(move || {
                    wake_count.fetch_add(1, Ordering::Relaxed);
                }),
                move |_| {
                    gate.recv().unwrap();
                },
            )
            .unwrap();
        assert!(!owner.close_if_drained());
        assert_eq!(
            wakes.load(Ordering::Relaxed),
            0,
            "atexit must never restart GPU destruction after layer cleanup may have begun"
        );
        assert!(ContextLease::acquire_from(owner.clone()).is_err());
        release.send(()).unwrap();
        owner.shutdown(WAIT).unwrap();
        assert!(owner.close_if_drained());
    }

    #[test]
    fn context_and_external_task_ownership_prevent_false_shutdown_completion() {
        let owner = Registry::new();
        let context = ContextLease::acquire_from(owner.clone()).unwrap();
        let task = TaskLease::acquire_from(owner.clone()).unwrap();
        let status = owner.shutdown(Duration::ZERO).unwrap_err();
        assert_eq!((status.live_contexts, status.external_tasks), (1, 1));
        assert!(ContextLease::acquire_from(owner.clone()).is_err());
        assert!(TaskLease::acquire_from(owner.clone()).is_err());
        drop(context);
        assert_eq!(
            owner.shutdown(Duration::ZERO).unwrap_err().external_tasks,
            1
        );
        drop(task);
        owner.shutdown(WAIT).unwrap();
        owner.shutdown(Duration::ZERO).unwrap();
    }

    struct ReenterOnDrop {
        owner: Arc<Registry>,
        done: mpsc::Sender<()>,
    }
    impl Drop for ReenterOnDrop {
        fn drop(&mut self) {
            let _state = self.owner.lock();
            self.done.send(()).unwrap();
        }
    }
    #[test]
    fn spawn_failure_drops_gpu_captures_without_holding_registry_mutex() {
        let owner = Registry::new();
        let (done, observed) = mpsc::channel();
        let captured = ReenterOnDrop {
            owner: owner.clone(),
            done,
        };
        let result = owner.spawn_with(
            "failed",
            Arc::new(|| {}),
            move |_| {
                drop(captured);
            },
            |run| {
                // Builder::spawn owns and drops the closure on failure too.
                drop(run);
                Err(io::Error::other("injected thread creation failure"))
            },
        );
        assert!(result.is_err());
        observed.recv_timeout(WAIT).unwrap();
        assert!(owner.lock().workers.is_empty());
        owner.shutdown(WAIT).unwrap();
    }

    #[test]
    fn shutdown_tracks_reserved_worker_before_thread_start_and_rejects_later_admission() {
        let owner = Registry::new();
        let (reserved, seen) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let (closed, observed) = mpsc::channel();
        let producer = owner.clone();
        let admission = std::thread::spawn(move || {
            producer.spawn_with(
                "reserved",
                Arc::new(|| {}),
                move |stop| {
                    closed.send(stop.is_closing()).unwrap();
                },
                move |run| {
                    reserved.send(()).unwrap();
                    gate.recv().unwrap();
                    std::thread::Builder::new().spawn(run)
                },
            )
        });
        seen.recv_timeout(WAIT).unwrap();
        assert_eq!(
            owner.shutdown(Duration::ZERO).unwrap_err().workers,
            ["reserved"]
        );
        release.send(()).unwrap();
        admission.join().unwrap().unwrap();
        assert!(observed.recv_timeout(WAIT).unwrap());
        owner.shutdown(WAIT).unwrap();
        assert_eq!(
            owner
                .spawn("late", Arc::new(|| {}), |_| {})
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
        owner.shutdown(Duration::ZERO).unwrap();
    }

    #[test]
    fn admission_capacity_is_hard_bounded_until_join_and_capture_ack() {
        let owner = Registry::new();
        for _ in 0..MAX_WORKERS {
            let (send, receive) = mpsc::channel();
            owner
                .spawn(
                    "held",
                    Arc::new(move || {
                        let _ = send.send(());
                    }),
                    move |_| {
                        receive.recv().unwrap();
                    },
                )
                .unwrap();
        }
        let (done, observed) = mpsc::channel();
        let captured = ReenterOnDrop {
            owner: owner.clone(),
            done,
        };
        assert_eq!(
            owner
                .spawn("overflow", Arc::new(|| {}), move |_| drop(captured))
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        observed.recv_timeout(WAIT).unwrap();
        assert_eq!(owner.lock().workers.len(), MAX_WORKERS);
        owner.shutdown(WAIT).unwrap();
    }

    #[test]
    fn worker_panic_is_joined_and_reported_without_poisoning_shutdown() {
        let owner = Registry::new();
        owner
            .spawn("panic", Arc::new(|| {}), |_| {
                panic!("isolated worker failure")
            })
            .unwrap();
        assert_eq!(owner.shutdown(WAIT).unwrap().worker_panics, 1);
        assert_eq!(owner.shutdown(Duration::ZERO).unwrap().worker_panics, 1);
    }
}
