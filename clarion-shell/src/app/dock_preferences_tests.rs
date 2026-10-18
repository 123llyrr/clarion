//! Preference semantics and installed asset lookup without a real desktop.
use super::{
    append_recent_dock_apps, dock_icon_name, dock_icon_roots, output_dimensions, resolve_dock_icon,
    testing, AppCatalog, AppCategory,
};
use gpui::{px, size, TestAppContext};
use std::collections::HashMap;

struct Fixture {
    _root: tempfile::TempDir,
    catalog: AppCatalog,
    environment: HashMap<String, String>,
    original_icon: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        std::fs::create_dir_all(data.join("applications")).unwrap();
        let original_icon = root.path().join("original.svg");
        std::fs::write(&original_icon, "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\"><rect width=\"64\" height=\"64\" fill=\"red\"/></svg>").unwrap();
        for id in ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"] {
            std::fs::write(data.join("applications").join(format!("{id}.desktop")), format!("[Desktop Entry]\nType=Application\nDBusActivatable=true\nName={id}\nCategories=Utility;\nIcon={}\n", original_icon.display())).unwrap();
        }
        let environment = [
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            ("XDG_DATA_HOME".into(), data.to_string_lossy().into_owned()),
            (
                "XDG_DATA_DIRS".into(),
                root.path()
                    .join("empty-system")
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "XDG_CONFIG_HOME".into(),
                root.path().join("config").to_string_lossy().into_owned(),
            ),
            (
                "XDG_CONFIG_DIRS".into(),
                root.path()
                    .join("empty-config")
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "XDG_CACHE_HOME".into(),
                root.path().join("cache").to_string_lossy().into_owned(),
            ),
            ("XDG_CURRENT_DESKTOP".into(), "clarion".into()),
            ("GTK_ICON_THEME".into(), "PrivateTestTheme".into()),
        ]
        .into();
        let catalog = AppCatalog::load(&environment).unwrap();
        assert_eq!(catalog.apps().len(), 6);
        Self {
            _root: root,
            catalog,
            environment,
            original_icon,
        }
    }
}

#[test]
fn recent_dock_apps_skip_pins_running_duplicates_and_uninstalled_before_applying_the_limit() {
    let fixture = Fixture::new();
    let before = vec!["alpha.desktop".to_owned(), "beta.desktop".to_owned()];
    let recent = [
        "missing.desktop",
        "alpha.desktop",
        "beta.desktop",
        "gamma.desktop",
        "gamma.desktop",
        "delta.desktop",
        "epsilon.desktop",
        "zeta.desktop",
    ]
    .map(str::to_owned);
    let mut ids = before.clone();
    append_recent_dock_apps(&mut ids, &recent, &fixture.catalog, true);
    assert_eq!(
        ids,
        [
            "alpha.desktop",
            "beta.desktop",
            "gamma.desktop",
            "delta.desktop",
            "epsilon.desktop"
        ]
    );
    let mut disabled = before.clone();
    append_recent_dock_apps(&mut disabled, &recent, &fixture.catalog, false);
    assert_eq!(disabled, before);
}

#[test]
fn themed_icon_roles_and_install_prefixes_are_resolved_without_directory_scans() {
    assert_eq!(
        dock_icon_name("org.mozilla.firefox.desktop", None),
        "safari.svg"
    );
    assert_eq!(
        dock_icon_name("org.codeberg.dnkl.foot.desktop", None),
        "terminal.svg"
    );
    assert_eq!(dock_icon_name("org.kde.dolphin.desktop", None), "home.svg");
    assert_eq!(
        dock_icon_name("org.gnome.Calendar.desktop", None),
        "calendar.svg"
    );
    assert_eq!(
        dock_icon_name("unknown.desktop", Some(AppCategory::Creativity)),
        "preview.png"
    );
    assert_eq!(
        dock_icon_name("unknown.desktop", Some(AppCategory::Social)),
        "generic-application.svg"
    );
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("release");
    let assets = prefix.join("assets/icons");
    let share = prefix.join("share/clarion-shell/icons");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::create_dir_all(&share).unwrap();
    std::fs::write(assets.join("terminal.svg"), "private release asset").unwrap();
    std::fs::write(share.join("terminal.svg"), "private system-prefix asset").unwrap();
    let roots = dock_icon_roots(Some(&prefix.join("bin/clarion-shell")));
    assert_eq!(
        resolve_dock_icon("terminal.svg", &roots),
        Some(assets.join("terminal.svg"))
    );
    std::fs::remove_file(assets.join("terminal.svg")).unwrap();
    assert_eq!(
        resolve_dock_icon("terminal.svg", &roots),
        Some(share.join("terminal.svg"))
    );
    assert!(resolve_dock_icon("absent-private-test-icon.svg", &roots).is_none());
}

