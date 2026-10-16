//! Actual application regressions. All configuration and IPC state is isolated.
use super::*;
use eframe::egui::{Color32, Event, Id, ImeEvent, Key, Modifiers, Pos2, RawInput, Rect, Shape};

const SOURCE: &str = "// preserve this comment\nlayout { gaps 8; }\n";

fn fixture(tab: ActiveTab) -> (tempfile::TempDir, ClarionSettingsApp, egui::Context) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.kdl");
    std::fs::write(&path, SOURCE).unwrap();
    let mut app = super::tests::app();
    app.config_mgr = ConfigManager::from_path(&path).unwrap();
    app.active_tab = tab;
    let context = egui::Context::default();
    crate::theme::setup_custom_fonts_and_styles(&context);
    (directory, app, context)
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

fn input(width: f32, scale: f32, events: Vec<Event>) -> RawInput {
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 580.))),
        events,
        ..Default::default()
    };
    let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
    viewport.native_pixels_per_point = Some(scale);
    viewport.focused = Some(true);
    input
}

fn run(
    app: &mut ClarionSettingsApp,
    context: &egui::Context,
    mut input: RawInput,
) -> egui::FullOutput {
    // Exercise the same input hook ordering as eframe before the UI consumes events.
    // Avoid the production periodic polling deadline even on slow test machines.
    app.last_ipc_poll = std::time::Instant::now();
    eframe::App::raw_input_hook(app, context, &mut input);
    context.run(input, |context| app.update_ui(context))
}

fn frame(
    app: &mut ClarionSettingsApp,
    context: &egui::Context,
    events: Vec<Event>,
) -> egui::FullOutput {
    run(app, context, input(700., 1., events))
}

fn focus_search(app: &mut ClarionSettingsApp, context: &egui::Context) {
    frame(app, context, vec![]);
    frame(app, context, vec![key(Key::F, Modifiers::CTRL)]);
    frame(app, context, vec![]);
    assert!(context.memory(|memory| memory.has_focus(Id::new("preferences-search"))));
}

fn text_rects(output: &egui::FullOutput, wanted: &str) -> Vec<Rect> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.text() == wanted => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .collect()
}

fn home_center(output: &egui::FullOutput) -> Pos2 {
    // Locate the actual painted nine-dot "Show All" control, without depending
    // on auto-generated widget IDs or hardcoded screen coordinates.
    let dots: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Rect(rect)
                if rect.rect.width() == 2.5
                    && rect.rect.height() == 2.5
                    && rect.rect.top() >= 30.
                    && rect.rect.bottom() < 72. =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .collect();
    assert_eq!(dots.len(), 9, "Show All must remain visible");
    dots.into_iter().reduce(Rect::union).unwrap().center()
}

fn pointer(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn history_navigation_flushes_drafts_and_preserves_the_opposite_stack() {
    let (directory, mut app, context) = fixture(ActiveTab::FullConfig);
    app.back_stack = vec![ActiveTab::Appearance];
    app.forward_stack = vec![ActiveTab::Layout];
    app.full_config =
        crate::full_config::FullConfig::with_pending_argument_for_test(SOURCE, vec![0, 0], "24");
    frame(
        &mut app,
        &context,
        vec![key(Key::ArrowLeft, Modifiers::ALT)],
    );
    assert_eq!(app.active_tab, ActiveTab::Appearance);
    assert!(app.back_stack.is_empty());
    assert_eq!(
        app.forward_stack,
        [ActiveTab::Layout, ActiveTab::FullConfig]
    );
    assert_eq!(
        app.config_mgr.raw_content(),
        SOURCE.replace("gaps 8", "gaps 24")
    );
    assert!(!app.full_config.has_pending());
    assert!(app.config_mgr.is_dirty());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
        SOURCE
    );
    frame(
        &mut app,
        &context,
        vec![key(Key::ArrowRight, Modifiers::ALT)],
    );
    assert_eq!(app.active_tab, ActiveTab::FullConfig);
    assert_eq!(app.back_stack, [ActiveTab::Appearance]);
    assert_eq!(app.forward_stack, [ActiveTab::Layout]);
}

