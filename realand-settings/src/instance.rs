use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

pub const FOCUS: u8 = 1;
pub const TOGGLE: u8 = 2;
pub const COMMAND_PALETTE: u8 = 3;
pub type Signal = Arc<AtomicU8>;

const PRIVATE_MODE: u32 = 0o600;

pub fn signal(path: &Path, toggle: bool) -> io::Result<()> {
    let mut stream = crate::socket_io::connect(path)?;
    stream.write_all(if toggle { b"TOGGLE\n" } else { b"FOCUS\n" })
}

/// Acknowledgement distinguishes an upgraded instance from an older settings
/// window which would otherwise silently ignore the new activation command.
/// A peer that closed the connection without acknowledging the command. The
/// close can surface on the write (EPIPE) or on the acknowledgement read
/// (EOF/reset), depending on scheduling; both mean the peer never understood
/// the command and must report the same outcome.
fn closed_without_reply(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset | io::ErrorKind::UnexpectedEof
    )
}

pub fn signal_command_palette(path: &Path) -> io::Result<()> {
    let mut stream = crate::socket_io::connect(path)?;
    let acknowledged = match stream.write_all(b"COMMAND_PALETTE\n") {
        Ok(()) => crate::socket_io::read_line(&mut stream, 16, std::time::Duration::from_secs(1)),
        Err(error) if closed_without_reply(&error) => Err(error),
        Err(error) => return Err(error),
    };
    if let Err(error) = &acknowledged {
        if matches!(
            error.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "设置窗口未及时响应命令中心请求，请稍后重试",
            ));
        }
        if !closed_without_reply(error) {
            return Err(io::Error::new(error.kind(), error.to_string()));
        }
    }
    if !matches!(acknowledged.as_deref(), Ok(b"OK\n")) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "当前设置窗口尚不支持命令中心，请关闭设置窗口后重新打开（无需重启桌面）",
        ));
    }
    Ok(())
}

/// Keep the lock for the GUI's lifetime, so simultaneous launches cannot unlink
/// each other's listener or create two independently running settings windows.
pub struct Instance {
    _lock: File,
    path: PathBuf,
    pub listener: UnixListener,
}

impl Instance {
    pub fn bind(path: PathBuf) -> io::Result<Self> {
        let lock_path = path.with_extension("lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(PRIVATE_MODE)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)?;
        // The fallback path lives in the system temporary directory when
        // XDG_RUNTIME_DIR is unavailable.  Keep the lock private even when
        // the process inherits a permissive umask or an older lock file.
        lock.set_permissions(fs::Permissions::from_mode(PRIVATE_MODE))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if !metadata.file_type().is_socket() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "settings socket path is not a socket",
                ));
            }
            // Only remove a stale socket, never one with an unresponsive owner.
            match crate::socket_io::connect(&path) {
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(&path)?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "settings is already running",
                    ))
                }
            }
        }
        let listener = UnixListener::bind(&path)?;
        // UnixListener::bind(2) applies the process umask, which is not a
        // security boundary: a permissive umask would let another local user
        // connect to the settings control socket.  Tighten the mode before
        // exposing the listener to the rest of the application.
        let chmod_result =
            unsafe { libc::fchmod(listener.as_raw_fd(), PRIVATE_MODE as libc::mode_t) };
        if chmod_result != 0 {
            let error = io::Error::last_os_error();
            drop(listener);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        // Linux does not propagate fchmod(2) on an AF_UNIX listener to the
        // directory entry, so also tighten the published socket path.  The
        // listener is still held open while this happens, preventing a normal
        // replacement race.
        if let Err(error) = fs::set_permissions(&path, fs::Permissions::from_mode(PRIVATE_MODE)) {
            drop(listener);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        Ok(Self {
            _lock: lock,
            path,
            listener,
        })
    }

    pub fn start(&self, ctx: eframe::egui::Context) -> io::Result<Signal> {
        let listener = self.listener.try_clone()?;
        let signal = Signal::default();
        let shared = signal.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                if let Ok(command) = read_command(&mut stream) {
                    shared.store(command, Ordering::SeqCst);
                    ctx.request_repaint();
                    if command == COMMAND_PALETTE {
                        let _ = stream.write_all(b"OK\n");
                    }
                }
            }
        });
        Ok(signal)
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn read_command(stream: &mut UnixStream) -> io::Result<u8> {
    ensure_same_user(stream)?;
    match crate::socket_io::read_line(stream, 16, crate::socket_io::TIMEOUT)?.as_slice() {
        b"FOCUS\n" => Ok(FOCUS),
        b"TOGGLE\n" => Ok(TOGGLE),
        b"COMMAND_PALETTE\n" => Ok(COMMAND_PALETTE),
        _ => Err(io::ErrorKind::InvalidData.into()),
    }
}

