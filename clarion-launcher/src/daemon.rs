use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use gpui::{
    px, size, App, AppContext, Application, Bounds, Context, Entity, LayerShellOptions,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions,
};

use crate::catalog::AppCatalog;
use crate::ipc::{Command, Event, Events, Input, Presentation, Server};
use crate::launch::launch_desktop_file;
use crate::preferences::{AppsViewState, ShellSnapshot};
use crate::ui::{self, AppsPanel, Request};

static TERMINATE: AtomicBool = AtomicBool::new(false);

pub struct Options {
    pub serve: bool,
    pub output: Option<String>,
    pub parent_pid: Option<u32>,
    pub quit_after: Option<u64>,
}

struct Host {
    catalog: Arc<AppCatalog>,
    environment: Arc<HashMap<String, String>>,
    preferences: ShellSnapshot,
    view: AppsViewState,
    events: Events,
    sender: async_channel::Sender<ui::Message>,
    view_sender: async_channel::Sender<AppsViewState>,
    window: Option<WindowHandle<AppsPanel>>,
    output: Option<String>,
    request_id: u64,
    owner: Option<u64>,
    epoch: u64,
    desired: bool,
    launching: bool,
    reload_revision: u64,
}

impl Host {
    fn emit(&self, event: Event) {
        self.events.send_to(self.owner.unwrap_or(0), event);
    }

    fn apply(&mut self, presentation: Presentation, cx: &mut Context<Self>) -> bool {
        self.request_id = presentation.request_id;
        let old_output = self.output.clone();
        if let Some(output) = presentation.output {
            self.output = Some(output);
        }
        if let Some(theme) = presentation
            .theme
            .filter(|theme| matches!(theme.as_str(), "light" | "dark" | "system"))
        {
            self.preferences.theme = theme;
        }
        if let Some(accent) = presentation.accent.filter(|accent| {
            accent.len() == 7
                && accent.starts_with('#')
                && accent.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
        }) {
            self.preferences.accent = accent;
        }
        if let Some(recent) = presentation.recent {
            let mut seen = std::collections::HashSet::new();
            self.preferences.recent_app_ids = recent
                .into_iter()
                .map(|id| id.trim().to_owned())
                .filter(|id| !id.is_empty() && id.len() <= 512 && seen.insert(id.clone()))
                .take(24)
                .collect();
        }
        self.update_preferences(cx);
        old_output != self.output
    }

    fn update_preferences(&self, cx: &mut Context<Self>) {
        if let Some(window) = self.window {
            let preferences = self.preferences.clone();
            let _ = window.update(cx, |panel, _, cx| {
                panel.update_preferences(
                    preferences.theme,
                    preferences.accent,
                    preferences.recent_app_ids,
                    cx,
                )
            });
        }
    }