#[test]
fn invalid_form_blocks_both_history_directions_without_popping_entries() {
    let (directory, mut app, context) = fixture(ActiveTab::FullConfig);
    app.back_stack = vec![ActiveTab::Dashboard];
    app.forward_stack = vec![ActiveTab::Appearance];
    app.full_config = crate::full_config::FullConfig::with_pending_argument_for_test(
        SOURCE,
        vec![0, 0],
        "invalid number",
    );
    for direction in [Key::ArrowLeft, Key::ArrowRight] {
        frame(&mut app, &context, vec![key(direction, Modifiers::ALT)]);
        assert_eq!(app.active_tab, ActiveTab::FullConfig);
        assert_eq!(app.back_stack, [ActiveTab::Dashboard]);
        assert_eq!(app.forward_stack, [ActiveTab::Appearance]);
        assert!(app.full_config.has_pending());
        assert_eq!(app.config_mgr.raw_content(), SOURCE);
        assert_eq!(
            std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
            SOURCE
        );
    }
}

#[test]
fn disabled_search_does_not_consume_enter_or_navigate_under_confirmation() {
    for modal in 0..3 {
        let (_directory, mut app, context) = fixture(ActiveTab::Dashboard);
        focus_search(&mut app, &context);
        frame(&mut app, &context, vec![Event::Text("显示器".into())]);
        match modal {
            0 => app.confirm_close = true,
            1 => app.confirm_reload = true,
            _ => app.show_hot_corners = true,
        }
        frame(&mut app, &context, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(app.active_tab, ActiveTab::Dashboard, "modal {modal}");
        assert!(app.back_stack.is_empty());
        assert_eq!(app.preferences_search, "显示器");
        if modal == 2 {
            assert!(
                !app.show_hot_corners,
                "Enter should activate the sheet's Done action"
            );
        } else {
            assert!(
                context.input(|input| input.key_pressed(Key::Enter)),
                "search stole modal Enter {modal}"
            );
        }
    }
}

#[test]
fn search_ime_preedit_and_commit_enter_do_not_open_a_pane_until_next_enter() {
    let (_directory, mut app, context) = fixture(ActiveTab::Dashboard);
    focus_search(&mut app, &context);
    frame(
        &mut app,
        &context,
        vec![
            Event::Ime(ImeEvent::Enabled),
            Event::Ime(ImeEvent::Preedit("显示器".into())),
            key(Key::Enter, Modifiers::NONE),
        ],
    );
    assert_eq!(app.active_tab, ActiveTab::Dashboard);
    assert_eq!(app.preferences_search, "显示器");
    frame(
        &mut app,
        &context,
        vec![
            Event::Ime(ImeEvent::Commit("显示器".into())),
            key(Key::Enter, Modifiers::NONE),
        ],
    );
    assert_eq!(app.active_tab, ActiveTab::Dashboard);
    assert_eq!(app.preferences_search, "显示器");
    assert!(context.memory(|memory| memory.has_focus(Id::new("preferences-search"))));
    frame(&mut app, &context, vec![key(Key::Enter, Modifiers::NONE)]);
    assert_eq!(app.active_tab, ActiveTab::Outputs);
    assert_eq!(app.back_stack, [ActiveTab::Dashboard]);
    assert!(app.preferences_search.is_empty());
}

#[test]
fn arrow_selection_highlight_and_enter_open_the_same_search_result() {
    let (_directory, mut app, context) = fixture(ActiveTab::Dashboard);
    // Inspect the settled highlight color rather than the popup's fade-in alpha.
    context.style_mut(|style| style.animation_time = 0.);
    focus_search(&mut app, &context);
    frame(&mut app, &context, vec![Event::Text("快捷键".into())]);
    frame(&mut app, &context, vec![]); // settle the popup's measured size
    let output = frame(
        &mut app,
        &context,
        vec![key(Key::ArrowDown, Modifiers::NONE)],
    );
    let search = context
        .read_response(Id::new("preferences-search"))
        .unwrap()
        .rect;
    let result = text_rects(&output, "更多设置")
        .into_iter()
        .find(|rect| {
            rect.left() >= search.left() - 12.
                && rect.top() >= search.bottom()
                && rect.bottom() < search.bottom() + 100.
        })
        .expect("second result must be painted in the search popup");
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| matches!(&shape.shape, Shape::Rect(rect)
        if rect.rect.contains(result.center()) && rect.fill == crate::theme::colors::ACCENT_SOFT)),
        "ArrowDown must highlight the result it will open: selected={} result={result:?} rectangles={:?}",
        app.preferences_search_selected,
        output.shapes.iter().filter_map(|shape| match &shape.shape {
            Shape::Rect(rect) if rect.rect.contains(result.center()) => Some((rect.rect, rect.fill)),
            _ => None,
        }).collect::<Vec<_>>()
    );
    frame(&mut app, &context, vec![key(Key::Enter, Modifiers::NONE)]);
    assert_eq!(app.active_tab, ActiveTab::FullConfig);
    assert_eq!(app.back_stack, [ActiveTab::Dashboard]);
}

