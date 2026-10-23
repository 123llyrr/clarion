//! The GPUI test platform draws and dispatches real widget events without any
//! display server, session bus, clipboard connection or launched application.
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    point, px, Entity, EntityInputHandler, Modifiers, MouseButton, TestAppContext,
    VisualTestContext, WindowHandle,
};

use super::{Host, Panel, ShellView};
use crate::app::testing;

struct Fixture {
    _root: tempfile::TempDir,
    catalog: clarion_launcher::catalog::AppCatalog,
    environment: HashMap<String, String>,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        std::fs::create_dir_all(data.join("applications")).unwrap();
        for (id, name) in [
            ("alpha", "Alpha Browser"),
            ("beta", "Beta Notes"),
            ("chinese", "中文编辑器"),
        ] {
            std::fs::write(
                data.join("applications").join(format!("{id}.desktop")),
                format!("[Desktop Entry]\nType=Application\nDBusActivatable=true\nName={name}\n"),
            )
            .unwrap();
        }
        let environment = [
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            (
                "XDG_CONFIG_HOME".into(),
                root.path().join("config").to_string_lossy().into_owned(),
            ),
            (
                "XDG_CONFIG_DIRS".into(),
                root.path()
                    .join("no-system-config")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("XDG_DATA_HOME".into(), data.to_string_lossy().into_owned()),
            (
                "XDG_DATA_DIRS".into(),
                root.path()
                    .join("no-system-data")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("GTK_ICON_THEME".into(), "PrivateTestTheme".into()),
            ("XDG_CURRENT_DESKTOP".into(), "clarion".into()),
        ]
        .into();
        let catalog = clarion_launcher::catalog::AppCatalog::load(&environment).unwrap();
        assert_eq!(catalog.apps().len(), 3);
        Self {
            _root: root,
            catalog,
            environment,
        }
    }
}

fn panel(
    cx: &mut TestAppContext,
    kind: Panel,
) -> (
    Fixture,
    Entity<Host>,
    Entity<ShellView>,
    VisualTestContext,
    WindowHandle<ShellView>,
) {
    let fixture = Fixture::new();
    cx.update(|cx| {
        crate::input::TextInput::bind_keys(cx);
        super::bind_keys(cx);
    });
    let host = cx
        .update(|cx| testing::test_host(cx, fixture.catalog.clone(), fixture.environment.clone()));
    let window = cx.update(|cx| testing::open_panel(&host, kind, cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.activate_window();
        window.refresh();
    });
    visual.run_until_parked();
    (fixture, host, view, visual, window)
}

#[gpui::test]
fn search_navigation_and_enter_release_the_current_view_and_window_before_launch(
    cx: &mut TestAppContext,
) {
    let (_fixture, host, view, mut visual, window) = panel(cx, Panel::Apps);
    visual.simulate_input("Browser");
    assert_eq!(
        view.read_with(&visual, |view, _| view.query.clone()),
        "Browser"
    );
    assert_eq!(
        view.read_with(&visual, |view, _| view
            .state
            .catalog
            .search(&view.query)
            .len()),
        1
    );
    visual.simulate_keystrokes("ctrl-a backspace");
    visual.simulate_keystrokes("down");
    assert_eq!(view.read_with(&visual, |view, _| view.selection), 1);
    visual.simulate_keystrokes("up");
    assert_eq!(view.read_with(&visual, |view, _| view.selection), 0);
    visual.simulate_input("中文");
    assert_eq!(
        view.read_with(&visual, |view, _| view.query.clone()),
        "中文"
    );
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert!(host.panel.is_none());
        assert_eq!(
            host.prefs.recent_app_ids.first().map(String::as_str),
            Some("chinese.desktop")
        );
    });
    assert!(
        !cx.read(|cx| cx
            .windows()
            .iter()
            .any(|handle| handle.window_id() == window.window_id())),
        "launch must remove the actual exclusive popup, not only its Host handle"
    );
}

