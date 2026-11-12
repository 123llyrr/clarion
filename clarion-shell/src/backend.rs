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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use async_channel::{Receiver, Sender};
use clarion_ipc::state::{EventStreamState, EventStreamStatePart};
use clarion_ipc::{Action, KeyboardLayouts, Output, Reply, Request, Response, Window, Workspace};
use clarion_launcher::catalog::AppCatalog;

use crate::power::PowerProfile;
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
    "battery",
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
    /// The averaged charge of the machine's system batteries, 0–1.
    pub battery: Option<f64>,
    /// What the batteries are doing, before the charge level folds in.
    pub battery_status: BatteryStatus,
    /// Full-charge capacity divided by design capacity. Battery wear, not the
    /// current charge: 50% here means the pack holds half of its original Wh.
    pub battery_health: Option<f64>,
    /// Mains present. Inferred from a charging/full battery when the platform
    /// exposes no AC device of its own.
    pub ac_online: bool,
    /// The active platform power profile, when the machine exposes one.
    pub power_profile: Option<PowerProfile>,
    /// The profiles this machine advertises, in canonical order.
    pub power_profiles: Vec<PowerProfile>,
    pub trash_full: bool,
    pub clock: String,
    pub date: String,
    /// The local calendar day, so the notification center's month grid does
    /// not have to parse the localized `date` label back into numbers.
    pub today: Option<chrono::NaiveDate>,
    /// Machine facts for the About card, collected once at worker start.
    pub facts: crate::system_info::SystemFacts,
    /// The active MPRIS player's metadata, when one is running.
    pub media: Option<Box<MediaSnapshot>>,
    /// The active network/Bluetooth connection, refreshed with the hardware
    /// poll.
    pub network: Box<NetworkSnapshot>,
}

/// The two connectivity rows at the top of the Control Center.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkSnapshot {
    /// SSID of the active Wi-Fi, or the connection name of an active wired
    /// link. `None` means nothing is connected.
    pub network: Option<String>,
    pub wireless: bool,
    /// The connected Bluetooth device's name, "已开启" when the adapter is on
    /// without a device, or `None` when Bluetooth is off/unavailable.
    pub bluetooth: Option<String>,
}

/// What the Control Center's media card shows. `playerctl` is the bridge; the
/// shell never speaks MPRIS itself, matching the other tool-backed rows.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaSnapshot {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// Elapsed time and track length in seconds, when the player reports them.
    pub position: Option<f64>,
    pub length: Option<f64>,
    /// Cover art re-encoded to a small PNG, ready for GPUI's image cache.
    pub art_png: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaControl {
    PlayPause,
    Next,
    Previous,
}

/// The kernel's `POWER_SUPPLY_STATUS`, reduced to the values the UI uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BatteryStatus {
    #[default]
    Unknown,
    Charging,
    Discharging,
    NotCharging,
    Full,
}

/// One pass over `/sys/class/power_supply`, kept separate from the snapshot so
/// the bar, the popup and the tests read the same aggregation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatterySnapshot {
    pub percent: Option<f64>,
    pub status: BatteryStatus,
    pub health: Option<f64>,
    pub ac_online: bool,
    /// The charge ceiling the firmware stops at, 0–1, but only when the
    /// platform exposes a writable end-threshold attribute. `None` means the
    /// popup cannot adjust it on this machine.
    pub charge_limit: Option<f64>,
}

