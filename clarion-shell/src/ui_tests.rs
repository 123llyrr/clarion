//! The GPUI test platform draws and dispatches real widget events without any
//! display server, session bus, clipboard connection or launched application.
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    point, px, size, Entity, EntityInputHandler, Modifiers, MouseButton, TestAppContext,
    VisualTestContext, WindowHandle,
};

use super::{Action, Host, LauncherPresentation, Panel, ShellView, SystemAction};
use crate::app::testing;
use crate::backend;
use clarion_launcher::session::Mode;

/// A closing bar popup keeps its surface alive for the exit fade and a
/// background release timer. The test clock must run to that timer before the
/// window is really removed; the ride itself samples at every frame in between.
fn settle_exit(visual: &mut VisualTestContext) {
    visual
        .cx
        .background_executor
        .advance_clock(std::time::Duration::from_millis(400));
    visual.run_until_parked();
}

/// The palette header's leading mark, by file name (the tests resolve theme
/// icons, so this is the icon the header really paints).
fn header_mark(view: &Entity<ShellView>, visual: &mut VisualTestContext) -> String {
    view.read_with(visual, |view, _| {
        view.palette_glyph_path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    })
}

/// The `Ctrl+B` card is a surface of its own over the sheet, so its rows live
/// in a second window: hand back that window and a context that draws it.
fn spotlight_menu(
    host: &Entity<Host>,
    cx: &mut TestAppContext,
) -> (WindowHandle<ShellView>, VisualTestContext) {
    let window = host
        .read_with(cx, |host, _| host.spotlight_menu)
        .expect("the action card is its own surface");
    (window, VisualTestContext::from_window(window.into(), cx))
}


/// One clipboard history entry for the palette tests: no thumbnail and plain
/// text metadata, so the row layout is what the assertions see.
fn clipboard_entry(
    id: &str,
    kind: crate::services::HistoryKind,
    text: &str,
    created_at: u64,
) -> crate::services::HistoryEntry {
    crate::services::HistoryEntry {
        id: id.to_owned(),
        kind,
        text: text.to_owned(),
        thumbnail_png: None,
        pinned: false,
        created_at,
        file_path: None,
        mime: "text/plain".into(),
        bytes: text.len(),
        width: None,
        height: None,
    }
}

/// One opaque clipboard payload (a video, a PDF): no text to search, just the
/// mime the row and the preview name.
fn clipboard_blob_entry(
    id: &str,
    mime: &str,
    bytes: usize,
    created_at: u64,
) -> crate::services::HistoryEntry {
    crate::services::HistoryEntry {
        id: id.to_owned(),
        kind: crate::services::HistoryKind::Binary,
        text: String::new(),
        thumbnail_png: None,
        pinned: false,
        created_at,
        file_path: None,
        mime: mime.to_owned(),
        bytes,
        width: None,
        height: None,
    }
}

pub(super) struct Fixture {
    _root: tempfile::TempDir,
    catalog: clarion_launcher::catalog::AppCatalog,
    environment: HashMap<String, String>,
}
impl Fixture {
    fn new() -> Self {
        Self::with_apps(&[
            ("alpha", "Alpha Browser", ""),
            ("beta", "Beta Notes", ""),
            ("chinese", "中文编辑器", ""),
        ])
    }

    fn with_apps(apps: &[(&str, &str, &str)]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        std::fs::create_dir_all(data.join("applications")).unwrap();
        for (id, name, categories) in apps {
            let categories = if categories.is_empty() {
                String::new()
            } else {
                format!("Categories={categories}\n")
            };
            std::fs::write(
                data.join("applications").join(format!("{id}.desktop")),
                format!(
                    "[Desktop Entry]\nType=Application\nDBusActivatable=true\nName={name}\n{categories}"
                ),
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
        assert_eq!(catalog.apps().len(), apps.len());
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
    panel_with_fixture(cx, kind, Fixture::new())
}

pub(super) fn panel_with_apps(
    cx: &mut TestAppContext,
    kind: Panel,
    apps: &[(&str, &str, &str)],
) -> (
    Fixture,
    Entity<Host>,
    Entity<ShellView>,
    VisualTestContext,
    WindowHandle<ShellView>,
) {
    panel_with_fixture(cx, kind, Fixture::with_apps(apps))
}

pub(super) fn panel_with_apps_and_windows(
    cx: &mut TestAppContext,
    kind: Panel,
    apps: &[(&str, &str, &str)],
    windows: Vec<clarion_ipc::Window>,
) -> (
    Fixture,
    Entity<Host>,
    Entity<ShellView>,
    VisualTestContext,
    WindowHandle<ShellView>,
) {
    panel_with_fixture_and_windows(cx, kind, Fixture::with_apps(apps), windows)
}

fn panel_with_fixture(
    cx: &mut TestAppContext,
    kind: Panel,
    fixture: Fixture,
) -> (
    Fixture,
    Entity<Host>,
    Entity<ShellView>,
    VisualTestContext,
    WindowHandle<ShellView>,
) {
    panel_with_fixture_and_windows(cx, kind, fixture, Vec::new())
}

fn panel_with_fixture_and_windows(
    cx: &mut TestAppContext,
    kind: Panel,
    fixture: Fixture,
    windows: Vec<clarion_ipc::Window>,
) -> (
    Fixture,
    Entity<Host>,
    Entity<ShellView>,
    VisualTestContext,
    WindowHandle<ShellView>,
) {
    cx.update(|cx| {
        crate::input::TextInput::bind_keys(cx);
        super::bind_keys(cx);
    });
    let host = cx
        .update(|cx| testing::test_host(cx, fixture.catalog.clone(), fixture.environment.clone()));
    host.update(cx, |host, _| host.desktop.windows = windows);
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
    settle_exit(&mut visual);
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
    settle_exit(&mut visual);
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
        Panel::Battery,
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
        settle_exit(&mut visual);
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
fn preferences_wm_close_clears_host_and_the_bar_mark_reopens_it_from_the_system_menu(
    cx: &mut TestAppContext,
) {
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
    host.read_with(cx, |host, _| assert_eq!(host.panel, Some(Panel::System)));
    let menu_window = cx.update(|cx| testing::popup(&host, cx));
    let mut menu = VisualTestContext::from_window(menu_window.into(), cx);
    menu.run_until_parked();
    assert!(
        menu.debug_bounds("system-menu-about").is_some(),
        "the mark's menu carries the About row"
    );
    let settings = menu
        .debug_bounds("system-menu-settings")
        .expect("the menu carries the settings row");
    menu.simulate_click(settings.center(), Modifiers::default());
    menu.run_until_parked();
    host.read_with(cx, |host, _| {
        assert_eq!(host.panel, Some(Panel::Preferences))
    });
    assert!(cx.read(|cx| cx
        .windows()
        .iter()
        .any(|handle| handle.window_id() != bar.window_id())));
}

#[gpui::test]
fn the_system_menu_carries_every_row_and_fits_its_popup(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::System);
    for id in [
        "system-menu-about",
        "system-menu-settings",
        "system-menu-recent",
        "system-menu-sleep",
        "system-menu-reboot",
        "system-menu-shutdown",
        "system-menu-lock",
        "system-menu-logout",
    ] {
        assert!(
            visual.debug_bounds(id).is_some(),
            "the {id} row is painted"
        );
    }
    let about = visual
        .debug_bounds("system-menu-about")
        .expect("the About row is painted");
    let settings = visual
        .debug_bounds("system-menu-settings")
        .expect("the settings row is painted");
    assert!(
        about.top() < settings.top(),
        "About leads the menu, like the reference"
    );
    let logout = visual
        .debug_bounds("system-menu-logout")
        .expect("the log-out row is painted");
    let (bar, height) = host.read_with(cx, |host, _| {
        (
            host.prefs.bar_height as f32,
            crate::app::panels::system_menu_height(),
        )
    });
    assert!(
        f32::from(logout.bottom()) <= bar + 8. + height + 1.,
        "the last row stays inside the popup: {logout:?}"
    );
}

#[gpui::test]
fn the_system_menu_dispatches_session_actions_without_touching_the_system(
    cx: &mut TestAppContext,
) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::System);
    let commands = cx.update(|cx| testing::capture_backend(&host, cx));
    let shutdown = visual
        .debug_bounds("system-menu-shutdown")
        .expect("the shutdown row is painted");
    visual.simulate_click(shutdown.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        matches!(
            commands.try_recv(),
            Ok(backend::Command::System(SystemAction::Shutdown))
        ),
        "the row only queues the worker command"
    );
    host.read_with(cx, |host, _| assert!(host.panel.is_none()));
}

#[gpui::test]
fn the_about_row_opens_the_machine_card(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::System);
    host.update(cx, |host, cx| {
        host.system.facts = crate::system_info::SystemFacts {
            model: "Blade".into(),
            hostname: "clarion-box".into(),
            os_name: "Arch Linux".into(),
            kernel: "6.17.6-arch1-1".into(),
            cpu: "Test CPU".into(),
            gpu: "GP106M [GeForce GTX 1060 Mobile]".into(),
            serial: "PF12345678".into(),
            memory_total: Some(16 * 1024 * 1024 * 1024),
            disk_total: Some(512 * 1024 * 1024 * 1024),
            disk_free: Some(128 * 1024 * 1024 * 1024),
        };
        host.changed(cx);
    });
    visual.run_until_parked();
    let about = visual
        .debug_bounds("system-menu-about")
        .expect("the About row is painted");
    visual.simulate_click(about.center(), Modifiers::default());
    visual.run_until_parked();
    host.read_with(cx, |host, _| assert_eq!(host.panel, Some(Panel::About)));
    let card = cx.update(|cx| testing::popup(&host, cx));
    let mut about = VisualTestContext::from_window(card.into(), cx);
    about.run_until_parked();
    assert!(about.debug_bounds("about-card").is_some());
    assert!(about.debug_bounds("about-title").is_some());
    assert!(about.debug_bounds("about-close").is_some());
    assert!(about.debug_bounds("about-more").is_some());
    for row in [
        "about-row-cpu",
        "about-row-gpu",
        "about-row-memory",
        "about-row-disk",
        "about-row-serial",
        "about-row-os",
        "about-row-kernel",
    ] {
        assert!(
            about.debug_bounds(row).is_some(),
            "the {row} fact is painted"
        );
    }
}

#[gpui::test]
fn a_click_outside_the_system_menu_dismisses_it(cx: &mut TestAppContext) {
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::System);
    let (w, h) = visual.update(|window, _| {
        let size = window.viewport_size();
        (f32::from(size.width), f32::from(size.height))
    });
    let popup = view
        .read_with(&visual, |view, _| {
            crate::app::panels::placement(view.state.as_ref(), &Panel::System, w, h).popup
        })
        .expect("the system menu reports its visible popup");
    let outside = if popup.origin.y > px(60.) || popup.origin.x > px(60.) {
        point(px(4.), px(4.))
    } else {
        point(
            popup.origin.x + popup.size.width + px(40.),
            popup.origin.y,
        )
    };
    assert!(!popup.contains(&outside));
    visual.simulate_mouse_down(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    host.read_with(cx, |host, _| assert!(host.panel.is_none()));
}

#[gpui::test]
fn the_bar_battery_indicator_opens_the_standalone_battery_popup(cx: &mut TestAppContext) {
    let (_fixture, host, view, _visual, _window) = panel(cx, Panel::Preferences);
    host.update(cx, |host, cx| {
        host.system.battery = Some(0.42);
        host.system.battery_status = crate::backend::BatteryStatus::Discharging;
        host.system.battery_health = Some(0.5);
        host.system.ac_online = false;
        host.changed(cx);
    });
    let state = view.read_with(cx, |view, _| view.state.clone());
    let bar = cx.add_window(|window, cx| {
        window.resize(gpui::size(px(1000.), px(24.)));
        ShellView::new(host.clone(), super::Surface::Bar, "test".into(), state, cx)
    });
    let mut bar_visual = VisualTestContext::from_window(*bar, cx);
    bar_visual.run_until_parked();
    let battery = bar_visual
        .debug_bounds("menu-battery")
        .expect("a laptop bar carries the battery indicator");
    bar_visual.simulate_click(battery.center(), Modifiers::default());
    bar_visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert_eq!(host.panel, Some(Panel::Battery));
    });
}

