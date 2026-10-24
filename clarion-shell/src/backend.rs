//! Blocking platform operations live on bounded worker queues, never on GPUI's
//! thread. Each compositor reconnection starts a fresh generation and state.
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use async_channel::{Receiver, Sender};
use clarion_ipc::state::{EventStreamState, EventStreamStatePart};
use clarion_ipc::{Action, KeyboardLayouts, Output, Reply, Request, Response, Window, Workspace};
use clarion_launcher::catalog::AppCatalog;

use crate::preferences::{Environment, Preferences};

const IPC_TIMEOUT: Duration = Duration::from_millis(900);
const TOOL_TIMEOUT: Duration = Duration::from_millis(800);
const LINE_LIMIT: usize = clarion_ipc::socket::MAX_LINE_BYTES;
const PANEL_COMMANDS: &[&str] = &[
    "spotlight",
    "launcher",
    "close",
    "spaces",
    "mission-control",
    "control-center",
    "quick-settings",
    "notifications",
    "overview",
    "recent-content",
    "recent",
    "clipboard",
    "screenshot",
    "preferences",
    "reload",
];

#[derive(Clone, Debug, Default)]
pub struct DesktopSnapshot {
    pub generation: u64,
    pub connected: bool,
    pub workspaces: Vec<Workspace>,
    pub windows: Vec<Window>,
    pub outputs: HashMap<String, Output>,
    pub keyboard_layouts: Option<KeyboardLayouts>,
    pub reduced_motion: Option<bool>,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemSnapshot {
    pub volume: Option<f64>,
    pub muted: bool,
    pub brightness: Option<f64>,
    pub battery: Option<f64>,
    pub charging: bool,
    pub trash_full: bool,
    pub clock: String,
    pub date: String,
}

#[derive(Debug)]
pub enum Event {
    Desktop(DesktopSnapshot),
    System(SystemSnapshot),
    Catalog(AppCatalog),
    Preferences(Preferences),
    Command(String),
    Screenshot {
        path: Option<String>,
        is_preview: bool,
    },
    Error(String),
}

#[derive(Clone, Debug)]
pub enum Command {
    Action(Action),
    PlacePreferences {
        id: u64,
        width: i32,
        height: i32,
        x: f64,
        y: f64,
    },
    LaunchDesktop(PathBuf),
    LaunchDesktopWithPaths(PathBuf, Vec<PathBuf>),
    OpenPath(PathBuf),
    OpenSettings,
    SetVolume(f64),
    SetMuted(bool),
    SetOutputBrightness {
        output: String,
        value: f64,
    },
    RefreshCatalog,
    SavePreferences(Preferences),
}

#[derive(Clone, Debug)]
pub struct Options {
    pub environment: Environment,
    pub preview: bool,
    pub command_server: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            environment: std::env::vars().collect(),
            preview: false,
            command_server: true,
        }
    }
}

pub struct Backend {
    commands: mpsc::SyncSender<Command>,
    alive: Arc<AtomicBool>,
    _server: Option<CommandServer>,
}

impl Backend {
    /// Capture the actual command queue without workers or desktop I/O.
    #[cfg(test)]
    pub(crate) fn test_channel(capacity: usize) -> (Self, mpsc::Receiver<Command>) {
        let (commands, receiver) = mpsc::sync_channel(capacity);
        (
            Self {
                commands,
                alive: Arc::new(AtomicBool::new(true)),
                _server: None,
            },
            receiver,
        )
    }

    pub fn start(options: Options) -> Result<(Self, Receiver<Event>)> {
        let alive = Arc::new(AtomicBool::new(true));
        let (events, receiver) = async_channel::bounded(128);
        let server = if options.command_server && !options.preview {
            Some(CommandServer::bind(
                &options.environment,
                events.clone(),
                alive.clone(),
            )?)
        } else {
            None
        };
        let (commands, incoming) = mpsc::sync_channel(64);
        {
            let options = options.clone();
            let events = events.clone();
            let alive = alive.clone();
            thread::Builder::new()
                .name("clarion-shell-commands".into())
                .spawn(move || {
                    match Preferences::load(&options.environment, options.preview) {
                        Ok(preferences) => emit(&events, Event::Preferences(preferences), &alive),
                        Err(error) => {
                            emit(
                                &events,
                                Event::Error(format!("读取桌面偏好设置失败：{error:#}")),
                                &alive,
                            );
                            emit(&events, Event::Preferences(Preferences::default()), &alive);
                        }
                    }
                    load_catalog(&options.environment, &events, &alive);
                    while alive.load(Ordering::Relaxed) {
                        let command = match incoming.recv_timeout(Duration::from_millis(100)) {
                            Ok(command) => command,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(_) => break,
                        };
                        if let Err(error) = perform(command, &options, &events, &alive) {
                            emit(
                                &events,
                                Event::Error(format!("操作未完成：{error:#}")),
                                &alive,
                            );
                        }
                    }
                })?;
        }
        if !options.preview {
            let options = options.clone();
            let events = events.clone();
            let alive = alive.clone();
            thread::Builder::new()
                .name("clarion-shell-desktop".into())
                .spawn(move || desktop_worker(options, events, alive))?;
        }
        {
            let events = events.clone();
            let alive = alive.clone();
            thread::Builder::new()
                .name("clarion-shell-system".into())
                .spawn(move || system_worker(options, events, alive))?;
        }
        Ok((
            Self {
                commands,
                alive,
                _server: server,
            },
            receiver,
        ))
    }