#[derive(Debug)]
pub enum Event {
    Desktop(DesktopSnapshot),
    System(SystemSnapshot),
    /// The machine's power profile, kept out of the one-second `System`
    /// heartbeat so a cached snapshot can never undo a fresh switch.
    PowerProfile {
        profile: Option<PowerProfile>,
        profiles: Vec<PowerProfile>,
        /// True for the read-back that immediately follows a user request;
        /// false for the ordinary three-second poll.
        settled: bool,
        /// Monotonic stamp shared by both workers. A poll that started before
        /// a switch finishes carries an older stamp and must not paint the old
        /// mode back after the settled read-back.
        epoch: u64,
    },
    /// The battery's charge ceiling, owned by its own event for the same
    /// reason as the power profile: the one-second heartbeat may carry a
    /// sample taken before a slider write finished.
    BatteryChargeLimit {
        /// 0–1, or `None` when the platform has no writable threshold.
        limit: Option<f64>,
        /// Unique sample stamp. Every poll and every read-back takes a fresh
        /// one, so an older sample can never overwrite a newer value.
        epoch: u64,
    },
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
    /// Show one local file in the desktop's quick previewer (GNOME Sushi, the
    /// previewer Nautilus itself opens on space).
    PreviewFile(PathBuf),
    /// Open a web URL through the desktop handler.
    OpenUrl(String),
    OpenSettings,
    SetVolume(f64),
    SetMuted(bool),
    /// Drive the active MPRIS player through `playerctl`.
    Media(MediaControl),
    SetOutputBrightness {
        output: String,
        value: f64,
    },
    /// Switch the machine's ACPI/daemon power profile.
    SetPowerProfile(PowerProfile),
    /// Write the battery's charge ceiling, in percent (0–100).
    SetBatteryChargeLimit(u8),
    RefreshCatalog,
    SavePreferences(Preferences),
    /// Count one successful application launch in the palette history.
    RecordLaunch(String),
    /// Lock, suspend or power the session off through logind/systemd.
    System(crate::app::SystemAction),
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
        let power_epoch = Arc::new(AtomicU64::new(0));
        let charge_epoch = Arc::new(AtomicU64::new(0));
        {
            let options = options.clone();
            let events = events.clone();
            let alive = alive.clone();
            let power_epoch = power_epoch.clone();
            let charge_epoch = charge_epoch.clone();
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
                        if let Err(error) = perform(
                            command,
                            &options,
                            &events,
                            &alive,
                            &power_epoch,
                            &charge_epoch,
                        ) {
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
            let power_epoch = power_epoch.clone();
            let charge_epoch = charge_epoch.clone();
            thread::Builder::new()
                .name("clarion-shell-system".into())
                .spawn(move || system_worker(options, events, alive, power_epoch, charge_epoch))?;
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
    power_epoch: &AtomicU64,
    charge_epoch: &AtomicU64,
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
            if let Err(error) = open_location(&path, &options.environment) {
                report_failure(&options.environment, "无法打开文件", &path, &error)?;
            }
        }
        Command::PreviewFile(path) => {
            anyhow::ensure!(
                path.is_absolute() && !path.as_os_str().as_bytes().contains(&0),
                "Invalid location"
            );
            if let Err(error) = preview_location(&path, &options.environment) {
                report_failure(&options.environment, "无法预览文件", &path, &error)?;
            }
        }
        Command::OpenUrl(url) => {
            let scheme = url.to_ascii_lowercase();
            anyhow::ensure!(
                (scheme.starts_with("https://") || scheme.starts_with("http://"))
                    && !url.contains(char::is_whitespace)
                    && !url.contains(char::is_control),
                "Invalid URL"
            );
            run_tool(
                "gio",
                &["open", "--", url.as_str()],
                &options.environment,
                Duration::from_secs(3),
            )?;
        }
        Command::System(action) => run_system_action(action, options)?,
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
        Command::Media(control) => {
            let command = match control {
                MediaControl::PlayPause => "play-pause",
                MediaControl::Next => "next",
                MediaControl::Previous => "previous",
            };
            run_tool("playerctl", &[command], &options.environment, TOOL_TIMEOUT)?;
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
        Command::SetPowerProfile(profile) => {
            let result = set_power_profile(profile, &options.environment);
            // Everything polled before the switch is now stale. Bump the
            // shared stamp, then read the platform back so the popup learns
            // whether the switch actually took effect, even when the tool
            // exits successfully but the daemon keeps the old profile.
            let epoch = power_epoch.fetch_add(1, Ordering::SeqCst) + 1;
            let (profile, profiles) =
                power_profile_snapshot(Path::new("/sys/firmware/acpi"), &options.environment);
            emit(
                events,
                Event::PowerProfile {
                    profile,
                    profiles,
                    settled: true,
                    epoch,
                },
                alive,
            );
            result?;
        }
        Command::SetBatteryChargeLimit(limit) => {
            let result =
                set_battery_charge_limit(Path::new("/sys/class/power_supply"), limit.min(100));
            // Stamp the read-back after the write finished: a poll that began
            // before this point carries an older stamp and cannot paint the
            // old ceiling back over it.
            let epoch = charge_epoch.fetch_add(1, Ordering::SeqCst) + 1;
            let limit = battery_snapshot(Path::new("/sys/class/power_supply")).charge_limit;
            emit(
                events,
                Event::BatteryChargeLimit { limit, epoch },
                alive,
            );
            result?;
        }
        Command::RefreshCatalog => load_catalog(&options.environment, events, alive),
        Command::SavePreferences(preferences) => {
            preferences.save(&options.environment, options.preview)?;
            emit(events, Event::Preferences(preferences), alive);
        }
        Command::RecordLaunch(id) => {
            let mut history = clarion_launcher::usage::LaunchHistory::load(&options.environment);
            history.record_launch(&id, clarion_launcher::usage::unix_millis());
            history.save(&options.environment)?;
        }
    }
    Ok(())
}

/// Route a profile switch through power-profiles-daemon first; it is the
/// unprivileged path every distribution ships. Kernels that expose the ACPI
/// attribute directly are the fallback, which may require a writable profile
/// (a udev rule or root-owned shell).
fn set_power_profile(profile: PowerProfile, environment: &Environment) -> Result<()> {
    if run_tool(
        "powerprofilesctl",
        &["set", profile.id()],
        environment,
        Duration::from_secs(3),
    )
    .is_ok()
    {
        return Ok(());
    }
    fs::write(
        Path::new("/sys/firmware/acpi/platform_profile"),
        profile.sysfs_name(),
    )
    .with_context(|| format!("无法切换到{}电源模式", profile.label()))
}

/// Read the ACPI platform profile, falling back to power-profiles-daemon.
/// `powerprofilesctl list` carries both the active star and the available
/// names, so one process call answers the whole question.
fn power_profile_snapshot(
    root: &Path,
    environment: &Environment,
) -> (Option<PowerProfile>, Vec<PowerProfile>) {
    if let Some(snapshot) = crate::power::sysfs_snapshot(root) {
        return snapshot;
    }
    match run_tool(
        "powerprofilesctl",
        &["list"],
        environment,
        TOOL_TIMEOUT,
    ) {
        Ok(text) => crate::power::parse_powerprofilesctl_list(&text),
        Err(_) => (None, Vec::new()),
    }
}

/// Run one of the session's power actions. `loginctl`/`systemctl` return once
/// the request is queued, and the caller already runs on the worker thread.
fn run_system_action(action: crate::app::SystemAction, options: &Options) -> Result<()> {
    use crate::app::SystemAction;
    let timeout = Duration::from_secs(10);
    match action {
        SystemAction::Lock => {
            // Locking the caller's own session needs no id; passing the known
            // one keeps it deterministic under nested sessions.
            match options.environment.get("XDG_SESSION_ID") {
                Some(session) => run_tool(
                    "loginctl",
                    &["lock-session", session],
                    &options.environment,
                    timeout,
                )?,
                None => run_tool("loginctl", &["lock-session"], &options.environment, timeout)?,
            };
        }
        SystemAction::Logout => {
            let session = options
                .environment
                .get("XDG_SESSION_ID")
                .context("无法确定当前会话（XDG_SESSION_ID）")?;
            run_tool(
                "loginctl",
                &["terminate-session", session],
                &options.environment,
                timeout,
            )?;
        }
        SystemAction::Suspend => {
            run_tool("systemctl", &["suspend"], &options.environment, timeout)?;
        }
        SystemAction::Reboot => {
            run_tool("systemctl", &["reboot"], &options.environment, timeout)?;
        }
        SystemAction::Shutdown => {
            run_tool("systemctl", &["poweroff"], &options.environment, timeout)?;
        }
    }
    Ok(())
}

/// Open a local location the way a file manager's double click would: a
/// directory goes to the desktop file manager, while a file goes to the
/// application registered for its content type.
///
/// `gio open` cannot do the file case here: it turns the path into a `file:`
/// URI and therefore follows `x-scheme-handler/file`. On systems where that
/// scheme points at the file manager, every local file (an image, a PDF, an
/// archive) would open the containing folder instead of its own application.
fn open_location(path: &Path, environment: &Environment) -> Result<()> {
    let location = path.to_str().context("Location is not UTF-8")?;
    if path.is_dir() {
        run_tool(
            "gio",
            &["open", "--", location],
            environment,
            Duration::from_secs(3),
        )?;
        return Ok(());
    }
    if let Some(desktop) = default_desktop_file(path, environment) {
        return clarion_launcher::launch::launch_desktop_file_with_paths(
            &desktop,
            &[path.to_path_buf()],
            environment,
        );
    }
    // No application claims this type: let GIO show the location so the click
    // still does something visible instead of failing silently.
    run_tool(
        "gio",
        &["open", "--", location],
        environment,
        Duration::from_secs(3),
    )?;
    Ok(())
}

/// Show a local file in the desktop's quick previewer. Nautilus itself shells
/// out to GNOME Sushi, so launching that previewer's desktop entry gives the
/// same window without depending on the binary name or `PATH`.
fn preview_location(path: &Path, environment: &Environment) -> Result<()> {
    anyhow::ensure!(path.is_file(), "File is not available");
    let previewer = resolve_desktop_file("org.gnome.NautilusPreviewer.desktop", environment)
        .context("未安装文件快速预览（GNOME Sushi）")?;
    clarion_launcher::launch::launch_desktop_file_with_paths(
        &previewer,
        &[path.to_path_buf()],
        environment,
    )
}

/// Surface a failed open or preview through the session's notification service,
/// the channel desktop applications already use. If the notification tool is
/// unavailable the original error is returned so the shell can still show its
/// own banner instead of failing silently.
fn report_failure(
    environment: &Environment,
    summary: &str,
    path: &Path,
    error: &anyhow::Error,
) -> Result<()> {
    let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
    let body = format!("{name}：{error:#}");
    run_tool(
        "notify-send",
        &[
            "--app-name=Clarion",
            "--icon=dialog-error",
            "--",
            summary,
            &body,
        ],
        environment,
        Duration::from_millis(900),
    )
    .with_context(|| format!("无法发送系统通知（{error:#}）"))?;
    Ok(())
}

/// Resolve the application registered for `path`'s content type, without
/// consulting the `file:` scheme handler that `gio open` would use.
fn default_desktop_file(path: &Path, environment: &Environment) -> Option<PathBuf> {
    let mime = mime_guess::from_path(path).first_raw()?;
    let id = default_handler_id(mime, environment)?;
    resolve_desktop_file(&id, environment)
}

fn default_handler_id(mime: &str, environment: &Environment) -> Option<String> {
    let mut environment = environment.clone();
    // The sentence around the desktop id is localized; C keeps one stable
    // shape, and the id is always the last whitespace-separated token.
    environment.insert("LC_ALL".into(), "C".into());
    let output = run_tool("gio", &["mime", mime], &environment, Duration::from_secs(2)).ok()?;
    parse_default_handler(&output)
}

fn parse_default_handler(output: &str) -> Option<String> {
    let id = output.lines().next()?.split_whitespace().last()?;
    id.ends_with(".desktop").then(|| id.to_owned())
}

/// Find a desktop entry by the id GIO reports. Entries in subdirectories are
/// addressed with the directory separator folded into a hyphen, e.g.
/// `applications/foo/bar.desktop` becomes `foo-bar.desktop`.
fn resolve_desktop_file(id: &str, environment: &Environment) -> Option<PathBuf> {
    if id.contains('/') || id.contains('\\') || id == "." || id == ".." || !id.ends_with(".desktop")
    {
        return None;
    }
    let mut roots = Vec::new();
    if let Some(data_home) = environment.get("XDG_DATA_HOME") {
        roots.push(PathBuf::from(data_home));
    } else if let Some(home) = environment.get("HOME") {
        roots.push(PathBuf::from(home).join(".local/share"));
    }
    let data_dirs = environment
        .get("XDG_DATA_DIRS")
        .map(String::as_str)
        .unwrap_or("/usr/local/share:/usr/share");
    roots.extend(
        data_dirs
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from),
    );
    roots
        .into_iter()
        .find_map(|root| desktop_file_in(&root.join("applications"), id))
}

