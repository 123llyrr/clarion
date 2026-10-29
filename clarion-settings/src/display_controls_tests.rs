use std::sync::{Arc, Mutex};

use egui::{Event, Key, Modifiers, Pos2, Rect, Shape};

use super::*;

const SOURCE: &str =
    "// display fixture\nlayout { gaps 8; }\noutput \"DP-QA-1\" { scale 1; hdr; }\n";

fn output(name: &str, x: i32) -> Output {
    serde_json::from_value(serde_json::json!({
        "name":name,"make":"Clarion QA","model":"Mock","serial":null,"physical_size":[600,340],
        "modes":[{"width":1920,"height":1080,"refresh_rate":60000,"is_preferred":true}],
        "current_mode":0,"is_custom_mode":false,"vrr_supported":true,"vrr_enabled":false,
        "hdr_supported":true,"hdr_active":true,"hdr_status":null,
        "logical":{"x":x,"y":0,"width":1920,"height":1080,"scale":1.,"transform":"Normal"}
    }))
    .unwrap()
}

fn fixture() -> (tempfile::TempDir, ClarionSettingsApp, egui::Context) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.kdl");
    std::fs::write(&path, SOURCE).unwrap();
    let mut app = super::super::tests::app();
    app.config_mgr = ConfigManager::from_path(&path).unwrap();
    app.active_tab = ActiveTab::Outputs;
    app.cached_outputs = vec![output("DP-QA-1", 0), output("DP-QA-2", 1920)];
    app.selected_output = Some("DP-QA-1".into());
    let context = egui::Context::default();
    crate::theme::setup_custom_fonts_and_styles(&context, false);
    (directory, app, context)
}

fn raw(events: Vec<Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(700., 2600.))),
        events,
        ..Default::default()
    }
}

fn frame(
    app: &mut ClarionSettingsApp,
    context: &egui::Context,
    events: Vec<Event>,
) -> egui::FullOutput {
    context.run(raw(events), |context| app.update_ui(context))
}

fn pointer(position: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn shapes<'a>(shape: &'a Shape, visit: &mut impl FnMut(&'a Shape)) {
    visit(shape);
    if let Shape::Vec(children) = shape {
        for child in children {
            shapes(child, visit);
        }
    }
}

fn text(output: &egui::FullOutput, wanted: &str) -> Vec<Rect> {
    let mut result = Vec::new();
    for clipped in &output.shapes {
        shapes(&clipped.shape, &mut |shape| {
            if let Shape::Text(text) = shape {
                if text.galley.text() == wanted {
                    result.push(Rect::from_min_size(text.pos, text.galley.size()));
                }
            }
        });
    }
    result
}

fn script(
    app: &mut ClarionSettingsApp,
    fail_reload: bool,
) -> Arc<Mutex<Vec<clarion_ipc::Request>>> {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let record = requests.clone();
    app.ipc = IpcClient::scripted_for_test(move |request| {
        record.lock().unwrap().push(request.clone());
        match request {
            clarion_ipc::Request::PreviewOutputConfig { .. } => {
                Ok(clarion_ipc::Response::OutputPreview {
                    id: 71,
                    timeout_ms: 15_000,
                })
            }
            clarion_ipc::Request::Action(clarion_ipc::Action::LoadConfigFile { .. })
                if fail_reload =>
            {
                Err("fixture reload failure".into())
            }
            _ => Ok(clarion_ipc::Response::Handled),
        }
    });
    requests
}

fn poll(app: &mut ClarionSettingsApp, context: &egui::Context, ready: bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while if ready {
        app.config_mgr.preview_seconds().is_none()
    } else {
        app.config_mgr.preview_pending()
    } {
        app.poll_display_preview(context);
        std::thread::sleep(Duration::from_millis(2));
        assert!(
            Instant::now() < deadline,
            "preview did not reach the requested phase"
        );
    }
}

#[test]
fn direct_hdr_switch_changes_only_the_display_model_before_debounce() {
    let (directory, mut app, context) = fixture();
    frame(&mut app, &context, vec![]);
    let output = frame(&mut app, &context, vec![]);
    let label = text(&output, "高动态范围（HDR）")[0];
    let mut switches = Vec::new();
    for clipped in &output.shapes {
        shapes(&clipped.shape, &mut |shape| {
            if let Shape::Rect(rect) = shape {
                if (rect.rect.width() - 42.).abs() < 0.1
                    && (rect.rect.height() - 24.).abs() < 0.1
                    && rect.rect.center().y >= label.top()
                    && rect.rect.center().y < label.bottom() + 40.
                {
                    switches.push(rect.rect.center());
                }
            }
        });
    }
    assert_eq!(switches.len(), 1);
    let position = switches[0];
    frame(
        &mut app,
        &context,
        vec![Event::PointerMoved(position), pointer(position, true)],
    );
    frame(&mut app, &context, vec![pointer(position, false)]);
    assert!(!app.display.settings.as_ref().unwrap().hdr);
    assert!(app.display.pending());
    assert!(!app.config_mgr.is_dirty());
    assert!(!app.config_mgr.preview_pending());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
        SOURCE
    );
    assert!(!directory.path().join("config.kdl.bak").exists());
}

