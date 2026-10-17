//! Bounded work on owned data. No desktop, protocol or renderer state enters a worker.
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

type Job = Box<dyn FnOnce() + Send>;
const WORKERS: usize = 2;
const OUTSTANDING: usize = 4;

#[derive(Clone)]
pub(crate) struct TaskPool {
    jobs: async_channel::Sender<Job>,
    slots: Arc<AtomicUsize>,
}

struct Slot(Arc<AtomicUsize>);
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Reserve capacity before freezing a large desktop-owned payload. Dropping an
/// unused permit releases admission without enqueueing work.
pub(crate) struct TaskPermit {
    jobs: async_channel::Sender<Job>,
    slot: Slot,
}

pub(crate) struct TaskResult<R> {
    result: async_channel::Receiver<(Result<R, String>, Slot)>,
}

impl<R> TaskResult<R> {
    pub(crate) async fn finish(self) -> Result<R, String> {
        let (result, _slot) = self.result.recv().await.map_err(|e| e.to_string())?;
        result
    }
}

impl TaskPool {
    pub(crate) fn new() -> Self {
        let (jobs, receiver) = async_channel::bounded::<Job>(OUTSTANDING);
        for index in 0..WORKERS {
            let receiver = receiver.clone();
            std::thread::Builder::new()
                .name(format!("clarion-owned-task-{index}"))
                .spawn(move || {
                    while let Ok(job) = receiver.recv_blocking() {
                        job();
                    }
                })
                .expect("could not start owned-data worker");
        }
        Self {
            jobs,
            slots: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(crate) fn reserve(&self) -> Result<TaskPermit, String> {
        self.slots
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < OUTSTANDING).then_some(n + 1)
            })
            .map_err(|_| "owned-data worker capacity reached".to_owned())?;
        Ok(TaskPermit {
            jobs: self.jobs.clone(),
            slot: Slot(self.slots.clone()),
        })
    }

    /// Admission never waits. The slot covers queued, executing and undelivered results.
    pub(crate) fn submit<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<R, String> + Send + 'static,
    ) -> Result<TaskResult<R>, String> {
        self.reserve()?.submit(work)
    }
}

impl TaskPermit {
    pub(crate) fn submit<R: Send + 'static>(
        self,
        work: impl FnOnce() -> Result<R, String> + Send + 'static,
    ) -> Result<TaskResult<R>, String> {
        let Self { jobs, slot } = self;
        let (sender, result) = async_channel::bounded(1);
        jobs.try_send(Box::new(move || {
            // A cancelled receiver does not release a running job's slot prematurely.
            if sender.is_closed() {
                return;
            }
            let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                .unwrap_or_else(|_| Err("owned-data worker panicked".to_owned()));
            let _ = sender.try_send((reply, slot));
        }))
        .map_err(|_| "owned-data worker unavailable".to_owned())?;
        Ok(TaskResult { result })
    }
}

/// Reject pipes/devices before reading and bound allocation even if the file grows.
/// A filesystem stuck inside a kernel read can still occupy one bounded worker slot.
pub(crate) fn read_regular_text(path: &Path, limit: usize) -> io::Result<String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file too large"));
    }
    String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use super::*;

    fn take<R>(task: TaskResult<R>) -> Result<R, String> {
        let (result, _slot) = task.result.recv_blocking().unwrap();
        result
    }

    #[test]
    fn result_stays_bounded_until_consumed() {
        let pool = TaskPool::new();
        let tasks: Vec<_> = (0..OUTSTANDING)
            .map(|i| pool.submit(move || Ok(i)).unwrap())
            .collect();
        assert!(pool.submit(|| Ok(())).is_err());
        for (i, task) in tasks.into_iter().enumerate() {
            assert_eq!(take(task).unwrap(), i);
        }
        assert_eq!(take(pool.submit(|| Ok(7)).unwrap()).unwrap(), 7);
    }

    #[test]
    fn reservations_bound_payloads_before_jobs_are_submitted() {
        let pool = TaskPool::new();
        let mut permits: Vec<_> = (0..OUTSTANDING).map(|_| pool.reserve().unwrap()).collect();
        assert!(pool.reserve().is_err());
        assert!(pool.submit(|| Ok(())).is_err());
        drop(permits.pop());
        let job = pool.submit(|| Ok(7)).unwrap();
        // Completed results and not-yet-submitted payloads share the same bound.
        assert!(pool.reserve().is_err());
        assert_eq!(take(job).unwrap(), 7);
        let reserved = permits.pop().unwrap().submit(|| Ok(11)).unwrap();
        assert_eq!(take(reserved).unwrap(), 11);
        drop(permits);
        assert_eq!(pool.slots.load(Ordering::Acquire), 0);
    }

    #[test]
    fn executing_cancelled_job_keeps_its_slot() {
        let pool = TaskPool::new();
        let started = Arc::new(Barrier::new(2));
        let finish = Arc::new(Barrier::new(2));
        let task = pool
            .submit({
                let started = started.clone();
                let finish = finish.clone();
                move || {
                    started.wait();
                    finish.wait();
                    Ok(())
                }
            })
            .unwrap();
        started.wait();
        drop(task);
        assert_eq!(pool.slots.load(Ordering::Acquire), 1);
        finish.wait();
        // A subsequent job must still execute on the surviving worker pool.
        assert_eq!(take(pool.submit(|| Ok(3)).unwrap()).unwrap(), 3);
    }

    #[test]
    fn panic_does_not_destroy_a_worker_or_leak_capacity() {
        let pool = TaskPool::new();
        let task = pool.submit::<()>(|| panic!("malformed task")).unwrap();
        assert_eq!(take(task).unwrap_err(), "owned-data worker panicked");
        assert_eq!(take(pool.submit(|| Ok(5)).unwrap()).unwrap(), 5);
    }

    #[test]
    fn keymap_text_rejects_oversized_invalid_utf8_and_non_regular_files() {
        let shell = xshell::Shell::new().unwrap();
        let dir = shell.create_temp_dir().unwrap();
        let path = dir.path().join("map");
        std::fs::write(&path, "keymap").unwrap();
        assert_eq!(read_regular_text(&path, 6).unwrap(), "keymap");
        assert!(read_regular_text(&path, 5).is_err());
        std::fs::write(&path, [0xff]).unwrap();
        assert!(read_regular_text(&path, 10).is_err());
        assert!(read_regular_text(Path::new("/dev/null"), 10).is_err());
        let fifo = dir.path().join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(read_regular_text(&fifo, 10).is_err());
    }
}