#[gpui::test]
fn preedit_enter_arrows_and_escape_never_launch_or_dismiss(cx: &mut TestAppContext) {
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Apps);
    visual.update(|window, cx| {
        let input = view.read(cx).input.clone();
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "中文", None, window, cx)
        });
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("enter down up escape");
    assert_eq!(view.read_with(&visual, |view, _| view.selection), 0);
    host.read_with(cx, |host, _| {
        assert_eq!(host.panel, Some(Panel::Apps));
        assert!(host.prefs.recent_app_ids.is_empty());
    });
    visual.update(|window, cx| {
        let input = view.read(cx).input.clone();
        input.update(cx, |input, cx| input.unmark_text(window, cx));
    });
    visual.simulate_keystrokes("escape");
    host.read_with(cx, |host, _| assert!(host.panel.is_none()));
}

#[gpui::test]
fn every_non_search_panel_has_a_real_escape_dispatch_path(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    cx.update(super::bind_keys);
    let host = cx
        .update(|cx| testing::test_host(cx, fixture.catalog.clone(), fixture.environment.clone()));
    for kind in [
        Panel::Controls,
        Panel::Preferences,
        Panel::Notifications,
        Panel::Screenshot,
        Panel::Tray("test-tray".into()),
        Panel::Dock("alpha.desktop".into()),
    ] {
        let window = cx.update(|cx| testing::open_panel(&host, kind.clone(), cx));
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.update(|window, _| {
            window.activate_window();
            window.refresh();
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        host.read_with(cx, |host, _| {
            assert!(host.panel.is_none(), "Escape failed for {kind:?}")
        });
        assert!(
            !cx.read(|cx| cx
                .windows()
                .iter()
                .any(|handle| handle.window_id() == window.window_id())),
            "Escape left an exclusive popup for {kind:?}"
        );
    }
}

#[gpui::test]
fn header_mouse_close_destroys_the_current_popup_after_event_dispatch(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, window) = panel(cx, Panel::Preferences);
    let close = visual
        .debug_bounds("preferences-close")
        .expect("Preferences exposes the real red close control");
    visual.simulate_click(close.center(), Modifiers::default());
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert!(
            host.panel.is_none(),
            "header close button did not dismiss panel"
        )
    });
    assert!(
        !cx.read(|cx| cx
            .windows()
            .iter()
            .any(|handle| handle.window_id() == window.window_id())),
        "mouse close lost the Host handle but leaked the exclusive surface"
    );
}

#[gpui::test]
fn preferences_wm_close_clears_host_and_one_real_bar_click_opens_it_again(cx: &mut TestAppContext) {
    let (_fixture, host, view, mut visual, window) = panel(cx, Panel::Preferences);
    assert!(
        !visual.simulate_close(),
        "Host owns destruction after the platform callback returns"
    );
    visual.run_until_parked();
    host.read_with(cx, |host, _| assert!(host.panel.is_none()));
    assert!(!cx.read(|cx| cx
        .windows()
        .iter()
        .any(|handle| handle.window_id() == window.window_id())));
    let state = view.read_with(cx, |view, _| view.state.clone());
    let bar = cx.add_window(|window, cx| {
        window.resize(gpui::size(px(640.), px(24.)));
        ShellView::new(host.clone(), super::Surface::Bar, "test".into(), state, cx)
    });
    let mut bar_visual = VisualTestContext::from_window(*bar, cx);
    bar_visual.run_until_parked();
    // The actual menu-mark control occupies x=10..42, across the menu-bar height.
    bar_visual.simulate_click(point(px(26.), px(12.)), Modifiers::default());
    bar_visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert_eq!(host.panel, Some(Panel::Preferences))
    });
    assert!(cx.read(|cx| cx
        .windows()
        .iter()
        .any(|handle| handle.window_id() != bar.window_id())));
}

#[gpui::test]
fn delayed_old_preferences_wm_close_cannot_dismiss_a_replacement_panel(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, _old) = panel(cx, Panel::Preferences);
    assert!(!visual.simulate_close());
    // Replace the window before the queued platform-close task runs.
    let replacement = cx.update(|cx| testing::open_panel(&host, Panel::Controls, cx));
    cx.run_until_parked();
    host.read_with(cx, |host, _| assert_eq!(host.panel, Some(Panel::Controls)));
    assert!(cx.read(|cx| cx
        .windows()
        .iter()
        .any(|handle| handle.window_id() == replacement.window_id())));
}

