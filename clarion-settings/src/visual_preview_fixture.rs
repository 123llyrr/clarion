//! An opt-in native preview of the real application, isolated from the desktop.
//! Run this exact ignored test alone. The window closes after fifteen seconds.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use winit::platform::wayland::EventLoopBuilderExtWayland;

use super::*;

/// The app's test constructor also creates its appearance watcher. Redirect its
/// lookup before construction so even that read stays inside the fixture.
struct IsolatedEnvironment {
    previous: Vec<(&'static str, Option<OsString>)>,
    _directory: tempfile::TempDir,
}

impl IsolatedEnvironment {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("create preview environment");
        let values = [
            ("XDG_CONFIG_HOME", directory.path().to_path_buf()),
            ("CLARION_CONFIG", directory.path().join("unused-config.kdl")),
            ("REALAND_CONFIG", directory.path().join("unused-config.kdl")),
        ];
        let previous = values
            .iter()
            .map(|(name, _)| (*name, std::env::var_os(name)))
            .collect();
        // This ignored fixture must be invoked with --exact --test-threads=1;
        // it does not run alongside the ordinary unit tests.
        for (name, value) in values {
            std::env::set_var(name, value);
        }
        Self {
            previous,
            _directory: directory,
        }
    }
}

impl Drop for IsolatedEnvironment {
    fn drop(&mut self) {
        for (name, value) in &self.previous {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

struct NativePreview {
    app: ClarionSettingsApp,
    started: Instant,
    scroll: f32,
    scroll_sent: bool,
}

fn synthetic_output(name: &str, active: bool) -> clarion_ipc::Output {
    clarion_ipc::Output {
        name: name.to_owned(),
        make: "Clarion QA".to_owned(),
        model: if active {
            "Synthetic active display"
        } else {
            "Synthetic disabled display"
        }
        .to_owned(),
        serial: Some(format!("QA-NOT-HARDWARE-{name}")),
        physical_size: Some((600, 340)),
        modes: vec![
            clarion_ipc::Mode {
                width: 3840,
                height: 2160,
                refresh_rate: 60_000,
                is_preferred: true,
            },
            clarion_ipc::Mode {
                width: 3840,
                height: 2160,
                refresh_rate: 120_000,
                is_preferred: false,
            },
            clarion_ipc::Mode {
                width: 2560,
                height: 1440,
                refresh_rate: 144_000,
                is_preferred: false,
            },
        ],
        current_mode: active.then_some(0),
        is_custom_mode: false,
        vrr_supported: true,
        vrr_enabled: active,
        logical: active.then_some(clarion_ipc::LogicalOutput {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 2.,
            transform: clarion_ipc::Transform::Normal,
        }),
        max_bpc: Some(10),
        hdr_active: active,
        hdr_supported: true,
        hdr_status: None,
        hdr_peak_luminance: Some(1000.),
        sdr_white_level: Some(203.),
        hdr_brightness: Some(100.),
    }
}

impl eframe::App for NativePreview {
    fn raw_input_hook(&mut self, _context: &egui::Context, input: &mut egui::RawInput) {
        // Keep the real enabled appearance while preventing any pointer/key
        // interaction from creating a configuration edit during visual QA.
        // Native close requests live in viewport events and still work.
        input.events.clear();
        input.dropped_files.clear();
        input.hovered_files.clear();
        if self.scroll > 0.
            && !self.scroll_sent
            && self.started.elapsed() >= Duration::from_millis(400)
        {
            input
                .events
                .push(egui::Event::PointerMoved(egui::pos2(25., 300.)));
            input.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0., -self.scroll),
                modifiers: egui::Modifiers::NONE,
            });
            self.scroll_sent = true;
        }
    }

    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.last_ipc_poll = Instant::now();
        self.app.update_ui(context);
        if self.started.elapsed() >= Duration::from_secs(15) {
            self.app.allow_close = true;
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        context.request_repaint_after(Duration::from_millis(16));
    }
}