#[test]
fn editor_ime_commit_precedes_toolbar_pointer_sequence_and_click_runs_once() {
    for release_same_frame in [false, true] {
        let (directory, mut app, context) = fixture(ActiveTab::ConfigEditor);
        let output = frame(&mut app, &context, vec![]);
        let home = home_center(&output);
        let editor = Id::new("config-source-editor");
        context.memory_mut(|memory| memory.request_focus(editor));
        let mut state = egui::text_edit::TextEditState::load(&context, editor).unwrap();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(SOURCE.find('8').unwrap() + 1),
            )));
        state.store(&context, editor);
        frame(
            &mut app,
            &context,
            vec![
                Event::Ime(ImeEvent::Enabled),
                Event::Ime(ImeEvent::Preedit("旧".into())),
            ],
        );
        let mut events = vec![
            Event::Ime(ImeEvent::Commit("0".into())),
            Event::PointerMoved(home),
            pointer(home, true),
        ];
        if release_same_frame {
            events.push(pointer(home, false));
        }
        frame(&mut app, &context, events);
        assert_eq!(
            app.config_mgr.raw_content(),
            SOURCE.replace("gaps 8", "gaps 80")
        );
        assert_eq!(
            app.active_tab,
            ActiveTab::ConfigEditor,
            "deferred click must wait for commit"
        );
        frame(
            &mut app,
            &context,
            if release_same_frame {
                vec![]
            } else {
                vec![pointer(home, false)]
            },
        );
        assert_eq!(app.active_tab, ActiveTab::Dashboard);
        assert_eq!(app.back_stack, [ActiveTab::ConfigEditor]);
        frame(&mut app, &context, vec![]);
        assert_eq!(
            app.back_stack,
            [ActiveTab::ConfigEditor],
            "replayed click must run once"
        );
        assert_eq!(
            app.config_mgr.raw_content(),
            SOURCE.replace("gaps 8", "gaps 80")
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
            SOURCE
        );
    }
}

#[test]
fn keyboard_history_navigation_includes_text_or_ime_from_the_same_frame() {
    for ime in [false, true] {
        let (directory, mut app, context) = fixture(ActiveTab::ConfigEditor);
        app.back_stack = vec![ActiveTab::Dashboard];
        frame(&mut app, &context, vec![]);
        let editor = Id::new("config-source-editor");
        context.memory_mut(|memory| memory.request_focus(editor));
        let mut state = egui::text_edit::TextEditState::load(&context, editor).unwrap();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(SOURCE.find('8').unwrap() + 1),
            )));
        state.store(&context, editor);
        let mut events = if ime {
            vec![
                Event::Ime(ImeEvent::Enabled),
                Event::Ime(ImeEvent::Commit("0".into())),
            ]
        } else {
            vec![Event::Text("0".into())]
        };
        events.push(key(Key::ArrowLeft, Modifiers::ALT));
        frame(&mut app, &context, events);
        assert_eq!(app.active_tab, ActiveTab::Dashboard);
        assert!(app.back_stack.is_empty());
        assert_eq!(app.forward_stack, [ActiveTab::ConfigEditor]);
        assert_eq!(
            app.config_mgr.raw_content(),
            SOURCE.replace("gaps 8", "gaps 80"),
            "IME {ime}"
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
            SOURCE
        );
    }
}

