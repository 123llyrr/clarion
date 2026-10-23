//! Host-level Dock folder state tests without a display server.

use std::collections::HashMap;
use std::fs;

use gpui::TestAppContext;

use super::{backend, testing, Action, AppCatalog};
use crate::preferences::{FolderDisplay, FolderLayout, FolderSort};

#[gpui::test]
fn open_paths_with_app_rejects_mixed_unsupported_paths(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let applications = root.path().join("data/applications");
    fs::create_dir_all(&applications).unwrap();
    fs::write(
        applications.join("viewer.desktop"),
        "[Desktop Entry]\nType=Application\nName=Viewer\nDBusActivatable=true\nMimeType=text/plain;\n",
    )
    .unwrap();
    let environment: HashMap<String, String> = [
        ("HOME", root.path().to_path_buf()),
        ("XDG_DATA_HOME", root.path().join("data")),
        ("XDG_DATA_DIRS", root.path().join("no-system-data")),
        ("XDG_CONFIG_HOME", root.path().join("config")),
        ("XDG_CURRENT_DESKTOP", root.path().to_path_buf()),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.to_string_lossy().into_owned()))
    .collect();
    let catalog = AppCatalog::load(&environment).unwrap();
    assert_eq!(catalog.apps().len(), 1);
    let text = root.path().join("note.txt");
    let image = root.path().join("photo.png");
    fs::write(&text, b"x").unwrap();
    fs::write(&image, b"x").unwrap();
    let host = cx.update(|cx| testing::test_host(cx, catalog, environment));
    let commands = cx.update(|cx| testing::capture_backend(&host, cx));
    host.update(cx, |host, cx| {
        host.handle(
            Action::OpenPathsWithApp {
                app_id: "viewer.desktop".into(),
                paths: vec![text.clone(), image],
            },
            cx,
        );
    });
    cx.run_until_parked();
    let mut launched = false;
    while let Ok(command) = commands.try_recv() {
        launched |= matches!(command, backend::Command::LaunchDesktopWithPaths(_, _));
    }
    assert!(!launched, "a mixed drop must not launch anything");
    host.update(cx, |host, cx| {
        host.handle(
            Action::OpenPathsWithApp {
                app_id: "viewer.desktop".into(),
                paths: vec![text],
            },
            cx,
        );
    });
    cx.run_until_parked();
    let mut launched = None;
    while let Ok(command) = commands.try_recv() {
        if let backend::Command::LaunchDesktopWithPaths(path, files) = command {
            launched = Some((path, files));
        }
    }
    let (desktop, files) = launched.expect("a fully supported drop launches");
    assert_eq!(desktop.file_name().unwrap(), "viewer.desktop");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_name().unwrap(), "note.txt");
}

#[gpui::test]
fn pinning_a_folder_adds_it_and_rejects_duplicates_and_non_directories(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("Projects");
    fs::create_dir(&folder).unwrap();
    let file = root.path().join("file.txt");
    fs::write(&file, b"x").unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(folder.clone()), cx);
    });
    cx.run_until_parked();
    host.read_with(cx, |host, _| {
        assert_eq!(host.prefs.dock_folders.len(), 1);
        assert_eq!(
            host.prefs.dock_folders[0].path,
            folder.canonicalize().unwrap().to_string_lossy()
        );
    });
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(folder.clone()), cx);
        host.handle(Action::PinFolder(file), cx);
    });
    cx.run_until_parked();
    host.read_with(cx, |host, _| {
        assert_eq!(host.prefs.dock_folders.len(), 1);
    });
}

#[gpui::test]
fn folder_count_is_capped_at_eight(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        for index in 0..10 {
            let path = root.path().join(format!("folder-{index}"));
            fs::create_dir(&path).unwrap();
            host.handle(Action::PinFolder(path), cx);
        }
    });
    cx.run_until_parked();
    host.read_with(cx, |host, _| {
        assert_eq!(host.prefs.dock_folders.len(), 8);
    });
}