// Permissions are tightened before the worker starts, but a peer could have
// connected in the bind-to-chmod interval when using the temporary-directory
// fallback. Check the accepted peer before reading any command.
fn ensure_same_user(stream: &UnixStream) -> io::Result<()> {
    let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut peer as *mut libc::ucred).cast(),
            &mut size,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if size as usize != std::mem::size_of::<libc::ucred>() || peer.uid != unsafe { libc::getuid() }
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_close_is_classified_as_unsupported_on_write_and_read_paths() {
        for kind in [
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::UnexpectedEof,
        ] {
            assert!(
                closed_without_reply(&io::Error::new(kind, "test")),
                "{kind:?}"
            );
        }
        for kind in [
            io::ErrorKind::TimedOut,
            io::ErrorKind::WouldBlock,
            io::ErrorKind::PermissionDenied,
        ] {
            assert!(
                !closed_without_reply(&io::Error::new(kind, "test")),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn palette_activation_requires_an_acknowledging_instance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.sock");
        let instance = Instance::bind(path.clone()).unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = instance.listener.accept().unwrap();
            assert_eq!(read_command(&mut stream).unwrap(), COMMAND_PALETTE);
            stream.write_all(b"OK\n").unwrap();
        });
        signal_command_palette(&path).unwrap();
        worker.join().unwrap();

        let old = Instance::bind(path.clone()).unwrap();
        let worker = std::thread::spawn(move || {
            let (_stream, _) = old.listener.accept().unwrap();
            // Legacy instances close an unrecognized request without replying.
        });
        assert_eq!(
            signal_command_palette(&path).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        worker.join().unwrap();
    }

    #[test]
    fn palette_request_cannot_block_indefinitely_on_a_silent_instance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.sock");
        let instance = Instance::bind(path.clone()).unwrap();
        let started = std::time::Instant::now();
        assert_eq!(
            signal_command_palette(&path).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        drop(instance);
    }
    #[test]
    fn silent_client_cannot_block_next_launch() {
        let dir = tempfile::tempdir().unwrap();
        let instance = Instance::bind(dir.path().join("settings.sock")).unwrap();
        let _silent = UnixStream::connect(&instance.path).unwrap();
        signal(&instance.path, false).unwrap();
        let (mut silent, _) = instance.listener.accept().unwrap();
        assert!(read_command(&mut silent).is_err());
        let (mut next, _) = instance.listener.accept().unwrap();
        assert_eq!(read_command(&mut next).unwrap(), FOCUS);
    }

    #[test]
    fn commands_are_framed_and_launch_lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.sock");
        let first = Instance::bind(path.clone()).unwrap();
        assert!(Instance::bind(path.clone()).is_err());
        signal(&path, true).unwrap();
        let (mut next, _) = first.listener.accept().unwrap();
        assert_eq!(read_command(&mut next).unwrap(), TOGGLE);
        drop(first);
        Instance::bind(path).unwrap();
    }

    #[test]
    fn socket_and_lock_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.sock");
        let instance = Instance::bind(path.clone()).unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            PRIVATE_MODE
        );
        assert_eq!(
            fs::metadata(path.with_extension("lock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            PRIVATE_MODE
        );
        drop(instance);
    }

    #[test]
    fn lock_symlink_is_not_followed() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.sock");
        let target = dir.path().join("unrelated");
        let lock = path.with_extension("lock");
        fs::write(&target, b"unchanged").unwrap();
        symlink(&target, &lock).unwrap();

        assert!(Instance::bind(path).is_err());
        assert_eq!(fs::read(target).unwrap(), b"unchanged");
    }
}