#[gpui::test]
fn continuous_slider_reaches_both_endpoints_and_finishes_an_outside_release(
    cx: &mut TestAppContext,
) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Preferences);
    let bounds = visual
        .debug_bounds("slider-dock-size")
        .expect("slider must be painted");
    let y = bounds.center().y;
    visual.simulate_mouse_down(
        point(bounds.left() + px(8.), y),
        gpui::MouseButton::Left,
        Modifiers::default(),
    );
    host.read_with(cx, |host, _| {
        assert_eq!(host.prefs.dock_size, 32., "track includes zero")
    });
    visual.simulate_mouse_move(
        point(bounds.center().x, y),
        Some(gpui::MouseButton::Left),
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point(bounds.center().x, y),
        gpui::MouseButton::Left,
        Modifiers::default(),
    );
    host.read_with(cx, |host, _| {
        assert!(
            (host.prefs.dock_size - 48.).abs() < 0.001,
            "release flushes even a throttled final position"
        )
    });
    visual.simulate_mouse_down(
        point(bounds.center().x, y),
        gpui::MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(
        point(bounds.right() + px(20.), y + px(50.)),
        Some(gpui::MouseButton::Left),
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point(bounds.right() + px(20.), y + px(50.)),
        gpui::MouseButton::Left,
        Modifiers::default(),
    );
    host.read_with(cx, |host, _| {
        assert_eq!(host.prefs.dock_size, 64., "dragging outside clamps to one")
    });
    visual.simulate_mouse_move(point(bounds.left(), y), None, Modifiers::default());
    host.read_with(cx, |host, _| {
        assert_eq!(host.prefs.dock_size, 64., "release ends the drag")
    });
}

#[gpui::test]
fn image_cache_survives_unrelated_snapshots_and_replaces_only_changed_tray_pixels(
    cx: &mut TestAppContext,
) {
    let (_fixture, _host, view, mut visual, _window) = panel(cx, Panel::Controls);
    visual.update(|_, cx| {
        view.update(cx, |view, _| {
            let mut state = (*view.state).clone();
            state.history = vec![crate::services::HistoryEntry {
                id: "image:stable-content-digest".into(),
                kind: crate::services::HistoryKind::Image,
                text: String::new(),
                thumbnail_png: Some(vec![1, 2, 3]),
                pinned: false,
                created_at: 1,
                file_path: None,
            }];
            state.tray = vec![crate::services::TrayItem {
                id: "native-tray".into(),
                owner: ":1.2".into(),
                path: "/Item".into(),
                title: "Fixture".into(),
                icon_name: String::new(),
                icon_png: Some(vec![4, 5, 6]),
                status: "Active".into(),
                menu_path: None,
                item_is_menu: false,
            }];
            view.state = Arc::new(state);
            view.cache_images();
            let history = view.image_cache["history:image:stable-content-digest"].clone();
            let tray = view.image_cache["tray:native-tray"].clone();
            let mut state = (*view.state).clone();
            state.system.clock = "unrelated clock refresh".into();
            view.state = Arc::new(state);
            view.cache_images();
            assert!(Arc::ptr_eq(
                &history,
                &view.image_cache["history:image:stable-content-digest"]
            ));
            assert!(Arc::ptr_eq(&tray, &view.image_cache["tray:native-tray"]));
            let mut state = (*view.state).clone();
            state.tray[0].icon_png = Some(vec![7, 8, 9]);
            view.state = Arc::new(state);
            view.cache_images();
            assert!(Arc::ptr_eq(
                &history,
                &view.image_cache["history:image:stable-content-digest"]
            ));
            assert!(!Arc::ptr_eq(&tray, &view.image_cache["tray:native-tray"]));
            let mut state = (*view.state).clone();
            state.history.clear();
            state.tray.clear();
            view.state = Arc::new(state);
            view.cache_images();
            assert!(view.image_cache.is_empty());
            assert!(view.tray_image_hashes.is_empty());
        });
    });
}

#[gpui::test]
fn wallpaper_chooser_releases_the_exclusive_panel_before_requesting_a_dialog(
    cx: &mut TestAppContext,
) {
    let (_fixture, host, _view, visual, window) = panel(cx, Panel::Controls);
    host.update(cx, |host, cx| {
        host.handle(crate::app::Action::PickWallpaper, cx)
    });
    visual.run_until_parked();
    host.read_with(cx, |host, _| assert!(host.panel.is_none()));
    assert!(
        !cx.read(|cx| cx
            .windows()
            .iter()
            .any(|handle| handle.window_id() == window.window_id())),
        "file chooser must not leave an exclusive layer above its dialog"
    );
}