fn desktop_file_in(applications: &Path, id: &str) -> Option<PathBuf> {
    let direct = applications.join(id);
    if direct.is_file() {
        return Some(direct);
    }
    let mut pending = vec![(applications.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if file_type.is_dir() {
                pending.push((entry.path(), format!("{prefix}{name}-")));
            } else if format!("{prefix}{name}") == id {
                return Some(entry.path());
            }
        }
    }
    None
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

fn system_worker(
    options: Options,
    events: Sender<Event>,
    alive: Arc<AtomicBool>,
    power_epoch: Arc<AtomicU64>,
    charge_epoch: Arc<AtomicU64>,
) {
    let mut facts = crate::system_info::SystemFacts::collect(&options.environment);
    // NVIDIA names the card in procfs, so hybrid laptops avoid launching a
    // tool at all. Other drivers get their marketing name from `lspci -mm`;
    // the sysfs driver + PCI id is the last resort.
    if facts.gpu.is_empty() {
        if let Ok(text) = run_tool(
            "lspci",
            &["-mm"],
            &options.environment,
            Duration::from_secs(2),
        ) {
            if let Some(gpu) = crate::system_info::parse_lspci_gpu(&text) {
                facts.gpu = gpu;
            }
        }
    }
    if facts.gpu.is_empty() {
        facts.gpu = crate::system_info::sysfs_gpu();
    }
    let mut hardware = SystemSnapshot {
        facts,
        ..SystemSnapshot::default()
    };
    let mut media_art_cache: Option<(String, Option<Vec<u8>>)> = None;
    let mut hardware_due = Instant::now();
    while alive.load(Ordering::Relaxed) {
        if !options.preview && Instant::now() >= hardware_due {
            // Stamp the poll before it reads anything. A switch that finishes
            // while this pass is running bumps the epoch past it.
            let poll_epoch = power_epoch.load(Ordering::SeqCst);
            // The charge-limit poll takes a fresh stamp: a write's read-back
            // that completes later owns a newer one and wins.
            let poll_charge_epoch = charge_epoch.fetch_add(1, Ordering::SeqCst) + 1;
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
            let battery = battery_snapshot(Path::new("/sys/class/power_supply"));
            hardware.battery = battery.percent;
            hardware.battery_status = battery.status;
            hardware.battery_health = battery.health;
            hardware.ac_online = battery.ac_online;
            let (profile, profiles) =
                power_profile_snapshot(Path::new("/sys/firmware/acpi"), &options.environment);
            hardware.power_profile = profile;
            hardware.power_profiles = profiles.clone();
            emit(
                &events,
                Event::PowerProfile {
                    profile,
                    profiles,
                    settled: false,
                    epoch: poll_epoch,
                },
                &alive,
            );
            // The charge ceiling has its own event so the one-second heartbeat
            // can never overwrite a slider read-back with a stale sample.
            emit(
                &events,
                Event::BatteryChargeLimit {
                    limit: battery.charge_limit,
                    epoch: poll_charge_epoch,
                },
                &alive,
            );
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
            hardware.media = media_snapshot(&options.environment, &mut media_art_cache).map(Box::new);
            *hardware.network = network_snapshot(&options.environment);
            hardware_due = Instant::now() + Duration::from_secs(3);
        }
        let now = chrono::Local::now();
        hardware.clock = now.format("%H:%M:%S").to_string();
        let weekday = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"]
            [chrono::Datelike::weekday(&now).num_days_from_monday() as usize];
        hardware.date = format!("{} {weekday}", now.format("%m月%d日"));
        hardware.today = Some(now.date_naive());
        emit(&events, Event::System(hardware.clone()), &alive);
        wait_alive(&alive, Duration::from_secs(1));
    }
}

/// Read the active MPRIS player through `playerctl`. No player exits non-zero,
/// which reads as "nothing to show"; the card then keeps its idle state.
fn media_snapshot(
    environment: &Environment,
    art_cache: &mut Option<(String, Option<Vec<u8>>)>,
) -> Option<MediaSnapshot> {
    let output = run_tool(
        "playerctl",
        &[
            "metadata",
            "--format",
            "{{status}}\u{1f}{{title}}\u{1f}{{artist}}\u{1f}{{mpris:artUrl}}\u{1f}{{position}}\u{1f}{{mpris:length}}",
        ],
        environment,
        TOOL_TIMEOUT,
    )
    .ok()?;
    let mut fields = output.trim_end().split('\u{1f}');
    let status = fields.next()?.trim();
    if status.is_empty() || status == "Stopped" {
        return None;
    }
    let title = fields.next().unwrap_or_default().trim().to_owned();
    let artist = fields.next().unwrap_or_default().trim().to_owned();
    let art_url = fields.next().unwrap_or_default().trim();
    let position = fields
        .next()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|micros| micros as f64 / 1_000_000.);
    let length = fields
        .next()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|micros| micros as f64 / 1_000_000.);
    let art_png = if art_url.is_empty() {
        None
    } else if let Some((cached_url, cached)) = art_cache.as_ref() {
        if cached_url == art_url {
            cached.clone()
        } else {
            let decoded = media_art(art_url, environment);
            *art_cache = Some((art_url.to_owned(), decoded.clone()));
            decoded
        }
    } else {
        let decoded = media_art(art_url, environment);
        *art_cache = Some((art_url.to_owned(), decoded.clone()));
        decoded
    };
    Some(MediaSnapshot {
        title,
        artist,
        playing: status == "Playing",
        position,
        length,
        art_png,
    })
}