    /// Never waits for a worker or a compositor. A full queue is reported.
    pub fn send(&self, command: Command) -> Result<()> {
        self.commands
            .try_send(command)
            .context("Desktop command queue is unavailable")
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

fn emit(events: &Sender<Event>, mut event: Event, alive: &AtomicBool) {
    while alive.load(Ordering::Relaxed) {
        match events.try_send(event) {
            Ok(()) => return,
            Err(async_channel::TrySendError::Full(value)) => {
                event = value;
                thread::sleep(Duration::from_millis(5));
            }
            Err(async_channel::TrySendError::Closed(_)) => return,
        }
    }
}

fn socket_path(environment: &Environment) -> Option<PathBuf> {
    environment
        .get("CLARION_SOCKET")
        .filter(|p| !p.is_empty())
        .or_else(|| environment.get("REALAND_SOCKET").filter(|p| !p.is_empty()))
        .map(PathBuf::from)
}

/// Nonblocking AF_UNIX connect also bounds an overfull listener's backlog wait.
fn connect(path: &Path, timeout: Duration) -> Result<UnixStream> {
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    anyhow::ensure!(
        path.is_absolute()
            && !bytes.is_empty()
            && !bytes.contains(&0)
            && bytes.len() < address.sun_path.len(),
        "Invalid IPC socket path"
    );
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dest, src) in address.sun_path.iter_mut().zip(bytes) {
        *dest = *src as libc::c_char;
    }
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    anyhow::ensure!(
        fd >= 0,
        "Create IPC socket: {}",
        std::io::Error::last_os_error()
    );
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    let status = unsafe {
        libc::connect(
            fd,
            &address as *const _ as *const libc::sockaddr,
            std::mem::size_of_val(&address) as libc::socklen_t,
        )
    };
    if status != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS)
            && error.raw_os_error() != Some(libc::EAGAIN)
        {
            return Err(error.into());
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let status = unsafe {
            libc::poll(
                &mut poll,
                1,
                timeout.as_millis().min(i32::MAX as u128) as i32,
            )
        };
        anyhow::ensure!(status > 0, "IPC connection timed out");
        let mut error: libc::c_int = 0;
        let mut length = std::mem::size_of_val(&error) as libc::socklen_t;
        let status = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                &mut error as *mut _ as *mut libc::c_void,
                &mut length,
            )
        };
        anyhow::ensure!(
            status == 0 && error == 0,
            "IPC connection failed: {}",
            std::io::Error::from_raw_os_error(error)
        );
        // Linux reports an unavailable UNIX backlog with EAGAIN without an
        // asynchronous connection. Confirm the peer rather than guessing.
        stream
            .peer_addr()
            .context("IPC connection did not complete")?;
    }
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(stream)
}

fn read_line(stream: &mut UnixStream, limit: usize, deadline: Instant) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    while Instant::now() < deadline {
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1));
        stream.set_read_timeout(Some(remaining))?;
        // Peek before consuming so the EventStream acknowledgement cannot eat
        // bytes belonging to its first event, while replies remain bulk reads.
        let length = unsafe {
            libc::recv(
                stream.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                libc::MSG_PEEK,
            )
        };
        if length < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        anyhow::ensure!(length > 0, "IPC closed before its reply");
        let length = length as usize;
        let newline = buffer[..length].iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(length, |end| end + 1);
        anyhow::ensure!(bytes.len() + count <= limit + 1, "IPC line is too large");
        stream.read_exact(&mut buffer[..count])?;
        if newline.is_some() {
            bytes.extend_from_slice(&buffer[..count - 1]);
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    bail!("IPC reply timed out")
}

