//! Two-slot gamma data reader. No output, resource or compositor handle enters this worker.
//! Pipe reads use an independently opened NONBLOCK description, preserving the sender's flags.
//! A regular-file/FUSE kernel read/open cannot be preempted by Rust: such a stuck read retains
//! its slot, so work remains bounded and the compositor stays responsive, but the two slots can
//! become unavailable until the kernel operation finishes.
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use calloop::channel::Channel;

const CAPACITY: usize = 2;
const MAX_BYTES: usize = 16 * 1024 * 1024;
pub(super) const DEADLINE: Duration = Duration::from_secs(1);

struct Permit(Arc<AtomicUsize>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}
struct Job {
    id: u64,
    fd: OwnedFd,
    bytes: usize,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
    permit: Permit,
}
pub(super) struct Completion {
    pub id: u64,
    pub result: io::Result<Vec<u16>>,
    _permit: Permit,
}
pub(super) struct Reader {
    tx: mpsc::SyncSender<Job>,
    slots: Arc<AtomicUsize>,
}
impl Reader {
    pub fn new() -> (Self, Channel<Completion>) {
        let (tx, rx) = mpsc::sync_channel::<Job>(CAPACITY);
        let rx = Arc::new(Mutex::new(rx));
        let slots = Arc::new(AtomicUsize::new(0));
        let (complete_tx, complete_rx) = calloop::channel::sync_channel::<Completion>(CAPACITY);
        for number in 0..CAPACITY {
            let rx = rx.clone();
            let tx = complete_tx.clone();
            if let Err(error) = std::thread::Builder::new()
                .name(format!("gamma-reader-{number}"))
                .spawn(move || loop {
                    let job = { rx.lock().unwrap().recv() };
                    let Ok(job) = job else {
                        break;
                    };
                    let result = read(job.fd, job.bytes, job.deadline, &job.cancel);
                    if tx
                        .send(Completion {
                            id: job.id,
                            result,
                            _permit: job.permit,
                        })
                        .is_err()
                    {
                        break;
                    }
                })
            {
                warn!("cannot start gamma reader: {error}");
            }
        }
        (Self { tx, slots }, complete_rx)
    }
    pub fn request(
        &self,
        id: u64,
        fd: OwnedFd,
        size: u32,
        cancel: Arc<AtomicBool>,
    ) -> io::Result<()> {
        let bytes = (size as usize)
            .checked_mul(6)
            .filter(|bytes| *bytes > 0 && *bytes <= MAX_BYTES)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "gamma size exceeds bound")
            })?;
        self.slots
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |slots| {
                (slots < CAPACITY).then_some(slots + 1)
            })
            .map_err(|_| {
                io::Error::new(io::ErrorKind::WouldBlock, "gamma reader capacity exhausted")
            })?;
        let permit = Permit(self.slots.clone());
        self.tx
            .try_send(Job {
                id,
                fd,
                bytes,
                cancel,
                deadline: Instant::now() + DEADLINE,
                permit,
            })
            .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "gamma reader unavailable"))
    }
}
fn read(
    fd: OwnedFd,
    expected: usize,
    deadline: Instant,
    cancel: &AtomicBool,
) -> io::Result<Vec<u16>> {
    if expected == 0 || expected > MAX_BYTES || !expected.is_multiple_of(2) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid gamma payload length",
        ));
    }
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "gamma request cancelled",
        ));
    }
    let mut original = File::from(fd);
    let position = original.stream_position().ok();
    // open(/proc/self/fd/N), unlike dup, creates an independent file description for files/pipes.
    // Socket descriptions cannot be reopened this way and fail explicitly, without flag mutation.
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(format!("/proc/self/fd/{}", original.as_raw_fd()))?;
    if let Some(position) = position {
        file.seek(io::SeekFrom::Start(position))?;
    }
    let mut data = vec![0u8; expected + 1];
    let mut filled = 0;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "gamma request cancelled",
            ));
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "gamma payload/EOF deadline exceeded",
            ));
        }
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout = deadline
            .saturating_duration_since(now)
            .as_millis()
            .clamp(1, 10) as i32;
        // SAFETY: poll points to one live stack descriptor for the duration of this call.
        let ready = unsafe { libc::poll(&mut poll, 1, timeout) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready == 0 {
            continue;
        }
        match file.read(&mut data[filled..]) {
            Ok(0) if filled == expected => {
                return Ok(data[..expected]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_ne_bytes(*pair))
                    .collect())
            }
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "gamma payload too short",
                ))
            }
            Ok(bytes) => {
                filled += bytes;
                if filled > expected {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "gamma payload too large",
                    ));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::{AsFd, FromRawFd};
    fn pipe() -> (OwnedFd, File) {
        let mut pair = [0; 2];
        // SAFETY: pipe2 writes exactly two descriptors into pair; both become owned below.
        assert_eq!(
            unsafe { libc::pipe2(pair.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        unsafe { (OwnedFd::from_raw_fd(pair[0]), File::from_raw_fd(pair[1])) }
    }
    fn flags(fd: &impl AsFd) -> i32 {
        unsafe { libc::fcntl(fd.as_fd().as_raw_fd(), libc::F_GETFL) }
    }
    #[test]
    fn pipe_with_exact_payload_but_open_writer_times_out_without_shared_flag_changes() {
        let (fd, mut writer) = pipe();
        writer.write_all(&[1, 0, 2, 0, 3, 0]).unwrap();
        let before = flags(&fd);
        // dup shares flags with fd; it remains open while the independent reader consumes data.
        let duplicate = fd.try_clone().unwrap();
        let error = read(
            fd,
            6,
            Instant::now() + Duration::from_millis(35),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(flags(&duplicate), before);
    }
    #[test]
    fn exact_lut_short_and_excess_payloads_are_distinct() {
        for (payload, expected_error) in [
            (&[1, 0, 2, 0, 3, 0][..], None),
            (&[1, 0][..], Some(io::ErrorKind::UnexpectedEof)),
            (&[1, 0, 2, 0, 3, 0, 4][..], Some(io::ErrorKind::InvalidData)),
        ] {
            let (fd, mut writer) = pipe();
            writer.write_all(payload).unwrap();
            drop(writer);
            let result = read(
                fd,
                6,
                Instant::now() + Duration::from_secs(1),
                &AtomicBool::new(false),
            );
            if let Some(error) = expected_error {
                assert_eq!(result.unwrap_err().kind(), error);
            } else {
                assert_eq!(result.unwrap(), [1, 2, 3]);
            }
        }
    }
    #[test]
    fn cancellation_is_checked_and_regular_file_offset_does_not_change_sender() {
        let (fd, _writer) = pipe();
        assert_eq!(
            read(
                fd,
                6,
                Instant::now() + Duration::from_secs(1),
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::Interrupted
        );
        let path =
            std::env::temp_dir().join(format!("clarion-gamma-reader-{}", std::process::id()));
        std::fs::write(&path, [9, 9, 1, 0, 2, 0, 3, 0]).unwrap();
        let mut file = File::open(&path).unwrap();
        file.seek(io::SeekFrom::Start(2)).unwrap();
        let mut duplicate = file.try_clone().unwrap();
        assert_eq!(
            read(
                file.into(),
                6,
                Instant::now() + Duration::from_secs(1),
                &AtomicBool::new(false)
            )
            .unwrap(),
            [1, 2, 3]
        );
        let mut first = [0];
        duplicate.read_exact(&mut first).unwrap();
        assert_eq!(first[0], 1);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn capacity_is_held_through_completion_and_rejects_a_third_job() {
        let (reader, source) = Reader::new();
        let (one, _writer_one) = pipe();
        let (two, _writer_two) = pipe();
        let (three, _writer_three) = pipe();
        let cancel = Arc::new(AtomicBool::new(false));
        reader.request(1, one, 1, cancel.clone()).unwrap();
        reader.request(2, two, 1, cancel.clone()).unwrap();
        assert_eq!(
            reader
                .request(3, three, 1, cancel.clone())
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        cancel.store(true, Ordering::Release);
        // The reader cannot acquire a third permit merely because workers have finished.
        std::thread::sleep(Duration::from_millis(25));
        assert_eq!(reader.slots.load(Ordering::Acquire), 2);
        for _ in 0..2 {
            drop(source.recv().unwrap());
        }
        assert_eq!(reader.slots.load(Ordering::Acquire), 0);
    }
}
