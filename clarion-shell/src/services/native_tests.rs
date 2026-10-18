//! Opt-in end-to-end protocol test. The guard runs before opening either socket.
//! Run through scripts/test-native-shell-services.py, never on a live session.
use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use futures_lite::StreamExt;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{interface, Connection, Proxy};

use super::*;

type Properties = HashMap<String, OwnedValue>;
type Layout = (i32, Properties, Vec<OwnedValue>);

fn isolated_root() -> Result<PathBuf> {
    ensure!(
        std::env::var("CLARION_CLIPBOARD_ISOLATED_TEST").as_deref() == Ok("1"),
        "run the private test driver"
    );
    let root = PathBuf::from(std::env::var("CLARION_CLIPBOARD_TEST_ROOT")?).canonicalize()?;
    ensure!(
        root.starts_with(std::env::temp_dir()),
        "fixture root must be disposable"
    );
    let runtime = PathBuf::from(std::env::var("XDG_RUNTIME_DIR")?).canonicalize()?;
    ensure!(
        runtime.parent() == Some(root.as_path()),
        "runtime is not under the private root"
    );
    let metadata = fs::metadata(&runtime)?;
    ensure!(
        metadata.permissions().mode() & 0o777 == 0o700
            && metadata.uid() == unsafe { libc::geteuid() },
        "unsafe private runtime permissions"
    );
    let display = PathBuf::from(std::env::var("WAYLAND_DISPLAY")?);
    ensure!(
        display.is_absolute()
            && display.parent() == Some(runtime.as_path())
            && fs::metadata(&display)?.file_type().is_socket(),
        "Wayland must be a private absolute socket"
    );
    ensure!(
        std::env::var_os("WAYLAND_SOCKET").is_none(),
        "inherited Wayland fd is forbidden"
    );
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS")?;
    ensure!(
        bus.starts_with(&format!("unix:path={}/", runtime.display())),
        "D-Bus must use the private runtime"
    );
    ensure!(
        fs::read_to_string(runtime.join("clipboard-test-owner"))? == root.to_string_lossy(),
        "private owner marker is missing"
    );
    let peer = PathBuf::from(std::env::var("CLARION_CLIPBOARD_PEER")?);
    ensure!(
        peer.parent() == Some(root.as_path()) && peer.is_file(),
        "clipboard peer is not private"
    );
    Ok(root)
}