fn send_request(stream: &mut UnixStream, request: &Request) -> Result<Response> {
    let mut bytes = serde_json::to_vec(request)?;
    bytes.push(b'\n');
    anyhow::ensure!(bytes.len() <= LINE_LIMIT, "IPC request is too large");
    stream.write_all(&bytes)?;
    let reply: Reply = serde_json::from_slice(&read_line(
        stream,
        LINE_LIMIT,
        Instant::now() + IPC_TIMEOUT,
    )?)?;
    reply.map_err(anyhow::Error::msg)
}

pub fn request(environment: &Environment, request: Request) -> Result<Response> {
    let path = socket_path(environment).context("当前会话没有 Clarion IPC 地址")?;
    send_request(&mut connect(&path, IPC_TIMEOUT)?, &request)
}

fn snapshot(state: &EventStreamState, base: &DesktopSnapshot) -> DesktopSnapshot {
    let mut snapshot = base.clone();
    snapshot.workspaces = state.workspaces.workspaces.values().cloned().collect();
    snapshot
        .workspaces
        .sort_by_key(|workspace| (workspace.output.clone(), workspace.idx, workspace.id));
    snapshot.windows = state.windows.windows.values().cloned().collect();
    snapshot.windows.sort_by_key(|window| window.id);
    snapshot.keyboard_layouts = state.keyboard_layouts.keyboard_layouts.clone();
    snapshot.reduced_motion = state.reduced_motion.enabled.or(base.reduced_motion);
    snapshot
}

fn desktop_worker(options: Options, events: Sender<Event>, alive: Arc<AtomicBool>) {
    let mut generation = 0;
    while alive.load(Ordering::Relaxed) {
        generation += 1;
        let mut base = DesktopSnapshot {
            generation,
            ..Default::default()
        };
        emit(&events, Event::Desktop(base.clone()), &alive);
        let result = (|| -> Result<()> {
            let path =
                socket_path(&options.environment).context("当前会话没有 Clarion IPC 地址")?;
            let mut stream = connect(&path, IPC_TIMEOUT)?;
            anyhow::ensure!(
                matches!(
                    send_request(&mut stream, &Request::EventStream)?,
                    Response::Handled
                ),
                "Compositor rejected event stream"
            );
            base.connected = true;
            if let Ok(Response::Outputs(outputs)) = request(&options.environment, Request::Outputs)
            {
                base.outputs = outputs;
            }
            if let Ok(Response::Capabilities(capabilities)) =
                request(&options.environment, Request::Capabilities)
            {
                base.capabilities = capabilities;
            }
            if let Ok(Response::ReducedMotionState(value)) =
                request(&options.environment, Request::ReducedMotionState)
            {
                base.reduced_motion = value;
            }
            let mut state = EventStreamState::default();
            emit(&events, Event::Desktop(base.clone()), &alive);
            stream.set_read_timeout(Some(Duration::from_millis(100)))?;
            let mut pending = Vec::new();
            let mut buffer = [0; 65536];
            let mut outputs_due = Instant::now() + Duration::from_secs(3);
            let mut snapshot_due = Instant::now();
            let mut dirty = false;
            while alive.load(Ordering::Relaxed) {
                match stream.read(&mut buffer) {
                    Ok(0) => bail!("Clarion IPC disconnected"),
                    Ok(length) => {
                        pending.extend_from_slice(&buffer[..length]);
                        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                            anyhow::ensure!(end <= LINE_LIMIT, "IPC event is too large");
                            let bytes: Vec<_> = pending.drain(..=end).collect();
                            let event = match serde_json::from_slice::<clarion_ipc::Event>(&bytes) {
                                Ok(event) => event,
                                Err(_) => continue, // A newer compositor may add an event variant.
                            };
                            match &event {
                                clarion_ipc::Event::ScreenshotCaptured { path, is_preview } => {
                                    if !is_preview {
                                        emit(
                                            &events,
                                            Event::Screenshot {
                                                path: path.clone(),
                                                is_preview: false,
                                            },
                                            &alive,
                                        );
                                    }
                                }
                                clarion_ipc::Event::ConfigLoaded { .. } => {
                                    outputs_due = Instant::now()
                                }
                                _ => {}
                            }
                            state.apply(event);
                            dirty = true;
                        }
                        anyhow::ensure!(pending.len() <= LINE_LIMIT, "IPC event is too large");
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) => return Err(error.into()),
                }
                if Instant::now() >= outputs_due {
                    if let Ok(Response::Outputs(outputs)) =
                        request(&options.environment, Request::Outputs)
                    {
                        base.outputs = outputs;
                        dirty = true;
                    }
                    outputs_due = Instant::now() + Duration::from_secs(3);
                }
                if dirty && Instant::now() >= snapshot_due {
                    emit(&events, Event::Desktop(snapshot(&state, &base)), &alive);
                    dirty = false;
                    snapshot_due = Instant::now() + Duration::from_millis(16);
                }
            }
            Ok(())
        })();
        if alive.load(Ordering::Relaxed) {
            // Never keep a previous compositor's IDs or outputs after disconnect.
            emit(
                &events,
                Event::Desktop(DesktopSnapshot {
                    generation,
                    ..Default::default()
                }),
                &alive,
            );
            if let Err(error) = result {
                emit(
                    &events,
                    Event::Error(format!("桌面连接已断开：{error:#}")),
                    &alive,
                );
            }
            wait_alive(&alive, Duration::from_secs(2));
        }
    }
}