#[test]
fn invalid_form_search_keeps_the_query_and_can_retry_after_correction() {
    let (directory, mut app, context) = fixture(ActiveTab::FullConfig);
    app.full_config = crate::full_config::FullConfig::with_pending_argument_for_test(
        SOURCE,
        vec![0, 0],
        "invalid number",
    );
    focus_search(&mut app, &context);
    frame(&mut app, &context, vec![Event::Text("kdl".into())]);
    frame(&mut app, &context, vec![key(Key::Enter, Modifiers::NONE)]);
    assert_eq!(app.active_tab, ActiveTab::FullConfig);
    assert!(app.back_stack.is_empty());
    assert!(app.full_config.has_pending());
    assert_eq!(app.preferences_search, "kdl");
    assert_eq!(app.config_mgr.raw_content(), SOURCE);
    app.full_config =
        crate::full_config::FullConfig::with_pending_argument_for_test(SOURCE, vec![0, 0], "24");
    frame(&mut app, &context, vec![key(Key::Enter, Modifiers::NONE)]);
    assert_eq!(app.active_tab, ActiveTab::ConfigEditor);
    assert_eq!(app.back_stack, [ActiveTab::FullConfig]);
    assert!(app.preferences_search.is_empty());
    assert_eq!(
        app.config_mgr.raw_content(),
        SOURCE.replace("gaps 8", "gaps 24")
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
        SOURCE
    );
}

#[test]
fn chrome_text_stays_inside_480_to_980_point_windows_at_fractional_dpi_without_sidebar() {
    let (_directory, mut app, context) = fixture(ActiveTab::Dashboard);
    for scale in [1., 1.25, 2.] {
        for width in [480., 700., 980.] {
            for tab in std::iter::once(ActiveTab::Dashboard)
                .chain(crate::preferences_home::PANES.iter().map(|pane| pane.tab))
            {
                app.active_tab = tab;
                run(&mut app, &context, input(width, scale, vec![]));
                let output = run(&mut app, &context, input(width, scale, vec![]));
                let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 580.));
                assert_eq!(
                    context.available_rect().left(),
                    0.,
                    "a sidebar consumed content width"
                );
                assert_eq!(context.available_rect().right(), width);
                for label in [
                    "命令中心",
                    "搜索设置",
                    "保存并应用",
                    "检查修改",
                    "当前没有未保存修改",
                ] {
                    let rects = text_rects(&output, label);
                    assert_eq!(rects.len(), 1, "{label} {tab:?} at {width}/{scale}");
                    assert!(
                        screen.contains_rect(rects[0]),
                        "{label} {tab:?} at {width}/{scale}: {:?}",
                        rects[0]
                    );
                }
                for label in ["桌面体验", "管理与高级"] {
                    assert!(text_rects(&output, label).is_empty());
                }
                let save = text_rects(&output, "保存并应用")[0];
                let check = text_rects(&output, "检查修改")[0];
                let summary = text_rects(&output, "当前没有未保存修改")[0];
                assert!(
                    !save.intersects(check)
                        && !summary.intersects(check)
                        && !summary.intersects(save),
                    "footer overlaps at {width}/{scale}"
                );
                assert!(
                    context
                        .read_response(Id::new("preferences-search"))
                        .unwrap()
                        .rect
                        .right()
                        <= width
                );
                let title = text_rects(&output, crate::preferences_home::title(tab))
                    .into_iter()
                    .find(|rect| rect.top() < 30.)
                    .expect("window title");
                assert!(screen.contains_rect(title));
            }
        }
    }
}