#[test]
fn direct_preview_confirm_saves_and_cancel_restores_controls_without_writing() {
    for keep in [false, true] {
        let (directory, mut app, context) = fixture();
        let requests = script(&mut app, false);
        frame(&mut app, &context, vec![]);
        app.display.settings.as_mut().unwrap().scale = Some(1.5);
        app.start_display_controls_preview();
        assert!(app.display.owns_preview());
        assert_eq!(
            std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
            SOURCE
        );
        poll(&mut app, &context, true);
        app.config_mgr.finish_preview(&app.ipc, keep).unwrap();
        poll(&mut app, &context, false);
        assert!(!app.display.owns_preview());
        assert!(!app.config_mgr.is_dirty());
        assert!(app.display.retry.is_none());
        let disk = std::fs::read_to_string(directory.path().join("config.kdl")).unwrap();
        assert_eq!(disk != SOURCE, keep);
        assert_eq!(directory.path().join("config.kdl.bak").exists(), keep);
        let requests = requests.lock().unwrap();
        assert!(requests
            .iter()
            .any(|r| matches!(r, clarion_ipc::Request::PreviewOutputConfig { .. })));
        assert_eq!(
            requests.iter().any(|r| matches!(
                r,
                clarion_ipc::Request::Action(clarion_ipc::Action::LoadConfigFile { .. })
            )),
            keep
        );
    }
}

#[test]
fn failed_preview_keeps_manual_drafts_and_refreshes_an_external_change() {
    let (directory, mut app, context) = fixture();
    frame(&mut app, &context, vec![]);
    app.display.settings.as_mut().unwrap().scale = Some(1.5);
    let draft = SOURCE.replace("gaps 8", "gaps 25");
    app.config_mgr.set_raw_content(draft.clone());
    app.start_display_controls_preview();
    assert_eq!(app.config_mgr.raw_content(), draft);
    assert!(app.config_mgr.is_dirty());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
        SOURCE
    );

    let (_directory, mut app, context) = fixture();
    frame(&mut app, &context, vec![]);
    app.display.settings.as_mut().unwrap().scale = Some(1.5);
    let path = app.config_mgr.root_path().to_owned();
    let external = SOURCE.replace("gaps 8", "gaps 19");
    std::fs::write(&path, &external).unwrap();
    app.start_display_controls_preview();
    assert_eq!(app.config_mgr.raw_content(), external);
    assert!(app.display.retry.is_some());
    assert!(!app.config_mgr.is_dirty());
    assert!(!path.with_extension("kdl.bak").exists());
}

#[test]
fn saved_but_failed_reload_does_not_leave_a_stale_display_retry_lock() {
    let (_directory, mut app, context) = fixture();
    let requests = script(&mut app, false);
    frame(&mut app, &context, vec![]);
    app.display.settings.as_mut().unwrap().scale = Some(1.5);
    app.start_display_controls_preview();
    poll(&mut app, &context, true);
    app.config_mgr.finish_preview(&app.ipc, true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| matches!(request, clarion_ipc::Request::ConfirmOutputPreview { .. }))
    {
        std::thread::sleep(Duration::from_millis(2));
        assert!(Instant::now() < deadline);
    }
    // The confirmation reply remains available, but the subsequent reload
    // notification can no longer be sent on the disconnected worker.
    app.ipc = IpcClient::disconnected_for_test();
    poll(&mut app, &context, false);
    assert!(!app.display.owns_preview());
    assert!(!app.display.selection_locked());
    assert!(matches!(app.display.status, DisplayStatus::Error(_)));
    assert_eq!(
        OutputSettings::read(
            &app.config_mgr,
            &OutputName::from_ipc_output(&app.cached_outputs[0])
        )
        .unwrap()
        .scale,
        Some(1.5)
    );
}