fn load_catalog(environment: &Environment, events: &Sender<Event>, alive: &AtomicBool) {
    match AppCatalog::load(environment) {
        Ok(catalog) => emit(events, Event::Catalog(catalog), alive),
        Err(error) => emit(
            events,
            Event::Error(format!("无法读取应用目录：{error:#}")),
            alive,
        ),
    }
}

fn perform(
    command: Command,
    options: &Options,
    events: &Sender<Event>,
    alive: &AtomicBool,
) -> Result<()> {
    if options.preview
        && !matches!(
            command,
            Command::RefreshCatalog | Command::SavePreferences(_)
        )
    {
        bail!("预览模式不执行桌面操作");
    }
    match command {
        Command::Action(action) => {
            request(&options.environment, Request::Action(action))?;
        }
        Command::PlacePreferences {
            id,
            width,
            height,
            x,
            y,
        } => {
            use clarion_ipc::{PositionChange, SizeChange};
            for action in [
                Action::MoveWindowToFloating { id: Some(id) },
                Action::SetWindowWidth {
                    id: Some(id),
                    change: SizeChange::SetFixed(width),
                },
                Action::SetWindowHeight {
                    id: Some(id),
                    change: SizeChange::SetFixed(height),
                },
                Action::MoveFloatingWindow {
                    id: Some(id),
                    x: PositionChange::SetFixed(x),
                    y: PositionChange::SetFixed(y),
                },
            ] {
                request(&options.environment, Request::Action(action))?;
            }
        }
        Command::LaunchDesktop(path) => {
            anyhow::ensure!(
                path.is_absolute() && path.extension().is_some_and(|ext| ext == "desktop"),
                "Invalid desktop entry path"
            );
            clarion_launcher::launch::launch_desktop_file(&path, &options.environment)?;
        }
        Command::LaunchDesktopWithPaths(path, files) => {
            anyhow::ensure!(
                path.is_absolute() && path.extension().is_some_and(|ext| ext == "desktop"),
                "Invalid desktop entry path"
            );
            clarion_launcher::launch::launch_desktop_file_with_paths(
                &path,
                &files,
                &options.environment,
            )?;
        }
        Command::OpenPath(path) => {
            anyhow::ensure!(
                path.is_absolute() && !path.as_os_str().as_bytes().contains(&0),
                "Invalid location"
            );
            run_tool(
                "gio",
                &[
                    "open",
                    "--",
                    path.to_str().context("Location is not UTF-8")?,
                ],
                &options.environment,
                Duration::from_secs(3),
            )?;
        }
        Command::OpenSettings => {
            request(
                &options.environment,
                Request::Action(Action::Spawn {
                    command: vec!["clarion-settings".into()],
                }),
            )?;
        }
        Command::SetVolume(value) => {
            anyhow::ensure!(
                value.is_finite() && (0. ..=1.).contains(&value),
                "Invalid volume"
            );
            run_tool(
                "wpctl",
                &["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{value:.4}")],
                &options.environment,
                TOOL_TIMEOUT,
            )?;
        }
        Command::SetMuted(muted) => {
            run_tool(
                "wpctl",
                &[
                    "set-mute",
                    "@DEFAULT_AUDIO_SINK@",
                    if muted { "1" } else { "0" },
                ],
                &options.environment,
                TOOL_TIMEOUT,
            )?;
        }
        Command::SetOutputBrightness { output, value } => {
            anyhow::ensure!(
                value.is_finite() && (0. ..=1.).contains(&value) && output.len() <= 1024,
                "Invalid output brightness"
            );
            let Response::Outputs(outputs) = request(&options.environment, Request::Outputs)?
            else {
                bail!("Cannot read display brightness");
            };
            let target = outputs
                .get(&output)
                .filter(|output| output.logical.is_some())
                .context("Display is disconnected")?;
            if target.hdr_active {
                request(
                    &options.environment,
                    Request::Output {
                        output,
                        action: clarion_ipc::OutputAction::HdrBrightness {
                            percent: (value * 100.).max(1.),
                        },
                    },
                )?;
            } else {
                run_tool(
                    "brightnessctl",
                    &["--class=backlight", "set", &format!("{:.2}%", value * 100.)],
                    &options.environment,
                    TOOL_TIMEOUT,
                )?;
            }
        }
        Command::RefreshCatalog => load_catalog(&options.environment, events, alive),
        Command::SavePreferences(preferences) => {
            preferences.save(&options.environment, options.preview)?;
            emit(events, Event::Preferences(preferences), alive);
        }
    }
    Ok(())
}

