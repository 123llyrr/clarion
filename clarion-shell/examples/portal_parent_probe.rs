//! Private-session regression probe for GPUI's portal parent export.
//! Run with scripts/test-native-portal-parent.py, never a desktop session bus.
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use gpui::prelude::*;
use gpui::{
    div, px, rgb, size, App, AppContext, Application, Bounds, Context as UiContext, IntoElement,
    LayerShellAnchor, LayerShellKeyboardInteractivity, LayerShellLayer, LayerShellOptions,
    PathPromptOptions, Render, Window, WindowBounds, WindowKind, WindowOptions,
};
use zbus::interface;
use zbus::message::Header;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

fn guard() -> Result<PathBuf> {
    ensure!(
        std::env::var("CLARION_PORTAL_ISOLATED_TEST").as_deref() == Ok("1"),
        "run the private portal test driver"
    );
    let root = PathBuf::from(std::env::var("CLARION_PORTAL_TEST_ROOT")?).canonicalize()?;
    ensure!(
        root.starts_with(std::env::temp_dir()),
        "test root must be disposable"
    );
    let runtime = PathBuf::from(std::env::var("XDG_RUNTIME_DIR")?).canonicalize()?;
    let metadata = fs::metadata(&runtime)?;
    ensure!(
        runtime.parent() == Some(root.as_path())
            && metadata.permissions().mode() & 0o777 == 0o700
            && metadata.uid() == unsafe { libc::geteuid() },
        "unsafe private runtime"
    );
    ensure!(
        std::env::var("DBUS_SESSION_BUS_ADDRESS")?
            .starts_with(&format!("unix:path={}/", runtime.display())),
        "portal bus is not private"
    );
    ensure!(
        fs::read_to_string(root.join(".portal-test-owner"))? == root.to_string_lossy(),
        "missing private owner marker"
    );
    ensure!(
        std::env::var_os("WAYLAND_SOCKET").is_none(),
        "inherited Wayland fd forbidden"
    );
    let display = PathBuf::from(std::env::var("WAYLAND_DISPLAY")?);
    ensure!(
        display.is_absolute()
            && display.parent() == Some(runtime.as_path())
            && fs::metadata(display)?.file_type().is_socket(),
        "Wayland must use the private compositor"
    );
    Ok(root)
}

struct FakeChooser {
    records: PathBuf,
}
#[interface(name = "org.freedesktop.portal.FileChooser")]
impl FakeChooser {
    #[zbus(property)]
    fn version(&self) -> u32 {
        4
    }
    fn open_file(
        &self,
        parent: &str,
        title: &str,
        options: HashMap<String, OwnedValue>,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("missing sender".into()))?
            .to_string();
        let token = options
            .get("handle_token")
            .and_then(|value| <&str>::try_from(value).ok())
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("missing handle_token".into()))?;
        let path = OwnedObjectPath::try_from(format!(
            "/org/freedesktop/portal/desktop/request/{}/{}",
            sender.trim_start_matches(':').replace('.', "_"),
            token
        ))
        .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        let record = serde_json::json!({"parent":parent, "title": title, "sender":sender});
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.records)
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        writeln!(file, "{record}").map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        let cancel_connection = connection.clone();
        let cancel_path = path.clone();
        connection
            .executor()
            .spawn(
                async move {
                    async_io::Timer::after(Duration::from_millis(80)).await;
                    let _ = cancel_connection
                        .emit_signal(
                            Some(sender.as_str()),
                            cancel_path.as_str(),
                            "org.freedesktop.portal.Request",
                            "Response",
                            &(1u32, HashMap::<String, OwnedValue>::new()),
                        )
                        .await;
                },
                "private fake chooser cancellation",
            )
            .detach();
        Ok(path)
    }
}

fn serve_portal(root: PathBuf) -> Result<()> {
    async_io::block_on(async move {
        let _connection = zbus::connection::Builder::session()?
            .name("org.freedesktop.portal.Desktop")?
            .serve_at(
                "/org/freedesktop/portal/desktop",
                FakeChooser {
                    records: root.join("portal-requests.jsonl"),
                },
            )?
            .build()
            .await?;
        fs::write(root.join("portal-ready"), b"ready")?;
        std::future::pending::<()>().await;
        Ok(())
    })
}
struct Probe;
impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut UiContext<Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0x345a70))
    }
}
fn run_probe(root: PathBuf, layer: bool) -> Result<()> {
    let kind = if layer { "layer" } else { "toplevel" };
    let report = root.join(format!("{kind}-probe.json"));
    let report_for_ui = report.clone();
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(420.), px(240.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            kind: if layer { WindowKind::LayerShell(LayerShellOptions {
                namespace: "clarion-panel", layer: LayerShellLayer::Overlay,
                anchor: LayerShellAnchor::default(), keyboard_interactivity: LayerShellKeyboardInteractivity::Exclusive,
                keep_alive: true, ..Default::default()
            }) } else { WindowKind::Normal },
            app_id: Some("io.github.clarion.PortalProbe".into()),
            ..Default::default()
        };
        let window = cx.open_window(options, |_, cx| cx.new(|_| Probe)).unwrap();
        cx.spawn(async move |cx| {
            let result: Result<serde_json::Value> = async {
                let deadline = Instant::now() + Duration::from_secs(20);
                loop {
                    let focused = cx.update(|cx| cx.active_window().is_some_and(|active| active.window_id() == window.window_id()))?;
                    if focused { break; }
                    ensure!(Instant::now() < deadline, "private {kind} window did not receive real keyboard focus");
                    cx.background_executor().timer(Duration::from_millis(50)).await;
                }
                fs::write(root.join(format!("{kind}-focused")), b"keyboard focus confirmed")?;
                let response = cx.update(|cx| cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Private chooser probe".into()) }))?;
                let selected = response.await.context("chooser response channel closed")??;
                ensure!(selected.is_none(), "fake chooser must cancel");
                cx.update(|cx| window.update(cx, |_, window, _| window.refresh()))??;
                cx.background_executor().timer(Duration::from_millis(180)).await;
                let focused_after = cx.update(|cx| cx.active_window().is_some_and(|active| active.window_id() == window.window_id()))?;
                ensure!(focused_after, "window connection/focus was lost after chooser");
                Ok(serde_json::json!({"passed":true,"kind":kind,"focusedBefore":true,"focusedAfter":focused_after,"cancelled":true}))
            }.await;
            let record = result.unwrap_or_else(|error| serde_json::json!({"passed":false,"kind":kind,"error":format!("{error:#}")}));
            let _ = fs::write(&report_for_ui, serde_json::to_vec_pretty(&record).unwrap());
            let _ = cx.update(|cx| cx.quit());
        }).detach();
    });
    let value: serde_json::Value = serde_json::from_slice(
        &fs::read(&report).context("probe ended before reporting a portal response")?,
    )?;
    ensure!(value["passed"] == true, "portal probe failed: {value}");
    Ok(())
}
fn main() -> Result<()> {
    let root = guard()?;
    match std::env::args().nth(1).as_deref() {
        Some("--serve-portal") => serve_portal(root),
        Some("--layer") => run_probe(root, true),
        Some("--toplevel") => run_probe(root, false),
        _ => anyhow::bail!("expected --serve-portal, --layer or --toplevel"),
    }
}