#[gpui::test]
fn the_battery_popup_carries_the_summary_and_every_power_profile(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Battery);
    host.update(cx, |host, cx| {
        host.system.battery = Some(0.42);
        host.system.battery_status = crate::backend::BatteryStatus::Discharging;
        host.system.battery_health = Some(0.84);
        host.system.ac_online = false;
        host.system.power_profile = Some(crate::power::PowerProfile::Balanced);
        host.system.power_profiles = crate::power::PowerProfile::ALL.to_vec();
        host.changed(cx);
    });
    visual.update(|window, _| {
        window.resize(size(px(344.), px(348.)));
        window.refresh();
    });
    visual.run_until_parked();
    for id in [
        "battery-summary",
        "battery-modes",
        "battery-health",
        "battery-source",
        "battery-profile-low-power",
        "battery-profile-balanced",
        "battery-profile-performance",
        "battery-charge-limit",
    ] {
        assert!(
            visual.debug_bounds(id).is_some(),
            "the battery popup paints {id}"
        );
    }
    // The summary is a compact three-column block: the icon, the percentage
    // with its state subtitle, and the two aligned metric rows.
    let summary = visual.debug_bounds("battery-summary").expect("summary");
    let modes = visual.debug_bounds("battery-modes").expect("modes");
    let charge = visual
        .debug_bounds("battery-charge-limit")
        .expect("charge limit card");
    let health = visual.debug_bounds("battery-health").expect("health");
    let source = visual.debug_bounds("battery-source").expect("source");
    assert!(
        (73. ..=75.).contains(&f32::from(summary.size.height)),
        "the summary keeps its compact height: {}",
        f32::from(summary.size.height)
    );
    assert!(
        summary.bottom() <= modes.top(),
        "the summary must not overlap the profile card"
    );
    assert!(
        source.right() <= summary.right() - px(8.),
        "the metrics must stay inside the card: {source:?} vs {summary:?}"
    );
    assert!(
        health.left() == source.left(),
        "health and power source share one column: {health:?} {source:?}"
    );
    assert!(
        health.bottom() <= source.top(),
        "the metric rows stack without overlapping: {health:?} {source:?}"
    );
    assert!(
        f32::from(modes.size.height) <= 141.,
        "the profile card stays at its compact height: {}",
        f32::from(modes.size.height)
    );
    assert!(
        modes.bottom() <= summary.top() + px(224.01),
        "the whole sheet fits without a scroll: {}",
        f32::from(modes.bottom())
    );
    assert!(
        modes.bottom() <= charge.top(),
        "the charge-limit card sits below the profile card: {modes:?} {charge:?}"
    );
    assert!(
        charge.bottom() <= summary.top() + px(320.01),
        "the charge-limit card still fits inside the taller sheet: {}",
        f32::from(charge.bottom())
    );
    assert!(
        visual
            .debug_bounds("battery-charge-limit-value-100%")
            .is_some(),
        "an unsupported platform still shows the 100% default"
    );
    assert!(
        visual
            .debug_bounds("battery-charge-limit-unavailable")
            .is_some(),
        "an unsupported platform says the ceiling is unavailable"
    );
    assert!(
        visual
            .debug_bounds("slider-battery-charge-limit")
            .is_none(),
        "no slider is offered when the platform cannot write a threshold"
    );
}

#[gpui::test]
fn clicking_a_power_profile_waits_for_confirmation_and_sends_the_switch(
    cx: &mut TestAppContext,
) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Battery);
    host.update(cx, |host, cx| {
        host.system.battery = Some(0.42);
        host.system.battery_status = crate::backend::BatteryStatus::Discharging;
        host.system.power_profile = Some(crate::power::PowerProfile::Balanced);
        host.system.power_profiles = crate::power::PowerProfile::ALL.to_vec();
        host.changed(cx);
    });
    visual.update(|window, _| {
        window.resize(size(px(344.), px(348.)));
        window.refresh();
    });
    visual.run_until_parked();
    let commands = cx.update(|cx| testing::capture_backend(&host, cx));
    let performance = visual
        .debug_bounds("battery-profile-performance")
        .expect("the performance button");
    visual.simulate_click(performance.center(), Modifiers::default());
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.system.power_profile,
            Some(crate::power::PowerProfile::Balanced),
            "the accent fill waits for the read-back"
        );
        assert_eq!(host.panel, Some(Panel::Battery));
    });
    assert!(
        visual.debug_bounds("battery-profile-spinner").is_some(),
        "the pending switch paints its ring outside the button"
    );
    let mut sent = Vec::new();
    while let Ok(command) = commands.try_recv() {
        if matches!(command, backend::Command::SetPowerProfile(_)) {
            sent.push(command);
        }
    }
    assert!(
        matches!(
            sent.as_slice(),
            [backend::Command::SetPowerProfile(
                crate::power::PowerProfile::Performance
            )]
        ),
        "the popup must send exactly one profile switch: {sent:?}"
    );

    // The worker's read-back settles the switch and only now turns the chosen
    // button's fill on, with the completion burst.
    host.update(cx, |host, cx| {
        host.backend_event(
            backend::Event::PowerProfile {
                profile: Some(crate::power::PowerProfile::Performance),
                profiles: crate::power::PowerProfile::ALL.to_vec(),
                settled: true,
                epoch: 1,
            },
            cx,
        );
    });
    visual.update(|window, _| window.refresh());
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.system.power_profile,
            Some(crate::power::PowerProfile::Performance)
        );
    });
    assert!(
        visual.debug_bounds("battery-profile-success").is_some(),
        "the settled switch paints its completion ring"
    );
    assert!(
        visual.debug_bounds("battery-profile-check").is_some(),
        "the settled switch paints its check badge"
    );
    assert!(
        visual
            .debug_bounds("battery-profile-spinner")
            .is_none(),
        "the pending ring gives way to the completion burst"
    );
}

#[gpui::test]
fn a_supported_charge_ceiling_paints_the_slider_and_writes_the_threshold(
    cx: &mut TestAppContext,
) {
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Battery);
    host.update(cx, |host, cx| {
        host.system.battery = Some(0.42);
        host.system.battery_status = crate::backend::BatteryStatus::Charging;
        host.backend_event(
            backend::Event::BatteryChargeLimit {
                limit: Some(0.8),
                epoch: 1,
            },
            cx,
        );
        host.changed(cx);
    });
    visual.update(|window, _| {
        window.resize(size(px(344.), px(348.)));
        window.refresh();
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("battery-charge-limit").is_some(),
        "the charge-limit card is painted on a supported platform"
    );
    assert!(
        visual
            .debug_bounds("battery-charge-limit-value-80%")
            .is_some(),
        "the card names the firmware's current ceiling"
    );
    assert!(
        visual
            .debug_bounds("battery-charge-limit-unavailable")
            .is_none(),
        "a writable threshold is not labelled unavailable"
    );
    let slider = visual
        .debug_bounds("slider-battery-charge-limit")
        .expect("the adjustable charge slider");
    let commands = cx.update(|cx| testing::capture_backend(&host, cx));
    // The track maps the middle of its bounds to 50% of the 50–100 span, so a
    // centre click asks the firmware for 75%.
    visual.simulate_click(slider.center(), Modifiers::default());
    visual.run_until_parked();
    let mut sent = Vec::new();
    while let Ok(command) = commands.try_recv() {
        sent.push(command);
    }
    assert!(
        matches!(
            sent.as_slice(),
            [backend::Command::SetBatteryChargeLimit(75)]
        ),
        "the slider must write exactly the clicked ceiling: {sent:?}"
    );
    host.read_with(cx, |host, _| {
        assert_eq!(host.panel, Some(Panel::Battery));
    });
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.state.battery_charge_limit,
            Some(0.75),
            "the thumb moves at once while the worker confirms"
        );
    });
}

#[gpui::test]
fn the_bar_icon_slots_leave_breathing_room(cx: &mut TestAppContext) {
    let (_fixture, host, view, _visual, _window) = panel(cx, Panel::Preferences);
    host.update(cx, |host, cx| {
        host.prefs.bar_height = 40.;
        host.system.battery = Some(0.42);
        host.system.battery_status = crate::backend::BatteryStatus::Discharging;
        host.changed(cx);
    });
    let state = view.read_with(cx, |view, _| view.state.clone());
    let bar = cx.add_window(|window, cx| {
        window.resize(gpui::size(px(1000.), px(40.)));
        ShellView::new(host.clone(), super::Surface::Bar, "test".into(), state, cx)
    });
    let mut bar_visual = VisualTestContext::from_window(*bar, cx);
    bar_visual.run_until_parked();
    let spotlight = bar_visual
        .debug_bounds("menu-spotlight")
        .expect("the bar carries the launcher button");
    assert!(
        f32::from(spotlight.size.width) >= 36.,
        "a status slot must be wider than the glyph it centres: {spotlight:?}"
    );
    let battery = bar_visual
        .debug_bounds("menu-battery")
        .expect("the laptop bar carries the battery indicator");
    assert!(
        f32::from(battery.size.width) >= 34.,
        "the battery glyph keeps its own side padding: {battery:?}"
    );
}

#[gpui::test]
fn the_bar_spotlight_button_opens_the_popup_launcher(cx: &mut TestAppContext) {
    let (_fixture, host, view, _visual, _window) = panel(cx, Panel::Preferences);
    let state = view.read_with(cx, |view, _| view.state.clone());
    let bar = cx.add_window(|window, cx| {
        window.resize(gpui::size(px(1000.), px(24.)));
        ShellView::new(host.clone(), super::Surface::Bar, "test".into(), state, cx)
    });
    let mut bar_visual = VisualTestContext::from_window(*bar, cx);
    bar_visual.run_until_parked();
    let button = bar_visual
        .debug_bounds("menu-spotlight")
        .expect("the bar carries the launcher search button");
    bar_visual.simulate_click(button.center(), Modifiers::default());
    bar_visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert_eq!(host.panel, Some(Panel::Spotlight));
    });
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
                mime: "image/png".into(),
                bytes: 3,
                width: Some(1920),
                height: Some(1080),
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
    let (_fixture, host, _view, mut visual, window) = panel(cx, Panel::Controls);
    host.update(cx, |host, cx| {
        host.handle(crate::app::Action::PickWallpaper, cx)
    });
    visual.run_until_parked();
    settle_exit(&mut visual);
    host.read_with(cx, |host, _| assert!(host.panel.is_none()));
    assert!(
        !cx.read(|cx| cx
            .windows()
            .iter()
            .any(|handle| handle.window_id() == window.window_id())),
        "file chooser must not leave an exclusive layer above its dialog"
    );
}

pub(super) fn dock_window(id: u64, app_id: &str, title: &str) -> clarion_ipc::Window {
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
    let window =
        cx.update(|cx| testing::open_panel(&host, Panel::Dock("alpha.desktop".into()), cx));
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

#[gpui::test]
fn a_tray_menu_click_opens_the_popup_under_that_icon(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    cx.update(super::bind_keys);
    let host = cx
        .update(|cx| testing::test_host(cx, fixture.catalog.clone(), fixture.environment.clone()));
    cx.update(|cx| {
        host.update(cx, |host, _| {
            host.tray = vec![crate::services::TrayItem {
                id: "menu-item".into(),
                owner: ":1.2".into(),
                path: "/Item".into(),
                title: "Fixture".into(),
                icon_name: String::new(),
                icon_png: None,
                status: "Active".into(),
                menu_path: None,
                item_is_menu: true,
            }];
        });
    });
    let state = cx.update(|cx| testing::snapshot(&host, cx));
    let bar = cx.add_window(|window, cx| {
        window.resize(gpui::size(px(900.), px(24.)));
        super::ShellView::new(host.clone(), super::Surface::Bar, "test".into(), state, cx)
    });
    let mut bar_visual = VisualTestContext::from_window(*bar, cx);
    bar_visual.update(|window, _| window.refresh());
    bar_visual.run_until_parked();
    let bounds = bar_visual
        .debug_bounds("tray-menu-item")
        .expect("the tray icon is painted");
    bar_visual.simulate_click(bounds.center(), Modifiers::default());
    bar_visual.run_until_parked();
    let mx = f32::from(bounds.center().x);
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.panel,
            Some(Panel::Tray("menu-item".into())),
            "the click opens the item's menu"
        );
        assert_eq!(
            host.anchored,
            Some(mx),
            "the pointer's bar-space x anchors the popup"
        );
        assert!(!host.panel_exit, "an open panel is not riding its exit");
    });
    // The test window reports pointer coordinates in its own pixel space, so
    // feed a known anchor instead: with it the popup drops under the icon,
    // never at the resting right margin of the output.
    cx.update(|cx| {
        let mut state = (*testing::snapshot(&host, cx)).clone();
        state.anchored = Some(400.);
        let anchored =
            crate::app::panels::placement(&state, &Panel::Tray("menu-item".into()), 900., 800.);
        let popup = anchored.popup.expect("the tray popup is reported");
        // The popup is centred on the click position; the clamp only pulls it
        // back when the centre would leave the popup clipped by an edge.
        assert_eq!(f32::from(popup.origin.x), 400. - 260. / 2.);
        assert_eq!(f32::from(popup.origin.y), 24. + 8.);
    });
}

#[gpui::test]
fn clicking_outside_a_bar_popup_dismisses_it_once_the_exit_ride_finishes(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, window) = panel(cx, Panel::Controls);
    // The modal window covers the whole output; this press lands on its
    // catcher, well away from the visible control center rectangle.
    visual.simulate_mouse_down(
        point(px(10.), px(760.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        // The dismissal keeps the surface alive for the closing fade: `panel`
        // stays visible to the view while the ride runs.
        assert!(host.panel_exit, "the close starts an exit ride");
        assert_eq!(host.panel, Some(Panel::Controls));
    });
    assert!(
        cx.read(|cx| cx
            .windows()
            .iter()
            .any(|handle| handle.window_id() == window.window_id())),
        "the fading surface is not cut mid-ride"
    );
    settle_exit(&mut visual);
    host.read_with(cx, |host, _| assert!(host.panel.is_none()));
    assert!(
        !cx.read(|cx| cx
            .windows()
            .iter()
            .any(|handle| handle.window_id() == window.window_id())),
        "the release timer frees the surface"
    );
}