fn run_tool(
    program: &str,
    args: &[&str],
    environment: &Environment,
    timeout: Duration,
) -> Result<String> {
    let mut child = ProcessCommand::new(program)
        .args(args)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("Start {program}"))?;
    let mut stdout = child.stdout.take().context("Command stdout")?;
    let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        let _ = child.kill();
        let _ = child.wait();
        bail!("Cannot configure command output");
    }
    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    loop {
        let mut bytes = [0; 4096];
        loop {
            match stdout.read(&mut bytes) {
                Ok(0) => break,
                Ok(count) => {
                    output.extend_from_slice(&bytes[..count]);
                    if output.len() > 65536 {
                        let _ = child.kill();
                        let _ = child.wait();
                        bail!("Command output too large");
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error.into());
                }
            }
        }
        match child.try_wait()? {
            Some(status) if status.success() => {
                // The child can write its last bytes between the nonblocking
                // drain and try_wait. Drain once more after observing exit.
                loop {
                    let mut bytes = [0; 4096];
                    match stdout.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(count) => {
                            output.extend_from_slice(&bytes[..count]);
                            anyhow::ensure!(output.len() <= 65536, "Command output too large");
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) => return Err(error.into()),
                    }
                }
                return Ok(String::from_utf8_lossy(&output).into_owned());
            }
            Some(status) => bail!("{program} exited with {status}"),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("{program} timed out");
            }
            None => thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn system_worker(options: Options, events: Sender<Event>, alive: Arc<AtomicBool>) {
    let mut hardware = SystemSnapshot::default();
    let mut hardware_due = Instant::now();
    while alive.load(Ordering::Relaxed) {
        if !options.preview && Instant::now() >= hardware_due {
            hardware.volume = None;
            hardware.muted = false;
            hardware.brightness = None;
            if let Ok(value) = run_tool(
                "wpctl",
                &["get-volume", "@DEFAULT_AUDIO_SINK@"],
                &options.environment,
                TOOL_TIMEOUT,
            ) {
                hardware.volume = parse_volume(&value);
                hardware.muted = value.contains("[MUTED]");
            }
            if let Ok(value) = run_tool(
                "brightnessctl",
                &["--class=backlight", "--machine-readable", "info"],
                &options.environment,
                TOOL_TIMEOUT,
            ) {
                hardware.brightness = value
                    .split(',')
                    .find_map(|item| {
                        item.trim()
                            .strip_suffix('%')
                            .and_then(|number| number.parse::<f64>().ok())
                    })
                    .map(|value| (value / 100.).clamp(0., 1.));
            }
            let (battery, charging) = battery_snapshot(Path::new("/sys/class/power_supply"));
            hardware.battery = battery;
            hardware.charging = charging;
            let data = options
                .environment
                .get("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    options
                        .environment
                        .get("HOME")
                        .map(|home| PathBuf::from(home).join(".local/share"))
                });
            hardware.trash_full = data
                .and_then(|data| fs::read_dir(data.join("Trash/files")).ok())
                .is_some_and(|mut files| files.next().is_some());
            hardware_due = Instant::now() + Duration::from_secs(3);
        }
        let now = chrono::Local::now();
        hardware.clock = now.format("%H:%M:%S").to_string();
        let weekday = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"]
            [chrono::Datelike::weekday(&now).num_days_from_monday() as usize];
        hardware.date = format!("{} {weekday}", now.format("%m月%d日"));
        emit(&events, Event::System(hardware.clone()), &alive);
        wait_alive(&alive, Duration::from_secs(1));
    }
}

fn parse_volume(value: &str) -> Option<f64> {
    value
        .strip_prefix("Volume:")?
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.)
        .map(|v| v.clamp(0., 1.))
}