#[test]
#[ignore = "native visual QA only; run this exact test alone on a Wayland desktop"]
fn native_common_preferences_preview() {
    let page = std::env::var("CLARION_QA_PAGE").unwrap_or_else(|_| "appearance".to_owned());
    let tab = match page.as_str() {
        "appearance" => ActiveTab::Appearance,
        "layout" => ActiveTab::Layout,
        "keyboard" => ActiveTab::InputHotCorners,
        "trackpad" => ActiveTab::Trackpad,
        "outputs" => ActiveTab::Outputs,
        other => panic!("unknown CLARION_QA_PAGE: {other}"),
    };
    let width = match std::env::var("CLARION_QA_WIDTH").as_deref() {
        Ok("480") => 480.,
        Ok("700") | Err(_) => 700.,
        Ok("900") => 900.,
        Ok(other) => panic!("CLARION_QA_WIDTH must be 480, 700 or 900: {other}"),
    };
    let dark = match std::env::var("CLARION_QA_DARK").as_deref() {
        Ok("1") => true,
        Ok("0") | Err(_) => false,
        Ok(other) => panic!("CLARION_QA_DARK must be 0 or 1: {other}"),
    };
    let scroll = std::env::var("CLARION_QA_SCROLL")
        .map(|value| {
            value
                .parse::<f32>()
                .expect("CLARION_QA_SCROLL must be a number")
        })
        .unwrap_or(0.);
    assert!(
        scroll.is_finite() && (0.0..=2000.0).contains(&scroll),
        "CLARION_QA_SCROLL must be between 0 and 2000 points"
    );
    let _environment = IsolatedEnvironment::new();
    let (directory, mut app, _) = fixture(tab);
    let config_path = directory.path().join("config.kdl");
    if tab == ActiveTab::Outputs {
        // Synthetic names cannot match hardware. The marker and every config
        // byte below are checked again when this isolated preview exits.
        std::fs::write(&config_path, format!("{SOURCE}output \"CLARION-QA-DP-1\" {{ hdr; scale 2; mode \"3840x2160@60\"; variable-refresh-rate on-demand=true; }}\noutput \"CLARION-QA-DP-2\" {{ off; }}\n")).expect("write synthetic display fixture");
        app.config_mgr
            .reload_from_disk()
            .expect("read synthetic display fixture");
        app.cached_outputs = vec![
            synthetic_output("CLARION-QA-DP-1", true),
            synthetic_output("CLARION-QA-DP-2", false),
        ];
        app.selected_output = Some("CLARION-QA-DP-1".to_owned());
    }
    let original_config = std::fs::read(&config_path).expect("read isolated preview config");
    let no_socket = directory.path().join("no-desktop-ipc.sock");
    app.ipc = crate::ipc::IpcClient::disconnected_for_test();
    app.last_ipc_poll = Instant::now();
    app.focus_pending = false;
    app.toggle_flag = None;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Clarion Settings QA")
            .with_app_id("clarion-settings-qa")
            .with_inner_size(egui::vec2(width, 720.))
            .with_min_inner_size(egui::vec2(480., 360.))
            .with_decorations(false)
            .with_transparent(false),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_wayland().with_any_thread(true);
        })),
        ..Default::default()
    };
    eprintln!(
        "CLARION_QA_PREVIEW pid={} page={page} width={width} dark={dark} scroll={scroll} lifetime_secs=15",
        std::process::id()
    );
    eframe::run_native(
        "Clarion Settings QA",
        options,
        Box::new(move |creation| {
            crate::theme::setup_custom_fonts_and_styles(&creation.egui_ctx, dark);
            app.common = CommonPreferencesState::new(creation.egui_ctx.clone());
            app.common.worker = crate::autosave::AutoSave::with_socket_for_test(
                creation.egui_ctx.clone(),
                no_socket,
            );
            Ok(Box::new(NativePreview {
                app,
                started: Instant::now(),
                scroll,
                scroll_sent: false,
            }))
        }),
    )
    .expect("run native isolated preview");
    assert_eq!(
        std::fs::read(&config_path).expect("reread isolated preview config"),
        original_config,
        "visual QA must not modify even its isolated configuration"
    );
    assert!(!config_path.with_extension("kdl.bak").exists());
}