fn window() -> clarion_ipc::Window {
    clarion_ipc::Window {
        id: 42,
        title: Some("Private fixture".into()),
        app_id: Some("beta".into()),
        pid: None,
        gtk_app_menu: None,
        app_menu: None,
        workspace_id: Some(7),
        is_focused: true,
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
fn dock_preferences_preserve_running_windows_and_choose_the_requested_icon_source(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let host = cx
        .update(|cx| testing::test_host(cx, fixture.catalog.clone(), fixture.environment.clone()));
    host.update(cx, |host, _| {
        host.prefs.pinned_app_ids = Some(vec!["alpha.desktop".into()]);
        host.desktop.windows = vec![window()];
        host.prefs.recent_app_ids = [
            "missing.desktop",
            "alpha.desktop",
            "beta.desktop",
            "gamma.desktop",
            "delta.desktop",
            "epsilon.desktop",
            "zeta.desktop",
        ]
        .map(str::to_owned)
        .into();
        let apps = host.dock_apps();
        assert_eq!(
            apps.iter().map(|app| app.id.as_str()).collect::<Vec<_>>(),
            [
                "alpha.desktop",
                "beta.desktop",
                "gamma.desktop",
                "delta.desktop",
                "epsilon.desktop"
            ]
        );
        assert!(apps[0].pinned);
        assert_eq!(apps[1].windows[0].id, 42);
        assert_eq!(apps[0].icon.as_ref(), Some(&fixture.original_icon));
        host.prefs.show_recent_applications = false;
        host.prefs.use_original_icons = false;
        let apps = host.dock_apps();
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[1].windows[0].id, 42);
        let icon = apps[0]
            .icon
            .as_ref()
            .expect("packaged icon must be available");
        assert_eq!(icon.file_name().unwrap(), "generic-application.svg");
        assert!(icon.is_file());
        assert_ne!(icon, &fixture.original_icon);
    });
}

#[gpui::test]
fn disabling_minimize_into_app_clears_an_existing_destination_without_losing_window_actions(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let host = cx
        .update(|cx| testing::test_host(cx, fixture.catalog.clone(), fixture.environment.clone()));
    host.update(cx, |host, cx| {
        host.prefs.pinned_app_ids = Some(vec!["beta.desktop".into()]);
        host.desktop.windows = vec![window()];
        host.reconcile_outputs(cx);
        let output = host
            .outputs
            .keys()
            .next()
            .expect("test display must exist")
            .clone();
        host.desktop.workspaces = vec![clarion_ipc::Workspace {
            id: 7,
            idx: 1,
            name: None,
            output: Some(output),
            is_urgent: false,
            is_active: true,
            is_focused: true,
            active_window_id: Some(42),
        }];
        host.targets();
        assert!(host.last_target.contains_key(&42));
        host.prefs.minimize_into_app = false;
        host.targets();
        assert!(host.last_target.is_empty());
        assert_eq!(host.dock_apps()[0].windows[0].id, 42);
    });
}

#[::core::prelude::v1::test]
fn compositor_logical_size_supersedes_a_stale_toolkit_display_cache_without_double_scaling() {
    let logical = clarion_ipc::LogicalOutput {
        x: 0,
        y: 0,
        width: 1920,
        height: 1200,
        scale: 2.,
        transform: clarion_ipc::Transform::Normal,
    };
    let stale = size(px(947.), px(1056.));
    assert_eq!(output_dimensions(Some(logical), stale), (1920., 1200.));
    assert_eq!(output_dimensions(None, stale), (947., 1056.));
    assert_eq!(
        output_dimensions(
            Some(clarion_ipc::LogicalOutput {
                width: 0,
                ..logical
            }),
            stale
        ),
        (947., 1056.)
    );
}