/// Turn an MPRIS `mpris:artUrl` into a small PNG. Local files are read
/// directly; http(s) covers are fetched with curl into a bounded temp file.
fn media_art(url: &str, environment: &Environment) -> Option<Vec<u8>> {
    let bytes = if url.starts_with("http://") || url.starts_with("https://") {
        fetch_remote_art(url, environment)?
    } else {
        let path = if let Some(rest) = url.strip_prefix("file://") {
            let rest = rest.strip_prefix("localhost").unwrap_or(rest);
            PathBuf::from(percent_decode(rest))
        } else if url.starts_with('/') {
            PathBuf::from(url)
        } else {
            return None;
        };
        if fs::metadata(&path).ok()?.len() > 8 * 1024 * 1024 {
            return None;
        }
        fs::read(&path).ok()?
    };
    let image = image::load_from_memory(&bytes).ok()?;
    let thumbnail = image.thumbnail(256, 256);
    let mut png = Vec::new();
    thumbnail
        .write_to(
            &mut std::io::Cursor::new(&mut png),
            image::ImageFormat::Png,
        )
        .ok()?;
    Some(png)
}

fn fetch_remote_art(url: &str, environment: &Environment) -> Option<Vec<u8>> {
    let path = std::env::temp_dir().join(format!(
        "clarion-media-art-{}.img",
        std::process::id()
    ));
    let status = ProcessCommand::new("curl")
        .args([
            "-sL",
            "--max-time",
            "3",
            "--max-filesize",
            "8388608",
            "-o",
        ])
        .arg(&path)
        .arg("--")
        .arg(url)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        let _ = fs::remove_file(&path);
        return None;
    }
    let bytes = fs::read(&path).ok();
    let _ = fs::remove_file(&path);
    bytes
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read the active network and Bluetooth state through the usual desktop
/// tools; a missing service simply leaves the row quiet.
fn network_snapshot(environment: &Environment) -> NetworkSnapshot {
    let mut snapshot = NetworkSnapshot::default();
    // Read the active device connections rather than the Wi-Fi scan list: the
    // scan reorders as access points appear, which made the pill flip between
    // equally valid names. The connection name of the connected Wi-Fi is its
    // current SSID; a wired link falls back to its profile name.
    if let Ok(text) = run_tool(
        "nmcli",
        &["-t", "-f", "TYPE,STATE,CONNECTION", "device"],
        environment,
        TOOL_TIMEOUT,
    ) {
        let mut wireless = None;
        let mut wired = None;
        for line in text.lines() {
            let mut parts = line.splitn(3, ':');
            let kind = parts.next().unwrap_or_default();
            let state = parts.next().unwrap_or_default();
            let name = parts.next().unwrap_or_default().trim();
            if state != "connected" || name.is_empty() {
                continue;
            }
            match kind {
                "wifi" => wireless = Some(name.replace("\\:", ":")),
                "ethernet" => wired = Some(name.to_owned()),
                _ => {}
            }
        }
        snapshot.wireless = wireless.is_some();
        snapshot.network = wireless.or(wired);
    }
    if let Ok(text) = run_tool(
        "bluetoothctl",
        &["devices", "Connected"],
        environment,
        TOOL_TIMEOUT,
    ) {
        snapshot.bluetooth = text.lines().find_map(|line| {
            let mut parts = line.splitn(3, ' ');
            parts.next()?; // "Device"
            parts.next()?; // MAC address
            let name = parts.next()?.trim();
            (!name.is_empty()).then(|| name.to_owned())
        });
        if snapshot.bluetooth.is_none() {
            if let Ok(text) = run_tool("bluetoothctl", &["show"], environment, TOOL_TIMEOUT) {
                snapshot.bluetooth = text.lines().find_map(|line| {
                    let powered = line.strip_prefix("Powered:")?.trim();
                    (powered == "yes").then(|| "已开启".to_owned())
                });
            }
        }
    }
    snapshot
}

