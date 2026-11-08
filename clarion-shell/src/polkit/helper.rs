// SPDX-License-Identifier: GPL-3.0-or-later
//! The privileged half of the agent: talk the line protocol to the system's
//! `polkit-agent-helper-1`.
//!
//! On polkit 126+ the helper is socket activated and trusts the peer pidfd of
//! the connecting process, so no setuid binary is involved. Older systems get
//! the setuid helper with the username in argv and the cookie on stdin.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};

use super::{parse_helper_line, HelperEvent};

pub const DEFAULT_SOCKET: &str = "/run/polkit/agent-helper.socket";

/// Where to find the helper. Both fields are optional so a caller can probe
/// one transport explicitly; the defaults fill in the distribution paths.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HelperPaths {
    pub socket: Option<PathBuf>,
    pub binary: Option<PathBuf>,
}

impl HelperPaths {
    /// Environment overrides make the protocol testable without touching the
    /// real PAM stack.
    pub fn detect() -> Self {
        let socket = std::env::var_os("CLARION_POLKIT_HELPER_SOCKET")
            .map(PathBuf::from)
            .or_else(|| {
                let path = PathBuf::from(DEFAULT_SOCKET);
                path.socket_exists().then_some(path)
            });
        let binary = std::env::var_os("CLARION_POLKIT_HELPER_BIN")
            .map(PathBuf::from)
            .or_else(find_setuid_helper);
        Self { socket, binary }
    }
}

trait PathExt {
    fn socket_exists(&self) -> bool;
}
impl PathExt for Path {
    fn socket_exists(&self) -> bool {
        use std::os::unix::fs::FileTypeExt;
        std::fs::metadata(self)
            .map(|meta| meta.file_type().is_socket())
            .unwrap_or(false)
    }
}

/// Only a setuid binary can authenticate; probing an ordinary copy would just
/// spam the journal. Socket activation is preferred anyway on current polkit.
fn find_setuid_helper() -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    for path in [
        "/usr/lib/polkit-1/polkit-agent-helper-1",
        "/usr/libexec/polkit-1/polkit-agent-helper-1",
        "/usr/lib64/polkit-1/polkit-agent-helper-1",
    ] {
        let path = PathBuf::from(path);
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.permissions().mode() & 0o4000 != 0 {
            return Some(path);
        }
    }
    None
}

/// A cloned handle that can stop the helper from another thread. Cancelling
/// the dialog must not leave a PAM conversation or a child process behind.
#[derive(Clone)]
pub enum HelperGuard {
    Socket(Arc<UnixStream>),
    Child(Arc<Mutex<Child>>),
}

