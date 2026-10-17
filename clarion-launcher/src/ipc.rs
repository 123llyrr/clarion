//! Private, single-instance JSON-line transport between Shell and the launcher.
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

const MAX_LINE: usize = 65_536;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Presentation {
    pub request_id: u64,
    pub output: Option<String>,
    pub theme: Option<String>,
    pub accent: Option<String>,
    pub recent: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
pub enum Command {
    Show {
        #[serde(flatten)]
        presentation: Presentation,
    },
    Hide {
        #[serde(default)]
        request_id: u64,
    },
    Toggle {
        #[serde(flatten)]
        presentation: Presentation,
    },
    Reload {
        #[serde(default)]
        request_id: u64,
    },
    Preferences {
        #[serde(flatten)]
        presentation: Presentation,
    },
}

#[derive(Debug)]
pub enum Input {
    Command { peer: u64, command: Command },
    Disconnected { peer: u64 },
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum Event {
    Visible {
        request_id: u64,
    },
    Hidden {
        request_id: u64,
    },
    Launched {
        app_id: String,
        request_id: u64,
    },
    Error {
        message: String,
        context: &'static str,
        request_id: u64,
    },
}

#[derive(Clone, Default)]
pub struct Events(Arc<Mutex<Vec<(u64, UnixStream)>>>);

impl Events {
    pub fn send_to(&self, peer: u64, event: Event) {
        let Ok(mut line) = serde_json::to_vec(&event) else {
            return;
        };
        line.push(b'\n');
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain_mut(|(id, stream)| *id != peer || stream.write_all(&line).is_ok());
    }
}

pub struct Server {
    pub events: Events,
    alive: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
    path: PathBuf,
    _lock: File,
}

impl Server {
    /// `None` means another instance holds the lock. Never replace its socket.
    pub fn bind(runtime: &Path) -> Result<Option<(Self, async_channel::Receiver<Input>)>> {
        anyhow::ensure!(
            runtime.is_absolute() && runtime.is_dir(),
            "XDG_RUNTIME_DIR must be an existing absolute directory"
        );
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(runtime.join("clarion-launcher.lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(error.into());
        }
        let path = runtime.join("clarion-launcher.sock");
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(&path)?,
            Ok(_) => bail!("Launcher socket path is occupied by a non-socket file"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let listener = UnixListener::bind(&path).context("Cannot bind launcher socket")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let events = Events::default();
        let alive = Arc::new(AtomicBool::new(true));
        let (sender, receiver) = async_channel::unbounded();
        let accept = {
            let events = events.clone();
            let alive = alive.clone();
            thread::spawn(move || {
                let next_id = AtomicU64::new(1);
                while alive.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let mut peers = events.0.lock().unwrap_or_else(|p| p.into_inner());
                            if peers.len() >= 32 {
                                continue;
                            }
                            let id = next_id.fetch_add(1, Ordering::Relaxed);
                            let Ok(writer) = stream.try_clone() else {
                                continue;
                            };
                            let _ = writer.set_write_timeout(Some(Duration::from_millis(100)));
                            peers.push((id, writer));
                            drop(peers);
                            let events = events.clone();
                            let alive = alive.clone();
                            let sender = sender.clone();
                            thread::spawn(move || receive(stream, id, sender, events, alive));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20))
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        Ok(Some((
            Self {
                events,
                alive,
                accept: Some(accept),
                path,
                _lock: lock,
            },
            receiver,
        )))
    }
}

fn receive(
    mut stream: UnixStream,
    id: u64,
    sender: async_channel::Sender<Input>,
    events: Events,
    alive: Arc<AtomicBool>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let mut pending = Vec::new();
    let mut buffer = [0; 4096];
    'receive: while alive.load(Ordering::Relaxed) {
        let count = match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue
            }
            Err(_) => break,
        };
        pending.extend_from_slice(&buffer[..count]);
        while let Some(end) = pending.iter().position(|&byte| byte == b'\n') {
            if end >= MAX_LINE {
                break 'receive;
            }
            let line: Vec<_> = pending.drain(..=end).collect();
            match serde_json::from_slice::<Command>(&line) {
                Ok(command) => {
                    if sender
                        .send_blocking(Input::Command { peer: id, command })
                        .is_err()
                    {
                        break 'receive;
                    }
                }
                Err(_) => events.send_to(
                    id,
                    Event::Error {
                        message: "Invalid launcher command".into(),
                        context: "ipc",
                        request_id: 0,
                    },
                ),
            }
        }
        // One read may finish a valid line and begin the next one. Apply the
        // limit to each complete line above, then to the remaining fragment.
        if pending.len() > MAX_LINE {
            break;
        }
    }
    let _ = sender.send_blocking(Input::Disconnected { peer: id });
    events
        .0
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|(peer, _)| *peer != id);
}

impl Drop for Server {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        self.events
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    #[test]
    fn split_json_lines_and_events_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let (server, receiver) = Server::bind(dir.path()).unwrap().unwrap();
        let mut peer = UnixStream::connect(dir.path().join("clarion-launcher.sock")).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        peer.write_all(br#"{"type":"Show","request_id":7,"recent":["org.test.desktop"],"out"#)
            .unwrap();
        peer.write_all(b"put\":\"winit\"}\n").unwrap();
        let Input::Command {
            peer: peer_id,
            command: Command::Show { presentation },
        } = receiver.recv_blocking().unwrap()
        else {
            panic!("Expected Show");
        };
        assert_eq!(presentation.output.as_deref(), Some("winit"));
        assert_eq!(presentation.request_id, 7);
        server
            .events
            .send_to(peer_id, Event::Visible { request_id: 7 });
        let mut response = String::new();
        std::io::BufReader::new(peer)
            .read_line(&mut response)
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&response).unwrap()["request_id"],
            7
        );
    }

    #[test]
    fn line_limit_accepts_a_full_size_command_between_other_commands() {
        let (stream, mut peer) = UnixStream::pair().unwrap();
        let (sender, receiver) = async_channel::unbounded();
        let mut large = serde_json::to_vec(&serde_json::json!({
            "type": "Show", "request_id": 2, "output": "",
        }))
        .unwrap();
        let padding = MAX_LINE - large.len() - 1;
        large = serde_json::to_vec(&serde_json::json!({
            "type": "Show", "request_id": 2, "output": "x".repeat(padding),
        }))
        .unwrap();
        large.push(b'\n');
        assert_eq!(large.len(), MAX_LINE);

        // The preceding short line makes the full-size line's boundary fall
        // inside a 4096-byte read shared with the following command.
        let mut batch = b"{\"type\":\"Hide\",\"request_id\":1}\n".to_vec();
        batch.extend(large);
        batch.extend_from_slice(b"{\"type\":\"Hide\",\"request_id\":3}\n");
        peer.write_all(&batch).unwrap();
        peer.shutdown(std::net::Shutdown::Write).unwrap();
        receive(
            stream,
            1,
            sender,
            Events::default(),
            Arc::new(AtomicBool::new(true)),
        );

        let mut request_ids = Vec::new();
        while let Ok(input) = receiver.try_recv() {
            match input {
                Input::Command {
                    command: Command::Hide { request_id },
                    ..
                } => request_ids.push(request_id),
                Input::Command {
                    command: Command::Show { presentation },
                    ..
                } => request_ids.push(presentation.request_id),
                Input::Disconnected { .. } => {}
                _ => panic!("unexpected command"),
            }
        }
        assert_eq!(request_ids, [1, 2, 3]);
    }

    #[test]
    fn oversized_lines_disconnect_with_or_without_a_trailing_newline() {
        for newline in [false, true] {
            let (stream, mut peer) = UnixStream::pair().unwrap();
            let (sender, receiver) = async_channel::unbounded();
            let mut oversized = vec![b' '; MAX_LINE + 1];
            if newline {
                oversized.push(b'\n');
                oversized.extend_from_slice(b"{\"type\":\"Hide\",\"request_id\":3}\n");
            }
            peer.write_all(&oversized).unwrap();
            peer.shutdown(std::net::Shutdown::Write).unwrap();
            receive(
                stream,
                1,
                sender,
                Events::default(),
                Arc::new(AtomicBool::new(true)),
            );
            assert!(matches!(
                receiver.try_recv().unwrap(),
                Input::Disconnected { peer: 1 }
            ));
            assert!(receiver.try_recv().is_err());
        }
    }

    #[test]
    fn colliding_request_ids_do_not_broadcast_events_between_shells() {
        let dir = tempfile::tempdir().unwrap();
        let (server, receiver) = Server::bind(dir.path()).unwrap().unwrap();
        let mut first = UnixStream::connect(dir.path().join("clarion-launcher.sock")).unwrap();
        let mut second = UnixStream::connect(dir.path().join("clarion-launcher.sock")).unwrap();
        first
            .set_read_timeout(Some(Duration::from_millis(150)))
            .unwrap();
        second
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        first
            .write_all(b"{\"type\":\"Show\",\"request_id\":1}\n")
            .unwrap();
        let Input::Command { peer: first_id, .. } = receiver.recv_blocking().unwrap() else {
            panic!("Expected first Show")
        };
        second
            .write_all(b"{\"type\":\"Show\",\"request_id\":1}\n")
            .unwrap();
        let Input::Command {
            peer: second_id, ..
        } = receiver.recv_blocking().unwrap()
        else {
            panic!("Expected second Show")
        };
        assert_ne!(first_id, second_id);
        server
            .events
            .send_to(second_id, Event::Visible { request_id: 1 });
        let mut response = String::new();
        std::io::BufReader::new(second)
            .read_line(&mut response)
            .unwrap();
        assert!(response.contains("Visible"));
        let mut byte = [0];
        assert!(matches!(
            first.read(&mut byte).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
    }

    #[test]
    fn single_instance_preserves_the_active_socket_and_cleanup_releases_it() {
        let dir = tempfile::tempdir().unwrap();
        let (server, _) = Server::bind(dir.path()).unwrap().unwrap();
        assert!(Server::bind(dir.path()).unwrap().is_none());
        assert!(UnixStream::connect(dir.path().join("clarion-launcher.sock")).is_ok());
        drop(server);
        assert!(!dir.path().join("clarion-launcher.sock").exists());
        assert!(Server::bind(dir.path()).unwrap().is_some());
    }

    #[test]
    fn regular_files_at_socket_path_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clarion-launcher.sock");
        fs::write(&path, "keep").unwrap();
        assert!(Server::bind(dir.path()).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "keep");
    }
}