#[test]
fn titlebar_close_request_still_runs_unsaved_guard_on_the_native_close_frame() {
    let (directory, mut app, context) = fixture(ActiveTab::ConfigEditor);
    app.config_mgr
        .set_raw_content(SOURCE.replace("gaps 8", "gaps 24"));
    let output = frame(&mut app, &context, vec![]);
    let close = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Circle(circle)
                if circle.radius == 6. && circle.fill == Color32::from_rgb(255, 95, 87) =>
            {
                Some(circle.center)
            }
            _ => None,
        })
        .expect("red close control");
    frame(
        &mut app,
        &context,
        vec![Event::PointerMoved(close), pointer(close, true)],
    );
    let output = frame(&mut app, &context, vec![pointer(close, false)]);
    assert!(output.viewport_output[&egui::ViewportId::ROOT]
        .commands
        .contains(&egui::ViewportCommand::Close));
    let mut native = input(700., 1., vec![]);
    native
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    let output = run(&mut app, &context, native);
    assert!(output.viewport_output[&egui::ViewportId::ROOT]
        .commands
        .contains(&egui::ViewportCommand::CancelClose));
    assert!(app.confirm_close);
    assert!(app.config_mgr.is_dirty());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
        SOURCE
    );
}

#[test]
fn keyboard_sliders_edit_real_repeat_fields_and_browsing_preserves_out_of_range_values() {
    let (directory, mut app, context) = fixture(ActiveTab::InputHotCorners);
    let find_rail = |output: &egui::FullOutput, label: &str| {
        let label = text_rects(output, label)[0];
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                Shape::Rect(rect)
                    if rect.rect.width() >= 100.
                        && rect.rect.height() <= 10.
                        && rect.rect.left() >= label.left() - 2.
                        && rect.rect.left() <= label.left() + 12.
                        && rect.rect.top() >= label.bottom()
                        && rect.rect.bottom() <= label.bottom() + 40. =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .max_by(|left, right| left.width().total_cmp(&right.width()))
            .expect("visible slider rail below its label")
    };
    let initial = app.config_mgr.effective_config().unwrap().input.keyboard;
    let output = frame(&mut app, &context, vec![]);
    let rail = find_rail(&output, "按键重复");
    let rate_target = egui::pos2(rail.left() + rail.width() * 0.85, rail.center().y);
    frame(
        &mut app,
        &context,
        vec![Event::PointerMoved(rate_target), pointer(rate_target, true)],
    );
    let output = frame(&mut app, &context, vec![pointer(rate_target, false)]);
    let rail = find_rail(&output, "重复前延迟");
    let delay_target = egui::pos2(rail.left() + rail.width() * 0.2, rail.center().y);
    frame(
        &mut app,
        &context,
        vec![
            Event::PointerMoved(delay_target),
            pointer(delay_target, true),
        ],
    );
    frame(&mut app, &context, vec![pointer(delay_target, false)]);
    let rate = app.config_mgr.visual_settings().keyboard_repeat_rate;
    let delay = app.config_mgr.visual_settings().keyboard_repeat_delay;
    assert!(
        rate > initial.repeat_rate && rate <= 60,
        "rate slider did not change the model: {rate}"
    );
    assert!(
        delay > initial.repeat_delay && delay <= 1200,
        "delay slider did not change the model: {delay}"
    );
    app.flush_config_form().unwrap();
    let parsed = app.config_mgr.effective_config().unwrap();
    assert_eq!(parsed.input.keyboard.repeat_rate, rate);
    assert_eq!(parsed.input.keyboard.repeat_delay, delay);
    assert!(app.config_mgr.raw_content().contains("repeat-rate"));
    assert!(app.config_mgr.raw_content().contains("repeat-delay"));
    assert!(app
        .config_mgr
        .raw_content()
        .contains("// preserve this comment"));
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
        SOURCE
    );

    let (directory, mut app, context) = fixture(ActiveTab::InputHotCorners);
    let source =
        format!("{SOURCE}input {{ keyboard {{ repeat-rate 255; repeat-delay 65535; }}; }}\n");
    let path = directory.path().join("config.kdl");
    std::fs::write(&path, &source).unwrap();
    app.config_mgr = ConfigManager::from_path(&path).unwrap();
    app.config_mgr.validate();
    assert_eq!(app.config_mgr.validation_status(), &ValidationStatus::Valid);
    for _ in 0..3 {
        frame(&mut app, &context, vec![]);
    }
    assert_eq!(app.config_mgr.visual_settings().keyboard_repeat_rate, 255);
    assert_eq!(
        app.config_mgr.visual_settings().keyboard_repeat_delay,
        65535
    );
    app.flush_config_form().unwrap();
    assert_eq!(
        app.config_mgr
            .effective_config()
            .unwrap()
            .input
            .keyboard
            .repeat_rate,
        255
    );
    assert_eq!(
        app.config_mgr
            .effective_config()
            .unwrap()
            .input
            .keyboard
            .repeat_delay,
        65535
    );
    assert!(!app.config_mgr.is_dirty());
    assert_eq!(app.config_mgr.raw_content(), source);
    assert_eq!(std::fs::read_to_string(path).unwrap(), source);
}