#[gpui::test]
fn reduced_motion_releases_a_bar_popup_right_away(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, window) = panel(cx, Panel::Controls);
    host.update(cx, |host, _| host.prefs.reduce_motion = true);
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert!(host.panel.is_none());
        assert!(!host.panel_exit, "reduced motion snaps the release");
    });
    assert!(
        !cx.read(|cx| cx
            .windows()
            .iter()
            .any(|handle| handle.window_id() == window.window_id())),
        "no fade keeps the surface alive"
    );
}

#[gpui::test]
fn a_bar_popup_lands_on_the_monitor_that_dispatched_the_click(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let host = cx
        .update(|cx| testing::test_host(cx, fixture.catalog.clone(), fixture.environment.clone()));
    // The test platform has a single display: the primary output holds the
    // focus, and the second monitor is only ever reached by its own bar.
    let primary = cx.update(|cx| {
        testing::open_dock(&host, cx);
        testing::output_name(&host, cx)
    });
    let secondary = format!("zz-{primary}");
    cx.update(|cx| testing::register_output(&host, &secondary, cx));
    assert_eq!(
        host.read_with(cx, |host, _| host.active_output()),
        primary,
        "focus must stay on the primary output for the click to prove anything"
    );
    let window = cx.update(|cx| {
        testing::dispatch_on_output(
            &host,
            &secondary,
            crate::app::Action::AnchoredPanel(Panel::Tray("tray".into()), 700.),
            cx,
        )
    });
    let opened = window
        .read_with(cx, |view, _| view.output().to_owned())
        .expect("the panel window is alive");
    assert_eq!(
        opened, secondary,
        "the tray menu belongs to the bar that was clicked, not to the focused output"
    );
}

#[gpui::test]
fn the_launcher_records_the_monitor_that_opened_it(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let host = cx
        .update(|cx| testing::test_host(cx, fixture.catalog.clone(), fixture.environment.clone()));
    let primary = cx.update(|cx| {
        testing::open_dock(&host, cx);
        testing::output_name(&host, cx)
    });
    let secondary = format!("zz-{primary}");
    cx.update(|cx| testing::register_output(&host, &secondary, cx));

    cx.update(|cx| {
        testing::dispatch_on_output(
            &host,
            &secondary,
            crate::app::Action::Panel(Panel::Apps),
            cx,
        )
    });

    let state = cx.update(|cx| testing::snapshot(&host, cx));
    assert_eq!(
        state.panel_output.as_deref(),
        Some(secondary.as_str()),
        "the launcher records its own output so only that Dock can hide"
    );
    assert_eq!(state.panel, Some(Panel::Apps));
}

#[gpui::test]
fn the_launcher_command_opens_the_presentation_the_preferences_select(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let host = cx
        .update(|cx| testing::test_host(cx, fixture.catalog.clone(), fixture.environment.clone()));
    host.update(cx, |host, cx| host.handle(Action::Launcher, cx));
    cx.run_until_parked();
    assert_eq!(
        host.read_with(cx, |host, _| host.panel.clone()),
        Some(Panel::Apps),
        "the fullscreen Launchpad stays the default"
    );
    // The same entry point, now with the palette preference, replaces the
    // fullscreen surface instead of stacking another panel beside it.
    host.update(cx, |host, cx| {
        host.prefs.launcher_presentation = LauncherPresentation::Palette;
        host.handle(Action::Launcher, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        host.read_with(cx, |host, _| host.panel.clone()),
        Some(Panel::Spotlight)
    );
    // A second launcher press closes the open palette. The palette keeps its
    // surface alive for the closing fade, so the release timer has to run.
    host.update(cx, |host, cx| host.handle(Action::Launcher, cx));
    cx.run_until_parked();
    assert_eq!(
        host.read_with(cx, |host, _| (host.panel.clone(), host.panel_exit)),
        (Some(Panel::Spotlight), true)
    );
    cx.background_executor
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(host.read_with(cx, |host, _| host.panel.clone()), None);
}

#[gpui::test]
fn the_launcher_palette_ranks_scored_matches_and_enter_launches(cx: &mut TestAppContext) {
    let (_fixture, host, view, mut visual, window) = panel(cx, Panel::Spotlight);
    visual.simulate_input("browser");
    assert_eq!(
        view.read_with(&visual, |view, _| view.query.clone()),
        "browser"
    );
    // The score ladder only keeps the application whose name matches.
    let names: Vec<String> = view.read_with(&visual, |view, _| {
        use clarion_launcher::search::AppOrder;
        super::spotlight::palette_apps(&view.state, &view.query, AppOrder::Name)
            .into_iter()
            .map(|app| app.name.clone())
            .collect()
    });
    assert_eq!(names, vec!["Alpha Browser".to_owned()]);
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    settle_exit(&mut visual);
    host.read_with(cx, |host, _| {
        assert!(host.panel.is_none());
        assert_eq!(
            host.prefs.recent_app_ids.first().map(String::as_str),
            Some("alpha.desktop")
        );
    });
    assert!(
        !cx.read(|cx| cx
            .windows()
            .iter()
            .any(|handle| handle.window_id() == window.window_id())),
        "launching must remove the actual palette window"
    );
}

#[gpui::test]
fn the_palette_runs_a_slash_mode_command_and_clears_the_draft(cx: &mut TestAppContext) {
    use clarion_launcher::session::Mode;
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.simulate_input("/apps");
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_row_count(&view.state)),
        1,
        "the slash draft offers its one matching command"
    );
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| (view.palette.mode, view.query.clone())),
        (Mode::Apps, String::new()),
        "a mode command consumes the draft and keeps the palette open"
    );
    host.read_with(cx, |host, _| {
        assert_eq!(host.panel, Some(Panel::Spotlight));
    });
    // The grid stays the default layout; the list override applies on demand
    // and drops out again when the saved value is chosen.
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_layout(&view.state)),
        super::LauncherLayout::Grid
    );
    visual.simulate_input("/list");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_layout(&view.state)),
        super::LauncherLayout::List
    );
}

#[gpui::test]
fn palette_backspace_unwinds_the_override_but_not_a_mode_switch(cx: &mut TestAppContext) {
    use clarion_launcher::session::Mode;
    let (_fixture, _host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.simulate_keystrokes("ctrl-1");
    visual.simulate_input("/list");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| (
            view.palette.mode,
            view.palette_layout(&view.state),
            view.query.clone()
        )),
        (Mode::Apps, super::LauncherLayout::List, String::new())
    );
    visual.simulate_keystrokes("backspace");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| (
            view.palette.mode,
            view.palette_layout(&view.state)
        )),
        (Mode::Apps, super::LauncherLayout::Grid),
        "the first backspace drops the layout override"
    );
    // Mode switches replace the session instead of stacking it (the reference
    // does the same), so a further backspace has nothing to unwind.
    visual.simulate_keystrokes("backspace");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette.mode),
        Mode::Apps
    );
}

#[gpui::test]
fn palette_commands_report_scope_and_unimplemented_entries(cx: &mut TestAppContext) {
    use clarion_launcher::session::Mode;
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    // `>` opens the command list without leaving a mode: the field keeps the
    // prefix and every accepted row is filtered by the current scope.
    visual.simulate_input(">");
    assert!(
        view.read_with(&visual, |view, _| view.palette_row_count(&view.state)) > 0,
        "the bare prefix lists the available commands"
    );
    visual.simulate_input("grid");
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_row_count(&view.state)),
        0,
        "a layout override is not offered outside apps mode"
    );
    visual.simulate_keystrokes("ctrl-a backspace");
    // Typing the out-of-scope command anyway explains instead of doing nothing.
    visual.simulate_input("/grid");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette.mode),
        Mode::Search
    );
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_error.clone()),
        Some("该命令只在对应模式下可用".to_owned())
    );
    // Tools have no page yet, so they are rejected instead of faking one.
    // A rejected draft is kept, so clear it before the next attempt.
    visual.simulate_keystrokes("ctrl-a backspace");
    visual.simulate_input("/calc 2*3");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_error.clone()),
        Some("该命令尚未接入".to_owned())
    );
    // Escape clears the rejected draft before it closes anything.
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| (
            view.query.clone(),
            view.palette_error.is_some()
        )),
        (String::new(), false)
    );
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.panel,
            Some(Panel::Spotlight),
            "escape starts the close ride"
        );
        assert!(host.panel_exit);
    });
}

#[gpui::test]
fn palette_system_actions_hand_off_to_the_worker(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    let received = cx.update(|cx| testing::capture_backend(&host, cx));
    visual.simulate_input("/lock");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let command = received
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("the lock request reaches the backend");
    assert!(
        matches!(
            command,
            crate::backend::Command::System(crate::app::SystemAction::Lock)
        ),
        "a slash action hands the session action to the worker"
    );
    host.read_with(cx, |host, _| {
        assert!(
            host.panel_exit,
            "the palette folds away after a power action"
        );
    });
}

#[gpui::test]
fn palette_settings_actions_toggle_preferences(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    assert!(!host.read_with(cx, |host, _| host.prefs.reduce_motion));
    visual.simulate_input("/reduce-motion");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert!(
            host.prefs.reduce_motion,
            "the toggle flipped the preference"
        );
    });
}

#[gpui::test]
fn the_web_page_searches_through_engines(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    let received = cx.update(|cx| testing::capture_backend(&host, cx));
    visual.simulate_input("/search");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("spotlight-web-google").is_some()
            && visual.debug_bounds("spotlight-web-bilibili").is_some(),
        "an empty draft lists the engines"
    );

    // A bare query keeps every engine, each with its own encoded search.
    visual.simulate_input("rust wayland");
    visual.run_until_parked();
    assert!(visual.debug_bounds("spotlight-web-duckduckgo").is_some());

    // An engine prefix narrows to one row and Enter opens it.
    visual.simulate_keystrokes("ctrl-a backspace");
    visual.simulate_input("gh tokio");
    visual.run_until_parked();
    assert!(visual.debug_bounds("spotlight-web-github").is_some());
    assert!(
        visual.debug_bounds("spotlight-web-google").is_none(),
        "the prefix hides the other engines"
    );
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let command = received
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("the search reaches the backend");
    assert!(
        matches!(
            command,
            crate::backend::Command::OpenUrl(url)
                if url == "https://github.com/search?q=tokio"
        ),
        "the engine prefix builds its search URL"
    );
    host.read_with(cx, |host, _| {
        assert!(
            host.panel_exit,
            "the sheet folds away after opening the search"
        );
    });
}

#[gpui::test]
fn the_web_page_opens_a_literal_url(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    let received = cx.update(|cx| testing::capture_backend(&host, cx));
    visual.simulate_input("/web");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.simulate_input("https://example.com/a?b=1");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("spotlight-web-url").is_some(),
        "a URL becomes its own row"
    );
    assert!(visual.debug_bounds("spotlight-web-google").is_none());
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let command = received
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("the URL reaches the backend");
    assert!(matches!(
        command,
        crate::backend::Command::OpenUrl(url) if url == "https://example.com/a?b=1"
    ));
}

#[gpui::test]
fn the_actions_page_lists_filters_and_runs_compositor_actions(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    let received = cx.update(|cx| testing::capture_backend(&host, cx));
    visual.simulate_input("/actions");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("spotlight-action-window.close")
            .is_some(),
        "the catalog opens on its first window action"
    );
    assert!(
        visual
            .debug_bounds("spotlight-action-window.close")
            .is_some()
            && visual
                .debug_bounds("spotlight-action-system.suspend")
                .is_some(),
        "the whole catalog is listed, not only one section"
    );

    // The draft narrows the catalog by title, keyword or section.
    visual.simulate_input("关闭窗口");
    visual.run_until_parked();
    assert!(visual
        .debug_bounds("spotlight-action-window.close")
        .is_some());
    assert!(
        visual
            .debug_bounds("spotlight-action-column.focus-left")
            .is_none(),
        "a filtered page hides the rest of the catalog"
    );

    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let command = received
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("the compositor action reaches the backend");
    assert!(
        matches!(
            command,
            crate::backend::Command::Action(clarion_ipc::Action::CloseWindow { id: None })
        ),
        "the catalog entry maps to its IPC action"
    );
    host.read_with(cx, |host, _| {
        assert!(
            host.panel_exit,
            "the sheet folds away after running an action"
        );
    });
}

#[gpui::test]
fn the_actions_page_runs_shell_actions_and_pops_back(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.simulate_input("/actions");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.simulate_input("减少动效");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert!(
            host.prefs.reduce_motion,
            "shell entries keep their command dispatch"
        );
    });

    // An empty draft backspaces out of the tool page, back to the sheet's
    // previous state instead of closing it.
    visual.simulate_keystrokes("ctrl-a backspace");
    visual.run_until_parked();
    visual.simulate_keystrokes("backspace");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("spotlight-action-window.close")
            .is_none(),
        "backspace leaves the catalog"
    );
    host.read_with(cx, |host, _| {
        assert_eq!(host.panel, Some(Panel::Spotlight), "the sheet stays open");
        assert!(!host.panel_exit);
    });
}