    fn request_show(&mut self, presentation: Presentation, peer: u64, cx: &mut Context<Self>) {
        if self.owner.is_some_and(|owner| owner != peer) {
            self.emit(Event::Hidden {
                request_id: self.request_id,
            });
            self.drop_window(cx);
        }
        self.owner = Some(peer);
        let changed_output = self.apply(presentation, cx);
        self.desired = true;
        self.epoch = self.epoch.wrapping_add(1);
        let epoch = self.epoch;
        if changed_output {
            self.drop_window(cx);
        }
        if let Some(window) = self.window {
            if window
                .update(cx, |panel, window, cx| {
                    panel.set_epoch(epoch, cx);
                    panel.focus(window, cx);
                })
                .is_ok()
            {
                self.emit(Event::Visible {
                    request_id: self.request_id,
                });
                return;
            }
            self.window = None;
        }
        let output = self.output.clone();
        cx.spawn(async move |host, cx| {
            // Let calloop start and wait for wl_output.name/done without blocking
            // Hide or a newer Show request from the Shell.
            cx.background_executor()
                .timer(Duration::from_millis(1))
                .await;
            let deadline = Instant::now() + Duration::from_secs(2);
            let display = loop {
                let found = cx.update(|cx| resolve_output(output.as_deref(), cx));
                match found {
                    Ok(display) if output.is_none() || display.is_some() => break display,
                    Err(_) => return,
                    Ok(_) if Instant::now() >= deadline => break None,
                    _ => {
                        cx.background_executor()
                            .timer(Duration::from_millis(20))
                            .await
                    }
                }
            };
            let _ = host.update(cx, |host, cx| {
                if !host.desired || host.epoch != epoch {
                    return;
                }
                if output.is_some() && display.is_none() {
                    host.desired = false;
                    host.emit(Event::Error {
                        message: format!("找不到显示器 {}", output.as_deref().unwrap_or_default()),
                        context: "window",
                        request_id: host.request_id,
                    });
                    return;
                }
                let bounds = Bounds::centered(
                    display,
                    size(px(host.view.width as f32), px(host.view.height as f32)),
                    cx,
                );
                let catalog = host.catalog.clone();
                let view = host.view.clone();
                let preferences = host.preferences.clone();
                let sender = host.sender.clone();
                let result = cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: None,
                        kind: WindowKind::LayerShell(LayerShellOptions {
                            namespace: "clarion-launcher",
                            output: display,
                            keep_alive: true,
                            ..Default::default()
                        }),
                        display_id: display,
                        window_background: WindowBackgroundAppearance::Transparent,
                        is_movable: false,
                        is_resizable: false,
                        is_minimizable: false,
                        app_id: Some("clarion-launcher".into()),
                        ..Default::default()
                    },
                    move |window, cx| {
                        let panel = cx.new(|cx| {
                            AppsPanel::new(
                                catalog,
                                view,
                                preferences.theme,
                                preferences.accent,
                                preferences.recent_app_ids,
                                sender,
                                epoch,
                                cx,
                            )
                        });
                        panel.read(cx).focus(window, cx);
                        panel
                    },
                );
                match result {
                    Ok(window) => {
                        host.window = Some(window);
                        host.emit(Event::Visible {
                            request_id: host.request_id,
                        });
                    }
                    Err(error) => {
                        host.desired = false;
                        host.emit(Event::Error {
                            message: format!("无法显示应用面板：{error}"),
                            context: "window",
                            request_id: host.request_id,
                        });
                    }
                }
            });
        })
        .detach();
    }

    fn drop_window(&mut self, cx: &mut Context<Self>) {
        if let Some(window) = self.window.take() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
    }

    fn hide(&mut self, request_id: u64, cx: &mut Context<Self>) {
        self.request_id = request_id;
        self.desired = false;
        self.epoch = self.epoch.wrapping_add(1);
        self.drop_window(cx);
        self.emit(Event::Hidden { request_id });
    }

    fn reload(&mut self, request_id: u64, peer: u64, cx: &mut Context<Self>) {
        self.reload_revision = self.reload_revision.wrapping_add(1);
        let revision = self.reload_revision;
        let environment = self.environment.clone();
        cx.spawn(async move |host, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { AppCatalog::load(&environment).map(Arc::new) })
                .await;
            let _ = host.update(cx, |host, cx| {
                if host.reload_revision != revision {
                    return;
                }
                match result {
                    Ok(catalog) => {
                        host.catalog = catalog.clone();
                        if let Some(window) = host.window {
                            let _ =
                                window.update(cx, |panel, _, cx| panel.update_catalog(catalog, cx));
                        }
                    }
                    Err(error) => host.events.send_to(
                        peer,
                        Event::Error {
                            message: format!("无法刷新应用列表：{error}"),
                            context: "catalog",
                            request_id,
                        },
                    ),
                }
            });
        })
        .detach();
    }

    fn launch(&mut self, app_id: String, cx: &mut Context<Self>) {
        if self.launching {
            return;
        }
        let Some(app) = self.catalog.get(&app_id) else {
            self.show_error("应用已不在目录中，请刷新后重试。".into(), cx);
            return;
        };
        let path = app.desktop_file.clone();
        let environment = self.environment.clone();
        let request_id = self.request_id;
        let peer = self.owner.unwrap_or(0);
        let epoch = self.epoch;
        self.launching = true;
        cx.spawn(async move |host, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { launch_desktop_file(&path, &environment) })
                .await;
            let _ = host.update(cx, |host, cx| {
                host.launching = false;
                if let Some(window) = host.window {
                    let _ = window.update(cx, |panel, _, cx| panel.set_launching(false, cx));
                }
                match result {
                    Ok(()) => {
                        host.preferences.recent_app_ids.retain(|id| id != &app_id);
                        host.preferences.recent_app_ids.insert(0, app_id.clone());
                        host.preferences.recent_app_ids.truncate(24);
                        host.events
                            .send_to(peer, Event::Launched { app_id, request_id });
                        if host.epoch == epoch {
                            host.hide(request_id, cx);
                        } else {
                            host.update_preferences(cx);
                        }
                    }
                    Err(error) => {
                        let message = format!("应用启动失败：{error}");
                        if host.epoch == epoch {
                            host.show_error(message.clone(), cx);
                        }
                        host.events.send_to(
                            peer,
                            Event::Error {
                                message,
                                context: "launch",
                                request_id,
                            },
                        );
                    }
                }
            });
        })
        .detach();
    }

    fn show_error(&self, message: String, cx: &mut Context<Self>) {
        if let Some(window) = self.window {
            let _ = window.update(cx, |panel, _, cx| panel.report_error(message, cx));
        }
    }

    fn view_changed(&mut self, view: AppsViewState, _cx: &mut Context<Self>) {
        self.view = view.clone();
        let _ = self.view_sender.try_send(view);
    }
}