#[test]
fn hot_corners_sheet_fits_all_widths_blocks_parent_commands_and_yields_to_close_guard() {
    for width in [480., 700., 980.] {
        let (directory, mut app, context) = fixture(ActiveTab::Windows);
        context.style_mut(|style| style.animation_time = 0.);
        focus_search(&mut app, &context);
        frame(&mut app, &context, vec![Event::Text("显示器".into())]);
        // An unsaved source edit guarantees the native-close guard is exercised,
        // independently of the machine's default hot-corner values.
        app.config_mgr
            .set_raw_content(SOURCE.replace("gaps 8", "gaps 24"));
        {
            let settings = app.config_mgr.visual_settings_mut();
            settings.hot_corner_top_left = true;
            settings.hot_corner_top_right = true;
            settings.hot_corner_bottom_left = true;
        }
        app.show_hot_corners = true;
        run(&mut app, &context, input(width, 1., vec![]));
        let output = run(&mut app, &context, input(width, 1., vec![]));
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 580.));
        let monitor = text_rects(&output, "屏幕")[0];
        for (label, right, bottom) in [
            ("工作区概览", false, false),
            ("Expo 全景工作区", true, false),
            ("系统偏好设置", false, true),
        ] {
            let rect = text_rects(&output, label)
                .into_iter()
                .find(|rect| {
                    (rect.center().x > monitor.center().x) == right
                        && (rect.center().y > monitor.center().y) == bottom
                })
                .unwrap_or_else(|| panic!("missing {label} in its corner at width {width}"));
            assert!(
                screen.contains_rect(rect),
                "{label} extends past the window at {width}"
            );
        }
        let unbound = text_rects(&output, "—")
            .into_iter()
            .find(|rect| {
                rect.center().x > monitor.center().x && rect.center().y > monitor.center().y
            })
            .expect("visible fourth corner, explicitly unbound");
        assert!(screen.contains_rect(unbound));
        assert!(screen.contains_rect(text_rects(&output, "好")[0]));
        for shortcut in [Key::K, Key::S] {
            run(
                &mut app,
                &context,
                input(width, 1., vec![key(shortcut, Modifiers::CTRL)]),
            );
            assert!(app.show_hot_corners);
            assert!(
                !app.command_palette.is_open(),
                "Ctrl+K opened behind the sheet"
            );
            assert_eq!(app.active_tab, ActiveTab::Windows);
            assert_eq!(
                std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
                SOURCE
            );
        }
        // Even a stale parent search focus must not steal the sheet's Enter.
        context.memory_mut(|memory| memory.request_focus(Id::new("preferences-search")));
        run(
            &mut app,
            &context,
            input(width, 1., vec![key(Key::Enter, Modifiers::NONE)]),
        );
        assert!(!app.show_hot_corners);
        assert_eq!(app.active_tab, ActiveTab::Windows);
        assert_eq!(app.preferences_search, "显示器");
        assert!(app.back_stack.is_empty());
        app.show_hot_corners = true;
        run(&mut app, &context, input(width, 1., vec![]));
        run(
            &mut app,
            &context,
            input(width, 1., vec![key(Key::Escape, Modifiers::NONE)]),
        );
        assert!(!app.show_hot_corners);

        app.show_hot_corners = true;
        run(&mut app, &context, input(width, 1., vec![]));
        let mut native = input(width, 1., vec![]);
        native
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        let output = run(&mut app, &context, native);
        assert!(output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::CancelClose));
        assert!(app.confirm_close);
        assert!(
            app.show_hot_corners,
            "sheet state should survive the higher-priority confirmation"
        );
        let output = run(&mut app, &context, input(width, 1., vec![]));
        assert!(
            text_rects(&output, "触发角").is_empty(),
            "lower-priority sheet obscures close confirmation"
        );
        let resume = text_rects(&output, "继续编辑")[0].center();
        run(
            &mut app,
            &context,
            input(
                width,
                1.,
                vec![Event::PointerMoved(resume), pointer(resume, true)],
            ),
        );
        run(
            &mut app,
            &context,
            input(width, 1., vec![pointer(resume, false)]),
        );
        assert!(!app.confirm_close);
        assert!(app.show_hot_corners);
        run(
            &mut app,
            &context,
            input(width, 1., vec![key(Key::Enter, Modifiers::NONE)]),
        );
        assert!(!app.show_hot_corners);
        assert_eq!(
            std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
            SOURCE
        );
    }
}