#[gpui::test]
fn right_clicking_a_palette_tile_opens_a_menu_surface_over_the_palette(cx: &mut TestAppContext) {
    let (_fixture, host, view, mut visual, window) = panel(cx, Panel::Spotlight);
    // The fixture has three applications, so the first grid row is centred:
    // its first tile spans x 381..519 inside the palette popup, below the
    // header block and the first category header.
    let tile = point(px(450.), px(320.));
    visual.simulate_mouse_down(tile, MouseButton::Right, Modifiers::none());
    visual.simulate_mouse_up(tile, MouseButton::Right, Modifiers::none());
    visual.run_until_parked();
    let menu = host
        .read_with(cx, |host, _| host.menu)
        .expect("right-clicking a tile opens its application menu");
    assert_ne!(
        menu.window_id(),
        window.window_id(),
        "the menu needs a surface of its own for the compositor to frost it"
    );
    assert!(
        cx.read(|cx| cx
            .windows()
            .iter()
            .any(|handle| handle.window_id() == window.window_id())),
        "the palette window stays open under its menu"
    );
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.panel,
            Some(Panel::Spotlight),
            "the launcher stays open"
        )
    });
    // The menu surface catches every pointer event over the palette: a click
    // on the card keeps it open...
    let mut menu_visual = VisualTestContext::from_window(menu.into(), cx);
    menu_visual.simulate_mouse_down(
        point(px(444.), px(336.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    menu_visual.run_until_parked();
    assert!(host.read_with(cx, |host, _| host.menu.is_some()));
    // ...while a click outside folds it away and leaves the palette alone.
    menu_visual.simulate_mouse_down(
        point(px(750.), px(600.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    menu_visual.run_until_parked();
    assert!(
        host.read_with(cx, |host, _| host.menu.is_none()),
        "a click outside folds the menu"
    );
    host.read_with(cx, |host, _| {
        assert_eq!(host.panel, Some(Panel::Spotlight));
        assert!(!host.panel_exit, "the launcher is not on its way out");
    });
    // The palette still has the keyboard: it types, and it still closes on its
    // own escape once the draft is gone.
    visual.simulate_input("a");
    assert_eq!(view.read_with(&visual, |view, _| view.query.clone()), "a");
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert_eq!(view.read_with(&visual, |view, _| view.query.clone()), "");
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    host.read_with(cx, |host, _| assert!(host.panel_exit));
}

#[gpui::test]
fn palette_enter_opens_the_selected_tile_after_hidden_category_apps(cx: &mut TestAppContext) {
    let apps = [
        ("utility-1", "Utility 1", "Utility;"),
        ("utility-2", "Utility 2", "Utility;"),
        ("utility-3", "Utility 3", "Utility;"),
        ("utility-4", "Utility 4", "Utility;"),
        ("utility-5", "Utility 5", "Utility;"),
        ("utility-6", "Utility 6", "Utility;"),
        ("utility-7", "Utility 7", "Utility;"),
        ("office-1", "Office 1", "Office;"),
        ("office-2", "Office 2", "Office;"),
        ("office-3", "Office 3", "Office;"),
    ];
    let (_fixture, host, view, mut visual, _window) = panel_with_apps(cx, Panel::Spotlight, &apps);

    // The Utilities section shows six tiles and hides utility-7 behind
    // "更多". The next visible tile is Office 1, at flat selection 6.
    assert!(
        visual.debug_bounds("spotlight-more-utilities").is_some(),
        "the Utilities header still advertises its hidden apps"
    );
    let state = view.read_with(&visual, |view, _| view.state.clone());
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_row_count(&state)),
        9,
        "keyboard navigation must not reach tiles hidden behind 更多"
    );
    for _ in 0..6 {
        visual.simulate_keystrokes("down");
    }
    visual.run_until_parked();
    assert_eq!(view.read_with(&visual, |view, _| view.selection), 6);

    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    settle_exit(&mut visual);
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.prefs.recent_app_ids.first().map(String::as_str),
            Some("office-1.desktop"),
            "Enter must launch the highlighted tile, not a hidden Utility"
        );
    });
}

#[gpui::test]
fn a_bare_slash_needs_navigation_before_enter_runs_a_command(cx: &mut TestAppContext) {
    let (_fixture, _host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.simulate_input("/");
    visual.run_until_parked();

    // Enter right after typing `/` explains the syntax instead of running the
    // first row, so an accidental Enter never executes a command.
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    view.read_with(&visual, |view, _| {
        assert!(view.palette_error.is_some());
        assert_eq!(view.query, "/");
    });

    // Once the keyboard walked the list, Enter runs the highlighted row.
    visual.simulate_keystrokes("down");
    visual.run_until_parked();
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    view.read_with(&visual, |view, _| {
        assert!(view.palette_error.is_none(), "the highlighted command ran");
        assert_ne!(view.query, "/", "the command applied to the session");
    });
}

#[gpui::test]
fn slash_routes_swap_the_header_glyph(cx: &mut TestAppContext) {
    let (_fixture, _host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.run_until_parked();
    assert_eq!(
        header_mark(&view, &mut visual),
        "software-store-symbolic.svg",
        "the store mark rests"
    );

    visual.simulate_input("/lock");
    visual.run_until_parked();
    assert_eq!(
        header_mark(&view, &mut visual),
        "explore-symbolic.svg",
        "a draft that composes a command takes the command mark"
    );

    for _ in 0..5 {
        visual.simulate_keystrokes("backspace");
    }
    visual.run_until_parked();
    assert_eq!(
        header_mark(&view, &mut visual),
        "software-store-symbolic.svg",
        "the glyph rides back"
    );
}

#[gpui::test]
fn the_command_list_keeps_the_command_mark(cx: &mut TestAppContext) {
    let (_fixture, _host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.simulate_input("/command");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        header_mark(&view, &mut visual),
        "layers-button-symbolic.svg",
        "the command list leads with the mode's own mark, not the store mark"
    );
    assert_eq!(
        view.read_with(&visual, |view, _| view.query.clone()),
        "",
        "the mode switch consumed the draft"
    );
}

#[gpui::test]
fn the_header_mark_names_the_mode_not_the_selection(cx: &mut TestAppContext) {
    let (_fixture, _host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.simulate_input("/");
    visual.run_until_parked();
    assert_eq!(header_mark(&view, &mut visual), "explore-symbolic.svg");
    // The completion list narrows to the clipboard command, but the header is
    // about the mode the sheet is in, so the row under the keyboard does not
    // move it.
    visual.simulate_input("clipboard");
    visual.run_until_parked();
    assert_eq!(
        header_mark(&view, &mut visual),
        "explore-symbolic.svg",
        "the selection does not move the header"
    );

    // Entering the clipboard mode does: the mark stays for as long as the user
    // keeps searching there.
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        header_mark(&view, &mut visual),
        "category-search-symbolic.svg",
        "the clipboard mode takes its own mark"
    );
    visual.simulate_input("abc");
    visual.run_until_parked();
    assert_eq!(
        header_mark(&view, &mut visual),
        "category-search-symbolic.svg",
        "and keeps it while the mode is searched"
    );

    // The applications grid is the store mark's home.
    let (_fixture, _host, apps_view, mut apps_visual, _window) = panel(cx, Panel::Spotlight);
    apps_visual.simulate_keystrokes("ctrl-1");
    apps_visual.run_until_parked();
    assert_eq!(
        header_mark(&apps_view, &mut apps_visual),
        "software-store-symbolic.svg",
        "the application mode rests on the store mark"
    );
}

#[test]
fn clipboard_commands_lead_with_the_search_mark() {
    assert_eq!(
        crate::view::spotlight::command_symbol("shell.recent"),
        Some("category-search-symbolic")
    );
    assert_eq!(
        crate::view::spotlight::command_symbol("mode.clipboard"),
        Some("category-search-symbolic")
    );
    assert_eq!(
        crate::view::spotlight::command_symbol("mode.wallpapers"),
        Some("image-round-symbolic")
    );
    assert_eq!(
        crate::view::spotlight::command_symbol("mode.files"),
        Some("folder-symbolic")
    );
}

#[gpui::test]
fn the_clipboard_mode_copies_the_highlighted_entry(cx: &mut TestAppContext) {
    use crate::services::{Command, HistoryKind};
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    let received = cx.update(|cx| testing::capture_service(&host, cx));
    host.update(cx, |host, cx| {
        host.history = vec![
            clipboard_entry("text:newer", HistoryKind::Text, "newer", 2),
            clipboard_entry("text:older", HistoryKind::Text, "older", 1),
        ];
        host.clipboard_available = true;
        host.changed(cx);
    });

    visual.simulate_input("/clipboard");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("spotlight-clip-text:newer").is_some(),
        "the clipboard mode paints its rows"
    );

    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    match received.try_recv().expect("the copy reaches the service") {
        Command::Copy(id) => assert_eq!(id, "text:newer"),
        other => panic!("expected a copy command, got {other:?}"),
    }
    settle_exit(&mut visual);
    host.read_with(cx, |host, _| {
        assert_eq!(host.panel, None, "a pick folds the sheet away");
    });
}

#[gpui::test]
fn shift_enter_copies_and_keeps_the_clipboard_open(cx: &mut TestAppContext) {
    use crate::services::{Command, HistoryKind};
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    let received = cx.update(|cx| testing::capture_service(&host, cx));
    host.update(cx, |host, cx| {
        host.history = vec![clipboard_entry("text:one", HistoryKind::Text, "one", 1)];
        host.clipboard_available = true;
        host.changed(cx);
    });

    visual.simulate_input("/clipboard");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.simulate_keystrokes("shift-enter");
    visual.run_until_parked();
    assert!(matches!(received.try_recv(), Ok(Command::Copy(_))));
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.panel,
            Some(Panel::Spotlight),
            "Shift+Enter keeps the sheet up for the next pick"
        );
        assert!(!host.panel_exit);
    });
}

#[gpui::test]
fn delete_drops_the_highlighted_clipboard_row(cx: &mut TestAppContext) {
    use crate::services::{Command, HistoryKind};
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    let received = cx.update(|cx| testing::capture_service(&host, cx));
    host.update(cx, |host, cx| {
        host.history = vec![
            clipboard_entry("text:newer", HistoryKind::Text, "newer", 2),
            clipboard_entry("text:older", HistoryKind::Text, "older", 1),
        ];
        host.clipboard_available = true;
        host.changed(cx);
    });

    visual.simulate_input("/clipboard");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.simulate_keystrokes("delete");
    visual.run_until_parked();
    match received.try_recv().expect("the delete reaches the service") {
        Command::Delete(id) => assert_eq!(id, "text:newer"),
        other => panic!("expected a delete command, got {other:?}"),
    }
    assert_eq!(
        view.read_with(&visual, |view, _| view.selection),
        0,
        "the keyboard lands on the neighbour"
    );
    host.read_with(cx, |host, _| {
        assert_eq!(host.panel, Some(Panel::Spotlight), "Delete does not close");
    });
}

#[gpui::test]
fn the_clipboard_details_layout_previews_the_selected_entry(cx: &mut TestAppContext) {
    use crate::services::HistoryKind;
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    host.update(cx, |host, cx| {
        host.history = vec![
            clipboard_entry("text:newer", HistoryKind::Text, "newer", 2),
            clipboard_entry("text:older", HistoryKind::Text, "older", 1),
        ];
        host.clipboard_available = true;
        host.changed(cx);
    });

    visual.simulate_input("/clipboard");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("spotlight-clipboard-list").is_none(),
        "the compact layout keeps one column"
    );

    visual.simulate_input("/detail");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("spotlight-clipboard-list").is_some(),
        "the details layout adds the list column"
    );
    assert!(
        visual.debug_bounds("spotlight-clipboard-text").is_some(),
        "and previews the selected text entry"
    );
}

#[gpui::test]
fn the_clipboard_layout_chip_flips_the_columns(cx: &mut TestAppContext) {
    use crate::services::HistoryKind;
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    host.update(cx, |host, cx| {
        host.history = vec![clipboard_entry("text:one", HistoryKind::Text, "one", 1)];
        host.clipboard_available = true;
        host.changed(cx);
    });
    visual.simulate_input("/clipboard");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("spotlight-clipboard-list").is_none(),
        "the clipboard starts on the compact layout"
    );

    // The chip next to the count is the clickable half of `/detail`: the
    // override commands are not listable, so this is how the layout is found.
    let chip = visual
        .debug_bounds("spotlight-clipboard-layout")
        .expect("the layout chip is painted");
    visual.simulate_click(chip.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("spotlight-clipboard-list").is_some(),
        "the chip opens the details columns"
    );
    assert!(
        visual.debug_bounds("spotlight-clipboard-text").is_some(),
        "and previews the selected entry"
    );

    let chip = visual
        .debug_bounds("spotlight-clipboard-layout")
        .expect("the chip stays painted in the details layout");
    visual.simulate_click(chip.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("spotlight-clipboard-list").is_none(),
        "a second click folds it back to the compact list"
    );
}