fn battery_snapshot(root: &Path) -> (Option<f64>, bool) {
    let Ok(entries) = fs::read_dir(root) else {
        return (None, false);
    };
    let mut capacities = Vec::new();
    let mut charging = false;
    for entry in entries.flatten().take(32) {
        let path = entry.path();
        if fs::read_to_string(path.join("type"))
            .ok()
            .is_none_or(|kind| kind.trim() != "Battery")
        {
            continue;
        }
        let present = fs::read_to_string(path.join("present"))
            .ok()
            .and_then(|present| present.trim().parse::<u8>().ok());
        if present == Some(0) {
            continue;
        }
        // HID pens, touch controllers and similar per-device batteries are not
        // the system battery. Only `scope=Device` marks them explicitly.
        let peripheral =
            fs::read_to_string(path.join("scope")).is_ok_and(|scope| scope.trim() == "Device");
        if peripheral {
            continue;
        }
        if let Some(capacity) = fs::read_to_string(path.join("capacity"))
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite() && (0. ..=100.).contains(v))
        {
            capacities.push(capacity / 100.)
        }
        charging |= fs::read_to_string(path.join("status"))
            .ok()
            .is_some_and(|s| matches!(s.trim(), "Charging" | "Full"));
    }
    (
        (!capacities.is_empty()).then(|| capacities.iter().sum::<f64>() / capacities.len() as f64),
        charging,
    )
}

fn wait_alive(alive: &AtomicBool, duration: Duration) {
    let end = Instant::now() + duration;
    while alive.load(Ordering::Relaxed) && Instant::now() < end {
        thread::sleep(Duration::from_millis(25));
    }
}