fn parse_volume(value: &str) -> Option<f64> {    value
        .strip_prefix("Volume:")?
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.)
        .map(|v| v.clamp(0., 1.))
}

fn battery_snapshot(root: &Path) -> BatterySnapshot {
    let Ok(entries) = fs::read_dir(root) else {
        return BatterySnapshot::default();
    };
    let mut snapshot = BatterySnapshot::default();
    let mut capacities = Vec::new();
    let mut healths = Vec::new();
    let mut charging = false;
    let mut full = false;
    let mut discharging = false;
    let mut not_charging = false;
    let mut present = false;
    for entry in entries.flatten().take(32) {
        let path = entry.path();
        let kind = fs::read_to_string(path.join("type"))
            .ok()
            .map(|kind| kind.trim().to_owned());
        match kind.as_deref() {
            Some("Mains") => {
                // Some platforms only expose the mains state here; not every
                // AC adapter ships an `online` attribute.
                if fs::read_to_string(path.join("online")).is_ok_and(|line| line.trim() == "1") {
                    snapshot.ac_online = true;
                }
                continue;
            }
            Some("Battery") => {}
            _ => continue,
        }
        let battery_present = fs::read_to_string(path.join("present"))
            .ok()
            .and_then(|present| present.trim().parse::<u8>().ok());
        if battery_present == Some(0) {
            continue;
        }
        // HID pens, touch controllers and similar per-device batteries are not
        // the system battery. Only `scope=Device` marks them explicitly.
        let peripheral =
            fs::read_to_string(path.join("scope")).is_ok_and(|scope| scope.trim() == "Device");
        if peripheral {
            continue;
        }
        present = true;
        if snapshot.charge_limit.is_none() {
            snapshot.charge_limit = battery_charge_limit(&path);
        }
        let number = |key: &str| {
            fs::read_to_string(path.join(key))
                .ok()
                .and_then(|value| value.trim().parse::<f64>().ok())
                .filter(|value: &f64| value.is_finite() && *value >= 0.)
        };
        if let Some(capacity) = number("capacity").filter(|value| *value <= 100.) {
            capacities.push(capacity / 100.);
        }
        // Packs report either charge (Ah) or energy (Wh); the ratio is the
        // same wear figure either way.
        let full_capacity = number("charge_full").or_else(|| number("energy_full"));
        let design_capacity = number("charge_full_design").or_else(|| number("energy_full_design"));
        if let (Some(full_capacity), Some(design_capacity)) = (full_capacity, design_capacity) {
            if design_capacity > 0. && full_capacity > 0. {
                healths.push((full_capacity / design_capacity).clamp(0., 1.));
            }
        }
        match fs::read_to_string(path.join("status"))
            .ok()
            .map(|status| status.trim().to_owned())
            .as_deref()
        {
            Some("Charging") => charging = true,
            Some("Full") => full = true,
            Some("Discharging") => discharging = true,
            Some("Not charging") => not_charging = true,
            _ => {}
        }
    }
    if !present {
        return snapshot;
    }
    snapshot.status = if charging {
        BatteryStatus::Charging
    } else if full {
        BatteryStatus::Full
    } else if discharging {
        BatteryStatus::Discharging
    } else if not_charging {
        BatteryStatus::NotCharging
    } else {
        BatteryStatus::Unknown
    };
    // Charging or full is proof of mains even when no AC device is exposed.
    snapshot.ac_online |= matches!(
        snapshot.status,
        BatteryStatus::Charging | BatteryStatus::Full
    );
    if !capacities.is_empty() {
        snapshot.percent = Some(capacities.iter().sum::<f64>() / capacities.len() as f64);
    }
    if !healths.is_empty() {
        snapshot.health = Some(healths.iter().sum::<f64>() / healths.len() as f64);
    }
    snapshot
}

