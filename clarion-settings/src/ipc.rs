use std::io::Write;
#[cfg(test)]
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;
use std::{env, fs};

use anyhow::{Context, Result};
use clarion_ipc::{Action, Output, Reply, Request, Response, TilingMode, Window, Workspace};

// IPC paths are interpreted in the compositor's working directory.
pub(crate) fn config_reload_action(path: &std::path::Path) -> Result<Action> {
    let path = std::path::absolute(path).context("Cannot resolve config path")?;
    let path = path
        .to_str()
        .context("Config path must be valid UTF-8 for IPC")?;
    Ok(Action::LoadConfigFile {
        path: Some(path.to_owned()),
    })
}

pub struct Connection {
    socket_path: Option<PathBuf>,
}

impl Connection {
    pub fn new() -> Self {
        let socket_path = Self::discover_socket();
        Self { socket_path }
    }

    pub fn is_connected(&self) -> bool {
        self.socket_path.is_some()
    }

    pub fn socket_path(&self) -> Option<&PathBuf> {
        self.socket_path.as_ref()
    }

    fn discover_socket() -> Option<PathBuf> {
        // An explicit session socket is authoritative. Do not accidentally send actions to a
        // different session when its socket is stale or the selected compositor is unavailable.
        if let Some(path) = clarion_ipc::socket::socket_path_from_environment() {
            return crate::socket_io::connect(&path).ok().map(|_| path);
        }

        // 2. Scan XDG_RUNTIME_DIR for clarion.*.sock
        if let Ok(runtime_dir) = env::var("XDG_RUNTIME_DIR") {
            if let Ok(entries) = fs::read_dir(&runtime_dir) {
                let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
                paths.sort();
                for prefix in ["clarion.", "realand."] {
                    for path in &paths {
                        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                            continue;
                        };
                        if name.starts_with(prefix)
                            && name.ends_with(".sock")
                            && crate::socket_io::connect(path).is_ok()
                        {
                            return Some(path.clone());
                        }
                    }
                }
            }
        }

        None
    }

    fn connect(&self) -> Result<UnixStream> {
        let path = self
            .socket_path
            .as_ref()
            .context("Clarion IPC socket not found")?;
        crate::socket_io::connect(path).context("Failed to connect to Clarion IPC socket")
    }

    pub fn send(&self, request: Request) -> Result<Response> {
        // Modesets may take longer than ordinary IPC. This runs on the worker,
        // while the compositor owns the independent 15-second recovery timer.
        let timeout = if matches!(
            request,
            Request::PreviewOutputConfig { .. }
                | Request::ConfirmOutputPreview { .. }
                | Request::CancelOutputPreview { .. }
        ) {
            Duration::from_secs(16)
        } else {
            crate::socket_io::TIMEOUT
        };
        let mut socket = self.connect()?;
        socket.set_read_timeout(Some(timeout))?;
        let mut request = serde_json::to_vec(&request)?;
        request.push(b'\n');
        socket.write_all(&request)?;
        let response = crate::socket_io::read_line(&mut socket, 16 * 1024 * 1024, timeout)?;
        let reply: Reply = serde_json::from_slice(&response)?;
        match reply {
            Ok(resp) => Ok(resp),
            Err(e) => anyhow::bail!("Clarion IPC error: {e}"),
        }
    }

    pub fn reload_config_path(&self, path: &std::path::Path) -> Result<()> {
        self.perform_action(config_reload_action(path)?)
    }
    pub fn reload_config(&self) -> Result<()> {
        self.perform_action(Action::LoadConfigFile { path: None })
    }

    pub fn switch_tiling_mode(&self, mode: Option<TilingMode>) -> Result<()> {
        self.perform_action(Action::SwitchTilingMode { mode })
    }

    pub fn toggle_overview(&self) -> Result<()> {
        self.perform_action(Action::ToggleOverview {})
    }

    pub fn toggle_expo(&self) -> Result<()> {
        self.perform_action(Action::ToggleExpo {})
    }

    pub fn perform_action(&self, action: Action) -> Result<()> {
        match self.send(Request::Action(action))? {
            Response::Handled => Ok(()),
            _ => anyhow::bail!("Unexpected action response"),
        }
    }

    pub fn get_windows(&self) -> Result<Vec<Window>> {
        match self.send(Request::Windows)? {
            Response::Windows(windows) => Ok(windows),
            _ => anyhow::bail!("Unexpected windows response"),
        }
    }

    pub fn get_outputs(&self) -> Result<Vec<Output>> {
        match self.send(Request::Outputs)? {
            Response::Outputs(outputs) => Ok(outputs.into_values().collect()),
            _ => anyhow::bail!("Unexpected outputs response"),
        }
    }

    pub fn get_workspaces(&self) -> Result<Vec<Workspace>> {
        match self.send(Request::Workspaces)? {
            Response::Workspaces(workspaces) => Ok(workspaces),
            _ => anyhow::bail!("Unexpected workspaces response"),
        }
    }

    pub fn get_version(&self) -> Result<String> {
        match self.send(Request::Version)? {
            Response::Version(version) => Ok(version),
            _ => anyhow::bail!("Unexpected version response"),
        }
    }
}

