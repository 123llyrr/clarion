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
}

impl eframe::App for NativePreview {
    fn raw_input_hook(&mut self, _context: &egui::Context, input: &mut egui::RawInput) {
        // Keep the real enabled appearance while preventing any pointer/key
        // interaction from creating a configuration edit during visual QA.
        // Native close requests live in viewport events and still work.
        input.events.clear();
        input.dropped_files.clear();
        input.hovered_files.clear();
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
    let _environment = IsolatedEnvironment::new();
    let (directory, mut app, _) = fixture(tab);
    let config_path = directory.path().join("config.kdl");
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
        "CLARION_QA_PREVIEW pid={} page={page} width={width} dark={dark} lifetime_secs=15",
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