fn resolve_output(name: Option<&str>, cx: &App) -> Option<gpui::DisplayId> {
    let name = name?;
    let uuid = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, name.as_bytes());
    cx.displays()
        .into_iter()
        .find(|display| display.uuid().ok() == Some(uuid))
        .map(|display| display.id())
}

extern "C" fn signal(_signal: libc::c_int) {
    TERMINATE.store(true, Ordering::Relaxed);
}

fn configure_lifetime(parent: Option<u32>) -> Result<()> {
    unsafe {
        libc::signal(libc::SIGINT, signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, signal as *const () as libc::sighandler_t);
    }
    if let Some(parent) = parent {
        anyhow::ensure!(
            parent > 1 && unsafe { libc::getppid() } as u32 == parent,
            "Launcher parent has already exited"
        );
        anyhow::ensure!(
            unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) } == 0,
            "Cannot set launcher parent-death signal"
        );
        anyhow::ensure!(
            unsafe { libc::getppid() } as u32 == parent,
            "Launcher parent exited during startup"
        );
    }
    Ok(())
}

pub fn run(options: Options) -> Result<()> {
    let started = Instant::now();
    let trace = std::env::var("GPUI_STARTUP_TRACE").as_deref() == Ok("1");
    anyhow::ensure!(
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        "clarion-launcher requires a Wayland session with wlr-layer-shell"
    );
    let environment: Arc<HashMap<String, String>> = Arc::new(std::env::vars().collect());
    let runtime = PathBuf::from(
        environment
            .get("XDG_RUNTIME_DIR")
            .context("XDG_RUNTIME_DIR is required")?,
    );
    let Some((server, receiver)) = Server::bind(&runtime)? else {
        return Ok(());
    };
    configure_lifetime(options.parent_pid)?;
    let catalog = Arc::new(AppCatalog::load(&environment)?);
    if trace {
        eprintln!(
            "Clarion launcher startup: catalog_ms={:.3} apps={}",
            started.elapsed().as_secs_f64() * 1000.0,
            catalog.apps().len(),
        );
    }
    let preferences = ShellSnapshot::load(&environment);
    let view = AppsViewState::load(&environment);
    let events = server.events.clone();
    Application::new().run(move |cx: &mut App| {
        if trace {
            eprintln!(
                "Clarion launcher startup: application_ready_ms={:.3}",
                started.elapsed().as_secs_f64() * 1000.0,
            );
        }
        ui::bind_keys(cx);
        let (sender, ui_receiver) = async_channel::unbounded();
        let (view_sender, view_receiver) = async_channel::unbounded::<AppsViewState>();
        let view_environment = environment.clone();
        let host: Entity<Host> = cx.new(|_| Host {
            catalog,
            environment,
            preferences,
            view,
            events,
            sender,
            view_sender,
            window: None,
            output: options.output,
            request_id: 0,
            owner: None,
            epoch: 0,
            desired: false,
            launching: false,
            reload_revision: 0,
        });
        let weak = host.downgrade();
        cx.on_window_closed(move |cx| {
            let weak = weak.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |host, cx| {
                    if host
                        .window
                        .is_some_and(|window| window.update(cx, |_, _, _| ()).is_err())
                    {
                        host.window = None;
                        host.desired = false;
                        host.epoch = host.epoch.wrapping_add(1);
                        host.emit(Event::Hidden {
                            request_id: host.request_id,
                        });
                    }
                });
            });
        })
        .detach();
        let ipc_host = host.clone();
        cx.spawn(async move |cx| {
            while let Ok(input) = receiver.recv().await {
                let _ = ipc_host.update(cx, |host, cx| {
                    let (peer, command) = match input {
                        Input::Command { peer, command } => (peer, command),
                        Input::Disconnected { peer } => {
                            if host.owner == Some(peer) {
                                host.hide(host.request_id, cx);
                                host.owner = None;
                            }
                            return;
                        }
                    };
                    match command {
                        Command::Show { presentation } => host.request_show(presentation, peer, cx),
                        Command::Hide { request_id } => {
                            if host.owner == Some(peer) {
                                host.hide(request_id, cx);
                            } else {
                                host.events.send_to(peer, Event::Hidden { request_id });
                            }
                        }
                        Command::Toggle { presentation } => {
                            if host.desired && host.owner == Some(peer) {
                                host.hide(presentation.request_id, cx);
                            } else {
                                host.request_show(presentation, peer, cx);
                            }
                        }
                        Command::Preferences { presentation } => {
                            if host.owner == Some(peer) {
                                host.apply(presentation, cx);
                            }
                        }
                        Command::Reload { request_id } => host.reload(request_id, peer, cx),
                    }
                });
            }
        })
        .detach();
        // Serialize atomic writes so rapid toolbar changes cannot save an older
        // snapshot after a newer one. File I/O stays off the foreground thread.
        let view_host = host.clone();
        cx.spawn(async move |cx| {
            while let Ok(view) = view_receiver.recv().await {
                let environment = view_environment.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { view.save(&environment) })
                    .await;
                if let Err(error) = result {
                    let _ = view_host.update(cx, |host, cx| {
                        host.show_error(format!("无法保存显示设置：{error}"), cx)
                    });
                }
            }
        })
        .detach();
        let ui_host = host.clone();
        cx.spawn(async move |cx| {
            while let Ok(message) = ui_receiver.recv().await {
                let _ = ui_host.update(cx, |host, cx| {
                    if message.epoch != host.epoch {
                        return;
                    }
                    match message.request {
                        Request::Hide => host.hide(host.request_id, cx),
                        Request::Launch(id) => host.launch(id, cx),
                        Request::View(view) => host.view_changed(view, cx),
                    }
                });
            }
        })
        .detach();
        if !options.serve {
            host.update(cx, |host, cx| {
                host.request_show(Presentation::default(), 0, cx)
            });
        }
        cx.spawn(async move |cx| {
            let deadline = options
                .quit_after
                .map(|seconds| Instant::now() + Duration::from_secs(seconds));
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if TERMINATE.load(Ordering::Relaxed)
                    || deadline.is_some_and(|deadline| Instant::now() >= deadline)
                {
                    let _ = host.update(cx, |host, cx| host.hide(host.request_id, cx));
                    let _ = cx.update(|cx| cx.quit());
                    break;
                }
            }
        })
        .detach();
    });
    drop(server);
    Ok(())
}