#[derive(Clone)]
pub struct Snapshot {
    pub version: String,
    pub outputs: Vec<Output>,
    pub windows: Vec<Window>,
    pub workspaces: Vec<Workspace>,
    pub management: Option<clarion_ipc::management::ManagementState>,
    pub frame_timings: Option<Vec<clarion_ipc::diagnostics::OutputFrameTimings>>,
}

enum Command {
    Request(
        Request,
        std::sync::mpsc::Sender<std::result::Result<Response, String>>,
    ),
    Action(Action),
    FocusWindow(u64),
    Refresh,
    SaveSession,
    RestoreSession,
}

pub struct IpcClient {
    commands: std::sync::mpsc::SyncSender<Command>,
    snapshot: std::sync::Arc<std::sync::Mutex<Option<std::result::Result<Snapshot, String>>>>,
    errors: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    notices: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl IpcClient {
    #[cfg(test)]
    pub(crate) fn disconnected_for_test() -> Self {
        let (commands, _) = std::sync::mpsc::sync_channel(1);
        Self {
            commands,
            snapshot: Default::default(),
            errors: Default::default(),
            notices: Default::default(),
        }
    }
    #[cfg(test)]
    pub(crate) fn scripted_for_test(
        mut handler: impl FnMut(Request) -> std::result::Result<Response, String> + Send + 'static,
    ) -> Self {
        let (commands, receiver) = std::sync::mpsc::sync_channel(64);
        std::thread::spawn(move || {
            while let Ok(command) = receiver.recv() {
                match command {
                    Command::Request(request, reply) => {
                        let _ = reply.send(handler(request));
                    }
                    Command::Action(action) => {
                        let _ = handler(Request::Action(action));
                    }
                    _ => {}
                }
            }
        });
        Self {
            commands,
            snapshot: Default::default(),
            errors: Default::default(),
            notices: Default::default(),
        }
    }
    pub fn new(context: eframe::egui::Context) -> Self {
        Self::with_socket(context, None)
    }
    fn with_socket(context: eframe::egui::Context, socket_path: Option<PathBuf>) -> Self {
        let (commands, receiver) = std::sync::mpsc::sync_channel(64);
        let snapshot = std::sync::Arc::new(std::sync::Mutex::new(None));
        let errors = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let notices = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let messages = notices.clone();
        let shared = snapshot.clone();
        let failures = errors.clone();
        std::thread::spawn(move || {
            use clarion_ipc::state::EventStreamState;
            let mut connection: Option<Connection> = None;
            let mut stream: Option<UnixStream> = None;
            let mut pending = Vec::new();
            let mut state = EventStreamState::default();
            let mut version = String::new();
            let mut outputs = Vec::new();
            let mut management = None;
            let mut management_supported = true;
            let mut frame_timings = None;
            let mut frame_timings_supported = true;
            let mut retry = std::time::Instant::now();
            let mut output_refresh = std::time::Instant::now();
            loop {
                let command = match receiver.recv_timeout(Duration::from_millis(25)) {
                    Ok(command) => Some(command),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                    Err(_) => break,
                };
                let mut changed = false;
                if let Some(command) = command {
                    match command {
                        Command::Request(request, reply) => {
                            let result = connection
                                .as_ref()
                                .context("Clarion 未连接")
                                .and_then(|c| c.send(request))
                                .map_err(|e| e.to_string());
                            let _ = reply.send(result);
                            output_refresh = std::time::Instant::now();
                            context.request_repaint();
                        }
                        Command::Refresh => {
                            retry = std::time::Instant::now();
                            output_refresh = retry;
                        }
                        Command::SaveSession | Command::RestoreSession => {
                            let result = (|| -> Result<String> {
                                let connection = connection.as_ref().context("Clarion 未连接")?;
                                let path = session_path()?;
                                if matches!(command, Command::SaveSession) {
                                    let state = connection.management_state()?;
                                    state.session.validate().map_err(|e| anyhow::anyhow!(e))?;
                                    crate::storage::atomic_write(
                                        &path,
                                        &serde_json::to_vec_pretty(&state.session)?,
                                    )?;
                                    Ok(format!("已保存会话布局: {}", path.display()))
                                } else {
                                    let session = read_session(&path)?;
                                    connection.perform_action(Action::RestoreSessionLayout {
                                        json: serde_json::to_string(&session)?,
                                    })?;
                                    Ok("已恢复可匹配窗口的布局；缺少的应用可打开后再次恢复".into())
                                }
                            })();
                            match result {
                                Ok(message) => messages.lock().unwrap().push(message),
                                Err(error) => failures.lock().unwrap().push(error.to_string()),
                            }
                            output_refresh = std::time::Instant::now();
                            context.request_repaint();
                        }
                        Command::Action(_) | Command::FocusWindow(_) => {
                            let result = connection.as_ref().context("Clarion 未连接").and_then(
                                |connection| match command {
                                    Command::Action(action) => connection.perform_action(action),
                                    Command::FocusWindow(id) => {
                                        connection.perform_action(Action::CloseOverview {})?;
                                        connection.perform_action(Action::FocusWindow { id })
                                    }
                                    _ => unreachable!(),
                                },
                            );
                            output_refresh = std::time::Instant::now();
                            if let Err(error) = result {
                                failures.lock().unwrap().push(error.to_string());
                                context.request_repaint();
                            }
                        }
                    }
                }
                if connection.is_none() && std::time::Instant::now() >= retry {
                    let connect = (|| -> Result<_> {
                        let connection = socket_path
                            .clone()
                            .map(|path| Connection {
                                socket_path: Some(path),
                            })
                            .unwrap_or_else(Connection::new);
                        let version = connection.get_version()?;
                        let outputs = connection.get_outputs()?;
                        let mut stream = connection.connect()?;
                        stream.write_all(b"\"EventStream\"\n")?;
                        stream.set_nonblocking(true)?;
                        Ok((connection, stream, version, outputs))
                    })();
                    match connect {
                        Ok((new, socket, v, o)) => {
                            connection = Some(new);
                            stream = Some(socket);
                            version = v;
                            outputs = o;
                            state = EventStreamState::default();
                            pending.clear();
                            changed = true;
                            management_supported = true;
                            management = new_management(&connection, &mut management_supported);
                            frame_timings_supported = true;
                            frame_timings =
                                new_frame_timings(&connection, &mut frame_timings_supported);
                            output_refresh = std::time::Instant::now() + Duration::from_secs(3);
                        }
                        Err(error) => {
                            *shared.lock().unwrap() = Some(Err(error.to_string()));
                            retry = std::time::Instant::now() + Duration::from_secs(3);
                            context.request_repaint();
                        }
                    }
                }
                let read_result = (|| -> Result<()> {
                    let Some(socket) = stream.as_mut() else {
                        return Ok(());
                    };
                    changed |= read_event_batch(socket, &mut pending, &mut state)?;
                    if std::time::Instant::now() >= output_refresh {
                        outputs = connection.as_ref().unwrap().get_outputs()?;
                        management = new_management(&connection, &mut management_supported);
                        frame_timings =
                            new_frame_timings(&connection, &mut frame_timings_supported);
                        output_refresh = std::time::Instant::now() + Duration::from_secs(3);
                        changed = true;
                    }
                    Ok(())
                })();
                if let Err(error) = read_result {
                    connection = None;
                    stream = None;
                    *shared.lock().unwrap() = Some(Err(error.to_string()));
                    retry = std::time::Instant::now() + Duration::from_secs(1);
                    context.request_repaint();
                } else if changed {
                    *shared.lock().unwrap() = Some(Ok(Snapshot {
                        version: version.clone(),
                        outputs: outputs.clone(),
                        management: management.clone(),
                        frame_timings: frame_timings.clone(),
                        windows: state.windows.windows.values().cloned().collect(),
                        workspaces: state.workspaces.workspaces.values().cloned().collect(),
                    }));
                    context.request_repaint();
                }
            }
        });
        Self {
            commands,
            snapshot,
            errors,
            notices,
        }
    }
    pub fn take_snapshot(&self) -> Option<std::result::Result<Snapshot, String>> {
        self.snapshot.lock().unwrap().take()
    }
    pub fn request(
        &self,
        request: Request,
    ) -> Result<std::sync::mpsc::Receiver<std::result::Result<Response, String>>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.commands
            .try_send(Command::Request(request, sender))
            .map_err(|e| anyhow::anyhow!("操作队列不可用: {e}"))?;
        Ok(receiver)
    }
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.notices.lock().unwrap())
    }
    pub fn save_session(&self) -> Result<()> {
        self.commands
            .try_send(Command::SaveSession)
            .map_err(|e| anyhow::anyhow!(e))
    }
    pub fn restore_session(&self) -> Result<()> {
        self.commands
            .try_send(Command::RestoreSession)
            .map_err(|e| anyhow::anyhow!(e))
    }
    pub fn take_errors(&self) -> Vec<String> {
        std::mem::take(&mut *self.errors.lock().unwrap())
    }
    pub fn refresh(&self) {
        let _ = self.commands.try_send(Command::Refresh);
    }
    pub fn perform_action(&self, action: Action) -> Result<()> {
        self.commands
            .try_send(Command::Action(action))
            .map_err(|e| anyhow::anyhow!("操作队列不可用: {e}"))
    }
    pub fn focus_window(&self, id: u64) -> Result<()> {
        self.commands
            .try_send(Command::FocusWindow(id))
            .map_err(|e| anyhow::anyhow!(e))
    }
    pub fn reload_config_path(&self, path: &std::path::Path) -> Result<()> {
        self.perform_action(config_reload_action(path)?)
    }
    pub fn reload_config(&self) -> Result<()> {
        self.perform_action(Action::LoadConfigFile { path: None })
    }
    pub fn switch_tiling_mode(&self, mode: Option<TilingMode>) -> Result<()> {
        self.perform_action(Action::SwitchTilingMode { mode })
    }
    pub fn toggle_overview(&self) -> Result<()> {
        self.perform_action(Action::ToggleOverview {})
    }
    pub fn toggle_expo(&self) -> Result<()> {
        self.perform_action(Action::ToggleExpo {})
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_SOCKET: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn mismatched_snapshot_replies_are_errors_instead_of_empty_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ipc.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                assert!(matches!(
                    serde_json::from_str::<Request>(&line).unwrap(),
                    Request::Outputs | Request::Workspaces | Request::Version
                ));
                socket.write_all(b"{\"Ok\":\"Handled\"}\n").unwrap();
            }
        });
        let connection = Connection {
            socket_path: Some(path),
        };
        assert!(connection
            .get_outputs()
            .unwrap_err()
            .to_string()
            .contains("outputs"));
        assert!(connection
            .get_workspaces()
            .unwrap_err()
            .to_string()
            .contains("workspaces"));
        assert!(connection
            .get_version()
            .unwrap_err()
            .to_string()
            .contains("version"));
        server.join().unwrap();
    }

    #[test]
    fn frame_timings_are_optional_on_older_compositors() {
        for supported_by_server in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("ipc.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut request = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut request)
                    .unwrap();
                assert!(matches!(
                    serde_json::from_str::<Request>(&request).unwrap(),
                    Request::FrameTimings
                ));
                let response: Reply = if supported_by_server {
                    Ok(Response::FrameTimings(vec![]))
                } else {
                    Err("unknown variant `FrameTimings`".into())
                };
                writeln!(socket, "{}", serde_json::to_string(&response).unwrap()).unwrap();
            });
            let connection = Some(Connection {
                socket_path: Some(path),
            });
            let mut supported = true;
            let timings = new_frame_timings(&connection, &mut supported);
            assert_eq!(supported, supported_by_server);
            assert_eq!(timings.is_some(), supported_by_server);
            server.join().unwrap();
            if !supported_by_server {
                // The server has closed: an unsupported feature must not be retried on every poll.
                assert!(new_frame_timings(&connection, &mut supported).is_none());
            }
        }
    }

    #[test]
    fn display_preview_accepts_modeset_reply_slower_than_normal_ipc_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ipc.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(socket.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert!(matches!(
                serde_json::from_str::<Request>(&line).unwrap(),
                Request::PreviewOutputConfig { .. }
            ));
            std::thread::sleep(Duration::from_millis(450));
            socket
                .write_all(b"{\"Ok\":{\"OutputPreview\":{\"id\":42,\"timeout_ms\":15000}}}\n")
                .unwrap();
        });
        let connection = Connection {
            socket_path: Some(path),
        };
        let result = connection
            .send(Request::PreviewOutputConfig {
                root_path: "/test.kdl".into(),
                edited_path: "/test.kdl".into(),
                content: String::new(),
            })
            .unwrap();
        assert!(matches!(
            result,
            Response::OutputPreview {
                id: 42,
                timeout_ms: 15_000
            }
        ));
        server.join().unwrap();
    }

    #[test]
    fn queued_config_reload_uses_absolute_path() {
        let (commands, receiver) = std::sync::mpsc::sync_channel(1);
        let client = IpcClient {
            commands,
            snapshot: Default::default(),
            errors: Default::default(),
            notices: Default::default(),
        };
        client
            .reload_config_path(std::path::Path::new("config.kdl"))
            .unwrap();
        let Command::Action(Action::LoadConfigFile { path: Some(path) }) = receiver.recv().unwrap()
        else {
            panic!("expected config reload");
        };
        assert_eq!(
            PathBuf::from(path),
            std::env::current_dir().unwrap().join("config.kdl")
        );
    }

    #[test]
    fn config_reload_rejects_non_utf8_instead_of_loading_another_path() {
        use std::os::unix::ffi::OsStringExt;
        let (commands, receiver) = std::sync::mpsc::sync_channel(1);
        let client = IpcClient {
            commands,
            snapshot: Default::default(),
            errors: Default::default(),
            notices: Default::default(),
        };
        let path = PathBuf::from(std::ffi::OsString::from_vec(b"config-\xff.kdl".to_vec()));
        assert!(client.reload_config_path(&path).is_err());
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn action_uses_explicit_ids_over_the_socket() {
        let path = std::env::temp_dir().join(format!(
            "clarion-settings-test-{}-{}.sock",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed),
        ));
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut line = String::new();
            BufReader::new(socket.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            socket.write_all(b"{\"Ok\":\"Handled\"}\n").unwrap();
            serde_json::from_str::<serde_json::Value>(&line).unwrap()
        });
        let client = Connection {
            socket_path: Some(path.clone()),
        };
        let result = client.perform_action(Action::MoveWindowToWorkspace {
            window_id: Some(42),
            reference: clarion_ipc::WorkspaceReferenceArg::Id(900),
            focus: false,
        });
        let request = server.join().unwrap();
        std::fs::remove_file(path).unwrap();
        result.unwrap();
        assert_eq!(request["Action"]["MoveWindowToWorkspace"]["window_id"], 42);
        assert_eq!(
            request["Action"]["MoveWindowToWorkspace"]["reference"]["Id"],
            900
        );
        assert_eq!(request["Action"]["MoveWindowToWorkspace"]["focus"], false);
    }
}