fn dock_window(id: u64, app_id: &str, title: &str) -> clarion_ipc::Window {
    clarion_ipc::Window {
        id,
        title: Some(title.to_owned()),
        app_id: Some(app_id.into()),
        pid: None,
        gtk_app_menu: None,
        app_menu: None,
        workspace_id: Some(1),
        is_focused: false,
        is_minimized: false,
        is_floating: false,
        is_urgent: false,
        focus_timestamp: None,
        layout: clarion_ipc::WindowLayout {
            pos_in_scrolling_layout: None,
            tile_size: (600., 400.),
            window_size: (600, 400),
            tile_pos_in_workspace_view: None,
            window_offset_in_tile: (0., 0.),
        },
    }
}

#[gpui::test]
fn a_long_window_title_stays_on_one_truncated_dock_menu_row(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    cx.update(|cx| {
        crate::input::TextInput::bind_keys(cx);
        super::bind_keys(cx);
    });
    let host = cx
        .update(|cx| testing::test_host(cx, fixture.catalog.clone(), fixture.environment.clone()));
    host.update(cx, |host, _| {
        host.desktop.windows = vec![dock_window(
            1,
            "alpha",
            "A very long window title that must not wrap or push the dock menu rows apart",
        )];
    });
    let window = cx.update(|cx| testing::open_panel(&host, Panel::Dock("alpha.desktop".into()), cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    let focus = visual
        .debug_bounds("focus-1")
        .expect("the window title row is painted");
    let pin = visual
        .debug_bounds("pin-app")
        .expect("the pin row is painted");
    let (w, h) = visual.update(|window, _| {
        let size = window.viewport_size();
        (f32::from(size.width), f32::from(size.height))
    });
    let popup = view
        .read_with(&visual, |view, _| {
            crate::app::panels::placement(
                view.state.as_ref(),
                &Panel::Dock("alpha.desktop".into()),
                w,
                h,
            )
            .popup
        })
        .expect("the Dock menu reports its visible popup");
    assert!(
        focus.size.height <= px(crate::app::panels::DOCK_MENU_ROW + 0.01),
        "a long title must stay on one {}-point row, got {:?}",
        crate::app::panels::DOCK_MENU_ROW,
        focus.size.height,
    );
    assert!(
        focus.right() <= popup.right() + px(0.01),
        "the truncated title must stay inside the popup: {focus:?} vs {popup:?}",
    );
    assert!(
        pin.bottom() <= popup.bottom() + px(0.01),
        "the rows after a long title must still fit the popup: {pin:?} vs {popup:?}",
    );
}

#[gpui::test]
fn clicking_outside_a_dock_menu_dismisses_it(cx: &mut TestAppContext) {
    let (_fixture, host, view, mut visual, _window) =
        panel(cx, Panel::Dock("alpha.desktop".into()));
    let (w, h) = visual.update(|window, _| {
        let size = window.viewport_size();
        (f32::from(size.width), f32::from(size.height))
    });
    let popup = view
        .read_with(&visual, |view, _| {
            crate::app::panels::placement(
                view.state.as_ref(),
                &Panel::Dock("alpha.desktop".into()),
                w,
                h,
            )
            .popup
        })
        .expect("a Dock menu reports its visible popup");
    // A Dock menu is modal: the window covers the whole output and a press
    // outside the visible popup dismisses it.
    let outside = if popup.origin.y > px(60.) || popup.origin.x > px(60.) {
        point(px(4.), px(4.))
    } else {
        point(popup.origin.x + popup.size.width + px(40.), popup.origin.y)
    };
    assert!(
        !popup.contains(&outside),
        "the dismissal point must be outside the popup: {popup:?} {outside:?}"
    );
    // A press on the visible popup's own padding must not dismiss it.
    let inside = point(
        popup.origin.x + popup.size.width * 0.5,
        popup.origin.y + px(4.),
    );
    visual.simulate_mouse_down(inside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert!(
            host.panel.is_some(),
            "a press inside the Dock menu must keep it open"
        );
    });
    visual.simulate_mouse_down(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert!(
            host.panel.is_none(),
            "a press outside the Dock menu must dismiss it"
        );
    });
}
