//! GPUI's test platform supplies windows, text, events and deterministic executors.
//! These tests connect to no display server, socket, input method or application.

use std::fs;
use std::path::PathBuf;

use gpui::{TestAppContext, VisualTestContext};
use tempfile::TempDir;

use super::*;

struct CatalogFixture {
    root: TempDir,
    catalog: Arc<AppCatalog>,
}

impl CatalogFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let applications = root.path().join("data/applications");
        fs::create_dir_all(&applications).unwrap();
        for (id, fields) in [
            (
                "alpha.desktop",
                "Name=Alpha Browser\nComment=Read web pages\nKeywords=browser;Internet;\nCategories=Network;WebBrowser;\n",
            ),
            (
                "beta.desktop",
                "Name=Beta Notes\nComment=Write a document\nKeywords=notes;word;\nCategories=Office;\n",
            ),
            (
                "chinese.desktop",
                "Name=中文编辑器\nGenericName=Chinese Editor\nComment=写作文档\nKeywords=中文;编辑;\nCategories=Utility;\n",
            ),
        ] {
            fs::write(
                applications.join(id),
                format!("[Desktop Entry]\nType=Application\nDBusActivatable=true\n{fields}"),
            )
            .unwrap();
        }
        let environment = [
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            (
                "XDG_DATA_HOME".into(),
                root.path().join("data").to_string_lossy().into_owned(),
            ),
            (
                "XDG_DATA_DIRS".into(),
                root.path()
                    .join("empty-system")
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "XDG_CONFIG_DIRS".into(),
                root.path()
                    .join("empty-config")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("GTK_ICON_THEME".into(), "PrivateTestTheme".into()),
            ("XDG_CURRENT_DESKTOP".into(), "clarion".into()),
            ("LANG".into(), "zh_CN.UTF-8".into()),
        ]
        .into();
        let catalog = Arc::new(AppCatalog::load(&environment).unwrap());
        assert_eq!(catalog.apps().len(), 3);
        Self { root, catalog }
    }

    fn view_state_environment(&self) -> crate::preferences::Environment {
        [(
            "XDG_CONFIG_HOME".into(),
            self.root
                .path()
                .join("config")
                .to_string_lossy()
                .into_owned(),
        )]
        .into()
    }
}

fn open_panel(
    cx: &mut TestAppContext,
    catalog: Arc<AppCatalog>,
    view: AppsViewState,
    recent: Vec<String>,
) -> (
    Entity<AppsPanel>,
    &mut VisualTestContext,
    async_channel::Receiver<Message>,
) {
    cx.update(bind_keys);
    let (sender, receiver) = async_channel::bounded(16);
    let (panel, cx) = cx.add_window_view(move |window, cx| {
        let panel = AppsPanel::new(
            catalog,
            view,
            "dark".into(),
            "#6750a4".into(),
            recent,
            sender,
            42,
            cx,
        );
        panel.focus(window, cx);
        panel
    });
    cx.update(|window, cx| {
        panel.read(cx).focus(window, cx);
        window.activate_window();
        window.refresh();
    });
    cx.run_until_parked();
    (panel, cx, receiver)
}

fn visible_ids(panel: &Entity<AppsPanel>, cx: &VisualTestContext) -> Vec<String> {
    panel.read_with(cx, |panel, _| panel.visible_ids())
}

fn selected(panel: &Entity<AppsPanel>, cx: &VisualTestContext) -> Option<String> {
    panel.read_with(cx, |panel, _| panel.selected.clone())
}

#[gpui::test]
fn typed_english_and_chinese_filter_the_rendered_search_field(cx: &mut TestAppContext) {
    let fixture = CatalogFixture::new();
    let (panel, cx, requests) = open_panel(
        cx,
        fixture.catalog.clone(),
        AppsViewState::default(),
        Vec::new(),
    );

    cx.simulate_input("browser");
    assert_eq!(visible_ids(&panel, cx), ["alpha.desktop"]);
    assert_eq!(selected(&panel, cx).as_deref(), Some("alpha.desktop"));
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.query.clone()),
        "browser"
    );

    cx.simulate_keystrokes("ctrl-a backspace");
    cx.simulate_input("中文");
    assert_eq!(visible_ids(&panel, cx), ["chinese.desktop"]);
    assert_eq!(selected(&panel, cx).as_deref(), Some("chinese.desktop"));
    assert_eq!(panel.read_with(cx, |panel, _| panel.query.clone()), "中文");

    cx.simulate_input("missing");
    assert!(visible_ids(&panel, cx).is_empty());
    assert!(selected(&panel, cx).is_none());
    cx.simulate_keystrokes("enter");
    assert!(
        requests.is_empty(),
        "Enter with no match must not request a launch"
    );
    cx.simulate_keystrokes("escape");
    let message = requests.try_recv().unwrap();
    assert_eq!(message.epoch, 42);
    assert!(matches!(message.request, Request::Hide));
}