#[gpui::test]
fn clipboard_type_tags_filter_by_content_kind(cx: &mut TestAppContext) {
    use crate::services::HistoryKind;
    use crate::view::spotlight::ClipboardFilter;
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    host.update(cx, |host, cx| {
        host.history = vec![
            clipboard_entry("text:a", HistoryKind::Text, "alpha", 3),
            clipboard_entry("image:b", HistoryKind::Image, "capture.png", 2),
            clipboard_entry("files:c", HistoryKind::Files, "/tmp/c.txt", 1),
            clipboard_blob_entry("blob:d", "video/mp4", 2_400_000, 4),
        ];
        host.clipboard_available = true;
        host.changed(cx);
    });

    visual.simulate_input("/clipboard");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(visual.debug_bounds("spotlight-clip-text:a").is_some());
    assert!(visual.debug_bounds("spotlight-clip-image:b").is_some());
    assert!(visual.debug_bounds("spotlight-clip-files:c").is_some());
    assert!(visual.debug_bounds("spotlight-clip-blob:d").is_some());
    assert!(visual.debug_bounds("spotlight-clip-tag-text").is_some());
    assert!(visual.debug_bounds("spotlight-clip-tag-image").is_some());
    assert!(visual.debug_bounds("spotlight-clip-tag-files").is_some());

    // Alt+T keeps only the text entries; the shortcut is the same switch the
    // tag itself makes.
    visual.simulate_keystrokes("alt-t");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_clipboard_filter),
        Some(ClipboardFilter::Text)
    );
    assert!(visual.debug_bounds("spotlight-clip-text:a").is_some());
    assert!(
        visual.debug_bounds("spotlight-clip-image:b").is_none(),
        "the image row leaves the list"
    );
    assert!(visual.debug_bounds("spotlight-clip-files:c").is_none());

    // A repeated shortcut clears the tag again.
    visual.simulate_keystrokes("alt-t");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_clipboard_filter),
        None
    );
    assert!(visual.debug_bounds("spotlight-clip-image:b").is_some());

    visual.simulate_keystrokes("alt-i");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_clipboard_filter),
        Some(ClipboardFilter::Image)
    );
    assert!(visual.debug_bounds("spotlight-clip-image:b").is_some());
    assert!(visual.debug_bounds("spotlight-clip-text:a").is_none());

    visual.simulate_keystrokes("alt-f");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_clipboard_filter),
        Some(ClipboardFilter::Files)
    );
    assert!(visual.debug_bounds("spotlight-clip-files:c").is_some());
    assert!(
        visual.debug_bounds("spotlight-clip-blob:d").is_some(),
        "an opaque video rides the file tag"
    );
    assert!(visual.debug_bounds("spotlight-clip-image:b").is_none());

    // The tag itself is clickable and toggles off.
    let tag = visual
        .debug_bounds("spotlight-clip-tag-files")
        .expect("the files tag is painted");
    visual.simulate_click(tag.center(), Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_clipboard_filter),
        None,
        "a second press on the tag shows every type"
    );
    assert!(visual.debug_bounds("spotlight-clip-text:a").is_some());
}

#[gpui::test]
fn the_clipboard_details_preview_an_opaque_payload(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    host.update(cx, |host, cx| {
        host.history = vec![clipboard_blob_entry(
            "blob:video",
            "video/mp4",
            2_400_000,
            1,
        )];
        host.clipboard_available = true;
        host.changed(cx);
    });
    visual.simulate_input("/clipboard");
    visual.simulate_keystrokes("enter");
    visual.simulate_input("/detail");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("spotlight-clipboard-binary").is_some(),
        "the preview names the payload class instead of an empty text block"
    );
    assert!(
        visual.debug_bounds("spotlight-clipboard-text").is_none(),
        "there are no words to show"
    );

    // The compact row copies it back unchanged.
    visual.simulate_input("/compact");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(visual.debug_bounds("spotlight-clip-blob:video").is_some());
}

#[gpui::test]
fn clipboard_shortcuts_only_apply_inside_the_clipboard_mode(cx: &mut TestAppContext) {
    let (_fixture, _host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.run_until_parked();
    visual.simulate_keystrokes("alt-t");
    visual.simulate_keystrokes("alt-d");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_clipboard_filter),
        None,
        "the type shortcuts belong to the clipboard body"
    );
    assert_eq!(
        view.read_with(&visual, |view, _| {
            view.palette.effective("clipboardLayout", "default")
        }),
        "default",
        "so does the layout switch"
    );

    // Typing the command is not entering the mode either.
    visual.simulate_input("/clipboard");
    visual.run_until_parked();
    visual.simulate_keystrokes("alt-t");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_clipboard_filter),
        None,
        "the slash body is still the search mode"
    );
}

#[gpui::test]
fn alt_d_flips_the_clipboard_layout(cx: &mut TestAppContext) {
    use crate::services::HistoryKind;
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    host.update(cx, |host, cx| {
        host.history = vec![clipboard_entry("text:one", HistoryKind::Text, "one", 1)];
        host.clipboard_available = true;
        host.changed(cx);
    });
    visual.simulate_input("/clipboard");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(visual.debug_bounds("spotlight-clipboard-list").is_none());

    visual.simulate_keystrokes("alt-d");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("spotlight-clipboard-list").is_some(),
        "Alt+D opens the details columns"
    );
    assert!(visual.debug_bounds("spotlight-clipboard-text").is_some());

    visual.simulate_keystrokes("alt-d");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("spotlight-clipboard-list").is_none(),
        "and folds them away again"
    );
}

#[gpui::test]
fn ctrl_j_k_and_ctrl_n_p_walk_the_palette_lists(cx: &mut TestAppContext) {
    let (_fixture, _host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.simulate_keystrokes("ctrl-1");
    visual.run_until_parked();
    let selection =
        |visual: &mut VisualTestContext| view.read_with(visual, |view, _| view.selection);
    assert_eq!(selection(&mut visual), 0);

    visual.simulate_keystrokes("ctrl-j");
    visual.run_until_parked();
    assert_eq!(selection(&mut visual), 1, "Ctrl+J steps down like Down");
    visual.simulate_keystrokes("ctrl-n");
    visual.run_until_parked();
    assert_eq!(selection(&mut visual), 2, "Ctrl+N steps down like Down");
    visual.simulate_keystrokes("ctrl-k");
    visual.run_until_parked();
    assert_eq!(selection(&mut visual), 1, "Ctrl+K steps up like Up");
    visual.simulate_keystrokes("ctrl-p");
    visual.run_until_parked();
    assert_eq!(selection(&mut visual), 0, "Ctrl+P steps up like Up");
}

#[gpui::test]
fn spotlight_app_list_rows_use_the_roomier_list_geometry(cx: &mut TestAppContext) {
    let (_fixture, _host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.simulate_keystrokes("ctrl-1");
    visual.simulate_input("/list");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();

    let row = visual
        .debug_bounds("spotlight-app-alpha.desktop")
        .expect("the application list paints alpha");
    assert_eq!(f32::from(row.size.height), 54.);
}

#[gpui::test]
fn spotlight_rules_are_hairlines(cx: &mut TestAppContext) {
    let apps = [
        ("utility-1", "Utility 1", "Utility;"),
        ("office-1", "Office 1", "Office;"),
    ];
    let (_fixture, _host, _view, mut visual, _window) =
        panel_with_apps(cx, Panel::Spotlight, &apps);

    for selector in ["spotlight-rule-search", "spotlight-rule-section"] {
        let line = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is painted"));
        assert_eq!(f32::from(line.size.height), 1.);
    }
}

#[gpui::test]
fn command_rows_have_a_leading_icon_and_a_slash_subtitle(cx: &mut TestAppContext) {
    let (_fixture, _host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.simulate_input(">dark");
    visual.run_until_parked();

    let row = visual
        .debug_bounds("spotlight-command-theme.dark")
        .expect("the dark-theme command is painted");
    assert_eq!(f32::from(row.size.height), 54.);

    let icon = visual
        .debug_bounds("spotlight-command-icon-theme.dark")
        .expect("the command carries a leading icon");
    assert_eq!(icon.size, gpui::size(px(36.), px(36.)));
    let subtitle = visual
        .debug_bounds("spotlight-command-subtitle-theme.dark")
        .expect("the slash command is the subtitle");
    assert!(icon.origin.x < subtitle.origin.x);
}

#[gpui::test]
fn selecting_a_category_tag_filters_the_grid_tiles(cx: &mut TestAppContext) {
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    // The three fixture applications carry no XDG categories, so they all sit
    // in "其他": their tiles paint, and the "其他" tag is the only one offered.
    let tag = visual
        .debug_bounds("spotlight-tag-other")
        .expect("the only present category offers a tag");
    assert!(visual.debug_bounds("spotlight-tile-beta.desktop").is_some());
    assert!(visual.debug_bounds("spotlight-tag-utilities").is_none());

    // Clicking the tag selects its category — the user-visible path.
    let center = tag.center();
    visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::none());
    visual.run_until_parked();
    view.read_with(&visual, |view, _| {
        assert_eq!(
            view.palette_category,
            Some(clarion_launcher::catalog::AppCategory::Other),
            "clicking a tag selects its category"
        );
    });

    // A tag for a category the catalog has no applications in must empty the
    // grid — the filter used to change the layout but leave every tile.
    visual.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.palette_toggle_category(clarion_launcher::catalog::AppCategory::Utilities, cx)
        });
    });
    let state = view.read_with(&visual, |view, _| view.state.clone());
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_row_count(&state)),
        0,
        "an empty category tag filters every application away"
    );

    // Picking the category the applications do belong to shows them flat.
    visual.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.palette_toggle_category(clarion_launcher::catalog::AppCategory::Other, cx)
        });
    });
    let state = view.read_with(&visual, |view, _| view.state.clone());
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_row_count(&state)),
        3
    );
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.panel,
            Some(Panel::Spotlight),
            "tag filtering keeps the palette open"
        );
    });
}

#[gpui::test]
fn the_wide_grid_fits_six_tiles_on_one_row(cx: &mut TestAppContext) {
    let apps = [
        ("t1", "Tile 1", "Utility;"),
        ("t2", "Tile 2", "Utility;"),
        ("t3", "Tile 3", "Utility;"),
        ("t4", "Tile 4", "Utility;"),
        ("t5", "Tile 5", "Utility;"),
        ("t6", "Tile 6", "Utility;"),
    ];
    let (_fixture, _host, _view, mut visual, _window) =
        panel_with_apps(cx, Panel::Spotlight, &apps);
    visual.run_until_parked();

    let mut tiles = Vec::new();
    for selector in [
        "spotlight-tile-t1.desktop",
        "spotlight-tile-t2.desktop",
        "spotlight-tile-t3.desktop",
        "spotlight-tile-t4.desktop",
        "spotlight-tile-t5.desktop",
        "spotlight-tile-t6.desktop",
    ] {
        let tile = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is painted"));
        assert_eq!(f32::from(tile.size.width), 128.);
        tiles.push(tile);
    }
    let row = tiles[0].origin.y;
    assert!(
        tiles.iter().all(|tile| tile.origin.y == row),
        "all six tiles sit on the same row"
    );
    assert!(
        tiles
            .windows(2)
            .all(|pair| pair[0].origin.x < pair[1].origin.x),
        "the row lays the tiles out left to right"
    );
}

#[gpui::test]
fn ctrl_b_opens_a_dynamic_action_card_for_the_selection(cx: &mut TestAppContext) {
    use crate::services::HistoryKind;
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    host.update(cx, |host, cx| {
        host.history = vec![clipboard_entry("text:one", HistoryKind::Text, "one", 1)];
        host.clipboard_available = true;
        host.changed(cx);
    });
    visual.run_until_parked();

    assert!(
        visual.debug_bounds("spotlight-status-bar").is_some(),
        "the sheet always carries its status strip"
    );
    assert!(visual.debug_bounds("spotlight-actions-menu").is_none());

    visual.simulate_keystrokes("ctrl-b");
    visual.run_until_parked();
    let (menu_window, mut menu) = spotlight_menu(&host, cx);
    assert!(menu.debug_bounds("spotlight-actions-menu").is_some());
    assert!(
        menu.debug_bounds("spotlight-action-open").is_some(),
        "an application offers its primary launch action"
    );
    assert!(
        menu.debug_bounds("spotlight-action-reveal").is_some(),
        "and can reveal its desktop entry"
    );
    assert!(
        menu.debug_bounds("spotlight-action-copy").is_none(),
        "clipboard actions do not leak into the application page"
    );

    // Up/Down walk the card, not the palette underneath.
    menu.simulate_keystrokes("down");
    menu.run_until_parked();
    assert_eq!(
        menu_window
            .update(cx, |view, _, _| view.spotlight_menu_selection)
            .unwrap(),
        1
    );
    assert_eq!(
        view.read_with(&visual, |view, _| view.selection),
        0,
        "the highlighted application stays put"
    );

    // Ctrl+J / Ctrl+K walk the card with the same reach as the arrow keys.
    menu.simulate_keystrokes("ctrl-k");
    menu.run_until_parked();
    assert_eq!(
        menu_window
            .update(cx, |view, _, _| view.spotlight_menu_selection)
            .unwrap(),
        0,
        "Ctrl+K steps back up the card"
    );
    menu.simulate_keystrokes("ctrl-j");
    menu.run_until_parked();
    assert_eq!(
        menu_window
            .update(cx, |view, _, _| view.spotlight_menu_selection)
            .unwrap(),
        1,
        "Ctrl+J steps down the card"
    );

    // The card is a second window now, so a sheet that still holds the
    // keyboard must forward the step instead of walking its hidden selection.
    visual.simulate_keystrokes("ctrl-k");
    visual.run_until_parked();
    assert_eq!(
        menu_window
            .update(cx, |view, _, _| view.spotlight_menu_selection)
            .unwrap(),
        0,
        "the sheet forwards Ctrl+K to the card"
    );
    visual.simulate_keystrokes("ctrl-j");
    visual.run_until_parked();
    assert_eq!(
        menu_window
            .update(cx, |view, _, _| view.spotlight_menu_selection)
            .unwrap(),
        1,
        "and forwards Ctrl+J too"
    );

    // Escape folds only the card.
    menu.simulate_keystrokes("escape");
    menu.run_until_parked();
    assert!(host.read_with(cx, |host, _| host.spotlight_menu.is_none()));
    assert!(visual.debug_bounds("spotlight-status-bar").is_some());
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.panel,
            Some(Panel::Spotlight),
            "the launcher stays up behind the card"
        );
    });

    // The clipboard body derives a different list from its own selection.
    visual.simulate_keystrokes("ctrl-3");
    visual.run_until_parked();
    visual.simulate_keystrokes("ctrl-b");
    visual.run_until_parked();
    let (_, mut menu) = spotlight_menu(&host, cx);
    assert!(menu.debug_bounds("spotlight-action-copy").is_some());
    assert!(menu.debug_bounds("spotlight-action-pin").is_some());
    assert!(menu.debug_bounds("spotlight-action-delete").is_some());
    assert!(menu.debug_bounds("spotlight-action-layout").is_some());
    assert!(menu.debug_bounds("spotlight-action-open").is_none());
}