#[test]
fn pending_display_edits_lock_the_arrangement_and_survive_hot_unplug() {
    let (_directory, mut app, context) = fixture();
    app.output_section = 1;
    frame(&mut app, &context, vec![]);
    app.display.settings.as_mut().unwrap().scale = Some(1.5);
    app.display.due = Some(Instant::now() + Duration::from_secs(10));
    let output = frame(&mut app, &context, vec![]);
    let position = text(&output, "DP-QA-2")
        .into_iter()
        .max_by(|a, b| a.center().y.total_cmp(&b.center().y))
        .unwrap()
        .center();
    frame(
        &mut app,
        &context,
        vec![Event::PointerMoved(position), pointer(position, true)],
    );
    frame(&mut app, &context, vec![pointer(position, false)]);
    assert_eq!(app.selected_output.as_deref(), Some("DP-QA-1"));
    assert_eq!(app.display.target.as_ref().unwrap().connector, "DP-QA-1");
    app.cached_outputs.remove(0);
    frame(&mut app, &context, vec![]);
    assert_eq!(app.selected_output.as_deref(), Some("DP-QA-1"));
    assert!(app.display.pending());
    assert_eq!(app.display.settings.as_ref().unwrap().scale, Some(1.5));
}

#[test]
fn explicit_display_save_commits_numeric_text_after_deferring_pending_preview() {
    for closing in [false, true] {
        let (directory, mut app, context) = fixture();
        script(&mut app, false);
        frame(&mut app, &context, vec![]);
        let output = frame(&mut app, &context, vec![]);
        let position = text(&output, "203 尼特")[0].center();
        frame(
            &mut app,
            &context,
            vec![Event::PointerMoved(position), pointer(position, true)],
        );
        frame(&mut app, &context, vec![pointer(position, false)]);
        let output = frame(&mut app, &context, vec![]);
        assert!(output.platform_output.ime.is_some());
        frame(&mut app, &context, vec![key(Key::A, Modifiers::CTRL)]);
        app.display.settings.as_mut().unwrap().scale = Some(1.5);
        app.display.due = Some(Instant::now() - Duration::from_millis(1));
        frame(&mut app, &context, vec![Event::Text("250".into())]);
        assert!(!app.config_mgr.preview_pending());
        assert_eq!(app.display.settings.as_ref().unwrap().sdr_white_level, None);
        let mut input = if closing {
            raw(vec![])
        } else {
            raw(vec![key(Key::S, Modifiers::CTRL)])
        };
        if closing {
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .events
                .push(egui::ViewportEvent::Close);
        }
        let output = context.run(input, |context| app.update_ui(context));
        assert!(app.config_mgr.preview_pending());
        assert!(app.display.owns_preview());
        assert_eq!(
            app.display
                .retry
                .as_ref()
                .unwrap()
                .target
                .as_ref()
                .unwrap()
                .2
                .sdr_white_level,
            Some(250.)
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
            SOURCE
        );
        assert!(text(&output, "保存并应用").is_empty());
        if closing {
            assert!(app.close_after_preview);
            assert!(output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::CancelClose));
        }
    }
}

#[test]
fn closing_a_preview_with_a_disconnected_worker_releases_the_owned_draft() {
    let (directory, mut app, context) = fixture();
    script(&mut app, false);
    frame(&mut app, &context, vec![]);
    app.display.settings.as_mut().unwrap().scale = Some(1.5);
    app.start_display_controls_preview();
    poll(&mut app, &context, true);
    app.ipc = IpcClient::disconnected_for_test();
    let mut input = raw(vec![]);
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    let output = context.run(input, |context| app.close_guard(context));
    assert!(output.viewport_output[&egui::ViewportId::ROOT]
        .commands
        .contains(&egui::ViewportCommand::CancelClose));
    assert!(!app.display.owns_preview());
    assert!(!app.config_mgr.preview_pending());
    assert!(!app.config_mgr.is_dirty());
    assert!(app.display.retry.is_some());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
        SOURCE
    );
}