#[gpui::test]
fn keyboard_navigation_launch_and_escape_use_the_focused_input_dispatch_path(
    cx: &mut TestAppContext,
) {
    let fixture = CatalogFixture::new();
    let (panel, cx, requests) = open_panel(
        cx,
        fixture.catalog.clone(),
        AppsViewState {
            mode: ViewMode::Name,
            ..Default::default()
        },
        Vec::new(),
    );
    assert_eq!(selected(&panel, cx).as_deref(), Some("alpha.desktop"));
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&panel, cx).as_deref(), Some("beta.desktop"));
    cx.simulate_keystrokes("up");
    assert_eq!(selected(&panel, cx).as_deref(), Some("alpha.desktop"));
    cx.simulate_keystrokes("up");
    assert_eq!(selected(&panel, cx).as_deref(), Some("chinese.desktop"));
    cx.simulate_keystrokes("enter");
    let message = requests.try_recv().unwrap();
    assert_eq!(message.epoch, 42);
    assert!(matches!(message.request, Request::Launch(ref id) if id == "chinese.desktop"));
    // A pending launch cannot be duplicated by a repeated Enter.
    cx.simulate_keystrokes("enter");
    assert!(requests.is_empty());
    panel.update(cx, |panel, cx| panel.set_epoch(43, cx));
    cx.simulate_keystrokes("escape");
    let message = requests.try_recv().unwrap();
    assert_eq!(message.epoch, 43);
    assert!(matches!(message.request, Request::Hide));
}

#[gpui::test]
fn mode_and_display_changes_emit_persistable_view_state_without_losing_selection(
    cx: &mut TestAppContext,
) {
    let fixture = CatalogFixture::new();
    let environment = fixture.view_state_environment();
    let (panel, cx, requests) = open_panel(
        cx,
        fixture.catalog.clone(),
        AppsViewState::default(),
        vec!["beta.desktop".into()],
    );
    assert_eq!(selected(&panel, cx).as_deref(), Some("beta.desktop"));
    panel.update(cx, |panel, cx| panel.set_mode(ViewMode::Name, cx));
    panel.update(cx, |panel, cx| panel.set_display(DisplayMode::List, cx));
    cx.run_until_parked();
    assert_eq!(
        visible_ids(&panel, cx),
        ["alpha.desktop", "beta.desktop", "chinese.desktop"]
    );
    assert_eq!(selected(&panel, cx).as_deref(), Some("beta.desktop"));

    for expected_display in [DisplayMode::Icons, DisplayMode::List] {
        let message = requests.try_recv().unwrap();
        assert_eq!(message.epoch, 42);
        let Request::View(view) = message.request else {
            panic!("Expected a view-state request")
        };
        assert_eq!(view.mode, ViewMode::Name);
        assert_eq!(view.display, expected_display);
        view.save(&environment).unwrap();
        assert_eq!(AppsViewState::load(&environment), view);
    }
    assert!(requests.is_empty());
    assert!(!PathBuf::from(&environment["XDG_CONFIG_HOME"])
        .join("clarion-shell/launchpad-layout.json")
        .exists());
}

#[gpui::test]
fn an_empty_catalog_keeps_search_and_dismiss_usable_without_launch_requests(
    cx: &mut TestAppContext,
) {
    let (panel, cx, requests) = open_panel(
        cx,
        Arc::new(AppCatalog::default()),
        AppsViewState::default(),
        vec!["removed.desktop".into()],
    );
    assert!(visible_ids(&panel, cx).is_empty());
    assert!(selected(&panel, cx).is_none());
    cx.simulate_resize(gpui::size(gpui::px(480.), gpui::px(320.)));
    cx.run_until_parked();
    cx.simulate_input("中文 no apps");
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.query.clone()),
        "中文 no apps"
    );
    cx.simulate_keystrokes("up down enter");
    assert!(requests.is_empty());
    assert!(selected(&panel, cx).is_none());
    cx.simulate_keystrokes("escape");
    assert!(matches!(
        requests.try_recv().unwrap().request,
        Request::Hide
    ));
}

#[gpui::test]
fn suggested_duplicates_have_distinct_scroll_anchors_and_search_uses_results(
    cx: &mut TestAppContext,
) {
    let fixture = CatalogFixture::new();
    let (panel, cx, _requests) = open_panel(
        cx,
        fixture.catalog.clone(),
        AppsViewState::default(),
        vec!["beta.desktop".into()],
    );
    panel.read_with(cx, |panel, _| {
        assert!(panel.anchors.contains_key("suggested-beta.desktop"));
        assert!(panel
            .anchors
            .contains_key("productivity_finance-beta.desktop"));
        assert_eq!(
            panel.navigation_anchor_key("beta.desktop"),
            "suggested-beta.desktop"
        );
    });
    cx.simulate_keystrokes("down up");
    assert_eq!(selected(&panel, cx).as_deref(), Some("beta.desktop"));
    cx.simulate_input("notes");
    cx.run_until_parked();
    panel.read_with(cx, |panel, _| {
        assert!(panel.anchors.contains_key("results-beta.desktop"));
        assert_eq!(
            panel.navigation_anchor_key("beta.desktop"),
            "results-beta.desktop"
        );
    });
    cx.simulate_keystrokes("ctrl-a backspace");
    panel.update(cx, |panel, cx| panel.set_mode(ViewMode::Name, cx));
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.navigation_anchor_key("beta.desktop")),
        "results-beta.desktop"
    );
    panel.update(cx, |panel, cx| panel.set_mode(ViewMode::Category, cx));
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.navigation_anchor_key("beta.desktop")),
        "suggested-beta.desktop"
    );
}