#[gpui::test]
fn the_action_card_runs_the_clicked_row(cx: &mut TestAppContext) {
    use crate::services::HistoryKind;
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    host.update(cx, |host, cx| {
        host.history = vec![clipboard_entry("text:one", HistoryKind::Text, "one", 1)];
        host.clipboard_available = true;
        host.changed(cx);
    });
    visual.simulate_keystrokes("ctrl-3");
    visual.simulate_keystrokes("ctrl-b");
    visual.run_until_parked();

    let (_, mut menu) = spotlight_menu(&host, cx);
    let row = menu
        .debug_bounds("spotlight-action-layout")
        .expect("the details switch is offered");
    let center = row.center();
    menu.simulate_mouse_down(center, MouseButton::Left, Modifiers::none());
    menu.simulate_mouse_up(center, MouseButton::Left, Modifiers::none());
    menu.run_until_parked();

    view.read_with(&visual, |view, _| {
        assert_eq!(
            view.palette.effective("clipboardLayout", "default"),
            "details",
            "the clicked row runs the same command as Alt+D"
        );
        assert!(!view.spotlight_menu_open, "the card folds after a run");
    });
    assert!(
        host.read_with(cx, |host, _| host.spotlight_menu.is_none()),
        "the card surface folds away after a run"
    );
    assert!(
        visual.debug_bounds("spotlight-clipboard-list").is_some(),
        "and the sheet repaints into the details layout"
    );
}

#[gpui::test]
fn the_status_strip_is_full_bleed_and_balanced(cx: &mut TestAppContext) {
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.run_until_parked();
    let bar = visual
        .debug_bounds("spotlight-status-bar")
        .expect("the sheet carries its status strip");
    let chip = visual
        .debug_bounds("spotlight-status-actions")
        .expect("the strip carries its action trigger");
    let title = visual
        .debug_bounds("spotlight-status-title")
        .expect("the strip names the selection");
    // The strip ignores the content margin: it spans the sheet's full inner
    // width so its top hairline reaches both rounded edges.
    assert_eq!(
        f32::from(bar.size.width),
        crate::app::panels::SPOTLIGHT_WIDTH - 4.,
        "the strip runs edge to edge inside the sheet rim"
    );
    let left_inset = f32::from(title.left()) - f32::from(bar.left());
    let right_inset = f32::from(bar.right()) - f32::from(chip.right());
    assert!(
        (left_inset - 24.).abs() <= 0.5 && (right_inset - 24.).abs() <= 0.5,
        "the strip's contents keep the content margin ({left_inset} / {right_inset})"
    );
    let divider = 1.;
    let above = f32::from(chip.top()) - f32::from(bar.top()) - divider;
    let below = f32::from(bar.bottom()) - f32::from(chip.bottom());
    assert!(
        (above - below).abs() <= 1.,
        "the trigger sits centred in the strip ({above} above, {below} below)"
    );

    visual.simulate_keystrokes("ctrl-b");
    visual.run_until_parked();
    let (_, mut menu) = spotlight_menu(&host, cx);
    let card = menu
        .debug_bounds("spotlight-actions-menu")
        .expect("Ctrl+B opens the card");
    assert!(
        (f32::from(card.right()) - f32::from(chip.right())).abs() <= 0.5,
        "the card lines up with the trigger's right edge"
    );
    // Both windows cover the same output, so their coordinates share an origin.
    // The strip itself is still riding its opening spring at this point, so
    // compare against its settled top from the placement: the card keeps an
    // 8px gap above the strip and the same 24px inset as its control.
    let state = view.read_with(&visual, |view, _| view.state.clone());
    let viewport = visual.update(|window, _| window.viewport_size());
    let popup = crate::app::panels::placement(
        &state,
        &Panel::Spotlight,
        f32::from(viewport.width),
        f32::from(viewport.height),
    )
    .popup
    .expect("the sheet has a visible rectangle");
    let strip_top = f32::from(popup.origin.y) + f32::from(popup.size.height) - 2. - 42.;
    assert!(
        (f32::from(card.bottom()) - (strip_top - 8.)).abs() <= 0.5,
        "the card floats 8px above the strip ({} vs {})",
        f32::from(card.bottom()),
        strip_top
    );
    assert!(
        (f32::from(card.right())
            - (f32::from(popup.origin.x) + f32::from(popup.size.width) - 2. - 24.))
            .abs()
            <= 0.5,
        "the card's right edge sits on the strip's content inset"
    );
}

#[gpui::test]
fn the_resting_grid_hides_the_rule_below_its_third_row(cx: &mut TestAppContext) {
    let apps = [
        ("util-1", "Utility One", "Utility;"),
        ("util-2", "Utility Two", "Utility;"),
        ("office-1", "Office One", "Office;"),
        ("office-2", "Office Two", "Office;"),
        ("social-1", "Social One", "Network;"),
        ("social-2", "Social Two", "Network;"),
        ("graphics-1", "Graphics One", "Graphics;"),
        ("graphics-2", "Graphics Two", "Graphics;"),
    ];
    let (_fixture, _host, _view, mut visual, _window) =
        panel_with_apps(cx, Panel::Spotlight, &apps);
    visual.run_until_parked();

    let bar = visual
        .debug_bounds("spotlight-status-bar")
        .expect("the sheet carries its status strip");
    // The results scroll ends one body gap above the strip.
    let viewport_bottom = f32::from(bar.top()) - 8.;
    let third_row = visual
        .debug_bounds("spotlight-tile-social-1.desktop")
        .expect("the third category row is painted");
    assert!(
        f32::from(third_row.bottom()) <= viewport_bottom,
        "the third row ({}) stays above the viewport edge ({viewport_bottom})",
        f32::from(third_row.bottom())
    );
    let rule = visual
        .debug_bounds("spotlight-rule-section")
        .expect("the last category divider is painted");
    assert!(
        f32::from(rule.top()) >= viewport_bottom,
        "the rule below the third row ({}) stays clipped behind the strip edge ({viewport_bottom})",
        f32::from(rule.top())
    );
}

#[gpui::test]
fn the_file_action_card_previews_the_selected_file(cx: &mut TestAppContext) {
    use clarion_launcher::files::FileHit;
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    let commands = cx.update(|cx| testing::capture_backend(&host, cx));
    visual.simulate_keystrokes("ctrl-4");
    visual.run_until_parked();

    let image = std::path::PathBuf::from("/home/test/Pictures/photo.png");
    host.update(cx, |host, cx| {
        host.file_query = "photo".into();
        host.file_hits = Arc::new(vec![FileHit {
            path: image.clone(),
            name: "photo.png".into(),
            parent: image.parent().unwrap().to_path_buf(),
            is_dir: false,
            extension: "png".into(),
            size: 1024,
            modified: None,
        }]);
        host.changed(cx);
    });
    visual.run_until_parked();

    visual.simulate_keystrokes("ctrl-b");
    visual.run_until_parked();
    let (_, mut menu) = spotlight_menu(&host, cx);
    assert!(
        menu.debug_bounds("spotlight-action-reveal").is_some(),
        "a file keeps its reveal action"
    );
    let row = menu
        .debug_bounds("spotlight-action-preview")
        .expect("images and files offer the quick preview");
    let center = row.center();
    menu.simulate_mouse_down(center, MouseButton::Left, Modifiers::none());
    menu.simulate_mouse_up(center, MouseButton::Left, Modifiers::none());
    menu.run_until_parked();

    let mut previewed = None;
    while let Ok(command) = commands.try_recv() {
        if let backend::Command::PreviewFile(path) = command {
            previewed = Some(path);
        }
    }
    assert_eq!(
        previewed.as_deref(),
        Some(image.as_path()),
        "the row asks Sushi for the selected file"
    );
    host.read_with(cx, |host, _| {
        assert!(host.panel_exit, "the sheet folds away for the previewer");
    });
}

#[gpui::test]
fn a_copied_file_offers_open_and_preview_in_the_clipboard_card(cx: &mut TestAppContext) {
    use crate::services::HistoryKind;
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    let commands = cx.update(|cx| testing::capture_backend(&host, cx));
    let file = std::path::PathBuf::from("/home/test/Downloads/report.pdf");
    let mut entry = clipboard_entry(
        "files:one",
        HistoryKind::Files,
        &file.display().to_string(),
        1,
    );
    entry.file_path = Some(file.clone());
    host.update(cx, |host, cx| {
        host.history = vec![entry];
        host.clipboard_available = true;
        host.changed(cx);
    });
    visual.simulate_keystrokes("ctrl-3");
    visual.simulate_keystrokes("ctrl-b");
    visual.run_until_parked();

    let (_, mut menu) = spotlight_menu(&host, cx);
    assert!(
        menu.debug_bounds("spotlight-action-preview").is_some(),
        "a copied file can be previewed too"
    );
    let row = menu
        .debug_bounds("spotlight-action-open-file")
        .expect("a file-list capture offers its own open action");
    let center = row.center();
    menu.simulate_mouse_down(center, MouseButton::Left, Modifiers::none());
    menu.simulate_mouse_up(center, MouseButton::Left, Modifiers::none());
    menu.run_until_parked();

    let mut opened = None;
    while let Ok(command) = commands.try_recv() {
        if let backend::Command::OpenPath(path) = command {
            opened = Some(path);
        }
    }
    assert_eq!(
        opened.as_deref(),
        Some(file.as_path()),
        "opening the copied file follows the desktop handler"
    );
    host.read_with(cx, |host, _| {
        assert!(host.panel_exit, "the sheet folds away after opening");
    });
}

#[gpui::test]
fn escape_in_the_launcher_menu_folds_only_the_menu(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    let tile = point(px(450.), px(320.));
    visual.simulate_mouse_down(tile, MouseButton::Right, Modifiers::none());
    visual.simulate_mouse_up(tile, MouseButton::Right, Modifiers::none());
    visual.run_until_parked();
    let menu = host
        .read_with(cx, |host, _| host.menu)
        .expect("right-clicking a tile opens its application menu");
    let mut menu_visual = VisualTestContext::from_window(menu.into(), cx);
    menu_visual.simulate_keystrokes("escape");
    menu_visual.run_until_parked();
    assert!(
        host.read_with(cx, |host, _| host.menu.is_none()),
        "escape folds the menu away"
    );
    host.read_with(cx, |host, _| {
        assert_eq!(host.panel, Some(Panel::Spotlight), "the launcher stays up");
        assert!(!host.panel_exit, "the launcher is not on its way out");
    });
    // The menu gave the keyboard back, so the launcher still closes.
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    host.read_with(cx, |host, _| assert!(host.panel_exit));
}