/// Yield to queued actions between event bursts, and compact the buffer once per
/// batch rather than moving its whole tail for each tiny event.
fn read_event_batch(
    socket: &mut UnixStream,
    pending: &mut Vec<u8>,
    state: &mut clarion_ipc::state::EventStreamState,
) -> Result<bool> {
    use std::io::Read;

    use clarion_ipc::state::EventStreamStatePart;
    const MAX_LINE: usize = 16 * 1024 * 1024;
    if !pending.contains(&b'\n') {
        let mut bytes = [0u8; 8192];
        for _ in 0..8 {
            match socket.read(&mut bytes) {
                Ok(0) => anyhow::bail!("Clarion 连接已断开"),
                Ok(count) => pending.extend_from_slice(&bytes[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
            anyhow::ensure!(pending.len() <= MAX_LINE, "IPC event too large");
        }
    }
    let mut consumed = 0;
    let mut changed = false;
    for _ in 0..256 {
        let Some(end) = pending[consumed..].iter().position(|b| *b == b'\n') else {
            break;
        };
        let end = consumed + end + 1;
        let line = &pending[consumed..end];
        if let Ok(event) = serde_json::from_slice::<clarion_ipc::Event>(line) {
            state.apply(event);
            changed = true;
        } else {
            let reply: Reply = serde_json::from_slice(line)?;
            reply.map_err(|e| anyhow::anyhow!(e))?;
        }
        consumed = end;
    }
    pending.drain(..consumed);
    Ok(changed)
}

fn new_management(
    connection: &Option<Connection>,
    supported: &mut bool,
) -> Option<clarion_ipc::management::ManagementState> {
    if !*supported {
        return None;
    }
    match connection.as_ref()?.management_state() {
        Ok(state) => Some(state),
        Err(error) => {
            if error.to_string().contains("unknown variant")
                || error.to_string().contains("尚不支持")
            {
                *supported = false;
            }
            None
        }
    }
}

fn new_frame_timings(
    connection: &Option<Connection>,
    supported: &mut bool,
) -> Option<Vec<clarion_ipc::diagnostics::OutputFrameTimings>> {
    if !*supported {
        return None;
    }
    match connection.as_ref()?.send(Request::FrameTimings) {
        Ok(Response::FrameTimings(timings)) => Some(timings),
        Ok(_) => {
            *supported = false;
            None
        }
        Err(error) => {
            if error.to_string().contains("unknown variant") {
                *supported = false;
            }
            None
        }
    }
}
impl Connection {
    fn management_state(&self) -> Result<clarion_ipc::management::ManagementState> {
        match self.send(Request::ManagementState)? {
            Response::ManagementState(json) => Ok(serde_json::from_str(&json)?),
            _ => anyhow::bail!("运行中的 Clarion 尚不支持布局管理，请更新后重新登录"),
        }
    }
}
pub fn session_path() -> Result<PathBuf> {
    let root = env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .context("无法确定会话保存目录")?;
    Ok(clarion_config::identity::prefer_existing_path(
        root.join("clarion/session-layout.json"),
        root.join("realand/session-layout.json"),
    ))
}

fn read_session(path: &std::path::Path) -> Result<clarion_ipc::management::SessionLayout> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    const MAX_SESSION_BYTES: usize = 4 * 1024 * 1024;
    // The worker also serves actions and display confirmations. A FIFO or
    // device masquerading as a saved session must not stall that worker.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .context("无法读取保存的会话布局")?;
    let metadata = file.metadata()?;
    anyhow::ensure!(metadata.is_file(), "会话布局必须为普通文件");
    anyhow::ensure!(metadata.len() <= MAX_SESSION_BYTES as u64, "会话文件过大");
    let mut bytes = Vec::new();
    file.take((MAX_SESSION_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= MAX_SESSION_BYTES, "会话文件过大");
    let session: clarion_ipc::management::SessionLayout = serde_json::from_slice(&bytes)?;
    session.validate().map_err(|e| anyhow::anyhow!(e))?;
    Ok(session)
}

#[cfg(test)]
mod worker_tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use super::*;

    #[test]
    fn session_restore_rejects_special_and_oversized_files_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        fs::write(
            &path,
            br#"{"version":1,"workspaces":[],"windows":[],"groups":[]}"#,
        )
        .unwrap();
        assert_eq!(read_session(&path).unwrap().version, 1);
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(4 * 1024 * 1024 + 1).unwrap();
        assert!(read_session(&path)
            .unwrap_err()
            .to_string()
            .contains("过大"));
        fs::remove_file(&path).unwrap();
        let fifo = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let start = std::time::Instant::now();
        assert!(read_session(&path)
            .unwrap_err()
            .to_string()
            .contains("普通文件"));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn event_bursts_yield_and_preserve_order_and_partial_messages() {
        let (mut socket, mut writer) = UnixStream::pair().unwrap();
        socket.set_nonblocking(true).unwrap();
        let mut state = clarion_ipc::state::EventStreamState::default();
        let event = b"{\"OverviewOpenedOrClosed\":{\"is_open\":true}}\n";
        let mut pending = event.repeat(1024);
        let partial = b"{\"OverviewOpenedOrClosed\":{\"is_open\":fal";
        pending.extend_from_slice(partial);
        assert!(read_event_batch(&mut socket, &mut pending, &mut state).unwrap());
        assert!(state.overview.is_open);
        assert_eq!(pending.len(), event.len() * 768 + partial.len());
        for _ in 0..3 {
            assert!(read_event_batch(&mut socket, &mut pending, &mut state).unwrap());
        }
        assert!(!pending.is_empty());
        writer.write_all(b"se}}\n").unwrap();
        assert!(read_event_batch(&mut socket, &mut pending, &mut state).unwrap());
        assert!(!state.overview.is_open);
        assert!(pending.is_empty());
    }
    #[test]
    fn slow_action_reply_does_not_block_ui_and_events_populate_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ipc.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let server = std::thread::spawn(move || {
            let mut event_stream = None;
            while !stopped.load(Ordering::Relaxed) {
                let (mut socket, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut line = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: Request = serde_json::from_str(&line).unwrap();
                let response = match request {
                    Request::Version => Response::Version("test".into()),
                    Request::Outputs => Response::Outputs(Default::default()),
                    Request::ManagementState | Request::FrameTimings => {
                        socket
                            .write_all(b"{\"Err\":\"unknown variant\"}\n")
                            .unwrap();
                        continue;
                    }
                    Request::EventStream => {
                        socket.write_all(b"{\"Ok\":\"Handled\"}\n{\"WorkspacesChanged\":{\"workspaces\":[]}}\n{\"WindowsChanged\":{\"windows\":[]}}\n").unwrap();
                        event_stream = Some(socket);
                        continue;
                    }
                    Request::Action(_) => {
                        std::thread::sleep(Duration::from_millis(200));
                        Response::Handled
                    }
                    _ => panic!("unexpected test request"),
                };
                let mut response = serde_json::to_vec(&Ok::<_, String>(response)).unwrap();
                response.push(b'\n');
                let _ = socket.write_all(&response);
            }
            drop(event_stream);
        });
        let client = IpcClient::with_socket(eframe::egui::Context::default(), Some(path));
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(Ok(snapshot)) = client.take_snapshot() {
                assert_eq!(snapshot.version, "test");
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "background connection timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let start = std::time::Instant::now();
        client.perform_action(Action::ToggleExpo {}).unwrap();
        assert!(start.elapsed() < Duration::from_millis(50));
        drop(client);
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }
}
