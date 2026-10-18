//! Phase 0 probe. This binary does not replace the Flutter launcher until the
//! layer-shell, input and shell-composition acceptance checks have passed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    actions, div, px, rgb, rgba, size, App, Application, Bounds, Context, Entity, Focusable,
    KeyBinding, LayerShellOptions, Render, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions,
};

use crate::spike_input::SpikeInput;

pub struct Options {
    pub output: Option<String>,
    pub lifecycle_probe: bool,
    pub quit_after: Option<u64>,
}

actions!(launcher_spike, [Close]);

struct SpikePanel {
    input: Entity<SpikeInput>,
}

impl Render for SpikePanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_5().child(
            div()
                .size_full()
                .rounded(px(28.))
                .bg(rgba(0x15202ee8))
                .border_1()
                .border_color(rgba(0xffffff38))
                .shadow_lg()
                .text_color(rgb(0xf5f5fa))
                .p_8()
                .flex()
                .flex_col()
                .gap_6()
                .child(div().text_2xl().child("Apps · Clarion"))
                .child(
                    div()
                        .rounded_full()
                        .bg(rgba(0xffffff16))
                        .p_4()
                        .child(self.input.clone()),
                )
                .child(div().text_lg().child("Phase 0 · GPUI Layer Shell"))
                .child("980 × 680 · Overlay · Exclusive keyboard · text-input-v3")
                .child("输入中文以检查候选框、预编辑和提交；按 Esc 关闭。")
                .child("This is the feasibility probe. The Flutter launcher remains available."),
        )
    }
}

fn selected_output(options: &Options, cx: &App) -> Option<gpui::DisplayId> {
    let name = options.output.as_ref()?;
    let uuid = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, name.as_bytes());
    cx.displays()
        .into_iter()
        .find(|display| display.uuid().ok() == Some(uuid))
        .map(|display| display.id())
}

fn open_spike(options: &Options, cx: &mut App) -> anyhow::Result<gpui::WindowHandle<SpikePanel>> {
    // WaylandDisplay uses a stable v5 UUID derived from the output name.
    let output = selected_output(options, cx);
    anyhow::ensure!(
        options.output.is_none() || output.is_some(),
        "Wayland output {:?} was not found",
        options.output.as_deref().unwrap_or_default()
    );
    let bounds = Bounds::centered(output, size(px(980.), px(680.)), cx);
    let window = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: "clarion-launcher",
                output,
                keep_alive: options.lifecycle_probe,
                ..Default::default()
            }),
            display_id: output,
            window_background: WindowBackgroundAppearance::Transparent,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            app_id: Some("clarion-launcher".into()),
            ..Default::default()
        },
        |window, cx| {
            let input = cx.new(SpikeInput::new);
            window.focus(&input.focus_handle(cx));
            cx.new(|_| SpikePanel { input })
        },
    )?;
    Ok(window)
}

pub fn run(options: Options) -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        "clarion-launcher requires a Wayland session with wlr-layer-shell"
    );
    let failed = Arc::new(AtomicBool::new(false));
    let failed_in_app = failed.clone();
    Application::new().run(move |cx: &mut App| {
        SpikeInput::bind_keys(cx);
        cx.bind_keys([KeyBinding::new("escape", Close, None)]);
        cx.on_action(|_: &Close, cx| cx.quit());
        cx.spawn(async move |cx| {
            // A synchronous quit in on_finish_launching is lost when calloop
            // starts. Let the event loop start before any fallible work/exit.
            cx.background_executor()
                .timer(Duration::from_millis(1))
                .await;
            // GPUI initially knows registry globals, but wl_output.name/done
            // arrive later. Wait a bounded interval for the requested output.
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let ready = cx.update(|cx| {
                    options.output.is_none() || selected_output(&options, cx).is_some()
                });
                match ready {
                    Ok(true) => break,
                    Err(_) => return,
                    Ok(false) if Instant::now() >= deadline => break,
                    Ok(false) => {
                        cx.background_executor()
                            .timer(Duration::from_millis(20))
                            .await
                    }
                }
            }
            let mut window = match cx.update(|cx| open_spike(&options, cx)) {
                Ok(Ok(window)) => window,
                Ok(Err(error)) => {
                    eprintln!("clarion-launcher: {error:#}");
                    failed_in_app.store(true, Ordering::Relaxed);
                    cx.update(|cx| cx.quit()).ok();
                    return;
                }
                Err(_) => return,
            };
            if options.lifecycle_probe {
                for cycle in 1..=2 {
                    cx.background_executor()
                        .timer(Duration::from_millis(200))
                        .await;
                    if !matches!(
                        cx.update(|cx| window.update(cx, |_, window, _| window.remove_window())),
                        Ok(Ok(()))
                    ) {
                        eprintln!("clarion-launcher: lifecycle probe lost its window");
                        failed_in_app.store(true, Ordering::Relaxed);
                        cx.update(|cx| cx.quit()).ok();
                        return;
                    }
                    eprintln!("launcher-spike: last window dropped, cycle {cycle}");
                    cx.background_executor()
                        .timer(Duration::from_millis(200))
                        .await;
                    window = match cx.update(|cx| open_spike(&options, cx)) {
                        Ok(Ok(window)) => window,
                        Ok(Err(error)) => {
                            eprintln!("clarion-launcher: {error:#}");
                            failed_in_app.store(true, Ordering::Relaxed);
                            cx.update(|cx| cx.quit()).ok();
                            return;
                        }
                        Err(_) => return,
                    };
                }
            }
            if let Some(seconds) = options.quit_after {
                cx.background_executor()
                    .timer(Duration::from_secs(seconds))
                    .await;
                cx.update(|cx| cx.quit()).ok();
            }
        })
        .detach();
    });
    anyhow::ensure!(!failed.load(Ordering::Relaxed), "launcher probe failed");
    Ok(())
}