#[gpui::test]
fn right_clicking_a_launchpad_tile_keeps_the_launchpad_closeable(cx: &mut TestAppContext) {
    let (_fixture, host, _view, mut visual, window) = panel(cx, Panel::Apps);
    // The first tile of the fullscreen grid, wherever the page puts it.
    let first = _view
        .read_with(&visual, |view, _| {
            view.state
                .catalog
                .search("")
                .first()
                .map(|app| app.id.clone())
        })
        .expect("the catalog has fixture apps");
    let selector: &'static str = Box::leak(format!("launchpad-app-{first}").into_boxed_str());
    let tile = visual
        .debug_bounds(selector)
        .expect("the first tile is painted")
        .center();
    visual.simulate_mouse_down(tile, MouseButton::Right, Modifiers::none());
    visual.simulate_mouse_up(tile, MouseButton::Right, Modifiers::none());
    visual.run_until_parked();
    let menu = host
        .read_with(cx, |host, _| host.menu)
        .expect("right-clicking a tile opens its application menu");
    assert_ne!(menu.window_id(), window.window_id());
    host.read_with(cx, |host, _| assert_eq!(host.panel, Some(Panel::Apps)));
    // Escape in the menu folds it away and the launchpad stays up...
    let mut menu_visual = VisualTestContext::from_window(menu.into(), cx);
    menu_visual.simulate_keystrokes("escape");
    menu_visual.run_until_parked();
    assert!(host.read_with(cx, |host, _| host.menu.is_none()));
    host.read_with(cx, |host, _| assert_eq!(host.panel, Some(Panel::Apps)));
    // ...so the next escape can still close the launcher. The surface rides
    // its fade out, so the panel stays visible until the Host releases it.
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert!(host.panel_exit, "the launchpad rides its fade out");
        assert_eq!(host.panel, Some(Panel::Apps));
    });
}