#[test]
fn the_last_active_display_cannot_be_disabled_by_the_direct_preview_path() {
    let (directory, mut app, context) = fixture();
    let requests = script(&mut app, false);
    app.cached_outputs.truncate(1);
    frame(&mut app, &context, vec![]);
    app.display.settings.as_mut().unwrap().enabled = false;
    app.start_display_controls_preview();
    assert!(!app.config_mgr.preview_pending());
    assert!(!app.config_mgr.is_dirty());
    assert!(matches!(app.display.status, DisplayStatus::Error(_)));
    assert!(!requests
        .lock()
        .unwrap()
        .iter()
        .any(|r| matches!(r, clarion_ipc::Request::PreviewOutputConfig { .. })));
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
        SOURCE
    );
}

fn drag(app: &mut ClarionSettingsApp, context: &egui::Context, from: Pos2, to: Pos2) {
    // The first small move starts the drag; the remaining move must then
    // follow the pointer without jumping.
    let near = from + (to - from) * 0.1;
    frame(app, context, vec![Event::PointerMoved(from)]);
    frame(app, context, vec![pointer(from, true)]);
    frame(app, context, vec![Event::PointerMoved(near)]);
    frame(app, context, vec![Event::PointerMoved(to)]);
    frame(app, context, vec![Event::PointerMoved(to)]);
    frame(app, context, vec![pointer(to, false)]);
}

#[test]
fn dragging_a_monitor_changes_only_the_arrangement_draft() {
    let (directory, mut app, context) = fixture();
    script(&mut app, false);
    app.output_section = 1;
    frame(&mut app, &context, vec![]);
    let output = frame(&mut app, &context, vec![]);
    let from = text(&output, "DP-QA-2")
        .into_iter()
        .max_by(|a, b| a.center().y.total_cmp(&b.center().y))
        .unwrap()
        .center();
    drag(
        &mut app,
        &context,
        from,
        from + egui::vec2(80., 0.),
    );
    assert!(app.display.arrangement_pending());
    assert!(!app.display.settings_pending());
    assert!(app.display.due.is_some());
    assert!(!app.display.owns_preview());
    assert!(!app.config_mgr.is_dirty());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
        SOURCE
    );
}

#[test]
fn scale_change_reflows_the_arrangement_and_pins_every_output() {
    let (_directory, mut app, context) = fixture();
    script(&mut app, false);
    frame(&mut app, &context, vec![]);
    app.display.settings.as_mut().unwrap().scale = Some(1.5);
    app.start_display_controls_preview();
    assert!(app.display.owns_preview());
    let first = OutputSettings::read(
        &app.config_mgr,
        &OutputName::from_ipc_output(&app.cached_outputs[0]),
    )
    .unwrap();
    let second = OutputSettings::read(
        &app.config_mgr,
        &OutputName::from_ipc_output(&app.cached_outputs[1]),
    )
    .unwrap();
    assert_eq!(first.scale, Some(1.5));
    assert_eq!(first.position, Some((0, 0)));
    // 1920 / 1.5 = 1280 logical pixels, placed immediately after the scaled
    // first monitor instead of keeping the old overlapping position.
    assert_eq!(second.position, Some((1280, 0)));
    assert!(app.display.arrangement_pending());
    assert_eq!(
        std::fs::read_to_string(_directory.path().join("config.kdl")).unwrap(),
        SOURCE
    );
}

#[test]
fn a_dragged_arrangement_enters_the_guarded_preview_without_touching_the_file() {
    let (directory, mut app, context) = fixture();
    script(&mut app, false);
    app.output_section = 1;
    frame(&mut app, &context, vec![]);
    let output = frame(&mut app, &context, vec![]);
    let from = text(&output, "DP-QA-2")
        .into_iter()
        .max_by(|a, b| a.center().y.total_cmp(&b.center().y))
        .unwrap()
        .center();
    drag(&mut app, &context, from, from + egui::vec2(80., 0.));
    app.display.due = Some(Instant::now());
    frame(&mut app, &context, vec![]);
    assert!(app.display.owns_preview());
    assert!(app.config_mgr.preview_pending());
    let second = OutputSettings::read(
        &app.config_mgr,
        &OutputName::from_ipc_output(&app.cached_outputs[1]),
    )
    .unwrap();
    assert_ne!(second.position, Some((1920, 0)));
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
        SOURCE
    );
}