/// End-of-charge attributes drivers expose. Newer kernels use
/// `charge_control_end_threshold`; older thinkpad_acpi packs use
/// `charge_stop_threshold`.
const CHARGE_LIMIT_ATTRS: [&str; 2] = ["charge_control_end_threshold", "charge_stop_threshold"];

/// The current end threshold of one battery, but only when the attribute is
/// writable by this process. A driver that exposes the number but not the
/// permission is not adjustable from the shell, so it reads as unsupported.
fn battery_charge_limit(dir: &Path) -> Option<f64> {
    for attr in CHARGE_LIMIT_ATTRS {
        let path = dir.join(attr);
        if OpenOptions::new().write(true).open(&path).is_err() {
            continue;
        }
        if let Some(value) = fs::read_to_string(&path)
            .ok()
            .and_then(|value| value.trim().parse::<u8>().ok())
            .filter(|value| *value <= 100)
        {
            return Some(f64::from(value) / 100.);
        }
    }
    None
}

/// Write the end threshold to the first battery that exposes a writable
/// attribute. The caller re-reads the value afterwards so the popup shows what
/// the firmware actually kept.
fn set_battery_charge_limit(root: &Path, limit: u8) -> Result<()> {
    let Ok(entries) = fs::read_dir(root) else {
        bail!("当前系统不支持电池充电阈值调节");
    };
    let mut last_error = None;
    for entry in entries.flatten().take(32) {
        let dir = entry.path();
        let battery =
            fs::read_to_string(dir.join("type")).is_ok_and(|kind| kind.trim() == "Battery");
        if !battery {
            continue;
        }
        // Match `battery_snapshot`: an absent pack and a HID/pen battery are
        // not the system battery, so never write their ceiling.
        let present = fs::read_to_string(dir.join("present"))
            .ok()
            .and_then(|present| present.trim().parse::<u8>().ok());
        if present == Some(0) {
            continue;
        }
        let peripheral =
            fs::read_to_string(dir.join("scope")).is_ok_and(|scope| scope.trim() == "Device");
        if peripheral {
            continue;
        }
        for attr in CHARGE_LIMIT_ATTRS {
            let path = dir.join(attr);
            if !path.exists() {
                continue;
            }
            match fs::write(&path, format!("{limit}\n")) {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some((path, error)),
            }
        }
    }
    match last_error {
        Some((path, error)) => {
            Err(error).with_context(|| format!("无法写入 {}", path.display()))
        }
        None => bail!("当前系统不支持电池充电阈值调节"),
    }
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
        write("AC0", &[("type", "Mains\n"), ("online", "1\n")]);
        let snapshot = battery_snapshot(directory.path());
        assert!(
            (snapshot.percent.unwrap() - 0.9).abs() < 1e-6,
            "the main battery must not be averaged with peripheral batteries: {:?}",
            snapshot.percent
        );
        assert_eq!(snapshot.status, BatteryStatus::NotCharging);
        assert!(snapshot.ac_online, "the mains adapter is online");
        assert_eq!(snapshot.health, None, "no full/design capacity was given");
    }

    #[test]
    fn battery_snapshot_reports_wear_and_infers_mains_from_charging() {
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
                ("capacity", "80\n"),
                ("status", "Charging\n"),
                ("charge_full", "4000000\n"),
                ("charge_full_design", "5000000\n"),
            ],
        );
        // No AC entry: a charging battery is itself proof of mains power.
        let snapshot = battery_snapshot(directory.path());
        assert_eq!(snapshot.status, BatteryStatus::Charging);
        assert!(snapshot.ac_online);
        assert!((snapshot.health.unwrap() - 0.8).abs() < 1e-6);
    }

    #[test]
    fn battery_snapshot_prefers_charge_and_full_over_discharging() {
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
                ("capacity", "100\n"),
                ("status", "Full\n"),
            ],
        );
        write(
            "BAT1",
            &[
                ("type", "Battery\n"),
                ("present", "1\n"),
                ("capacity", "40\n"),
                ("status", "Discharging\n"),
            ],
        );
        let snapshot = battery_snapshot(directory.path());
        // A full pack means the machine is on mains; the aggregate percent is
        // still the average the bar has always shown.
        assert_eq!(snapshot.status, BatteryStatus::Full);
        assert!(snapshot.ac_online);
        assert!((snapshot.percent.unwrap() - 0.7).abs() < 1e-6);
        assert_eq!(snapshot.health, None);
    }
    #[test]
    fn battery_snapshot_reads_a_writable_charge_ceiling() {
        let directory = tempfile::tempdir().unwrap();
        let battery = directory.path().join("BAT0");
        fs::create_dir_all(&battery).unwrap();
        for (file, value) in [
            ("type", "Battery\n"),
            ("present", "1\n"),
            ("capacity", "60\n"),
            ("status", "Charging\n"),
            ("charge_control_end_threshold", "80\n"),
        ] {
            fs::write(battery.join(file), value).unwrap();
        }
        let snapshot = battery_snapshot(directory.path());
        assert_eq!(
            snapshot.charge_limit,
            Some(0.8),
            "the writable end threshold is the charge ceiling"
        );

        // Writing through the helper moves the ceiling, and the next poll sees
        // exactly what the firmware kept.
        set_battery_charge_limit(directory.path(), 65).unwrap();
        assert_eq!(
            fs::read_to_string(battery.join("charge_control_end_threshold"))
                .unwrap()
                .trim(),
            "65"
        );
        assert_eq!(battery_snapshot(directory.path()).charge_limit, Some(0.65));
    }

    #[test]
    fn battery_without_a_threshold_reads_as_unsupported() {
        let directory = tempfile::tempdir().unwrap();
        let battery = directory.path().join("BAT0");
        fs::create_dir_all(&battery).unwrap();
        for (file, value) in [
            ("type", "Battery\n"),
            ("present", "1\n"),
            ("capacity", "60\n"),
            ("status", "Charging\n"),
        ] {
            fs::write(battery.join(file), value).unwrap();
        }
        // A peripheral battery may expose its own threshold; it must never be
        // mistaken for the system pack.
        let pen = directory.path().join("hid-pen-battery");
        fs::create_dir_all(&pen).unwrap();
        for (file, value) in [
            ("type", "Battery\n"),
            ("present", "1\n"),
            ("capacity", "20\n"),
            ("scope", "Device\n"),
            ("charge_control_end_threshold", "70\n"),
        ] {
            fs::write(pen.join(file), value).unwrap();
        }
        assert_eq!(battery_snapshot(directory.path()).charge_limit, None);
        assert!(
            set_battery_charge_limit(directory.path(), 80).is_err(),
            "no writable attribute means no adjustable ceiling"
        );
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
        let power_epoch = AtomicU64::new(0);
        let charge_epoch = AtomicU64::new(0);
        assert!(perform(
            Command::SetVolume(f64::NAN),
            &options,
            &tx,
            &alive,
            &power_epoch,
            &charge_epoch
        )
        .is_err());
        assert!(perform(
            Command::SetOutputBrightness {
                output: "fixture-output".into(),
                value: 2.
            },
            &options,
            &tx,
            &alive,
            &power_epoch,
            &charge_epoch
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
        let power_epoch = AtomicU64::new(0);
        let charge_epoch = AtomicU64::new(0);
        assert!(perform(
            Command::Action(Action::CloseWindow { id: Some(123) }),
            &options,
            &tx,
            &alive,
            &power_epoch,
            &charge_epoch
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
    #[test]
    fn the_content_type_handler_is_the_last_token_of_the_gio_line() {
        assert_eq!(
            parse_default_handler(
                "Default application for “image/png”: feh.desktop\nRegistered applications:\n"
            )
            .as_deref(),
            Some("feh.desktop")
        );
        assert_eq!(
            parse_default_handler("No default applications for “application/octet-stream”\n"),
            None
        );
        assert_eq!(parse_default_handler(""), None);
    }
    #[test]
    fn desktop_entry_ids_resolve_direct_and_nested_files() {
        let directory = tempfile::tempdir().unwrap();
        let applications = directory.path().join("applications");
        fs::create_dir_all(applications.join("nested")).unwrap();
        fs::write(applications.join("feh.desktop"), b"[Desktop Entry]\n").unwrap();
        fs::write(
            applications.join("nested").join("thing.desktop"),
            b"[Desktop Entry]\n",
        )
        .unwrap();

        assert_eq!(
            desktop_file_in(&applications, "feh.desktop"),
            Some(applications.join("feh.desktop"))
        );
        assert_eq!(
            desktop_file_in(&applications, "nested-thing.desktop"),
            Some(applications.join("nested").join("thing.desktop"))
        );
        assert_eq!(desktop_file_in(&applications, "missing.desktop"), None);
        assert_eq!(desktop_file_in(&applications, "../escape.desktop"), None);
    }
    #[test]
    fn the_quick_previewer_only_accepts_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        let environment = Environment::new();
        assert!(
            preview_location(&directory.path().join("missing.png"), &environment).is_err(),
            "a preview never opens a path that is not there"
        );
    }

    #[test]
    fn media_art_urls_decode_to_png_thumbnails() {
        assert_eq!(percent_decode("a%20b%2Fc"), "a b/c");
        let directory = tempfile::tempdir().unwrap();
        let cover = directory.path().join("cover with space.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([20, 80, 100, 255]))
            .save(&cover)
            .unwrap();
        let url = format!("file://{}", cover.display().to_string().replace(' ', "%20"));
        let environment = Environment::new();
        let png = media_art(&url, &environment).expect("a local cover decodes");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(media_art("ftp://example.com/cover.png", &environment).is_none());
        assert!(media_art("file:///missing/cover.png", &environment).is_none());
    }

    #[test]
    fn the_default_handler_is_looked_up_in_the_xdg_data_directories() {
        let directory = tempfile::tempdir().unwrap();
        let share = directory.path().join("share");
        let applications = share.join("applications");
        fs::create_dir_all(&applications).unwrap();
        fs::write(applications.join("feh.desktop"), b"[Desktop Entry]\n").unwrap();
        let environment: Environment = [
            (
                "HOME".into(),
                directory.path().to_string_lossy().into_owned(),
            ),
            ("XDG_DATA_HOME".into(), share.to_string_lossy().into_owned()),
            ("XDG_DATA_DIRS".into(), "/nonexistent".into()),
        ]
        .into();

        assert_eq!(
            resolve_desktop_file("feh.desktop", &environment),
            Some(applications.join("feh.desktop"))
        );
        assert_eq!(resolve_desktop_file("other.desktop", &environment), None);
        assert_eq!(resolve_desktop_file("../feh.desktop", &environment), None);
    }
}