#[gpui::test]
fn mode_shortcuts_switch_all_four_pages_and_tab_walks_them(cx: &mut TestAppContext) {
    let (_fixture, _host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.run_until_parked();
    let mode = |visual: &mut VisualTestContext| {
        view.read_with(visual, |view, _| view.palette.mode)
    };

    visual.simulate_keystrokes("ctrl-2");
    visual.run_until_parked();
    assert_eq!(mode(&mut visual), Mode::Wallpapers);
    visual.simulate_keystrokes("ctrl-3");
    visual.run_until_parked();
    assert_eq!(mode(&mut visual), Mode::Clipboard);
    visual.simulate_keystrokes("ctrl-4");
    visual.run_until_parked();
    assert_eq!(mode(&mut visual), Mode::Files);

    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    assert_eq!(mode(&mut visual), Mode::Apps, "Tab wraps to the first button");
    visual.simulate_keystrokes("shift-tab");
    visual.run_until_parked();
    assert_eq!(mode(&mut visual), Mode::Files, "Shift+Tab walks backwards");

    visual.simulate_keystrokes("ctrl-0");
    visual.run_until_parked();
    assert_eq!(mode(&mut visual), Mode::Search);
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    assert_eq!(
        mode(&mut visual),
        Mode::Apps,
        "a mode outside the rail enters it at the front"
    );
}

#[gpui::test]
fn file_rows_render_and_enter_opens_while_ctrl_enter_reveals(cx: &mut TestAppContext) {
    use clarion_launcher::files::FileHit;
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    let commands = cx.update(|cx| testing::capture_backend(&host, cx));
    visual.simulate_keystrokes("ctrl-4");
    visual.run_until_parked();

    let report = std::path::PathBuf::from("/home/test/Documents/report.txt");
    let notes = std::path::PathBuf::from("/home/test/Documents/notes.md");
    host.update(cx, |host, cx| {
        host.file_query = "notes".into();
        host.file_hits = Arc::new(vec![
            FileHit {
                path: report.clone(),
                name: "report.txt".into(),
                parent: report.parent().unwrap().to_path_buf(),
                is_dir: false,
                extension: "txt".into(),
                size: 2048,
                modified: None,
            },
            FileHit {
                path: notes.clone(),
                name: "notes.md".into(),
                parent: notes.parent().unwrap().to_path_buf(),
                is_dir: false,
                extension: "md".into(),
                size: 512,
                modified: None,
            },
        ]);
        host.changed(cx);
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("spotlight-file-0").is_some());
    assert!(visual.debug_bounds("spotlight-file-1").is_some());
    assert_eq!(
        view.read_with(&visual, |view, _| view.query.clone()),
        "",
        "the fixture injected rows without typing"
    );

    visual.simulate_keystrokes("down");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let mut opened = None;
    while let Ok(command) = commands.try_recv() {
        if let backend::Command::OpenPath(path) = command {
            opened = Some(path);
        }
    }
    assert_eq!(opened.as_deref(), Some(notes.as_path()), "Enter opens the row");

    // Ctrl+Enter opens the containing folder instead.
    let (_fixture, host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    let commands = cx.update(|cx| testing::capture_backend(&host, cx));
    visual.simulate_keystrokes("ctrl-4");
    visual.run_until_parked();
    host.update(cx, |host, cx| {
        host.file_query = "report".into();
        host.file_hits = Arc::new(vec![FileHit {
            path: report.clone(),
            name: "report.txt".into(),
            parent: report.parent().unwrap().to_path_buf(),
            is_dir: false,
            extension: "txt".into(),
            size: 2048,
            modified: None,
        }]);
        host.changed(cx);
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("ctrl-enter");
    visual.run_until_parked();
    let mut revealed = None;
    while let Ok(command) = commands.try_recv() {
        if let backend::Command::OpenPath(path) = command {
            revealed = Some(path);
        }
    }
    assert_eq!(
        revealed.as_deref(),
        Some(std::path::Path::new("/home/test/Documents")),
        "Ctrl+Enter reveals the containing folder"
    );
}

#[gpui::test]
fn the_wallpaper_gallery_marks_the_current_image_and_clicking_applies_one(
    cx: &mut TestAppContext,
) {
    use crate::wallpapers::WallpaperEntry;
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    let current = std::path::PathBuf::from("/home/test/Pictures/current.png");
    let other = std::path::PathBuf::from("/home/test/Pictures/other.png");
    let entry = |path: &std::path::Path| WallpaperEntry {
        path: path.to_path_buf(),
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        parent: path.parent().unwrap().to_path_buf(),
        modified: None,
        thumbnail: None,
    };
    host.update(cx, |host, cx| {
        host.wallpapers = Arc::new(vec![entry(&current), entry(&other)]);
        host.current_wallpaper = Some(current.clone());
        host.changed(cx);
    });
    visual.simulate_keystrokes("ctrl-2");
    visual.run_until_parked();
    assert_eq!(view.read_with(&visual, |view, _| view.palette.mode), Mode::Wallpapers);
    assert!(visual.debug_bounds("spotlight-wallpaper-0").is_some());
    assert!(
        visual.debug_bounds("spotlight-wallpaper-current").is_some(),
        "the applied wallpaper carries the current mark"
    );

    let tile = visual
        .debug_bounds("spotlight-wallpaper-1")
        .expect("the second preview is painted")
        .center();
    visual.simulate_click(tile, Modifiers::default());
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert_eq!(host.current_wallpaper.as_deref(), Some(other.as_path()));
    });
}

/// One polkit request, reduced to the fields the dialog reads.
fn auth_request(cookie: &str) -> crate::services::AuthRequest {
    crate::services::AuthRequest {
        cookie: cookie.to_owned(),
        action_id: "org.freedesktop.policykit.exec".into(),
        message: "需要认证才能继续".into(),
        icon: Some("dialog-password-symbolic".into()),
        app_name: Some("终端".into()),
        app_icon: Some("utilities-terminal".into()),
        details: vec![
            ("polkit.caller-pid".into(), "1234".into()),
            ("动作".into(), "org.freedesktop.policykit.exec".into()),
        ],
        identity: crate::services::AuthIdentity {
            uid: 1000,
            username: "tester".into(),
        },
        queued: 0,
    }
}

/// Open the launcher first, then deliver a polkit request the way the service
/// thread does. The dialog is a second window over it.
fn open_auth_with(
    cx: &mut TestAppContext,
    request: crate::services::AuthRequest,
) -> (
    Fixture,
    Entity<Host>,
    VisualTestContext,
    WindowHandle<ShellView>,
) {
    let (fixture, host, _view, _visual, _window) = panel(cx, Panel::Spotlight);
    cx.update(|cx| {
        testing::service_event(
            &host,
            crate::services::Event::AuthRequest(request),
            cx,
        )
    });
    let window = cx.update(|cx| testing::auth_window(&host, cx));
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, _| {
        window.activate_window();
        window.refresh();
    });
    visual.run_until_parked();
    (fixture, host, visual, window)
}

fn open_auth(
    cx: &mut TestAppContext,
) -> (
    Fixture,
    Entity<Host>,
    VisualTestContext,
    WindowHandle<ShellView>,
) {
    open_auth_with(cx, auth_request("cookie-1"))
}

fn card_height(visual: &mut VisualTestContext) -> f32 {
    visual
        .debug_bounds("auth-card")
        .expect("the auth card is painted")
        .size
        .height
        .into()
}

fn assert_card_matches_glass(host: &Entity<Host>, visual: &mut VisualTestContext, cx: &mut TestAppContext) {
    let card = visual.debug_bounds("auth-card").expect("auth card");
    let viewport_width = visual.update(|window, _| f32::from(window.viewport_size().width));
    let state = cx.update(|cx| testing::snapshot(host, cx));
    let (expected_width, expected_height) = super::polkit::auth_card_size(&state, viewport_width);
    assert!((f32::from(card.size.width) - expected_width).abs() < 0.01);
    assert!((f32::from(card.size.height) - expected_height).abs() < 0.01);
}

#[gpui::test]
fn a_polkit_request_opens_the_modal_card_over_the_open_panel(cx: &mut TestAppContext) {
    let (_fixture, host, mut visual, window) = open_auth(cx);
    visual.run_until_parked();

    assert!(visual.debug_bounds("auth-card").is_some());
    assert!(visual.debug_bounds("auth-field").is_some());
    assert!(visual.debug_bounds("auth-submit").is_some());
    assert!(visual.debug_bounds("auth-cancel").is_some());
    assert!(
        visual.debug_bounds("auth-details").is_some(),
        "a non-technical detail gets its inset block"
    );

    // The compositor blurs a rounded region derived from the same size, so the
    // painted card must not drift from `auth_card_size`.
    let card = visual
        .debug_bounds("auth-card")
        .expect("the card is painted");
    let viewport_width = visual.update(|window, _| {
        let size = window.viewport_size();
        f32::from(size.width)
    });
    let state = cx.update(|cx| testing::snapshot(&host, cx));
    let (expected_width, expected_height) = super::polkit::auth_card_size(&state, viewport_width);
    assert!((f32::from(card.size.width) - expected_width).abs() < 0.01);
    assert!(
        (f32::from(card.size.height) - expected_height).abs() < 0.01,
        "the frosted region would miss the painted card"
    );
    assert!(
        f32::from(card.size.height) > f32::from(card.size.width),
        "the permission prompt is portrait, unlike the wide launcher surfaces"
    );

    // macOS alerts split a row evenly between the actions; both buttons share
    // one line and one width.
    let cancel = visual.debug_bounds("auth-cancel").expect("cancel button");
    let submit = visual.debug_bounds("auth-submit").expect("primary button");
    assert!((f32::from(cancel.origin.y) - f32::from(submit.origin.y)).abs() < 0.5);
    assert!(
        (f32::from(cancel.size.width) - f32::from(submit.size.width)).abs() < 0.5,
        "two buttons share the row equally"
    );
    assert!((f32::from(cancel.size.height) - f32::from(submit.size.height)).abs() < 0.5);
    assert!(
        (f32::from(submit.size.height) - super::polkit::AUTH_BUTTON_H).abs() < 0.5,
        "the buttons use the taller capsule metric"
    );

    host.read_with(cx, |host, _| {
        assert!(
            host.auth.is_some(),
            "the dialog owns the pending request"
        );
        assert_eq!(
            host.auth_window.map(|window| window.window_id()),
            Some(window.window_id())
        );
        assert_eq!(
            host.panel,
            Some(Panel::Spotlight),
            "a permission prompt must not discard the launcher underneath"
        );
    });
}

#[gpui::test]
fn the_secure_field_keeps_the_real_secret_and_enter_submits_it(cx: &mut TestAppContext) {
    let (_fixture, host, mut visual, window) = open_auth(cx);
    let service = cx.update(|cx| testing::capture_service(&host, cx));
    visual.run_until_parked();

    let input = window
        .update(cx, |view, _, _| view.input.clone())
        .unwrap();
    visual.simulate_input("hunter2");
    visual.run_until_parked();
    input.read_with(cx, |input, _| {
        assert_eq!(
            input.text(),
            "hunter2",
            "the model carries the real password, not bullets"
        );
        assert_eq!(
            input.shaped_text().as_deref(),
            Some("●●●●●●●"),
            "the renderer must shape one bullet per character"
        );
    });

    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    match service.try_recv().expect("Enter sends one password") {
        crate::services::Command::AuthSubmit { cookie, secret } => {
            assert_eq!(cookie, "cookie-1");
            assert_eq!(secret, "hunter2");
        }
        other => panic!("unexpected command: {other:?}"),
    }
    input.read_with(cx, |input, _| {
        assert_eq!(input.text(), "", "the field is cleared after submitting");
    });
    host.read_with(cx, |host, _| {
        assert!(
            host.auth.as_ref().expect("still pending").submitting,
            "the card waits for the helper's verdict"
        );
    });
}

#[gpui::test]
fn escape_cancels_the_request_and_removes_the_dialog(cx: &mut TestAppContext) {
    let (_fixture, host, mut visual, window) = open_auth(cx);
    let service = cx.update(|cx| testing::capture_service(&host, cx));
    visual.run_until_parked();

    visual.simulate_keystrokes("escape");
    settle_exit(&mut visual);

    match service.try_recv().expect("Escape cancels the request") {
        crate::services::Command::AuthCancel { cookie } => assert_eq!(cookie, "cookie-1"),
        other => panic!("unexpected command: {other:?}"),
    }
    host.read_with(cx, |host, _| {
        assert!(host.auth.is_none());
        assert!(host.auth_window.is_none());
        assert_eq!(
            host.panel,
            Some(Panel::Spotlight),
            "cancelling a prompt leaves the panel alone"
        );
    });
    assert!(
        !cx.read(|cx| cx
            .windows()
            .iter()
            .any(|handle| handle.window_id() == window.window_id())),
        "the dialog surface is really gone"
    );
}

#[gpui::test]
fn a_closed_request_dismisses_only_the_dialog(cx: &mut TestAppContext) {
    let (_fixture, host, mut visual, window) = open_auth(cx);
    visual.run_until_parked();

    cx.update(|cx| {
        testing::service_event(
            &host,
            crate::services::Event::AuthClosed {
                cookie: "cookie-1".into(),
            },
            cx,
        )
    });
    settle_exit(&mut visual);

    host.read_with(cx, |host, _| {
        assert!(host.auth.is_none(), "a finished request closes the card");
        assert!(host.auth_window.is_none());
        assert_eq!(host.panel, Some(Panel::Spotlight));
    });
    assert!(
        !cx.read(|cx| cx
            .windows()
            .iter()
            .any(|handle| handle.window_id() == window.window_id()))
    );
}

#[gpui::test]
fn a_prompt_and_a_rejected_password_update_the_open_card(cx: &mut TestAppContext) {
    let (_fixture, host, mut visual, _window) = open_auth(cx);
    visual.run_until_parked();

    cx.update(|cx| {
        testing::service_event(
            &host,
            crate::services::Event::AuthPrompt {
                cookie: "cookie-1".into(),
                text: "密码：".into(),
                echo: false,
            },
            cx,
        );
        testing::service_event(
            &host,
            crate::services::Event::AuthFailed {
                cookie: "cookie-1".into(),
                text: "密码错误".into(),
            },
            cx,
        );
    });
    visual.run_until_parked();

    host.read_with(cx, |host, _| {
        let auth = host
            .auth
            .as_ref()
            .expect("a failed attempt keeps the dialog for a retry");
        assert_eq!(auth.prompt, "密码：");
        assert_eq!(auth.error.as_deref(), Some("密码错误"));
        assert!(!auth.submitting);
        assert!(host.auth_window.is_some());
    });
    assert!(
        visual.debug_bounds("auth-card").is_some(),
        "the card stays painted for the next attempt"
    );
}

#[gpui::test]
fn a_window_manager_close_cancels_the_pending_request(cx: &mut TestAppContext) {
    let (_fixture, host, mut visual, _window) = open_auth(cx);
    let service = cx.update(|cx| testing::capture_service(&host, cx));

    assert!(
        !visual.simulate_close(),
        "Host owns destruction after the platform callback returns"
    );
    visual.run_until_parked();

    match service.try_recv().expect("a platform close cancels the request") {
        crate::services::Command::AuthCancel { cookie } => assert_eq!(cookie, "cookie-1"),
        other => panic!("unexpected command: {other:?}"),
    }
    host.read_with(cx, |host, _| {
        assert!(host.auth.is_none(), "the request no longer waits for input");
        assert!(host.auth_window.is_none(), "the dead window is forgotten");
        assert_eq!(host.panel, Some(Panel::Spotlight));
    });
}

#[gpui::test]
fn a_long_request_wraps_without_widening_the_card(cx: &mut TestAppContext) {
    let mut request = auth_request("cookie-1");
    request.message = "需要认证才能以超级用户身份运行一个非常长的命令 /usr/bin/some-very-long-tool --with --many --arguments，并显示一段足够长的说明文字".into();
    let (_fixture, host, mut visual, _window) = open_auth_with(cx, request);

    let card = visual.debug_bounds("auth-card").expect("auth card");
    assert!((f32::from(card.size.width) - super::polkit::AUTH_WIDTH).abs() < 0.01);
    assert!(
        card_height(&mut visual) > super::polkit::AUTH_WIDTH,
        "a three-line title keeps the portrait card taller than it is wide"
    );
    assert_card_matches_glass(&host, &mut visual, cx);

    let icon = visual.debug_bounds("auth-icon").expect("icon tile");
    assert_eq!(
        f32::from(icon.size.width),
        super::polkit::AUTH_ICON_BOX,
        "the glyph grows inside the same tile; the background does not"
    );
}

#[gpui::test]
fn technical_details_leave_no_inset_block(cx: &mut TestAppContext) {
    let mut request = auth_request("cookie-1");
    request.details = vec![
        ("polkit.caller-pid".into(), "1234".into()),
        ("polkit.subject-pid".into(), "5678".into()),
    ];
    let (_fixture, host, mut visual, _window) = open_auth_with(cx, request);

    assert!(
        visual.debug_bounds("auth-details").is_none(),
        "process bookkeeping is never shown"
    );
    assert_card_matches_glass(&host, &mut visual, cx);
    let card = visual.debug_bounds("auth-card").expect("auth card");
    assert!(f32::from(card.size.height) > f32::from(card.size.width));
}

#[gpui::test]
fn an_echo_prompt_accepts_a_username_through_the_same_card(cx: &mut TestAppContext) {
    let (_fixture, host, mut visual, _window) = open_auth(cx);
    let service = cx.update(|cx| testing::capture_service(&host, cx));

    cx.update(|cx| {
        testing::service_event(
            &host,
            crate::services::Event::AuthPrompt {
                cookie: "cookie-1".into(),
                text: "用户名：".into(),
                echo: true,
            },
            cx,
        )
    });
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        assert!(
            host.auth.as_ref().expect("dialog").echo,
            "the PAM prompt says this field may echo"
        );
    });

    visual.simulate_input("alice");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    match service.try_recv().expect("the username is submitted") {
        crate::services::Command::AuthSubmit { cookie, secret } => {
            assert_eq!(cookie, "cookie-1");
            assert_eq!(secret, "alice");
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[gpui::test]
fn a_rejected_attempt_keeps_the_request_and_accepts_another_password(cx: &mut TestAppContext) {
    let (_fixture, host, mut visual, _window) = open_auth(cx);
    let service = cx.update(|cx| testing::capture_service(&host, cx));

    visual.simulate_input("wrong");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    match service.try_recv().expect("the first password is submitted") {
        crate::services::Command::AuthSubmit { cookie, secret } => {
            assert_eq!(cookie, "cookie-1");
            assert_eq!(secret, "wrong");
        }
        other => panic!("unexpected command: {other:?}"),
    }

    cx.update(|cx| {
        testing::service_event(
            &host,
            crate::services::Event::AuthFailed {
                cookie: "cookie-1".into(),
                text: "密码错误".into(),
            },
            cx,
        )
    });
    visual.run_until_parked();
    host.read_with(cx, |host, _| {
        let auth = host.auth.as_ref().expect("the dialog stays for a retry");
        assert!(!auth.submitting);
        assert_eq!(auth.error.as_deref(), Some("密码错误"));
    });

    visual.simulate_input("right");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    match service.try_recv().expect("the retry is submitted") {
        crate::services::Command::AuthSubmit { cookie, secret } => {
            assert_eq!(cookie, "cookie-1", "the cookie is reused, not replaced");
            assert_eq!(secret, "right");
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[gpui::test]
fn info_error_and_queue_rows_grow_the_card(cx: &mut TestAppContext) {
    let (_fixture, host, mut visual, _window) = open_auth(cx);
    let before = card_height(&mut visual);

    cx.update(|cx| {
        testing::service_event(
            &host,
            crate::services::Event::AuthInfo {
                cookie: "cookie-1".into(),
                text: "密码即将过期".into(),
            },
            cx,
        );
        testing::service_event(
            &host,
            crate::services::Event::AuthError {
                cookie: "cookie-1".into(),
                text: "PAM 返回错误".into(),
            },
            cx,
        );
    });
    host.update(cx, |host, cx| {
        if let Some(auth) = host.auth.as_mut() {
            auth.request.queued = 2;
        }
        host.changed(cx);
    });
    visual.run_until_parked();

    assert!(
        card_height(&mut visual) > before,
        "info, error and queue rows add height"
    );
    assert_card_matches_glass(&host, &mut visual, cx);
}

#[gpui::test]
fn enter_while_submitting_does_not_send_a_second_password(cx: &mut TestAppContext) {
    let (_fixture, host, mut visual, _window) = open_auth(cx);
    let service = cx.update(|cx| testing::capture_service(&host, cx));

    visual.simulate_input("first");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(matches!(
        service.try_recv().expect("the first submit lands"),
        crate::services::Command::AuthSubmit { .. }
    ));

    visual.simulate_input("second");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(
        service.try_recv().is_err(),
        "the card is waiting for the helper's verdict"
    );
}

fn file_hit(path: &str, is_dir: bool) -> clarion_launcher::files::FileHit {
    let path = std::path::PathBuf::from(path);
    clarion_launcher::files::FileHit {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        parent: path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default(),
        extension: if is_dir {
            String::new()
        } else {
            path.extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase()
        },
        path,
        is_dir,
        size: 1024,
        modified: None,
    }
}

fn six_file_kinds() -> Vec<clarion_launcher::files::FileHit> {
    vec![
        file_hit("/home/test/Pictures/photo.jpg", false),
        file_hit("/home/test/Videos/clip.mp4", false),
        file_hit("/home/test/Music/song.flac", false),
        file_hit("/home/test/Documents/report.pdf", false),
        file_hit("/home/test/Projects", true),
        file_hit("/home/test/backup.zip", false),
    ]
}

#[gpui::test]
fn file_type_tags_filter_the_list_and_enter_opens_the_filtered_row(cx: &mut TestAppContext) {
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.simulate_keystrokes("ctrl-4");
    visual.run_until_parked();
    host.update(cx, |host, cx| {
        host.file_query = "photo".into();
        host.file_hits = Arc::new(six_file_kinds());
        host.changed(cx);
    });
    visual.run_until_parked();

    for selector in [
        "spotlight-file-tag-image",
        "spotlight-file-tag-video",
        "spotlight-file-tag-audio",
        "spotlight-file-tag-document",
        "spotlight-file-tag-folder",
        "spotlight-file-tag-archive",
    ] {
        assert!(
            visual.debug_bounds(selector).is_some(),
            "the {selector} tag is missing"
        );
    }
    assert!(visual.debug_bounds("spotlight-file-5").is_some());

    let image = visual
        .debug_bounds("spotlight-file-tag-image")
        .expect("image tag")
        .center();
    visual.simulate_click(image, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_file_filter),
        Some(super::spotlight::FileFilter::Image)
    );
    assert_eq!(view.read_with(&visual, |view, _| view.selection), 0);
    assert!(visual.debug_bounds("spotlight-file-0").is_some());
    assert!(
        visual.debug_bounds("spotlight-file-1").is_none(),
        "only the image row survives the tag"
    );

    let commands = cx.update(|cx| testing::capture_backend(&host, cx));
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let mut opened = None;
    while let Ok(command) = commands.try_recv() {
        if let backend::Command::OpenPath(path) = command {
            opened = Some(path);
        }
    }
    assert_eq!(
        opened.as_deref(),
        Some(std::path::Path::new("/home/test/Pictures/photo.jpg")),
        "Enter opens the row inside the filtered list"
    );
}

#[gpui::test]
fn file_type_tags_toggle_with_alt_digits_and_clear_on_mode_switch(cx: &mut TestAppContext) {
    let (_fixture, host, view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.simulate_keystrokes("ctrl-4");
    visual.run_until_parked();
    host.update(cx, |host, cx| {
        host.file_query = "file".into();
        host.file_hits = Arc::new(six_file_kinds());
        host.changed(cx);
    });
    visual.run_until_parked();

    visual.simulate_keystrokes("alt-1");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_file_filter),
        Some(super::spotlight::FileFilter::Image)
    );
    visual.simulate_keystrokes("alt-1");
    visual.run_until_parked();
    assert!(
        view.read_with(&visual, |view, _| view.palette_file_filter).is_none(),
        "a second Alt+1 clears the tag"
    );

    visual.simulate_keystrokes("alt-4");
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.palette_file_filter),
        Some(super::spotlight::FileFilter::Document)
    );

    // Leaving the file body drops its tag; returning starts unfiltered.
    visual.simulate_keystrokes("ctrl-3");
    visual.run_until_parked();
    visual.simulate_keystrokes("ctrl-4");
    visual.run_until_parked();
    assert!(
        view.read_with(&visual, |view, _| view.palette_file_filter).is_none(),
        "switching modes resets the file type tag"
    );
}

#[gpui::test]
fn the_file_tag_row_stays_hidden_before_a_query(cx: &mut TestAppContext) {
    let (_fixture, _host, _view, mut visual, _window) = panel(cx, Panel::Spotlight);
    visual.simulate_keystrokes("ctrl-4");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("spotlight-file-tag-image").is_none(),
        "an idle file body has no type tags to filter"
    );
}
