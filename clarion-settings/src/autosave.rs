//! One-at-a-time common-settings commits on a worker. Saving a file and the
//! compositor accepting that file are separate states; an IPC action's Handled
//! reply alone does not establish that the configuration was loaded.
#[cfg(test)]
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clarion_ipc::{Reply, Request, Response};

use crate::common_settings::CommonSettings;
use crate::config_manager::ConfigManager;

const TRANSACTION_TIMEOUT: Duration = Duration::from_secs(5);
const RELOAD_SETTLE: Duration = Duration::from_millis(120);
const MAX_LINE: usize = 16 * 1024 * 1024;

type Completion = std::result::Result<CommonSettings, String>;

pub struct AutoSave {
    context: egui::Context,
    pending: Option<Receiver<Completion>>,
    socket_path: Option<PathBuf>,
}

impl AutoSave {
    pub fn new(context: egui::Context) -> Self {
        Self {
            context,
            pending: None,
            socket_path: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_socket_for_test(context: egui::Context, path: PathBuf) -> Self {
        Self {
            context,
            pending: None,
            socket_path: Some(path),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn submit(
        &mut self,
        root_path: PathBuf,
        before: CommonSettings,
        after: CommonSettings,
    ) -> Result<()> {
        anyhow::ensure!(!self.is_busy(), "正在保存上一项设置，请稍候");
        let (sender, receiver) = mpsc::channel();
        let context = self.context.clone();
        let socket_path = self.socket_path.clone();
        std::thread::Builder::new()
            .name("settings-auto-save".into())
            .spawn(move || {
                let result = save_transaction(&root_path, &before, &after, socket_path);
                let _ = sender.send(result);
                context.request_repaint();
            })
            .context("无法启动自动保存任务，尚未保存")?;
        self.pending = Some(receiver);
        Ok(())
    }

    pub fn poll(&mut self) -> Option<Completion> {
        let receiver = self.pending.as_ref()?;
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("自动保存任务意外结束；草稿已保留，请核对配置文件后重试".into())
            }
        };
        self.pending = None;
        Some(result)
    }
}

/// All I/O stays on this worker. An explicit socket is used by isolated tests;
/// production uses the same authoritative session discovery as the IPC client.
fn save_transaction(
    root_path: &Path,
    before: &CommonSettings,
    after: &CommonSettings,
    socket_path: Option<PathBuf>,
) -> Completion {
    let deadline = Instant::now() + TRANSACTION_TIMEOUT;
    let mut committed = false;
    let result = (|| -> Result<CommonSettings> {
        let mut manager = ConfigManager::from_path(root_path)?;
        let old_sources = Sources::read(&manager)?;
        let current = CommonSettings::read(&manager)?;
        anyhow::ensure!(
            &current == before || &current == after,
            "配置已被其他程序修改，请重新载入后再调整；未覆盖外部修改"
        );
        let already_saved = &current == after;
        committed = already_saved;
        let old_outputs = manager.effective_config()?.outputs;
        CommonSettings::apply(&mut manager, &current, after)?;
        anyhow::ensure!(
            manager.effective_config()?.outputs == old_outputs,
            "常用设置不能修改显示器参数，请使用显示器页面"
        );
        old_sources.check()?;
        let expected_content = manager.raw_content().to_owned();
        let expected_sources = old_sources.with_root_contents(&expected_content);
        let path = match socket_path {
            Some(path) => path,
            None => crate::ipc::Connection::new()
                .socket_path()
                .cloned()
                .context("Clarion IPC 未连接；连接恢复后可重试")?,
        };
        let endpoint = Endpoint { path, deadline };
        // Open the independent subscription BEFORE writing, since the file
        // watcher may load the new file before our explicit reload is handled.
        let mut events = endpoint.subscribe()?;
        events.discard_pending()?;
        old_sources.check()?;
        // A previous attempt may have saved successfully before losing its
        // confirmation connection. Retrying applies that exact existing file;
        // it must neither overwrite the backup nor accept an old state event.
        if already_saved {
            committed = true;
            endpoint
                .request(Request::Action(crate::ipc::config_reload_action(
                    root_path,
                )?))?
                .handled()?;
            events.await_config_loaded()?;
            expected_sources.check()?;
            return CommonSettings::read(&ConfigManager::from_path(root_path)?);
        }
        let save = manager.save_automatic_with(|action| {
            committed = true;
            expected_sources.check()?;
            endpoint.request(Request::Action(action))?.handled()?;
            events.await_config_loaded()?;
            expected_sources.check()?;
            Ok(())
        });
        // Directory fsync failures can follow a committed rename, before the
        // reload closure was called. Preserve that distinction in the notice.
        if !committed {
            committed = !manager.is_dirty()
                && read_optional(manager.config_path())?
                    .is_some_and(|bytes| bytes == expected_content.as_bytes());
        }
        save?;
        expected_sources.check()?;
        CommonSettings::read(&ConfigManager::from_path(root_path)?)
    })();
    result.map_err(|error| {
        if committed {
            format!("配置已保存，但尚未确认应用成功：{error:#}。草稿已保留，可重新载入或重试。")
        } else {
            format!("未保存：{error:#}。草稿已保留。")
        }
    })
}

#[derive(Clone)]
struct Sources {
    root: PathBuf,
    files: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl Sources {
    fn read(manager: &ConfigManager) -> Result<Self> {
        let mut files = manager.effective_source_contents()?;
        let root = manager.root_path().to_owned();
        files.push((root.clone(), read_optional(&root)?));
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files.dedup_by(|a, b| a.0 == b.0);
        Ok(Self { root, files })
    }

    fn with_root_contents(&self, content: &str) -> Self {
        let mut expected = self.clone();
        for (path, bytes) in &mut expected.files {
            if path == &expected.root {
                *bytes = Some(content.as_bytes().to_vec());
            }
        }
        expected
    }

    fn check(&self) -> Result<()> {
        for (path, expected) in &self.files {
            anyhow::ensure!(
                &read_optional(path)? == expected,
                "配置文件 {} 已被其他程序修改，请重新载入后合并修改",
                path.display()
            );
        }
        Ok(())
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match clarion_config::read_config_file(path) {
        Ok(content) => Ok(Some(content.into_bytes())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

trait Handled {
    fn handled(self) -> Result<()>;
}
impl Handled for Response {
    fn handled(self) -> Result<()> {
        anyhow::ensure!(matches!(self, Response::Handled), "Clarion 返回了意外响应");
        Ok(())
    }
}

struct Endpoint {
    path: PathBuf,
    deadline: Instant,
}

impl Endpoint {
    fn connect(&self) -> Result<UnixStream> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(!remaining.is_zero(), "连接 Clarion 超时");
        let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
        socket.connect_timeout(
            &socket2::SockAddr::unix(&self.path)?,
            remaining.min(crate::socket_io::TIMEOUT),
        )?;
        let descriptor: std::os::fd::OwnedFd = socket.into();
        let stream = UnixStream::from(descriptor);
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(!remaining.is_zero(), "连接 Clarion 超时");
        stream.set_write_timeout(Some(remaining))?;
        Ok(stream)
    }

    fn request(&self, request: Request) -> Result<Response> {
        let mut stream = self.connect()?;
        write_request(&mut stream, &request)?;
        let line = crate::socket_io::read_line(
            &mut stream,
            MAX_LINE,
            self.deadline.saturating_duration_since(Instant::now()),
        )?;
        parse_reply(&line)
    }

    fn subscribe(&self) -> Result<Events> {
        // Reduced-motion is optional in the initial state; query it so the
        // initial burst has an explicit end even before any Shell registered.
        let reduced_motion_known = match self.request(Request::ReducedMotionState)? {
            Response::ReducedMotionState(state) => state.is_some(),
            _ => anyhow::bail!("无法读取 Clarion 初始状态"),
        };
        let mut stream = self.connect()?;
        write_request(&mut stream, &Request::EventStream)?;
        let mut events = Events {
            stream,
            pending: Vec::new(),
            deadline: self.deadline,
        };
        parse_reply(&events.line(self.deadline)?)?.handled()?;
        let mut config_seen = false;
        loop {
            let event = events.event(self.deadline)?;
            config_seen |= event.get("ConfigLoaded").is_some();
            let complete = if reduced_motion_known {
                event.get("ReducedMotionChanged").is_some()
            } else {
                event.get("CastsChanged").is_some()
            };
            if complete {
                anyhow::ensure!(config_seen, "Clarion 初始配置状态不完整，尚未保存");
                break;
            }
        }
        Ok(events)
    }
}

fn write_request(stream: &mut UnixStream, request: &Request) -> Result<()> {
    let mut bytes = serde_json::to_vec(request)?;
    bytes.push(b'\n');
    stream.write_all(&bytes)?;
    Ok(())
}

fn parse_reply(line: &[u8]) -> Result<Response> {
    match serde_json::from_slice::<Reply>(line)? {
        Ok(response) => Ok(response),
        Err(error) => anyhow::bail!("Clarion IPC: {error}"),
    }
}

struct Events {
    stream: UnixStream,
    pending: Vec<u8>,
    deadline: Instant,
}

impl Events {
    fn line(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        let mut bytes = [0; 8192];
        loop {
            if let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                anyhow::ensure!(end < MAX_LINE, "Clarion 事件过大");
                return Ok(self.pending.drain(..=end).collect());
            }
            anyhow::ensure!(self.pending.len() < MAX_LINE, "Clarion 事件过大");
            let remaining = deadline.saturating_duration_since(Instant::now());
            anyhow::ensure!(!remaining.is_zero(), "等待 Clarion 应用配置超时");
            self.stream.set_read_timeout(Some(remaining))?;
            let count = match self.stream.read(&mut bytes) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            anyhow::ensure!(count != 0, "Clarion 事件连接已断开");
            self.pending.extend_from_slice(&bytes[..count]);
        }
    }

    fn event(&mut self, deadline: Instant) -> Result<serde_json::Value> {
        Ok(serde_json::from_slice(&self.line(deadline)?)?)
    }

    fn discard_pending(&mut self) -> Result<()> {
        // Preserve packet boundaries: the socket may have delivered only half
        // of an old event. Finish that line before declaring the stream clean.
        loop {
            self.stream.set_nonblocking(true)?;
            let result = (|| -> Result<()> {
                let mut bytes = [0; 8192];
                loop {
                    anyhow::ensure!(Instant::now() < self.deadline, "读取 Clarion 状态超时");
                    match self.stream.read(&mut bytes) {
                        Ok(0) => anyhow::bail!("Clarion 事件连接已断开"),
                        Ok(count) => {
                            self.pending.extend_from_slice(&bytes[..count]);
                            if let Some(end) = self.pending.iter().rposition(|byte| *byte == b'\n')
                            {
                                self.pending.drain(..=end);
                            }
                            anyhow::ensure!(self.pending.len() < MAX_LINE, "Clarion 事件过大");
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            return Ok(())
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => return Err(error.into()),
                    }
                }
            })();
            self.stream.set_nonblocking(false)?;
            result?;
            if self.pending.is_empty() {
                return Ok(());
            }
            self.line(self.deadline)?;
        }
    }

    fn await_config_loaded(&mut self) -> Result<()> {
        let mut success_at = None;
        loop {
            let until = success_at
                .map(|at: Instant| (at + RELOAD_SETTLE).min(self.deadline))
                .unwrap_or(self.deadline);
            if success_at.is_some() && Instant::now() >= until {
                return Ok(());
            }
            let event = match self.event(until) {
                Ok(event) => event,
                Err(error)
                    if success_at.is_some()
                        && error.downcast_ref::<std::io::Error>().is_some_and(|error| {
                            matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            )
                        }) =>
                {
                    return Ok(());
                }
                Err(error) => return Err(error),
            };
            if let Some(loaded) = event.get("ConfigLoaded") {
                match loaded.get("failed").and_then(serde_json::Value::as_bool) {
                    Some(false) => {
                        success_at.get_or_insert_with(Instant::now);
                    }
                    Some(true) => anyhow::bail!("Clarion 拒绝了这份配置，请检查运行日志"),
                    None => anyhow::bail!("Clarion 配置确认事件格式异常"),
                }
            }
            if success_at.is_some_and(|at| Instant::now() >= at + RELOAD_SETTLE) {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;

    #[derive(Clone, Copy)]
    enum Reload {
        Succeeded,
        Failed,
        Disconnected,
        SucceededThenFailed,
    }

    struct Mock {
        path: PathBuf,
        stop: Arc<AtomicBool>,
        actions: Arc<AtomicUsize>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl Mock {
        fn start(
            directory: &Path,
            reload: Reload,
            mutate_include: Option<PathBuf>,
            initial_failed: bool,
        ) -> Self {
            let path = directory.join("mock.sock");
            let listener = UnixListener::bind(&path).unwrap();
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let actions = Arc::new(AtomicUsize::new(0));
            let worker_actions = actions.clone();
            let worker = std::thread::spawn(move || {
                let mut event_stream: Option<UnixStream> = None;
                while !worker_stop.load(Ordering::Relaxed) {
                    let mut stream = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(error) => panic!("mock accept failed: {error}"),
                    };
                    let line = crate::socket_io::read_line(
                        &mut stream,
                        MAX_LINE,
                        Duration::from_millis(300),
                    )
                    .unwrap();
                    match serde_json::from_slice::<Request>(&line).unwrap() {
                        Request::ReducedMotionState => {
                            stream
                                .write_all(b"{\"Ok\":{\"ReducedMotionState\":false}}\n")
                                .unwrap();
                        }
                        Request::EventStream => {
                            if let Some(include) = &mutate_include {
                                fs::write(include, "layout { gaps 19; }\n").unwrap();
                            }
                            // One packet intentionally contains the ack and
                            // every initial event. Initial ConfigLoaded must
                            // never be mistaken for our transaction's success.
                            let initial = format!(
                                "{{\"Ok\":\"Handled\"}}\n\
                                {{\"WorkspacesChanged\":{{\"workspaces\":[]}}}}\n\
                                {{\"WindowsChanged\":{{\"windows\":[]}}}}\n\
                                {{\"OverviewOpenedOrClosed\":{{\"is_open\":false}}}}\n\
                                {{\"ConfigLoaded\":{{\"failed\":{initial_failed}}}}}\n\
                                {{\"CastsChanged\":{{\"casts\":[]}}}}\n\
                                {{\"ReducedMotionChanged\":{{\"enabled\":false}}}}\n"
                            );
                            stream.write_all(initial.as_bytes()).unwrap();
                            event_stream = Some(stream);
                        }
                        Request::Action(clarion_ipc::Action::LoadConfigFile { path }) => {
                            assert!(path.is_some());
                            worker_actions.fetch_add(1, Ordering::Relaxed);
                            stream.write_all(b"{\"Ok\":\"Handled\"}\n").unwrap();
                            match reload {
                                Reload::Disconnected => {
                                    event_stream.take();
                                }
                                Reload::Succeeded | Reload::SucceededThenFailed => {
                                    let stream = event_stream.as_mut().unwrap();
                                    stream
                                        .write_all(b"{\"ConfigLoaded\":{\"failed\":false}}\n")
                                        .unwrap();
                                    if matches!(reload, Reload::SucceededThenFailed) {
                                        stream
                                            .write_all(b"{\"ConfigLoaded\":{\"failed\":true}}\n")
                                            .unwrap();
                                    }
                                }
                                Reload::Failed => {
                                    event_stream
                                        .as_mut()
                                        .unwrap()
                                        .write_all(b"{\"ConfigLoaded\":{\"failed\":true}}\n")
                                        .unwrap();
                                }
                            }
                        }
                        _ => panic!("automatic save must not request output preview"),
                    }
                }
            });
            Self {
                path,
                stop,
                actions,
                worker: Some(worker),
            }
        }
    }

    impl Drop for Mock {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.worker.take().unwrap().join().unwrap();
        }
    }

    fn draft(path: &Path) -> (CommonSettings, CommonSettings) {
        let before = CommonSettings::read(&ConfigManager::from_path(path).unwrap()).unwrap();
        let mut after = before.clone();
        after.gaps = 12.;
        (before, after)
    }

    #[test]
    fn saves_backup_and_waits_for_fresh_success_preserving_outputs() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.kdl");
        let content =
            "// retain my note\nlayout { gaps 8; }\noutput \"test-output\" { scale 2; }\n";
        fs::write(&path, content).unwrap();
        let outputs = ConfigManager::from_path(&path)
            .unwrap()
            .effective_config()
            .unwrap()
            .outputs;
        let (before, after) = draft(&path);
        let server = Mock::start(directory.path(), Reload::Succeeded, None, true);
        let applied = save_transaction(&path, &before, &after, Some(server.path.clone())).unwrap();
        assert_eq!(applied, after);
        assert_eq!(server.actions.load(Ordering::Relaxed), 1);
        assert_eq!(
            fs::read_to_string(path.with_extension("kdl.bak")).unwrap(),
            content
        );
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("// retain my note"));
        assert_eq!(
            ConfigManager::from_path(&path)
                .unwrap()
                .effective_config()
                .unwrap()
                .outputs,
            outputs
        );
    }

    #[test]
    fn rejected_reload_and_disconnect_are_saved_but_unconfirmed() {
        for reload in [
            Reload::Failed,
            Reload::Disconnected,
            Reload::SucceededThenFailed,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("config.kdl");
            fs::write(&path, "layout { gaps 8; }\n").unwrap();
            let (before, after) = draft(&path);
            let server = Mock::start(directory.path(), reload, None, false);
            let failure =
                save_transaction(&path, &before, &after, Some(server.path.clone())).unwrap_err();
            assert!(
                failure.starts_with("配置已保存，但尚未确认应用成功"),
                "{failure}"
            );
            assert_eq!(
                CommonSettings::read(&ConfigManager::from_path(&path).unwrap()).unwrap(),
                after
            );
        }
    }

    #[test]
    fn retrying_saved_but_unconfirmed_file_preserves_original_backup() {
        let directory = tempfile::tempdir().unwrap();
        let socket_one = tempfile::tempdir().unwrap();
        let socket_two = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.kdl");
        let content = "layout { gaps 8; }\n";
        fs::write(&path, content).unwrap();
        let (before, after) = draft(&path);
        let first = Mock::start(socket_one.path(), Reload::Disconnected, None, false);
        assert!(save_transaction(&path, &before, &after, Some(first.path.clone())).is_err());
        drop(first);
        let second = Mock::start(socket_two.path(), Reload::Succeeded, None, false);
        assert_eq!(
            save_transaction(&path, &before, &after, Some(second.path.clone())).unwrap(),
            after
        );
        assert_eq!(second.actions.load(Ordering::Relaxed), 1);
        assert_eq!(
            fs::read_to_string(path.with_extension("kdl.bak")).unwrap(),
            content
        );
    }

    #[test]
    fn include_changed_while_subscribing_never_overwrites_root() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.kdl");
        let include = directory.path().join("shared.kdl");
        let content = "include \"shared.kdl\"\n// root stays intact\n";
        fs::write(&path, content).unwrap();
        fs::write(&include, "layout { gaps 8; }\n").unwrap();
        let (before, after) = draft(&path);
        let server = Mock::start(
            directory.path(),
            Reload::Succeeded,
            Some(include.clone()),
            false,
        );
        let failure =
            save_transaction(&path, &before, &after, Some(server.path.clone())).unwrap_err();
        assert!(failure.starts_with("未保存"), "{failure}");
        assert!(failure.contains("已被其他程序修改"));
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
        assert_eq!(
            fs::read_to_string(include).unwrap(),
            "layout { gaps 19; }\n"
        );
        assert!(!path.with_extension("kdl.bak").exists());
        assert_eq!(server.actions.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn externally_changed_common_setting_and_missing_connection_do_not_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.kdl");
        fs::write(&path, "layout { gaps 8; }\n").unwrap();
        let (before, after) = draft(&path);
        fs::write(&path, "layout { gaps 22; }\n").unwrap();
        let absent = directory.path().join("absent.sock");
        let failure = save_transaction(&path, &before, &after, Some(absent.clone())).unwrap_err();
        assert!(failure.starts_with("未保存"));
        assert!(failure.contains("已被其他程序修改"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "layout { gaps 22; }\n");
        let (before, after) = draft(&path);
        assert!(save_transaction(&path, &before, &after, Some(absent))
            .unwrap_err()
            .starts_with("未保存"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "layout { gaps 22; }\n");
    }

    #[test]
    fn output_changes_cannot_use_automatic_write_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.kdl");
        let content = "output \"test-output\" { scale 1; }\n";
        fs::write(&path, content).unwrap();
        let mut manager = ConfigManager::from_path(&path).unwrap();
        manager.set_raw_content("output \"test-output\" { scale 2; }\n".into());
        let mut called = false;
        let result = manager.save_automatic_with(|_| {
            called = true;
            Ok(())
        });
        assert!(result.unwrap_err().to_string().contains("不能自动应用"));
        assert!(!called);
        assert_eq!(fs::read_to_string(path).unwrap(), content);
    }

    #[test]
    fn an_ack_without_a_new_config_event_is_not_success() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let deadline = Instant::now() + Duration::from_millis(50);
        let mut events = Events {
            stream: reader,
            pending: Vec::new(),
            deadline,
        };
        writer.write_all(b"{\"FutureEvent\":{}}\n").unwrap();
        assert!(events.await_config_loaded().is_err());
    }

    /// Opt-in integration against a separately launched nested compositor.
    /// The launcher owns its lifetime; this test never discovers a session or
    /// falls back to the caller's runtime socket/configuration.
    #[test]
    #[ignore = "requires an explicitly marked isolated nested winit compositor"]
    fn isolated_nested_core_confirms_save_and_revert() {
        use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

        const MARKER: &str = "clarion-settings-autosave-isolated-fixture-v1\n";
        let requested_root = PathBuf::from(
            std::env::var_os("CLARION_AUTOSAVE_TEST_ROOT")
                .expect("set CLARION_AUTOSAVE_TEST_ROOT to a private marked fixture"),
        );
        assert!(
            requested_root.is_absolute(),
            "fixture root must be absolute"
        );
        assert!(
            !requested_root.is_symlink(),
            "fixture root must not be a link"
        );
        let root = requested_root.canonicalize().unwrap();
        let metadata = fs::metadata(&root).unwrap();
        // SAFETY: geteuid has no preconditions or memory arguments.
        let uid = unsafe { libc::geteuid() };
        assert_eq!(metadata.uid(), uid, "fixture must belong to this user");
        assert_eq!(
            metadata.permissions().mode() & 0o077,
            0,
            "fixture must be private"
        );
        let marker = root.join(".settings-autosave-fixture");
        assert!(fs::symlink_metadata(&marker).unwrap().file_type().is_file());
        assert_eq!(
            fs::read_to_string(marker).unwrap(),
            MARKER,
            "wrong fixture marker"
        );

        let runtime = root.join("runtime");
        assert!(!runtime.is_symlink(), "fixture runtime must not be a link");
        assert_eq!(runtime.canonicalize().unwrap(), runtime);
        assert_eq!(fs::metadata(&runtime).unwrap().uid(), uid);
        assert_eq!(
            fs::metadata(&runtime).unwrap().permissions().mode() & 0o077,
            0
        );
        let socket = PathBuf::from(
            std::env::var_os("CLARION_SOCKET").expect("set the isolated CLARION_SOCKET explicitly"),
        )
        .canonicalize()
        .unwrap();
        assert!(
            socket.starts_with(&runtime),
            "refusing a socket outside fixture runtime"
        );
        assert!(fs::symlink_metadata(&socket)
            .unwrap()
            .file_type()
            .is_socket());
        let path = root.join("config.kdl");
        assert!(fs::symlink_metadata(&path).unwrap().file_type().is_file());
        assert_eq!(path.canonicalize().unwrap().parent(), Some(root.as_path()));
        let manager = ConfigManager::from_path(&path).unwrap();
        for (source, _) in manager.effective_source_contents().unwrap() {
            assert!(
                source.starts_with(&root),
                "fixture includes must stay inside fixture root"
            );
            if source.exists() {
                assert!(source.canonicalize().unwrap().starts_with(&root));
            }
        }
        let endpoint = Endpoint {
            path: socket.clone(),
            deadline: Instant::now() + TRANSACTION_TIMEOUT,
        };
        let outputs = match endpoint.request(Request::Outputs).unwrap() {
            Response::Outputs(outputs) => outputs,
            _ => panic!("unexpected output response"),
        };
        assert_eq!(outputs.len(), 1, "test accepts a single nested output only");
        assert!(
            outputs.contains_key("winit"),
            "test refuses physical compositor outputs"
        );
        let original = fs::read_to_string(&path).unwrap();
        let before = CommonSettings::read(&manager).unwrap();
        let mut after = before.clone();
        after.gaps = if before.gaps < 64. {
            before.gaps + 3.
        } else {
            before.gaps - 3.
        };
        let first = save_transaction(&path, &before, &after, Some(socket.clone()));
        let inspected = (|| -> Result<()> {
            let applied = first.map_err(anyhow::Error::msg)?;
            anyhow::ensure!(
                applied == after,
                "confirmed settings differ from requested values"
            );
            anyhow::ensure!(
                fs::read_to_string(path.with_extension("kdl.bak"))? == original,
                "first save did not preserve the original backup"
            );
            anyhow::ensure!(
                CommonSettings::read(&ConfigManager::from_path(&path)?)? == after,
                "temporary on-disk settings did not match the confirmed values"
            );
            anyhow::ensure!(
                ConfigManager::from_path(&path)?.effective_config()?.outputs
                    == manager.effective_config()?.outputs,
                "automatic save changed output configuration"
            );
            Ok(())
        })();
        // Attempt the inverse even if an assertion above failed after the first
        // write. The worker still rejects any unrelated intervening edit.
        let restored = save_transaction(&path, &after, &before, Some(socket.clone()));
        assert_eq!(
            restored.unwrap(),
            before,
            "failed to confirm restored settings"
        );
        assert_eq!(
            CommonSettings::read(&ConfigManager::from_path(&path).unwrap()).unwrap(),
            before
        );
        let endpoint = Endpoint {
            path: socket,
            deadline: Instant::now() + TRANSACTION_TIMEOUT,
        };
        match endpoint.request(Request::Outputs).unwrap() {
            Response::Outputs(current) => assert_eq!(
                serde_json::to_value(current).unwrap(),
                serde_json::to_value(outputs).unwrap()
            ),
            _ => panic!("unexpected output response after restore"),
        }
        inspected.unwrap();
    }

    #[test]
    fn event_line_reader_preserves_coalesced_and_fragmented_lines() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let deadline = Instant::now() + Duration::from_millis(300);
        let mut events = Events {
            stream: reader,
            pending: Vec::new(),
            deadline,
        };
        writer
            .write_all(b"{\"FutureEvent\":{}}\n{\"ConfigLoaded\":")
            .unwrap();
        assert!(events.event(deadline).unwrap().get("FutureEvent").is_some());
        writer.write_all(b"{\"failed\":false}}\n").unwrap();
        assert_eq!(
            events.event(deadline).unwrap()["ConfigLoaded"]["failed"],
            false
        );
    }
}
