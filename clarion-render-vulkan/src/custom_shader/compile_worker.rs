//! Owned-source compiler process. Parent pipe ends are nonblocking, and inherited grandchild
//! pipe ends cannot extend the deadline. Only this Child is killed/reaped; no session/group kill.
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub source_bytes: usize,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub timeout: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            source_bytes: 16 * 1024 * 1024,
            stdout_bytes: 32 * 1024 * 1024,
            stderr_bytes: 1024 * 1024,
            timeout: Duration::from_secs(10),
        }
    }
}
fn nonblocking(fd: RawFd) -> io::Result<()> {
    // These are our private pipe ends, never a client/shared application descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn stop(child: &mut Child, already_reaped: bool) {
    if already_reaped {
        return;
    }
    let _ = child.kill();
    // A kernel-uninterruptible child can retain this fixed worker slot until SIGKILL completes.
    // No unbounded reaper thread or replacement compiler is spawned for such a child.
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}
fn take_pipe(pipe: &mut impl Read, bytes: &mut Vec<u8>, limit: usize) -> io::Result<bool> {
    let mut buffer = [0u8; 16 * 1024];
    // At most one chunk per turn keeps both stdout and stderr draining fairly.
    match pipe.read(&mut buffer) {
        Ok(0) => Ok(true),
        Ok(size) => {
            if bytes
                .len()
                .checked_add(size)
                .is_none_or(|length| length > limit)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "compiler output exceeds byte limit",
                ));
            }
            bytes.extend_from_slice(&buffer[..size]);
            Ok(false)
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}
pub(super) fn process(
    command: &mut Command,
    source: &[u8],
    limits: Limits,
    cancel: &AtomicBool,
) -> io::Result<Vec<u8>> {
    if source.len() > limits.source_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "shader source exceeds byte limit",
        ));
    }
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "shader compile cancelled",
        ));
    }
    let deadline = Instant::now() + limits.timeout;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut reaped = false;
    let result = (|| {
        let mut input = child.stdin.take();
        let mut output = child.stdout.take().unwrap();
        let mut errors = child.stderr.take().unwrap();
        nonblocking(input.as_ref().unwrap().as_raw_fd())?;
        nonblocking(output.as_raw_fd())?;
        nonblocking(errors.as_raw_fd())?;
        let mut written = 0;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let (mut out_eof, mut err_eof) = (false, false);
        let mut status = None;
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "shader compile cancelled",
                ));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "shader compiler/pipe deadline exceeded",
                ));
            }
            if status.is_none() {
                status = child.try_wait()?;
                reaped = status.is_some();
            }
            if let Some(pipe) = &mut input {
                if written == source.len() {
                    input.take();
                } else {
                    let end = (written + 16 * 1024).min(source.len());
                    match pipe.write(&source[written..end]) {
                        Ok(0) => {
                            return Err(io::Error::new(
                                io::ErrorKind::WriteZero,
                                "shader compiler input closed",
                            ))
                        }
                        Ok(size) => written += size,
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                            ) => {}
                        Err(error) => return Err(error),
                    }
                }
            }
            if !out_eof {
                out_eof = take_pipe(&mut output, &mut stdout, limits.stdout_bytes)?;
            }
            if !err_eof {
                err_eof = take_pipe(&mut errors, &mut stderr, limits.stderr_bytes)?;
            }
            if let Some(status) = status.filter(|_| out_eof && err_eof) {
                if status.success() {
                    return Ok(stdout);
                }
                let message = String::from_utf8_lossy(&stderr[..stderr.len().min(2048)]);
                return Err(io::Error::other(format!(
                    "shader compiler failed: {status}: {message}"
                )));
            }
            let mut descriptors = [
                libc::pollfd {
                    fd: input.as_ref().map_or(-1, AsRawFd::as_raw_fd),
                    events: libc::POLLOUT,
                    revents: 0,
                },
                libc::pollfd {
                    fd: if out_eof { -1 } else { output.as_raw_fd() },
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: if err_eof { -1 } else { errors.as_raw_fd() },
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let timeout = deadline
                .saturating_duration_since(now)
                .as_millis()
                .clamp(1, 10) as i32;
            // SAFETY: the descriptor array and all referenced private pipes remain live.
            if unsafe {
                libc::poll(
                    descriptors.as_mut_ptr(),
                    descriptors.len() as libc::nfds_t,
                    timeout,
                )
            } < 0
            {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    })();
    // All parent pipe ends have dropped before cleanup. Reaping never waits for inherited pipes.
    stop(&mut child, reaped);
    result
}
pub(super) fn compile(source: &str, stage: &str, cancel: &AtomicBool) -> io::Result<Vec<u32>> {
    let mut command = Command::new("glslc");
    command.args([
        "--target-env=vulkan1.2",
        &format!("-fshader-stage={stage}"),
        "-o",
        "-",
        "-",
    ]);
    let bytes = process(&mut command, source.as_bytes(), Limits::default(), cancel)?;
    if bytes.len() < 4
        || !bytes.len().is_multiple_of(4)
        || bytes[..4] != 0x0723_0203u32.to_le_bytes()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid SPIR-V compiler output",
        ));
    }
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| u32::from_le_bytes(*word))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }
    fn limits() -> Limits {
        Limits {
            source_bytes: 1024 * 1024,
            stdout_bytes: 4096,
            stderr_bytes: 4096,
            timeout: Duration::from_millis(80),
        }
    }
    #[test]
    fn small_success_and_failure_recover_on_the_same_worker_contract() {
        assert_eq!(
            process(
                &mut shell("cat"),
                b"shader",
                limits(),
                &AtomicBool::new(false)
            )
            .unwrap(),
            b"shader"
        );
        let error = process(
            &mut shell("printf 'syntax error' >&2; exit 1"),
            b"",
            limits(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.to_string().contains("syntax error"));
        assert_eq!(
            process(
                &mut shell("printf recovered"),
                b"",
                limits(),
                &AtomicBool::new(false)
            )
            .unwrap(),
            b"recovered"
        );
    }
    #[test]
    fn non_consumed_large_stdin_has_a_deadline() {
        let error = process(
            &mut shell("exec sleep 5"),
            &vec![b'a'; 1024 * 1024],
            limits(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
    #[test]
    fn excessive_stdout_and_stderr_are_bounded() {
        for script in ["exec yes output", "exec yes error >&2"] {
            assert_eq!(
                process(&mut shell(script), b"", limits(), &AtomicBool::new(false))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
    }
    #[test]
    fn inherited_grandchild_pipe_cannot_extend_deadline_after_parent_exit() {
        assert_eq!(
            process(
                &mut shell("sleep 0.2 & exit 0"),
                b"",
                limits(),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::TimedOut
        );
    }
    #[test]
    fn cancellation_during_compiler_lifetime_kills_only_its_child() {
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let trigger = cancelled.clone();
        let task = std::thread::spawn(move || {
            process(
                &mut shell("exec sleep 5"),
                b"",
                Limits {
                    timeout: Duration::from_secs(1),
                    ..limits()
                },
                &trigger,
            )
        });
        std::thread::sleep(Duration::from_millis(15));
        cancelled.store(true, Ordering::Release);
        assert_eq!(
            task.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
    }
    #[test]
    fn oversized_source_is_rejected_before_spawn() {
        assert_eq!(
            process(
                &mut Command::new("missing-compiler"),
                b"long",
                Limits {
                    source_bytes: 3,
                    ..limits()
                },
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