#[gpui::test]
fn folder_configuration_and_order_are_persisted_and_snapshot_exposes_views(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let a = a.canonicalize().unwrap();
    let b = b.canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(a.clone()), cx);
        host.handle(Action::PinFolder(b.clone()), cx);
    });
    cx.run_until_parked();
    host.update(cx, |host, cx| {
        host.handle(
            Action::FolderOrder(vec![
                b.to_string_lossy().into_owned(),
                a.to_string_lossy().into_owned(),
            ]),
            cx,
        );
        host.handle(Action::SetFolderDisplay(a.clone(), FolderDisplay::Folder), cx);
        host.handle(Action::SetFolderLayout(a.clone(), FolderLayout::Grid), cx);
        host.handle(Action::SetFolderSort(a.clone(), FolderSort::Size), cx);
        host.handle(Action::SetFolderDescending(a.clone(), true), cx);
    });
    cx.run_until_parked();
    host.read_with(cx, |host, _| {
        let state = host.snapshot();
        assert_eq!(state.dock_folders.len(), 2);
        assert_eq!(state.dock_folders[0].path, b);
        assert_eq!(state.dock_folders[1].path, a);
        assert_eq!(state.dock_folders[1].display, FolderDisplay::Folder);
        assert_eq!(state.dock_folders[1].layout, FolderLayout::Grid);
        assert_eq!(state.dock_folders[1].sort, FolderSort::Size);
        assert!(state.dock_folders[1].descending);
    });
}

#[gpui::test]
fn opening_and_navigating_a_folder_tracks_history(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let child = root.path().join("child");
    fs::create_dir_all(&child).unwrap();
    let root = root.path().canonicalize().unwrap();
    let child = child.canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.clone()), cx);
    });
    cx.run_until_parked();
    host.update(cx, |host, cx| {
        host.handle(Action::OpenFolder(root.clone()), cx);
    });
    cx.run_until_parked();
    host.read_with(cx, |host, _| {
        let navigation = host.folder_navigation.as_ref().unwrap();
        assert_eq!(navigation.current, root);
        assert!(navigation.history.is_empty());
    });
    host.update(cx, |host, cx| {
        host.handle(Action::NavigateFolder(child.clone()), cx);
    });
    cx.run_until_parked();
    host.read_with(cx, |host, _| {
        let navigation = host.folder_navigation.as_ref().unwrap();
        assert_eq!(navigation.current, child);
        assert_eq!(navigation.history, vec![root.clone()]);
    });
    host.update(cx, |host, cx| {
        host.handle(Action::FolderBack, cx);
    });
    cx.run_until_parked();
    host.read_with(cx, |host, _| {
        assert_eq!(host.folder_navigation.as_ref().unwrap().current, root);
    });
}

#[gpui::test]
fn navigating_into_a_subfolder_scans_and_exposes_its_entries(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let child = root.path().join("child");
    fs::create_dir_all(&child).unwrap();
    fs::write(child.join("inside.txt"), b"x").unwrap();
    let root = root.path().canonicalize().unwrap();
    let child = child.canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.clone()), cx);
    });
    cx.run_until_parked();
    host.update(cx, |host, cx| {
        host.handle(Action::OpenFolder(root.clone()), cx);
    });
    cx.run_until_parked();
    host.update(cx, |host, cx| {
        host.handle(Action::NavigateFolder(child.clone()), cx);
    });
    cx.run_until_parked();
    host.read_with(cx, |host, _| {
        let snapshot = host
            .snapshot()
            .folder_snapshots
            .get(&child)
            .cloned()
            .expect("the subfolder was scanned");
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].name, "inside.txt");
    });
}

#[gpui::test]
fn unpinning_a_folder_does_not_touch_the_filesystem(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("Keep");
    fs::create_dir(&folder).unwrap();
    let folder = folder.canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(folder.clone()), cx);
    });
    cx.run_until_parked();
    host.update(cx, |host, cx| {
        host.handle(Action::UnpinFolder(folder.clone()), cx);
    });
    cx.run_until_parked();
    host.read_with(cx, |host, _| {
        assert!(host.prefs.dock_folders.is_empty());
    });
    assert!(folder.is_dir());
}