#[derive(Default)]
struct Probe {
    history: Vec<HistoryEntry>,
    paused: bool,
    available: bool,
    notifications: Vec<Notification>,
    tray: Vec<TrayItem>,
    menu: Option<(String, Vec<MenuItem>)>,
    history_revision: u64,
    notification_revision: u64,
    errors: Vec<String>,
    receiver: Option<async_channel::Receiver<Event>>,
}
impl Probe {
    fn drain(&mut self) {
        while let Some(event) = self
            .receiver
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok())
        {
            match event {
                Event::History {
                    entries,
                    paused,
                    available,
                } => {
                    self.history = entries;
                    self.paused = paused;
                    self.available = available;
                    self.history_revision += 1;
                }
                Event::Notifications(items) => {
                    self.notifications = items;
                    self.notification_revision += 1;
                }
                Event::Tray(items) => self.tray = items,
                Event::TrayMenu { id, items } => self.menu = Some((id, items)),
                Event::Error(error) => {
                    eprintln!("service event: {error}");
                    self.errors.push(error);
                }
            }
        }
    }
    fn wait(&mut self, name: &str, condition: impl Fn(&Self) -> bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.drain();
            if condition(self) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!(
                    "{name}: timed out; errors={:?}, history={:?}, tray={:?}",
                    self.errors,
                    self.history,
                    self.tray
                );
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }
    fn settle(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            self.drain();
            std::thread::sleep(Duration::from_millis(10));
        }
        self.drain();
    }
    fn entry(&self, text: &str) -> Option<&HistoryEntry> {
        self.history.iter().find(|entry| entry.text == text)
    }
}
struct Producer {
    child: Child,
    requests: PathBuf,
}
impl Drop for Producer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn offer(root: &Path, name: &str, mime: &str, bytes: &[u8], flags: &[&str]) -> Result<Producer> {
    let file = root.join(format!("{name}.payload"));
    let ready = root.join(format!("{name}.ready"));
    let requests = root.join(format!("{name}.requests"));
    fs::write(&file, bytes)?;
    let mut child = Process::new(std::env::var("CLARION_CLIPBOARD_PEER")?)
        .args(["offer", "--mime", mime, "--file"])
        .arg(file)
        .arg("--ready")
        .arg(&ready)
        .arg("--requests")
        .arg(&requests)
        .args(flags)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.is_file() {
        if let Some(status) = child.try_wait()? {
            bail!("producer {name} exited: {status}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("producer {name} not ready");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(Producer { child, requests })
}
fn consume(root: &Path, name: &str, mime: &str) -> Result<(Vec<u8>, Vec<String>)> {
    let file = root.join(format!("{name}.received"));
    // Wait for the requested publish instead of accidentally consuming the
    // preceding external owner's selection before the command is dispatched.
    let required = if name == "screenshot-copy" {
        "application/x-clarion-screenshot".to_owned()
    } else {
        format!("application/x-clarion-native-owner-{}", std::process::id())
    };
    let output = Process::new(std::env::var("CLARION_CLIPBOARD_PEER")?)
        .args(["get", "--mime", mime, "--file"])
        .arg(&file)
        .arg("--require-mime")
        .arg(required)
        .stdin(Stdio::null())
        .output()?;
    ensure!(
        output.status.success(),
        "consumer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let mimes = report["mimes"]
        .as_array()
        .context("consumer MIME report")?
        .iter()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect();
    Ok((fs::read(file)?, mimes))
}
fn requested(producer: &Producer) -> bool {
    fs::metadata(&producer.requests).is_ok_and(|metadata| metadata.len() > 0)
}
fn wait_request(producer: &mut Producer) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !requested(producer) {
        ensure!(
            producer.child.try_wait()?.is_none(),
            "producer exited before receive"
        );
        ensure!(
            Instant::now() < deadline,
            "service did not request fixture data"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
fn step(steps: &mut Vec<&'static str>, name: &'static str) {
    println!("passed: {name}");
    steps.push(name);
}

#[test]
fn preview_does_not_start_native_services() {
    let (events, receiver) = async_channel::unbounded();
    let handle = start(events, true);
    let mut count = 0;
    while let Ok(event) = receiver.try_recv() {
        match event {
            Event::History {
                entries,
                paused,
                available,
            } => {
                assert!(entries.is_empty());
                assert!(!paused);
                assert!(!available);
            }
            Event::Notifications(items) => assert!(items.is_empty()),
            Event::Tray(items) => assert!(items.is_empty()),
            other => panic!("unexpected preview event: {other:?}"),
        }
        count += 1;
    }
    assert_eq!(count, 3);
    handle.send(Command::Copy("never-published".into()));
    assert!(receiver.try_recv().is_err());
}

#[test]
#[ignore = "requires the private nested compositor, bus and guarded C peer driver"]
fn isolated_native_protocols() -> Result<()> {
    let root = isolated_root()?;
    let (events, receiver) = async_channel::unbounded();
    let handle = start(events, false);
    let mut probe = Probe {
        receiver: Some(receiver),
        ..Probe::default()
    };
    let mut steps = Vec::new();
    probe.wait("data-control available", |state| state.available)?;
    let text = "Clarion 原生剪贴板\nhttps://example.test/中文";
    let _text = offer(
        &root,
        "text",
        "text/plain;charset=utf-8",
        text.as_bytes(),
        &[],
    )?;
    probe.wait("UTF-8 capture", |state| state.entry(text).is_some())?;
    let id = probe.entry(text).unwrap().id.clone();
    handle.send(Command::Copy(id.clone()));
    let (received, mimes) = consume(&root, "text-copy", "text/plain")?;
    ensure!(
        received == text.as_bytes() && mimes.contains(&"text/plain;charset=utf-8".into()),
        "text publish changed payload or MIME"
    );
    step(&mut steps, "UTF-8 capture and real selection ownership");
    handle.send(Command::Pin(id.clone()));
    probe.wait("pin", |state| {
        state.entry(text).is_some_and(|entry| entry.pinned)
    })?;
    let revision = probe.history_revision;
    handle.send(Command::Clear);
    probe.wait("clear preserves pinned", |state| {
        state.history_revision > revision && state.history.len() == 1 && state.history[0].pinned
    })?;
    handle.send(Command::Delete(id.clone()));
    probe.wait("delete pinned", |state| state.history.is_empty())?;
    let _again = offer(&root, "same-text", "text/plain", text.as_bytes(), &[])?;
    probe.wait("same text after delete", |state| {
        state.entry(text).is_some()
    })?;
    handle.send(Command::Clear);
    probe.wait("clear", |state| state.history.is_empty())?;
    let _again_clear = offer(&root, "same-text-clear", "text/plain", text.as_bytes(), &[])?;
    probe.wait("same text after clear", |state| state.entry(text).is_some())?;
    step(&mut steps, "pin, delete, clear and same-content recapture");

    handle.send(Command::Pause(true));
    probe.wait("pause", |state| state.paused)?;
    let paused = offer(
        &root,
        "paused",
        "text/plain",
        b"paused must not enter history",
        &[],
    )?;
    probe.settle(Duration::from_millis(250));
    ensure!(
        !requested(&paused) && probe.history.len() == 1,
        "paused content was read"
    );
    handle.send(Command::Pause(false));
    probe.wait("resume", |state| !state.paused)?;
    probe.settle(Duration::from_millis(150));
    ensure!(
        !requested(&paused) && probe.history.len() == 1,
        "resume captured paused selection"
    );
    handle.send(Command::Pause(true));
    probe.wait("pause before publish", |state| state.paused)?;
    handle.send(Command::Copy(id.clone()));
    ensure!(
        consume(&root, "copy-while-paused", "text/plain")?.0 == text.as_bytes(),
        "pause destroyed publishing"
    );
    handle.send(Command::Pause(false));
    probe.wait("resume after publish", |state| !state.paused)?;
    step(
        &mut steps,
        "pause skips acquisition and preserves publishing ownership",
    );

    for (name, flags) in [
        ("sensitive", vec!["--sensitive"]),
        ("primary", vec!["--primary"]),
    ] {
        let producer = offer(
            &root,
            name,
            "text/plain",
            b"private must not be requested",
            &flags,
        )?;
        probe.settle(Duration::from_millis(300));
        ensure!(
            !requested(&producer) && probe.history.len() == 1,
            "{name} selection was read"
        );
    }
    let _unsupported = offer(
        &root,
        "unsupported",
        "application/octet-stream",
        b"unhandled",
        &[],
    )?;
    probe.settle(Duration::from_millis(150));
    ensure!(!requested(&_unsupported), "unsupported MIME was read");
    step(&mut steps, "sensitive MIME and PRIMARY never requested");

    let mut delayed = offer(
        &root,
        "delayed-clear",
        "text/plain",
        b"late capture after clear",
        &["--delay-ms", "700"],
    )?;
    wait_request(&mut delayed)?;
    handle.send(Command::Clear);
    probe.wait("clear cancels capture", |state| state.history.is_empty())?;
    probe.settle(Duration::from_millis(850));
    ensure!(
        probe.history.is_empty(),
        "clear resurrected a stale in-flight selection"
    );
    let _text_before_delete = offer(
        &root,
        "text-before-delete",
        "text/plain",
        text.as_bytes(),
        &[],
    )?;
    probe.wait("entry before delayed delete", |state| {
        state.entry(text).is_some()
    })?;
    let mut delayed_delete = offer(
        &root,
        "delayed-delete",
        "text/plain",
        text.as_bytes(),
        &["--delay-ms", "700"],
    )?;
    wait_request(&mut delayed_delete)?;
    handle.send(Command::Delete(id));
    probe.wait("delete cancels capture", |state| state.history.is_empty())?;
    probe.settle(Duration::from_millis(850));
    ensure!(
        probe.history.is_empty(),
        "delete resurrected an in-flight duplicate"
    );
    let mut delayed_pause = offer(
        &root,
        "delayed-pause",
        "text/plain",
        b"pause cancels in-flight",
        &["--delay-ms", "700"],
    )?;
    wait_request(&mut delayed_pause)?;
    handle.send(Command::Pause(true));
    probe.wait("pause cancels in-flight", |state| state.paused)?;
    probe.settle(Duration::from_millis(850));
    ensure!(
        probe.history.is_empty(),
        "pause completed a stale in-flight capture"
    );
    handle.send(Command::Pause(false));
    probe.wait("resume after in-flight", |state| !state.paused)?;
    let _oversize = offer(
        &root,
        "over-limit",
        "text/plain",
        &vec![b'x'; 1024 * 1024 + 1],
        &[],
    )?;
    probe.settle(Duration::from_millis(350));
    ensure!(probe.history.is_empty(), "oversize text entered history");
    step(
        &mut steps,
        "generation cancels in-flight clear, delete and pause; text is bounded",
    );

    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        700,
        360,
        image::Rgba([40, 120, 210, 255]),
    ))
    .write_to(&mut png, image::ImageFormat::Png)?;
    let png = png.into_inner();
    let _image = offer(&root, "image", "image/png", &png, &[])?;
    probe.wait("PNG capture", |state| {
        state
            .history
            .iter()
            .any(|entry| entry.kind == HistoryKind::Image)
    })?;
    let image_id = probe
        .history
        .iter()
        .find(|entry| entry.kind == HistoryKind::Image)
        .unwrap()
        .id
        .clone();
    let thumbnail = image::load_from_memory(
        probe.history[0]
            .thumbnail_png
            .as_ref()
            .context("missing thumbnail")?,
    )?;
    ensure!(
        thumbnail.width() <= 320 && thumbnail.height() <= 320,
        "thumbnail is not bounded"
    );
    handle.send(Command::Copy(image_id.clone()));
    ensure!(
        consume(&root, "image-copy", "image/png")?.0 == png,
        "published PNG changed"
    );
    let file = root.join("recorded-screenshot.png");
    fs::write(&file, &png)?;
    handle.send(Command::RecordScreenshot(file.clone()));
    probe.wait("screenshot upgrade", |state| {
        state.history.iter().any(|entry| {
            entry.id == image_id
                && entry.kind == HistoryKind::Screenshot
                && entry.file_path.as_ref() == Some(&file)
        })
    })?;
    ensure!(
        probe.history.len() == 1,
        "screenshot duplicate was not upgraded in place"
    );
    handle.send(Command::Copy(image_id.clone()));
    let (bytes, mimes) = consume(&root, "screenshot-copy", "image/png")?;
    ensure!(
        bytes == png && mimes.contains(&"application/x-clarion-screenshot".into()),
        "screenshot marker missing"
    );
    handle.send(Command::Delete(image_id));
    probe.wait("screenshot delete", |state| state.history.is_empty())?;
    ensure!(
        file.is_file(),
        "history delete removed screenshot source file"
    );
    let _screenshot = offer(
        &root,
        "screenshot-marker",
        "image/png",
        &png,
        &["--screenshot"],
    )?;
    probe.wait("screenshot MIME capture", |state| {
        state
            .history
            .iter()
            .any(|entry| entry.kind == HistoryKind::Screenshot)
    })?;
    step(
        &mut steps,
        "PNG thumbnails, screenshot upgrade, marker and source-file preservation",
    );

    async_io::block_on(exercise_bus(&handle, &mut probe, &mut steps))?;
    handle.send(Command::Shutdown);
    probe.wait("shutdown availability", |state| !state.available)?;
    let evidence = serde_json::json!({"passed": true, "isolation": "private nested Wayland and private D-Bus only", "steps": steps, "serviceErrors": probe.errors});
    let artifacts = PathBuf::from(std::env::var("CLARION_NATIVE_SERVICE_ARTIFACTS")?);
    fs::create_dir_all(&artifacts)?;
    fs::write(
        artifacts.join("native-services-evidence.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    Ok(())
}

#[derive(Default)]
struct FixtureState {
    activated: Vec<(i32, i32)>,
    shown: usize,
    clicked: Vec<i32>,
}
struct FixtureItem(Arc<Mutex<FixtureState>>);
#[interface(name = "org.kde.StatusNotifierItem")]
impl FixtureItem {
    fn activate(&self, x: i32, y: i32) {
        self.0.lock().unwrap().activated.push((x, y));
    }
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        "clarion-native-fixture"
    }
    #[zbus(property)]
    fn title(&self) -> &str {
        "Native tray fixture"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        "fixture-native"
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        vec![(2, 2, [255, 80, 140, 220].repeat(4))]
    }
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/NativeMenu").unwrap()
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
}
struct FixtureMenu(Arc<Mutex<FixtureState>>);
fn menu_properties(label: &str, enabled: bool) -> Properties {
    HashMap::from([
        (
            "label".into(),
            OwnedValue::try_from(Value::from(label)).unwrap(),
        ),
        ("enabled".into(), OwnedValue::from(enabled)),
    ])
}
fn menu_variant(layout: Layout) -> OwnedValue {
    OwnedValue::try_from(Value::new(layout)).unwrap()
}
#[interface(name = "com.canonical.dbusmenu")]
impl FixtureMenu {
    fn about_to_show(&self, _id: i32) -> bool {
        self.0.lock().unwrap().shown += 1;
        false
    }
    fn get_layout(&self, _parent: i32, _depth: i32, _properties: Vec<String>) -> (u32, Layout) {
        let mut toggled = menu_properties("_Pinned", true);
        toggled.insert(
            "toggle-type".into(),
            OwnedValue::try_from(Value::from("checkmark")).unwrap(),
        );
        toggled.insert("toggle-state".into(), OwnedValue::from(1i32));
        (
            1,
            (
                0,
                Properties::new(),
                vec![
                    menu_variant((
                        1,
                        menu_properties("_Open", true),
                        vec![menu_variant((2, toggled, vec![]))],
                    )),
                    menu_variant((3, menu_properties("Disabled", false), vec![])),
                ],
            ),
        )
    }
    fn event(&self, id: i32, event: &str, _data: OwnedValue, _timestamp: u32) {
        if event == "clicked" {
            self.0.lock().unwrap().clicked.push(id);
        }
    }
}
async fn notification(proxy: &Proxy<'_>, replace: u32, title: &str) -> Result<u32> {
    Ok(proxy
        .call(
            "Notify",
            &(
                "Clarion native fixture",
                replace,
                "",
                title,
                "Native protocol body",
                vec!["open", "Open"],
                Properties::new(),
                0i32,
            ),
        )
        .await?)
}
async fn exercise_bus(
    handle: &ServiceHandle,
    probe: &mut Probe,
    steps: &mut Vec<&'static str>,
) -> Result<()> {
    let connection = zbus::connection::Builder::session()?
        .method_timeout(Duration::from_secs(3))
        .build()
        .await?;
    let dbus = zbus::fdo::DBusProxy::new(&connection).await?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !dbus
        .name_has_owner("org.freedesktop.Notifications".try_into()?)
        .await?
    {
        ensure!(
            Instant::now() < deadline,
            "native notification service did not acquire name"
        );
        async_io::Timer::after(Duration::from_millis(25)).await;
    }
    let notifications = Proxy::new(
        &connection,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )
    .await?;
    let server: (String, String, String, String) =
        notifications.call("GetServerInformation", &()).await?;
    ensure!(
        server.0 == "Clarion",
        "test reached an unexpected notification service"
    );
    let id = notification(&notifications, 0, "Native notification").await?;
    probe.wait("native notification", |state| {
        state
            .notifications
            .iter()
            .any(|item| item.id == id && item.unread)
    })?;
    ensure!(
        notification(&notifications, id, "Replacement").await? == id,
        "same-owner notification replacement failed"
    );
    let other = Connection::session().await?;
    let foreign = Proxy::new(
        &other,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )
    .await?;
    let foreign_id = notification(&foreign, id, "Foreign cannot replace").await?;
    ensure!(
        foreign_id != id,
        "foreign sender replaced another notification"
    );
    let mut actions = notifications.receive_signal("ActionInvoked").await?;
    handle.send(Command::NotificationAction {
        id,
        action: "open".into(),
    });
    let signal = futures_lite::future::or(
        async { actions.next().await.context("action stream closed") },
        async {
            async_io::Timer::after(Duration::from_secs(5)).await;
            bail!("notification action signal timed out")
        },
    )
    .await?;
    let action: (u32, String) = signal.body().deserialize()?;
    ensure!(
        action == (id, "open".into()),
        "notification action returned wrong ID"
    );
    let revision = probe.notification_revision;
    handle.send(Command::Dnd(true));
    handle.send(Command::MarkRead);
    probe.wait("mark read barrier", |state| {
        state.notification_revision > revision
            && state.notifications.iter().all(|item| !item.unread)
    })?;
    let quiet = notification(&notifications, 0, "Quiet notification").await?;
    probe.wait("DND native snapshot", |state| {
        state
            .notifications
            .iter()
            .any(|item| item.id == quiet && !item.unread)
    })?;
    handle.send(Command::DismissNotification(quiet));
    probe.wait("dismiss notification", |state| {
        !state.notifications.iter().any(|item| item.id == quiet)
    })?;
    step(
        steps,
        "real D-Bus notifications, sender ownership, replacement, actions, DND and dismissal",
    );

    let fixture_state = Arc::new(Mutex::new(FixtureState::default()));
    let fixture = zbus::connection::Builder::session()?
        .method_timeout(Duration::from_secs(3))
        .name("org.kde.StatusNotifierItem.ClarionNativeFixture")?
        .serve_at("/StatusNotifierItem", FixtureItem(fixture_state.clone()))?
        .serve_at("/NativeMenu", FixtureMenu(fixture_state.clone()))?
        .build()
        .await?;
    for interface in [
        "org.kde.StatusNotifierWatcher",
        "org.freedesktop.StatusNotifierWatcher",
    ] {
        let watcher = Proxy::new(&fixture, interface, "/StatusNotifierWatcher", interface).await?;
        let service = if interface.starts_with("org.kde") {
            "org.kde.StatusNotifierItem.ClarionNativeFixture"
        } else {
            "/StatusNotifierItem"
        };
        let _: () = watcher
            .call("RegisterStatusNotifierItem", &(service,))
            .await?;
    }
    probe.wait("tray owner dedup", |state| state.tray.len() == 1)?;
    probe.settle(Duration::from_millis(250));
    ensure!(
        probe.tray.len() == 1,
        "dual watcher registrations created duplicate tray entries"
    );
    let tray = probe.tray[0].clone();
    ensure!(
        tray.title == "Native tray fixture"
            && tray.status == "Active"
            && tray.menu_path.as_deref() == Some("/NativeMenu"),
        "tray properties were not read"
    );
    let icon = image::load_from_memory(
        tray.icon_png
            .as_ref()
            .context("tray pixmap was not decoded")?,
    )?
    .into_rgba8();
    ensure!(
        icon.get_pixel(0, 0).0 == [80, 140, 220, 255],
        "ARGB pixmap decoding changed channel order"
    );
    handle.send(Command::TrayActivate {
        id: tray.id.clone(),
        x: 17,
        y: 29,
    });
    probe.wait("tray activate", |_| {
        fixture_state.lock().unwrap().activated == vec![(17, 29)]
    })?;
    handle.send(Command::GetMenu(tray.id.clone()));
    probe.wait("dbusmenu layout", |state| {
        state
            .menu
            .as_ref()
            .is_some_and(|(id, items)| id == &tray.id && items.len() == 2)
    })?;
    let menu = &probe.menu.as_ref().unwrap().1;
    ensure!(
        menu[0].label == "Open" && menu[0].children[0].toggle == Some(true) && !menu[1].enabled,
        "dbusmenu lost submenu/toggle/enabled properties"
    );
    ensure!(
        fixture_state.lock().unwrap().shown > 0,
        "AboutToShow was not called"
    );
    handle.send(Command::MenuClick {
        id: tray.id.clone(),
        item: 2,
    });
    probe.wait("dbusmenu click", |_| {
        fixture_state.lock().unwrap().clicked == vec![2]
    })?;
    handle.send(Command::MenuClick {
        id: tray.id,
        item: 3,
    });
    probe.settle(Duration::from_millis(150));
    ensure!(
        fixture_state.lock().unwrap().clicked == vec![2],
        "disabled dbusmenu item was activated"
    );
    step(
        steps,
        "SNI dual-watcher owner dedup, ARGB pixmaps, Activate and native nested D-Bus menus",
    );
    drop(fixture);
    probe.wait("tray owner disconnected", |state| state.tray.is_empty())?;
    step(steps, "tray owner disappearance removes its item");
    Ok(())
}