struct CommandServer {
    path: PathBuf,
    identity: (u64, u64),
    _lock: File,
}
impl CommandServer {
    fn bind(
        environment: &Environment,
        events: Sender<Event>,
        alive: Arc<AtomicBool>,
    ) -> Result<Self> {
        let runtime = environment
            .get("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .context("XDG_RUNTIME_DIR is required")?;
        anyhow::ensure!(runtime.is_absolute(), "Runtime path must be absolute");
        let metadata = fs::symlink_metadata(&runtime)?;
        anyhow::ensure!(
            metadata.is_dir()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.permissions().mode() & 0o022 == 0,
            "Runtime directory must be owned and private"
        );
        let path = environment
            .get("CLARION_SHELL_SOCKET")
            .filter(|p| !p.is_empty())
            .or_else(|| {
                environment
                    .get("REALAND_SHELL_SOCKET")
                    .filter(|p| !p.is_empty())
            })
            .map(PathBuf::from)
            .unwrap_or_else(|| runtime.join("clarion-shell.sock"));
        anyhow::ensure!(
            path.is_absolute() && path.parent() == Some(runtime.as_path()),
            "Shell command socket must be inside XDG_RUNTIME_DIR"
        );
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path.with_extension("lock"))?;
        let metadata = lock.metadata()?;
        anyhow::ensure!(
            metadata.is_file()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.nlink() == 1,
            "Unsafe command lock file"
        );
        lock.set_permissions(fs::Permissions::from_mode(0o600))?;
        anyhow::ensure!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "Another Clarion Shell owns this command socket"
        );
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_socket()
                    && metadata.uid() == unsafe { libc::geteuid() } =>
            {
                fs::remove_file(&path)?
            }
            Ok(_) => bail!("Shell socket path is occupied by another file"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let metadata = fs::symlink_metadata(&path)?;
        let identity = (metadata.dev(), metadata.ino());
        listener.set_nonblocking(true)?;
        thread::Builder::new()
            .name("clarion-shell-panel-commands".into())
            .spawn(move || {
                while alive.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
                            let mut length = std::mem::size_of_val(&peer) as libc::socklen_t;
                            let status = unsafe {
                                libc::getsockopt(
                                    stream.as_raw_fd(),
                                    libc::SOL_SOCKET,
                                    libc::SO_PEERCRED,
                                    &mut peer as *mut _ as *mut libc::c_void,
                                    &mut length,
                                )
                            };
                            if status != 0 || peer.uid != unsafe { libc::geteuid() } {
                                continue;
                            }
                            let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                            match read_line(
                                &mut stream,
                                512,
                                Instant::now() + Duration::from_millis(500),
                            )
                            .ok()
                            .and_then(|line| String::from_utf8(line).ok())
                            {
                                Some(command) if PANEL_COMMANDS.contains(&command.trim()) => {
                                    emit(&events, Event::Command(command.trim().into()), &alive);
                                    let _ = stream.write_all(b"ok\n");
                                }
                                _ => {
                                    let _ = stream.write_all(b"invalid-command\n");
                                }
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20))
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            path,
            identity,
            _lock: lock,
        })
    }
}
impl Drop for CommandServer {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|m| (m.dev(), m.ino()) == self.identity) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn battery_snapshot_ignores_peripheral_and_absent_batteries() {
        let directory = tempfile::tempdir().unwrap();
        let write = |name: &str, files: &[(&str, &str)]| {
            let path = directory.path().join(name);
            fs::create_dir_all(&path).unwrap();
            for (file, value) in files {
                fs::write(path.join(file), value).unwrap();
            }
        };
        write(
            "BAT0",
            &[
                ("type", "Battery\n"),
                ("present", "1\n"),
                ("capacity", "90\n"),
                ("status", "Not charging\n"),
            ],
        );
        write(
            "hid-touch-battery",
            &[
                ("type", "Battery\n"),
                ("present", "0\n"),
                ("capacity", "0\n"),
                ("status", "Unknown\n"),
                ("scope", "Device\n"),
            ],
        );
        write(
            "hid-pen-battery",
            &[
                ("type", "Battery\n"),
                ("present", "1\n"),
                ("capacity", "20\n"),
                ("status", "Discharging\n"),
                ("scope", "Device\n"),
            ],
        );
        write("AC0", &[("type", "Mains\n"), ("status", "Online\n")]);
        let (battery, charging) = battery_snapshot(directory.path());
        assert!(
            (battery.unwrap() - 0.9).abs() < 1e-6,
            "the main battery must not be averaged with peripheral batteries: {battery:?}"
        );
        assert!(!charging);
    }
    #[test]
    fn new_generation_never_retains_previous_windows_or_outputs() {
        let mut state = EventStreamState::default();
        state.apply(clarion_ipc::Event::KeyboardLayoutsChanged {
            keyboard_layouts: KeyboardLayouts {
                names: vec!["English".into()],
                current_idx: 0,
            },
        });
        state.apply(clarion_ipc::Event::ReducedMotionChanged { enabled: true });
        let old = snapshot(
            &state,
            &DesktopSnapshot {
                generation: 3,
                connected: true,
                ..Default::default()
            },
        );
        assert!(old.keyboard_layouts.is_some());
        assert_eq!(old.reduced_motion, Some(true));
        let fresh = snapshot(
            &EventStreamState::default(),
            &DesktopSnapshot {
                generation: 4,
                ..Default::default()
            },
        );
        assert!(fresh.keyboard_layouts.is_none());
        assert!(fresh.reduced_motion.is_none());
        assert!(fresh.windows.is_empty());
        assert!(fresh.outputs.is_empty());
    }
    #[test]
    fn ipc_replies_are_typed_bounded_and_time_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let env: Environment =
            [("CLARION_SOCKET".into(), path.to_string_lossy().into_owned())].into();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let line = read_line(&mut stream, 100, Instant::now() + IPC_TIMEOUT).unwrap();
            assert!(matches!(
                serde_json::from_slice::<Request>(&line).unwrap(),
                Request::KeyboardLayouts
            ));
            stream
                .write_all(
                    b"{\"Ok\":{\"KeyboardLayouts\":{\"names\":[\"English\"],\"current_idx\":0}}}\n",
                )
                .unwrap();
        });
        assert!(matches!(
            request(&env, Request::KeyboardLayouts).unwrap(),
            Response::KeyboardLayouts(_)
        ));
        worker.join().unwrap();
        let (mut client, _peer) = UnixStream::pair().unwrap();
        let before = Instant::now();
        assert!(read_line(&mut client, 10, Instant::now() + Duration::from_millis(30)).is_err());
        assert!(before.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn event_worker_reconnects_with_fresh_generation_and_preserves_partial_lines() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("desktop.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let environment: Environment =
            [("CLARION_SOCKET".into(), path.to_string_lossy().into_owned())].into();
        let alive = Arc::new(AtomicBool::new(true));
        let (events, received) = async_channel::bounded(128);
        let options = Options {
            environment,
            preview: false,
            command_server: false,
        };
        let worker_alive = alive.clone();
        let worker = thread::spawn(move || desktop_worker(options, events, worker_alive));
        let (allow_close, close) = mpsc::channel();
        let server = thread::spawn(move || {
            for id in [11, 22] {
                let (mut stream, _) = listener.accept().unwrap();
                let line =
                    read_line(&mut stream, 100, Instant::now() + Duration::from_secs(3)).unwrap();
                assert!(matches!(
                    serde_json::from_slice::<Request>(&line).unwrap(),
                    Request::EventStream
                ));
                stream.write_all(b"{\"Ok\":\"Handled\"}\n").unwrap();
                for _ in 0..3 {
                    let (mut query, _) = listener.accept().unwrap();
                    let line = read_line(&mut query, 100, Instant::now() + IPC_TIMEOUT).unwrap();
                    let reply = match serde_json::from_slice::<Request>(&line).unwrap() {
                        Request::Outputs => Response::Outputs(HashMap::new()),
                        Request::Capabilities => Response::Capabilities(vec!["fixture".into()]),
                        Request::ReducedMotionState => Response::ReducedMotionState(Some(false)),
                        request => panic!("Unexpected request: {request:?}"),
                    };
                    let mut bytes = serde_json::to_vec(&Ok::<_, String>(reply)).unwrap();
                    bytes.push(b'\n');
                    query.write_all(&bytes).unwrap();
                }
                // Split one initial event across socket timeout boundaries.
                let event = clarion_ipc::Event::WorkspacesChanged {
                    workspaces: vec![Workspace {
                        id,
                        idx: 1,
                        name: None,
                        output: Some("fixture-output".into()),
                        is_urgent: false,
                        is_active: true,
                        is_focused: true,
                        active_window_id: None,
                    }],
                };
                let mut bytes = serde_json::to_vec(&event).unwrap();
                bytes.push(b'\n');
                let middle = bytes.len() / 2;
                stream.write_all(&bytes[..middle]).unwrap();
                thread::sleep(Duration::from_millis(130));
                stream.write_all(&bytes[middle..]).unwrap();
                close.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
        let deadline = Instant::now() + Duration::from_secs(9);
        let mut first_generation = None;
        let mut cleared = false;
        loop {
            if let Ok(Event::Desktop(snapshot)) = received.try_recv() {
                if snapshot.workspaces.first().is_some_and(|w| w.id == 11) {
                    first_generation = Some(snapshot.generation);
                    allow_close.send(()).unwrap();
                } else if first_generation.is_some() && !snapshot.connected {
                    assert!(snapshot.workspaces.is_empty());
                    assert!(snapshot.outputs.is_empty());
                    cleared = true;
                } else if snapshot.workspaces.first().is_some_and(|w| w.id == 22) {
                    assert!(cleared);
                    assert!(snapshot.generation > first_generation.unwrap());
                    assert_eq!(snapshot.workspaces.len(), 1);
                    allow_close.send(()).unwrap();
                    break;
                }
            }
            assert!(Instant::now() < deadline, "Reconnect event timed out");
            thread::sleep(Duration::from_millis(5));
        }
        alive.store(false, Ordering::Relaxed);
        worker.join().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn tools_reject_nonfinite_values_and_timeout_without_shell_interpolation() {
        let options = Options {
            environment: Environment::new(),
            preview: false,
            command_server: false,
        };
        let (tx, _) = async_channel::bounded(8);
        let alive = AtomicBool::new(true);
        assert!(perform(Command::SetVolume(f64::NAN), &options, &tx, &alive).is_err());
        assert!(perform(
            Command::SetOutputBrightness {
                output: "fixture-output".into(),
                value: 2.
            },
            &options,
            &tx,
            &alive
        )
        .is_err());
        let before = Instant::now();
        assert!(run_tool(
            "/bin/sleep",
            &["2"],
            &Environment::new(),
            Duration::from_millis(30)
        )
        .is_err());
        assert!(before.elapsed() < Duration::from_secs(1));
        assert_eq!(parse_volume("Volume: 0.42 [MUTED]"), Some(0.42));
        assert_eq!(parse_volume("Volume: NaN"), None);
    }
    #[test]
    fn command_server_is_private_locked_and_cleans_only_its_own_socket() {
        let dir = tempfile::tempdir().unwrap();
        let env: Environment = [(
            "XDG_RUNTIME_DIR".into(),
            dir.path().to_string_lossy().into_owned(),
        )]
        .into();
        let (tx, rx) = async_channel::bounded(8);
        let alive = Arc::new(AtomicBool::new(true));
        let server = CommandServer::bind(&env, tx.clone(), alive.clone()).unwrap();
        assert_eq!(
            fs::metadata(&server.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(CommandServer::bind(&env, tx, alive.clone()).is_err());
        let mut stream = UnixStream::connect(&server.path).unwrap();
        stream.write_all(b"recent-content\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Ok(Event::Command(command)) = rx.try_recv() {
                assert_eq!(command, "recent-content");
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        alive.store(false, Ordering::Relaxed);
        let path = server.path.clone();
        drop(server);
        assert!(!path.exists());
    }
    #[test]
    fn preview_never_connects_or_invokes_desktop_commands() {
        let options = Options {
            environment: Environment::new(),
            preview: true,
            command_server: false,
        };
        let (tx, _) = async_channel::bounded(8);
        let alive = AtomicBool::new(true);
        assert!(perform(
            Command::Action(Action::CloseWindow { id: Some(123) }),
            &options,
            &tx,
            &alive
        )
        .is_err());
        assert!(
            socket_path(
                &[
                    ("CLARION_SOCKET".into(), "/stale".into()),
                    ("REALAND_SOCKET".into(), "/other".into())
                ]
                .into()
            )
            .unwrap()
                == std::path::Path::new("/stale")
        );
    }
}