#[test]
fn initial_geometry_targets_only_its_own_window_once_and_respects_existing_floating_rules() {
    use clarion_ipc::{Action, Request, Response, SizeChange};

    for already_floating in [false, true] {
        let (directory, mut app, context) = fixture(ActiveTab::Dashboard);
        let (seen, actions) = std::sync::mpsc::channel();
        app.ipc = IpcClient::scripted_for_test(move |request| {
            if let Request::Action(action) = request {
                seen.send(action).unwrap();
            }
            Ok(Response::Handled)
        });
        let window = |id, pid, is_focused, is_floating| -> Window {
            serde_json::from_value(serde_json::json!({
                "id": id, "title": "Settings geometry fixture", "app_id": "clarion-settings",
                "pid": pid, "workspace_id": null, "is_focused": is_focused,
                "is_floating": is_floating, "is_urgent": false, "focus_timestamp": null,
                "layout": {"pos_in_scrolling_layout": null, "tile_size": [948, 1084],
                    "window_size": [948, 1084], "tile_pos_in_workspace_view": null,
                    "window_offset_in_tile": [0, 0]}
            }))
            .unwrap()
        };
        // Put another focused window first to catch an implicit focused-window
        // action or incorrect selection by app_id rather than this process's PID.
        app.cached_windows = vec![
            window(8801_u64, 0_i32, true, false),
            window(7319_u64, std::process::id() as i32, false, already_floating),
        ];
        app.initial_window_geometry = true;
        app.focus_pending = true;
        frame(&mut app, &context, vec![]);
        let drain = |app: &ClarionSettingsApp| {
            // This request is a FIFO barrier for the scripted worker, avoiding
            // timing-dependent sleeps when asserting the absence of actions.
            app.ipc
                .request(Request::Version)
                .unwrap()
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap();
            actions.try_iter().collect::<Vec<_>>()
        };
        let first = drain(&app);
        if already_floating {
            assert!(
                first.is_empty(),
                "an existing floating rule owns the geometry: {first:?}"
            );
        } else {
            assert!(
                matches!(
                    first.as_slice(),
                    [
                        Action::ToggleWindowFloating { id: Some(7319) },
                        Action::SetWindowWidth {
                            id: Some(7319),
                            change: SizeChange::SetFixed(700)
                        },
                        Action::SetWindowHeight {
                            id: Some(7319),
                            change: SizeChange::SetFixed(580)
                        },
                        Action::CenterWindow { id: Some(7319) },
                    ]
                ),
                "all geometry operations must explicitly target only our window: {first:?}"
            );
        }
        assert!(!app.initial_window_geometry);
        assert!(!app.focus_pending);
        // A later activation with even a stale tiled snapshot must not toggle
        // or resize the window again after the user has arranged it.
        app.focus_pending = true;
        frame(&mut app, &context, vec![]);
        frame(&mut app, &context, vec![]);
        assert!(
            drain(&app).is_empty(),
            "later activation reset the user's geometry"
        );
        assert!(!app.config_mgr.is_dirty());
        assert_eq!(app.config_mgr.raw_content(), SOURCE);
        assert_eq!(
            std::fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
            SOURCE
        );
    }
}