impl HelperGuard {
    pub fn cancel(&self) {
        match self {
            Self::Socket(stream) => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            Self::Child(child) => {
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
}

pub struct HelperSession {
    reader: Box<dyn BufRead + Send>,
    writer: Box<dyn Write + Send>,
    guard: HelperGuard,
}

impl HelperSession {
    /// Connect or spawn the helper and hand it the username and cookie. The
    /// cookie never appears in argv or the environment.
    pub fn open(username: &str, cookie: &str) -> Result<Self> {
        Self::open_with(username, cookie, &HelperPaths::detect())
    }

    pub fn open_with(username: &str, cookie: &str, paths: &HelperPaths) -> Result<Self> {
        if !crate::polkit::valid_cookie(cookie) {
            bail!("empty or malformed authentication cookie");
        }
        if username.is_empty() || username.contains(['\n', '\r', '\0']) {
            bail!("invalid user name for the authentication helper");
        }
        if let Some(socket) = &paths.socket {
            match Self::connect_socket(socket, username, cookie) {
                Ok(session) => return Ok(session),
                Err(error) if paths.binary.is_some() => {
                    eprintln!("polkit: helper socket failed ({error:#}), trying setuid helper");
                }
                Err(error) => return Err(error),
            }
        }
        let Some(binary) = &paths.binary else {
            bail!(
                "no polkit-agent-helper-1 available (looked for {} and a setuid helper)",
                DEFAULT_SOCKET
            );
        };
        Self::spawn_setuid(binary, username, cookie)
    }

    fn connect_socket(path: &Path, username: &str, cookie: &str) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .with_context(|| format!("connect {}", path.display()))?;
        let reader = BufReader::new(stream.try_clone()?);
        let mut writer = stream.try_clone()?;
        // Socket activation reads the user name first, then the cookie, both
        // newline terminated. The helper switches stdin to the PAM conversation
        // immediately afterwards.
        writeln!(writer, "{username}")?;
        writeln!(writer, "{cookie}")?;
        writer.flush()?;
        Ok(Self {
            reader: Box::new(reader),
            writer: Box::new(writer),
            guard: HelperGuard::Socket(Arc::new(stream)),
        })
    }

    fn spawn_setuid(binary: &Path, username: &str, cookie: &str) -> Result<Self> {
        let mut command = Command::new(binary);
        command
            .arg(username)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
        for key in ["LANG", "LANGUAGE", "LC_MESSAGES"] {
            if let Ok(value) = std::env::var(key) {
                command.env(key, value);
            }
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("spawn {}", binary.display()))?;
        let stdin = child.stdin.take().context("helper stdin missing")?;
        let stdout = child.stdout.take().context("helper stdout missing")?;
        let mut writer = stdin;
        writeln!(writer, "{cookie}")?;
        writer.flush()?;
        Ok(Self {
            reader: Box::new(BufReader::new(stdout)),
            writer: Box::new(writer),
            guard: HelperGuard::Child(Arc::new(Mutex::new(child))),
        })
    }

    /// Test seam: drive the protocol over any reader/writer pair.
    #[cfg(test)]
    pub(crate) fn from_io(
        reader: Box<dyn BufRead + Send>,
        writer: Box<dyn Write + Send>,
        guard: HelperGuard,
    ) -> Self {
        Self {
            reader,
            writer,
            guard,
        }
    }

    pub fn guard(&self) -> HelperGuard {
        self.guard.clone()
    }

    /// Read one line. `Ok(HelperEvent::Closed)` marks EOF without a verdict.
    pub fn read_event(&mut self) -> Result<HelperEvent> {
        let mut bytes = Vec::new();
        let read = self.reader.read_until(b'\n', &mut bytes)?;
        if read == 0 {
            return Ok(HelperEvent::Closed);
        }
        let line = String::from_utf8_lossy(&bytes);
        Ok(parse_helper_line(&line).unwrap_or(HelperEvent::Closed))
    }

    /// Send one PAM answer. The protocol is line oriented; a stray newline in
    /// the secret would be read as a second answer by the helper.
    pub fn answer(&mut self, answer: &str) -> Result<()> {
        // A stray newline would be read as a second answer by the helper.
        let answer = answer.split(['\r', '\n']).next().unwrap_or("");
        self.writer.write_all(answer.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};
    use std::os::unix::net::UnixStream;

    #[test]
    fn reads_prompts_and_answers_over_a_pipe() {
        let (mut peer, local) = UnixStream::pair().unwrap();
        let reader = BufReader::new(local.try_clone().unwrap());
        let writer = local.try_clone().unwrap();
        let mut session = HelperSession::from_io(
            Box::new(reader),
            Box::new(writer),
            HelperGuard::Socket(Arc::new(local)),
        );

        peer.write_all(b"PAM_PROMPT_ECHO_OFF Password: \n")
            .unwrap();
        assert_eq!(
            session.read_event().unwrap(),
            HelperEvent::Prompt {
                text: "Password: ".into(),
                echo: false
            }
        );
        session.answer("s3cret\nignored").unwrap();
        let mut answer = [0u8; 16];
        let n = peer.read(&mut answer).unwrap();
        assert_eq!(&answer[..n], b"s3cret\n");

        peer.write_all(b"SUCCESS\n").unwrap();
        assert_eq!(session.read_event().unwrap(), HelperEvent::Success);
        peer.write_all(b"").unwrap();
        peer.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!(session.read_event().unwrap(), HelperEvent::Closed);
    }

    #[test]
    fn rejects_unsafe_credentials_before_touching_the_helper() {
        let paths = HelperPaths {
            socket: Some(PathBuf::from("/nonexistent")),
            binary: None,
        };
        assert!(HelperSession::open_with("user", "bad\ncookie", &paths).is_err());
        assert!(HelperSession::open_with("bad\nuser", "cookie", &paths).is_err());
    }

    #[test]
    fn parses_partial_lines() {
        let input = Cursor::new(b"PAM_TEXT_INFO hi\n".to_vec());
        let mut session = HelperSession::from_io(
            Box::new(input),
            Box::new(Vec::new()),
            HelperGuard::Child(Arc::new(Mutex::new(
                Command::new("/bin/true").spawn().unwrap(),
            ))),
        );
        assert_eq!(
            session.read_event().unwrap(),
            HelperEvent::Info("hi".into())
        );
    }

    #[test]
    fn socket_helper_gets_username_then_cookie_and_continues_the_pam_dialog() {
        use std::io::BufRead;
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line.trim_end(), "tester");
            line.clear();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line.trim_end(), "cookie-123");
            stream
                .write_all(b"PAM_PROMPT_ECHO_OFF Password: \n")
                .unwrap();
            let mut answer = String::new();
            reader.read_line(&mut answer).unwrap();
            assert_eq!(answer.trim_end(), "hunter2");
            stream.write_all(b"SUCCESS\n").unwrap();
        });
        let mut session = HelperSession::open_with(
            "tester",
            "cookie-123",
            &HelperPaths {
                socket: Some(path),
                binary: None,
            },
        )
        .unwrap();
        assert_eq!(
            session.read_event().unwrap(),
            HelperEvent::Prompt {
                text: "Password: ".into(),
                echo: false,
            }
        );
        session.answer("hunter2").unwrap();
        assert_eq!(session.read_event().unwrap(), HelperEvent::Success);
        peer.join().unwrap();
    }

    #[test]
    fn setuid_fallback_spawns_the_helper_and_sends_the_cookie_on_stdin() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helper.sh");
        std::fs::write(
            &helper,
            "#!/bin/sh\nread cookie\nprintf 'PAM_PROMPT_ECHO_OFF Password: \\n'\nread answer\nprintf 'SUCCESS\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut session = HelperSession::open_with(
            "tester",
            "cookie-123",
            &HelperPaths {
                socket: None,
                binary: Some(helper),
            },
        )
        .unwrap();
        assert_eq!(
            session.read_event().unwrap(),
            HelperEvent::Prompt {
                text: "Password: ".into(),
                echo: false,
            }
        );
        session.answer("hunter2").unwrap();
        assert_eq!(session.read_event().unwrap(), HelperEvent::Success);
    }
}
