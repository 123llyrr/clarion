//! Dispatch actual Dock widgets on GPUI's isolated test platform. The recorded
//! masks are the exact rectangles sent by rendering, since TestWindow itself
//! does not implement compositor-side input-region routing.
use std::collections::HashMap;
use std::time::Duration;

use ::core::prelude::v1::test;
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, WindowHandle};

use super::*;
use crate::app::testing;

const APP_ID: &str = "org.clarion.DockFixture.desktop";
const APP_SELECTOR: &str = "dock-org.clarion.DockFixture.desktop";
const APP_A: &str = "org.clarion.DockA.desktop";
const APP_B: &str = "org.clarion.DockB.desktop";
const APP_C: &str = "org.clarion.DockC.desktop";
const APP_OVERFLOW: &[&str] = &[
    "org.clarion.OverflowA.desktop",
    "org.clarion.OverflowB.desktop",
    "org.clarion.OverflowC.desktop",
    "org.clarion.OverflowD.desktop",
    "org.clarion.OverflowE.desktop",
    "org.clarion.OverflowF.desktop",
    "org.clarion.OverflowG.desktop",
    "org.clarion.OverflowH.desktop",
    "org.clarion.OverflowI.desktop",
    "org.clarion.OverflowJ.desktop",
    "org.clarion.OverflowK.desktop",
    "org.clarion.OverflowL.desktop",
];

struct Fixture {
    _directory: tempfile::TempDir,
    host: Entity<Host>,
    view: Entity<ShellView>,
    window: WindowHandle<ShellView>,
    visual: VisualTestContext,
    commands: std::sync::mpsc::Receiver<backend::Command>,
}

impl Fixture {
    fn new(cx: &mut TestAppContext, animate_launch: bool) -> Self {
        Self::with_apps(cx, animate_launch, &[APP_ID])
    }

    fn with_apps(cx: &mut TestAppContext, animate_launch: bool, apps: &[&str]) -> Self {
        Self::with_mixed_apps(cx, animate_launch, apps, &[])
    }

    /// Creates desktop files for `pinned` and `extra`, pinning only `pinned`.
    fn with_mixed_apps(
        cx: &mut TestAppContext,
        animate_launch: bool,
        pinned: &[&str],
        extra: &[&str],
    ) -> Self {
        Self::with_mixed_apps_mime(cx, animate_launch, pinned, extra, "text/plain;")
    }

    fn with_directory_app(cx: &mut TestAppContext) -> Self {
        Self::with_mixed_apps_mime(cx, false, &[APP_ID], &[], "inode/directory;")
    }

    fn with_mixed_apps_mime(
        cx: &mut TestAppContext,
        animate_launch: bool,
        pinned: &[&str],
        extra: &[&str],
        mime: &str,
    ) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let applications = directory.path().join("data/applications");
        std::fs::create_dir_all(&applications).unwrap();
        for id in pinned.iter().chain(extra) {
            std::fs::write(
                applications.join(id),
                format!(
                    "[Desktop Entry]\nType=Application\nDBusActivatable=true\nName={id}\nCategories=Utility;\nMimeType={mime}\n"
                ),
            )
            .unwrap();
        }
        let environment: HashMap<String, String> = [
            ("HOME", directory.path().to_path_buf()),
            ("XDG_DATA_HOME", directory.path().join("data")),
            ("XDG_DATA_DIRS", directory.path().join("no-system-data")),
            ("XDG_CONFIG_HOME", directory.path().join("config")),
            ("XDG_CONFIG_DIRS", directory.path().join("no-system-config")),
            ("XDG_CACHE_HOME", directory.path().join("cache")),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.to_string_lossy().into_owned()))
        .collect();
        let catalog = clarion_launcher::catalog::AppCatalog::load(&environment).unwrap();
        assert_eq!(catalog.apps().len(), pinned.len() + extra.len());
        let host = cx.update(|cx| testing::test_host(cx, catalog, environment));
        host.update(cx, |host, _| {
            host.prefs.pinned_app_ids = Some(pinned.iter().map(|id| (*id).to_owned()).collect());
            host.prefs.dock_size = 48.;
            // Isolate launch translation from hover enlargement in bounce tests.
            host.prefs.magnification = !animate_launch;
            host.prefs.magnification_scale = 1.6;
            host.prefs.animate_opening_applications = animate_launch;
            host.prefs.reduce_motion = false;
            host.prefs.show_recent_applications = false;
        });
        let commands = cx.update(|cx| testing::capture_backend(&host, cx));
        let window = cx.update(|cx| testing::open_dock(&host, cx));
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.update(|window, _| {
            window.activate_window();
            window.refresh();
        });
        visual.run_until_parked();
        Self {
            _directory: directory,
            host,
            view,
            window,
            visual,
            commands,
        }
    }

    fn bounds(&mut self) -> Bounds<Pixels> {
        self.visual
            .debug_bounds(APP_SELECTOR)
            .expect("Dock app is painted")
    }

    fn advance_motion(&mut self, elapsed: Duration) {
        self.visual.update(|window, cx| {
            self.view.update(cx, |view, cx| {
                view.dock_motion.epoch -= elapsed;
                cx.notify();
            });
            window.refresh();
        });
        self.visual.run_until_parked();
    }

    fn contains_input(&self, point: Point<Pixels>) -> bool {
        self.view.read_with(&self.visual, |view, _| {
            view.dock_motion
                .input_regions
                .iter()
                .any(|region| region.contains(&point))
        })
    }

    fn contains_glass(&self, point: Point<Pixels>) -> bool {
        self.view.read_with(&self.visual, |view, _| {
            view.dock_motion
                .glass_regions
                .iter()
                .any(|region| region.contains(&point))
        })
    }

    fn glass_regions(&self) -> Vec<Bounds<Pixels>> {
        self.view
            .read_with(&self.visual, |view, _| view.dock_motion.glass_regions.clone())
    }

    fn animation_active(&self) -> bool {
        self.view
            .read_with(&self.visual, |view, _| view.dock_motion.animation_active)
    }
}

#[gpui::test]
fn actual_hover_enlarges_bounded_icons_and_the_raised_pixels_remain_clickable(
    cx: &mut TestAppContext,
) {
    let mut fixture = Fixture::new(cx, false);
    let idle = fixture.bounds();
    assert_eq!(idle.size, size(px(48.), px(48.)));
    assert!(
        !fixture.contains_input(point(px(1.), px(1.))),
        "Empty animation canvas stays outside the input mask"
    );
    fixture
        .visual
        .simulate_mouse_move(idle.center(), None, Modifiers::default());
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(
            view.dock_motion.hovered,
            "The real widget receives hover dispatch"
        );
        assert!(view.dock_motion.pointer_target.is_some());
    });
    fixture.advance_motion(Duration::from_millis(250));
    let enlarged = fixture.bounds();
    assert!(enlarged.size.width > idle.size.width + px(15.));
    assert!(enlarged.size.width <= px(48. * 1.6 + 0.01));
    let viewport = fixture.visual.update(|window, _| window.viewport_size());
    assert!(enlarged.left() >= px(0.) && enlarged.top() >= px(0.));
    assert!(enlarged.right() <= viewport.width && enlarged.bottom() <= viewport.height);
    let raised = point(enlarged.center().x, enlarged.top() + px(3.));
    assert!(
        !idle.contains(&raised),
        "Exercise pixels above the original icon"
    );
    assert!(fixture.contains_input(raised));
    assert!(
        !fixture.contains_glass(raised),
        "Lifted icon pixels stay clickable without smearing the backdrop beside the tray"
    );
    fixture.visual.simulate_click(raised, Modifiers::default());
    match fixture.commands.try_recv().unwrap() {
        backend::Command::LaunchDesktop(path) => assert_eq!(path.file_name().unwrap(), APP_ID),
        command => panic!("Raised Dock pixels did not launch the app: {command:?}"),
    }
    fixture.advance_motion(Duration::from_secs(2));
    assert!(
        !fixture.animation_active(),
        "A settled pointer stops requesting animation frames"
    );
    fixture
        .visual
        .simulate_mouse_move(point(px(-20.), px(-20.)), None, Modifiers::default());
    fixture.advance_motion(Duration::from_secs(2));
    assert_eq!(fixture.bounds().size, idle.size);
    assert!(!fixture.animation_active());
    assert!(!fixture.contains_input(point(px(1.), px(1.))));
}

#[gpui::test]
fn file_drag_over_an_app_keeps_the_hover_magnification(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx, false);
    let idle = fixture.bounds();
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("note.txt");
    std::fs::write(&file, b"x").unwrap();
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position: idle.center(),
        paths: gpui::ExternalPaths::new([file]),
    });
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(
            view.dock_motion.hovered,
            "dragging a file over the Dock must keep the hover state alive"
        );
        assert!(view.dock_motion.pointer_target.is_some());
    });
    fixture.advance_motion(Duration::from_millis(300));
    let enlarged = fixture.bounds();
    assert!(
        enlarged.size.width > idle.size.width + px(15.),
        "the dragged-over app should magnify like a normal hover: {:?} vs {:?}",
        idle.size,
        enlarged.size
    );
}

#[gpui::test]
fn the_glass_region_frosts_only_the_resting_tray(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx, false);
    // Idle: the glass follows the tray, so it is non-empty and bounded.
    let glass = fixture.glass_regions();
    assert!(!glass.is_empty(), "the resting tray is frosted");
    let idle = fixture.bounds();
    // The icon center sits inside the tray, so it is frosted on idle.
    assert!(fixture.contains_glass(idle.center()));
}

#[gpui::test]
fn turning_frost_off_clears_the_glass_but_keeps_the_tray_clickable(
    cx: &mut TestAppContext,
) {
    let mut fixture = Fixture::new(cx, false);
    let idle = fixture.bounds();
    assert!(!fixture.glass_regions().is_empty());
    assert!(fixture.contains_input(idle.center()));

    fixture.host.update(cx, |host, cx| {
        host.prefs.frost_strength = 0.;
        host.changed(cx);
    });
    assert_eq!(
        fixture
            .view
            .read_with(&fixture.visual, |view, _| view.state.prefs.frost_strength),
        0.
    );
    fixture.advance_motion(Duration::from_millis(1));

    assert!(
        fixture.glass_regions().is_empty(),
        "zero frost disables every compositor frost rectangle"
    );
    assert!(
        fixture.contains_input(idle.center()),
        "opacity and frost controls never make the Dock click-through"
    );
}

#[gpui::test]
fn platform_exit_ends_hover_without_a_followup_motion_and_contracts_the_dock(
    cx: &mut TestAppContext,
) {
    let mut fixture = Fixture::new(cx, false);
    let idle = fixture.bounds();
    fixture
        .visual
        .simulate_mouse_move(idle.center(), None, Modifiers::default());
    fixture.advance_motion(Duration::from_millis(250));
    assert!(fixture.bounds().size.width > idle.size.width + px(15.));

    let exit = || {
        MouseExitEvent {
            // Wayland Leave retains the last known position, which remains inside
            // the icon; leaving the surface does not require an offscreen Motion.
            position: idle.center(),
            pressed_button: None,
            modifiers: Modifiers::default(),
        }
    };
    fixture.visual.simulate_event(exit());
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(!view.dock_motion.hovered);
        assert!(view.dock_motion.pointer_target.is_none());
    });
    let revision = fixture
        .host
        .read_with(&fixture.visual, |host, _| host.revision);
    fixture.visual.simulate_event(exit());
    fixture.visual.run_until_parked();
    fixture.host.read_with(&fixture.visual, |host, _| {
        assert_eq!(
            host.revision, revision,
            "A repeated exit does not dispatch hover=false twice"
        );
    });
    fixture.advance_motion(Duration::from_secs(2));
    assert_eq!(fixture.bounds().size, idle.size);
    assert!(!fixture.animation_active());
    assert!(!fixture.contains_input(point(px(1.), px(1.))));
}

#[gpui::test]
fn launching_through_a_dock_click_bounces_once_and_then_stops(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx, true);
    let idle = fixture.bounds();
    fixture
        .visual
        .simulate_click(idle.center(), Modifiers::default());
    assert!(matches!(
        fixture.commands.try_recv().unwrap(),
        backend::Command::LaunchDesktop(_)
    ));
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(
            view.state.launches.contains_key(APP_ID),
            "Real launch reaches the Dock snapshot"
        );
    });
    fixture
        .visual
        .cx
        .update(|cx| testing::age_launch(&fixture.host, APP_ID, Duration::from_millis(180), cx));
    let hopping = fixture.bounds();
    assert!(hopping.top() < idle.top() - px(10.));
    assert_eq!(
        hopping.size, idle.size,
        "Launch feedback translates without changing icon size"
    );
    assert!(fixture.contains_input(hopping.center()));
    assert!(fixture.animation_active());
    fixture
        .visual
        .cx
        .update(|cx| testing::age_launch(&fixture.host, APP_ID, Duration::from_secs(2), cx));
    assert_eq!(fixture.bounds(), idle);
    assert!(
        !fixture.animation_active(),
        "The finite launch feedback leaves no recurring frame request"
    );
}

#[gpui::test]
fn auto_hide_slides_the_same_surface_outside_input_and_reveals_it_again(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx, false);
    let idle = fixture.bounds();
    let original_window = fixture.window.window_id();
    fixture
        .visual
        .cx
        .update(|cx| testing::set_dock_revealed(&fixture.host, false, cx));
    fixture.advance_motion(Duration::from_secs(2));
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(view.dock_motion.input_regions.is_empty());
    });
    assert!(!fixture.animation_active());
    let still_registered = fixture
        .visual
        .cx
        .update(|cx| testing::open_dock(&fixture.host, cx));
    assert_eq!(
        still_registered.window_id(),
        original_window,
        "Auto-hide keeps the original surface"
    );
    fixture
        .visual
        .cx
        .update(|cx| testing::set_dock_revealed(&fixture.host, true, cx));
    fixture.advance_motion(Duration::from_secs(2));
    assert_eq!(fixture.bounds(), idle);
    assert!(fixture.contains_input(idle.center()));
    assert!(!fixture.animation_active());
}

#[test]
fn folder_stack_preview_puts_files_on_top_and_folders_at_the_back() {
    use crate::dock_folder::{FolderEntry, FolderEntryKind};
    let entry = |name: &str, kind: FolderEntryKind| FolderEntry {
        path: std::path::PathBuf::from(name),
        name: name.to_owned(),
        kind,
        modified: None,
        size: 0,
    };
    let entries = vec![
        entry("Folder", FolderEntryKind::Directory),
        entry("a.txt", FolderEntryKind::Document),
        entry("Sub", FolderEntryKind::Directory),
        entry("b.png", FolderEntryKind::Image),
    ];
    let preview = super::macos_dock::folder_preview_entries(&entries);
    assert_eq!(
        preview
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["a.txt", "b.png", "Folder"]
    );
}

#[gpui::test]
fn folder_tiles_render_between_overflow_and_trash_with_a_preview(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("Projects");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("note.txt"), b"hello").unwrap();
    let mut fixture = Fixture::with_apps(cx, false, &[APP_A]);
    let host = fixture.host.clone();
    fixture.visual.cx.update(|cx| {
        host.update(cx, |host, cx| {
            host.prefs.dock_folders = vec![crate::preferences::DockFolderPreference {
                path: folder.to_string_lossy().into_owned(),
                display: crate::preferences::FolderDisplay::Stack,
                layout: crate::preferences::FolderLayout::Auto,
                sort: crate::preferences::FolderSort::Name,
                descending: false,
            }];
            host.changed(cx);
        });
    });
    fixture.visual.update(|window, _| window.refresh());
    fixture.visual.run_until_parked();
    let folder_bounds = fixture
        .visual
        .debug_bounds("dock-folder-0")
        .expect("the folder tile is painted");
    let trash = fixture.visual.debug_bounds("dock-trash").unwrap();
    assert!(folder_bounds.right() <= trash.left() + px(0.01));
    assert_eq!(folder_bounds.size, size(px(48.), px(48.)));
}

#[gpui::test]
fn a_long_folder_name_does_not_change_the_folder_tile_height(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory
        .path()
        .join("this-is-a-very-long-folder-name-that-must-not-wrap-or-grow-the-tile");
    std::fs::create_dir(&folder).unwrap();
    let mut fixture = Fixture::with_apps(cx, false, &[APP_A]);
    let host = fixture.host.clone();
    fixture.visual.cx.update(|cx| {
        host.update(cx, |host, cx| {
            host.prefs.dock_folders = vec![crate::preferences::DockFolderPreference {
                path: folder.to_string_lossy().into_owned(),
                display: crate::preferences::FolderDisplay::Folder,
                layout: crate::preferences::FolderLayout::Auto,
                sort: crate::preferences::FolderSort::Name,
                descending: false,
            }];
            host.changed(cx);
        });
    });
    fixture.visual.update(|window, _| window.refresh());
    fixture.visual.run_until_parked();
    let bounds = fixture
        .visual
        .debug_bounds("dock-folder-0")
        .expect("the folder tile is painted");
    assert_eq!(bounds.size, size(px(48.), px(48.)));
}

#[gpui::test]
fn dragging_a_folder_tile_reorders_only_the_folder_group(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("A");
    let b = root.path().join("B");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let mut fixture = Fixture::with_apps(cx, false, &[APP_A]);
    let host = fixture.host.clone();
    let a_path = a.to_string_lossy().into_owned();
    let b_path = b.to_string_lossy().into_owned();
    fixture.visual.cx.update(|cx| {
        host.update(cx, |host, cx| {
            host.prefs.dock_folders = [a_path.clone(), b_path.clone()]
                .into_iter()
                .map(|path| crate::preferences::DockFolderPreference {
                    path,
                    display: crate::preferences::FolderDisplay::Folder,
                    layout: crate::preferences::FolderLayout::Auto,
                    sort: crate::preferences::FolderSort::Name,
                    descending: false,
                })
                .collect();
            host.changed(cx);
        });
    });
    fixture.visual.update(|window, _| window.refresh());
    fixture.visual.run_until_parked();
    let first = fixture.visual.debug_bounds("dock-folder-0").unwrap();
    let second = fixture.visual.debug_bounds("dock-folder-1").unwrap();
    let drop = second.center();
    fixture
        .visual
        .simulate_mouse_down(first.center(), MouseButton::Left, Modifiers::default());
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(
            view.dock_motion.folder_press.is_some(),
            "the folder press was not recorded"
        );
    });
    fixture
        .visual
        .simulate_mouse_move(drop, Some(MouseButton::Left), Modifiers::default());
    fixture.view.read_with(&fixture.visual, |view, _| {
        let drag = view
            .dock_motion
            .folder_drag
            .as_ref()
            .expect("the folder drag did not begin");
        assert_eq!(drag.order, [b_path.clone(), a_path.clone()]);
    });
    fixture
        .visual
        .simulate_mouse_up(drop, MouseButton::Left, Modifiers::default());
    fixture.visual.run_until_parked();
    host.read_with(&fixture.visual, |host, _| {
        assert_eq!(host.prefs.dock_folders[0].path, b_path);
        assert_eq!(host.prefs.dock_folders[1].path, a_path);
    });
}

#[gpui::test]
fn dragging_a_folder_over_a_lifted_dock_icon_keeps_the_preview(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let pinned = directory.path().join("Pinned");
    let dragged = directory.path().join("Dragged");
    std::fs::create_dir(&pinned).unwrap();
    std::fs::create_dir(&dragged).unwrap();
    let mut fixture = Fixture::new(cx, false);
    let host = fixture.host.clone();
    fixture.visual.cx.update(|cx| {
        host.update(cx, |host, cx| {
            host.prefs.dock_folders = vec![crate::preferences::DockFolderPreference {
                path: pinned.to_string_lossy().into_owned(),
                display: crate::preferences::FolderDisplay::Folder,
                layout: crate::preferences::FolderLayout::Auto,
                sort: crate::preferences::FolderSort::Name,
                descending: false,
            }];
            host.changed(cx);
        });
    });
    fixture.visual.update(|window, _| window.refresh());
    fixture.visual.run_until_parked();
    let folder = fixture.visual.debug_bounds("dock-folder-0").unwrap();
    let tray_top = fixture
        .view
        .read_with(&fixture.visual, |view, _| {
            view.dock_motion.geometry.tray_cross_start
        });
    // Start inside the resting tray so the icon magnifies, then move onto the
    // lifted artwork that is still part of the Dock's input region.
    let low = point(folder.center().x, px(tray_top + 1.));
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position: low,
        paths: gpui::ExternalPaths::new([dragged.clone()]),
    });
    fixture.visual.run_until_parked();
    fixture.advance_motion(Duration::from_millis(350));
    let raised = fixture.visual.debug_bounds("dock-folder-0").unwrap();
    assert!(
        raised.top() < px(tray_top),
        "the magnified folder should extend above the resting tray: {raised:?} / {tray_top}"
    );
    let high = point(raised.center().x, raised.top() + px(2.));
    assert!(
        fixture.contains_input(high),
        "the lifted artwork should remain a real pointer target"
    );
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Pending { position: high });
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(view.dock_motion.folder_drop_active);
        assert!(view.dock_motion.folder_drop_visible);
    });
    assert!(
        fixture
            .visual
            .debug_bounds("dock-folder-drop-placeholder")
            .is_some(),
        "the folder slot preview stays visible over the lifted icon"
    );
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Submit { position: high });
    fixture.visual.run_until_parked();
    host.read_with(&fixture.visual, |host, _| {
        assert_eq!(host.prefs.dock_folders.len(), 2);
        assert_eq!(
            host.prefs.dock_folders[1].path,
            dragged.canonicalize().unwrap().to_string_lossy()
        );
    });
}

#[gpui::test]
fn leaving_a_folder_drag_clears_the_magnified_icon(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("Dragged");
    std::fs::create_dir(&folder).unwrap();
    let mut fixture = Fixture::new(cx, false);
    let idle = fixture.bounds();
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position: idle.center(),
        paths: gpui::ExternalPaths::new([folder]),
    });
    fixture.visual.run_until_parked();
    fixture.advance_motion(Duration::from_millis(350));
    let enlarged = fixture.bounds();
    assert!(
        enlarged.size.width > idle.size.width + px(10.),
        "the drag should magnify the icon: {idle:?} / {enlarged:?}"
    );
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Exited);
    fixture.visual.run_until_parked();
    fixture.advance_motion(Duration::from_millis(500));
    let settled = fixture.bounds();
    assert_eq!(settled.size, idle.size, "the icon returns to its resting size");
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(view.dock_motion.pointer_target.is_none());
        assert!(!view.dock_motion.hovered);
        assert!(view.dock_motion.tooltip_name.is_none());
        assert!(view.dock_motion.tooltip_armed.is_none());
    });
}

#[gpui::test]
fn dragging_a_folder_over_a_pinned_app_only_offers_opening(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let existing = directory.path().join("Existing");
    let dragged = directory.path().join("Dragged");
    std::fs::create_dir(&existing).unwrap();
    std::fs::create_dir(&dragged).unwrap();
    let mut fixture = Fixture::new(cx, false);
    let host = fixture.host.clone();
    fixture.visual.cx.update(|cx| {
        host.update(cx, |host, cx| {
            host.prefs.dock_folders = vec![crate::preferences::DockFolderPreference {
                path: existing.to_string_lossy().into_owned(),
                display: crate::preferences::FolderDisplay::Folder,
                layout: crate::preferences::FolderLayout::Auto,
                sort: crate::preferences::FolderSort::Name,
                descending: false,
            }];
            host.changed(cx);
        });
    });
    fixture.visual.update(|window, _| window.refresh());
    fixture.visual.run_until_parked();
    let app = fixture.bounds();
    let trash_before = fixture.visual.debug_bounds("dock-trash").unwrap();
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position: app.center(),
        paths: gpui::ExternalPaths::new([dragged.clone()]),
    });
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(
            !view.dock_motion.folder_drop_active,
            "the folder slot is not offered over the application area"
        );
        assert!(!view.dock_motion.folder_drop_visible);
    });
    assert!(
        fixture
            .visual
            .debug_bounds("dock-folder-drop-placeholder")
            .is_none(),
        "no placeholder over an application tile"
    );
    assert_eq!(
        fixture.visual.debug_bounds("dock-trash").unwrap(),
        trash_before,
        "the trash does not shift while the pointer is over an application"
    );
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Submit {
            position: app.center(),
        });
    fixture.visual.run_until_parked();
    host.read_with(&fixture.visual, |host, _| {
        assert_eq!(
            host.prefs.dock_folders.len(),
            1,
            "dropping on an application that cannot open folders does not pin"
        );
        assert_eq!(
            host.prefs.dock_folders[0].path,
            existing.canonicalize().unwrap().to_string_lossy()
        );
    });
}

#[gpui::test]
fn dropping_a_folder_on_an_app_that_opens_directories_launches_that_app(
    cx: &mut TestAppContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("Pinned");
    std::fs::create_dir(&folder).unwrap();
    let mut fixture = Fixture::with_directory_app(cx);
    let position = fixture.bounds().center();
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position,
        paths: gpui::ExternalPaths::new([folder.clone()]),
    });
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert_eq!(view.dock_motion.drag_accept.as_deref(), Some(APP_ID));
    });
    assert!(
        fixture
            .visual
            .debug_bounds("dock-folder-drop-placeholder")
            .is_none(),
        "an app that accepts directories keeps its own drop target"
    );
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Submit { position });
    fixture.visual.run_until_parked();
    fixture.host.read_with(&fixture.visual, |host, _| {
        assert!(host.prefs.dock_folders.is_empty());
    });
    let mut launched = None;
    while let Ok(command) = fixture.commands.try_recv() {
        if let backend::Command::LaunchDesktopWithPaths(desktop, paths) = command {
            launched = Some((desktop, paths));
        }
    }
    let (desktop, paths) = launched.expect("the directory opens in the accepting app");
    assert_eq!(desktop.file_name().unwrap(), APP_ID);
    assert_eq!(paths, [folder]);
}

#[gpui::test]
fn dropping_a_folder_on_empty_tray_space_pins_it(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("Pinned");
    std::fs::create_dir(&folder).unwrap();
    let mut fixture = Fixture::new(cx, false);
    let trash_before = fixture.visual.debug_bounds("dock-trash").unwrap();
    let position = point(trash_before.left() - px(6.), trash_before.center().y);
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position,
        paths: gpui::ExternalPaths::new([folder.clone()]),
    });
    fixture.visual.run_until_parked();
    let placeholder = fixture
        .visual
        .debug_bounds("dock-folder-drop-placeholder")
        .expect("a folder held over the tray shows a drop placeholder");
    let trash_after = fixture.visual.debug_bounds("dock-trash").unwrap();
    assert!(
        trash_after.left() > trash_before.left(),
        "the trash slides outward to make room for the new folder slot"
    );
    assert!(
        placeholder.right() <= trash_after.left() + px(0.01),
        "the placeholder owns a real slot before the trash instead of covering it"
    );
    let plus = fixture
        .visual
        .debug_bounds("dock-folder-drop-plus")
        .expect("the reserved slot shows the + pin affordance");
    assert!(
        placeholder.contains(&plus.center()),
        "the + badge marks the middle of the reserved slot: {placeholder:?} / {plus:?}"
    );
    let drop = placeholder.center();
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Pending { position: drop });
    fixture.visual.run_until_parked();
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Submit { position: drop });
    fixture.visual.run_until_parked();
    let folder_slot = fixture
        .visual
        .debug_bounds("dock-folder-0")
        .expect("the pinned folder fills the preview slot");
    let trash_final = fixture.visual.debug_bounds("dock-trash").unwrap();
    assert!(
        trash_final.left() > trash_before.left(),
        "pinning keeps the extra slot open instead of pulling the trash back over it"
    );
    assert!(
        folder_slot.right() <= trash_final.left() + px(0.01),
        "the folder and trash do not overlap"
    );
    fixture.host.read_with(&fixture.visual, |host, _| {
        assert_eq!(host.prefs.dock_folders.len(), 1);
        assert_eq!(
            host.prefs.dock_folders[0].path,
            folder.canonicalize().unwrap().to_string_lossy()
        );
    });
}

#[gpui::test]
fn a_new_folder_previews_after_the_existing_folder_group(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("First");
    let second = directory.path().join("Second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let mut fixture = Fixture::with_apps(cx, false, &[APP_A]);
    let host = fixture.host.clone();
    fixture.visual.cx.update(|cx| {
        host.update(cx, |host, cx| {
            host.prefs.dock_folders = vec![crate::preferences::DockFolderPreference {
                path: first.to_string_lossy().into_owned(),
                display: crate::preferences::FolderDisplay::Stack,
                layout: crate::preferences::FolderLayout::Auto,
                sort: crate::preferences::FolderSort::Name,
                descending: false,
            }];
            host.changed(cx);
        });
    });
    fixture.visual.update(|window, _| window.refresh());
    fixture.visual.run_until_parked();
    let first_slot = fixture
        .visual
        .debug_bounds("dock-folder-0")
        .expect("the pinned folder is painted");
    let trash_before = fixture.visual.debug_bounds("dock-trash").unwrap();
    let position = point(trash_before.left() - px(6.), trash_before.center().y);
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position,
        paths: gpui::ExternalPaths::new([second]),
    });
    fixture.visual.run_until_parked();
    let placeholder = fixture
        .visual
        .debug_bounds("dock-folder-drop-placeholder")
        .expect("the next folder slot is previewed");
    let trash_after = fixture.visual.debug_bounds("dock-trash").unwrap();
    assert!(
        placeholder.left() >= first_slot.right() - px(0.01),
        "the preview follows the existing folder group"
    );
    assert!(
        placeholder.right() <= trash_after.left() + px(0.01),
        "the preview owns the slot before the shifted trash"
    );
    assert!(
        trash_after.left() > trash_before.left(),
        "the trash makes room for the new folder slot"
    );
}

#[gpui::test]
fn dropping_a_folder_after_magnified_hovering_exits_is_cleared(
    cx: &mut TestAppContext,
) {
    // Regression for the follow-up to the placeholder work: completing a
    // folder drop must not leave the next hover stuck. Hover a neighbour,
    // then leave the Dock; the magnified tile must relax again.
    let directory = tempfile::tempdir().unwrap();
    let pinned = directory.path().join("Pinned");
    let dragged = directory.path().join("Dragged");
    for path in [&pinned, &dragged] {
        std::fs::create_dir(path).unwrap();
    }
    let mut fixture = Fixture::new(cx, false);
    let host = fixture.host.clone();
    let pinned = pinned.canonicalize().unwrap();
    fixture.visual.cx.update(|cx| {
        host.update(cx, |host, cx| {
            host.prefs.dock_folders = vec![crate::preferences::DockFolderPreference {
                path: pinned.to_string_lossy().into_owned(),
                display: crate::preferences::FolderDisplay::Folder,
                layout: crate::preferences::FolderLayout::Auto,
                sort: crate::preferences::FolderSort::Name,
                descending: false,
            }];
            host.changed(cx);
        });
    });
    fixture.visual.update(|window, _| window.refresh());
    fixture.visual.run_until_parked();
    let neighbour_idle = fixture
        .visual
        .debug_bounds("dock-folder-0")
        .unwrap()
        .size;
    // Drop the new folder on the reserved slot right after the folder tiles.
    let pinned_tile = fixture.visual.debug_bounds("dock-folder-0").unwrap();
    let offset = point(pinned_tile.right() + px(10.), pinned_tile.center().y);
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position: offset,
        paths: gpui::ExternalPaths::new([dragged.clone()]),
    });
    fixture.visual.run_until_parked();
    let slot = fixture
        .visual
        .debug_bounds("dock-folder-drop-placeholder")
        .expect("the reserved slot shows its placeholder");
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Submit {
            position: slot.center(),
        });
    fixture.visual.run_until_parked();
    let dragged = dragged.canonicalize().unwrap();
    host.read_with(&fixture.visual, |host, _| {
        assert_eq!(host.prefs.dock_folders.len(), 2);
        assert_eq!(
            host.prefs.dock_folders[1].path,
            dragged.to_string_lossy()
        );
    });
    // Hover the neighbouring folder with the plain pointer.
    let neighbour = fixture.visual.debug_bounds("dock-folder-0").unwrap();
    fixture
        .visual
        .simulate_mouse_move(neighbour.center(), None, Modifiers::default());
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(view.dock_motion.hovered);
        assert!(view.dock_motion.pointer_target.is_some());
    });
    fixture.advance_motion(Duration::from_millis(250));
    let neighbour_big = fixture
        .visual
        .debug_bounds("dock-folder-0")
        .unwrap()
        .size;
    assert!(
        neighbour_big.width > neighbour_idle.width,
        "the hovered folder magnifies: {neighbour_idle:?} -> {neighbour_big:?}"
    );
    // Leaving the surface loses no motion; the exit must reset the hover.
    fixture.visual.simulate_event(MouseExitEvent {
        position: neighbour.center(),
        pressed_button: None,
        modifiers: Modifiers::default(),
    });
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(!view.dock_motion.hovered);
        assert!(view.dock_motion.pointer_target.is_none());
    });
    fixture.advance_motion(Duration::from_secs(2));
    assert_eq!(
        fixture.visual.debug_bounds("dock-folder-0").unwrap().size,
        neighbour_idle,
        "the magnified folder returns to rest after the exit"
    );
    assert!(!fixture.animation_active());
}

#[gpui::test]
fn an_exit_after_plain_moves_clears_the_magnification_target(
    cx: &mut TestAppContext,
) {
    // Regression for the real Wayland ordering captured while chasing the
    // post-pin stuck icon: the drop's end-drag cleanup resets `hovered`, then
    // plain surface moves re-establish the magnification target through the
    // mouse-move handler without any hover transition, and the final exit
    // arrives while `hovered` is already false. The exit must clear the
    // target regardless of the current hover state.
    let mut fixture = Fixture::new(cx, false);
    let idle = fixture.bounds();
    let center = fixture.bounds().center();
    // Arm the element hover listeners with a real move first, so the surface
    // exit is actually delivered (the exit runs only for a queued hover).
    fixture
        .visual
        .simulate_mouse_move(center, None, Modifiers::default());
    fixture.visual.run_until_parked();
    fixture.visual.update(|window, cx| {
        // Exactly the stuck state from the hover log: `hovered` was reset by
        // the trailing cleanup render while plain moves re-magnified, and the
        // row no longer believes the pointer is hovering it.
        fixture.view.update(cx, |view, _| {
            view.dock_motion.hovered = false;
            view.dock_drag_seen = false;
        });
        window.refresh();
    });
    fixture.visual.run_until_parked();
    fixture.advance_motion(Duration::from_millis(250));
    assert!(
        fixture.bounds().size.width > idle.size.width + px(15.),
        "the magnification target set by plain moves magnifies the tile"
    );
    fixture.visual.simulate_event(MouseExitEvent {
        position: center,
        pressed_button: None,
        modifiers: Modifiers::default(),
    });
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(!view.dock_motion.hovered);
        assert!(view.dock_motion.pointer_target.is_none());
    });
    fixture.advance_motion(Duration::from_secs(2));
    assert_eq!(fixture.bounds().size, idle.size);
    assert!(!fixture.animation_active());
}

#[gpui::test]
fn an_inactive_dock_does_not_release_the_folder_slot_during_a_drag(
    cx: &mut TestAppContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("Dragged");
    std::fs::create_dir(&folder).unwrap();
    let mut fixture = Fixture::new(cx, false);
    let host = fixture.host.clone();
    let trash = fixture.visual.debug_bounds("dock-trash").unwrap();
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position: trash.center(),
        paths: gpui::ExternalPaths::new([folder]),
    });
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(!view.dock_motion.folder_drop_active);
    });
    // Simulate another output's Dock receiving the reservation. This view did
    // not see the drag, so it must not cancel the host-owned slot while the
    // global drag is still active.
    fixture.visual.cx.update(|cx| {
        host.update(cx, |host, cx| {
            host.folder_drop_preview = true;
            host.changed(cx);
        });
    });
    fixture.visual.run_until_parked();
    host.read_with(&fixture.visual, |host, _| {
        assert!(
            host.folder_drop_preview,
            "an inactive output must not cancel the active Dock's folder slot"
        );
    });
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Exited);
    fixture.visual.run_until_parked();
}

#[gpui::test]
fn dropping_a_folder_on_trash_does_not_pin_it(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("Pinned");
    std::fs::create_dir(&folder).unwrap();
    let mut fixture = Fixture::new(cx, false);
    let trash = fixture.visual.debug_bounds("dock-trash").unwrap();
    let position = trash.center();
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position,
        paths: gpui::ExternalPaths::new([folder]),
    });
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Submit { position });
    fixture.visual.run_until_parked();
    fixture.host.read_with(&fixture.visual, |host, _| {
        assert!(host.prefs.dock_folders.is_empty());
    });
}

#[gpui::test]
fn dropping_on_the_shifted_trash_still_does_not_pin(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("Pinned");
    std::fs::create_dir(&folder).unwrap();
    let mut fixture = Fixture::new(cx, false);
    let trash_before = fixture.visual.debug_bounds("dock-trash").unwrap();
    let empty = point(trash_before.left() - px(6.), trash_before.center().y);
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position: empty,
        paths: gpui::ExternalPaths::new([folder]),
    });
    fixture.visual.run_until_parked();
    let trash_after = fixture.visual.debug_bounds("dock-trash").unwrap();
    assert!(
        trash_after.left() > trash_before.left(),
        "the preview opens a slot and shifts the trash"
    );
    let drop = trash_after.center();
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Pending { position: drop });
    fixture.visual.run_until_parked();
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Submit { position: drop });
    fixture.visual.run_until_parked();
    fixture.host.read_with(&fixture.visual, |host, _| {
        assert!(
            host.prefs.dock_folders.is_empty(),
            "dropping on the shifted trash does not pin a folder"
        );
    });
}

#[gpui::test]
fn moving_a_folder_drag_over_the_trash_hides_the_slot(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let folder_empty = directory.path().join("Empty");
    let folder_dragged = directory.path().join("Dragged");
    std::fs::create_dir(&folder_empty).unwrap();
    std::fs::create_dir(&folder_dragged).unwrap();
    let mut fixture = Fixture::new(cx, false);
    let host = fixture.host.clone();
    fixture.visual.cx.update(|cx| {
        host.update(cx, |host, cx| {
            host.prefs.dock_folders = vec![crate::preferences::DockFolderPreference {
                path: folder_empty.to_string_lossy().into_owned(),
                display: crate::preferences::FolderDisplay::Folder,
                layout: crate::preferences::FolderLayout::Auto,
                sort: crate::preferences::FolderSort::Name,
                descending: false,
            }];
            host.changed(cx);
        });
    });
    fixture.visual.update(|window, _| window.refresh());
    fixture.visual.run_until_parked();
    let trash_before = fixture.visual.debug_bounds("dock-trash").unwrap();
    let empty = point(trash_before.left() - px(6.), trash_before.center().y);
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position: empty,
        paths: gpui::ExternalPaths::new([folder_dragged]),
    });
    fixture.visual.run_until_parked();
    let trash_after = fixture.visual.debug_bounds("dock-trash").unwrap();
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Pending {
            position: trash_after.center(),
        });
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(
            !view.dock_motion.folder_drop_active,
            "the strip ends before the trash; hovering the trash cancels the offer"
        );
        assert!(!view.dock_motion.folder_drop_visible);
    });
    assert!(
        fixture
            .visual
            .debug_bounds("dock-folder-drop-placeholder")
            .is_none(),
        "the placeholder is gone while the pointer holds over the trash"
    );
}

#[gpui::test]
fn dropping_supported_files_on_an_app_tile_opens_that_app(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("note.txt");
    std::fs::write(&file, b"x").unwrap();
    let mut fixture = Fixture::new(cx, false);
    let position = fixture.bounds().center();
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position,
        paths: gpui::ExternalPaths::new([file.clone()]),
    });
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Submit { position });
    fixture.visual.run_until_parked();
    let mut launched = None;
    while let Ok(command) = fixture.commands.try_recv() {
        if let backend::Command::LaunchDesktopWithPaths(path, files) = command {
            launched = Some((path, files));
        }
    }
    let (desktop, files) = launched.expect("the supported file opens in the target app");
    assert_eq!(desktop.file_name().unwrap(), APP_ID);
    assert_eq!(files[0].file_name().unwrap(), "note.txt");
}

#[gpui::test]
fn dropping_unsupported_files_on_an_app_tile_does_nothing(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("photo.png");
    std::fs::write(&file, b"x").unwrap();
    let mut fixture = Fixture::new(cx, false);
    let position = fixture.bounds().center();
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position,
        paths: gpui::ExternalPaths::new([file]),
    });
    fixture
        .visual
        .simulate_event(gpui::FileDropEvent::Submit { position });
    fixture.visual.run_until_parked();
    let mut launched = false;
    while let Ok(command) = fixture.commands.try_recv() {
        launched |= matches!(command, backend::Command::LaunchDesktopWithPaths(_, _));
    }
    assert!(!launched);
    fixture.host.read_with(&fixture.visual, |host, _| {
        assert!(host.prefs.dock_folders.is_empty());
    });
}

#[gpui::test]
fn the_icon_is_centered_with_the_same_clearance_on_every_side(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx, false);
    let state = fixture
        .view
        .read_with(&fixture.visual, |view, _| view.state.clone());
    let metrics = crate::dock_motion::DockMetrics::new(
        state.dock_apps().len(),
        state.prefs.dock_size as f32,
        state.prefs.magnification_scale as f32,
        state.prefs.magnification && !state.prefs.reduce_motion,
        state.prefs.show_launcher,
        false,
        1920.,
        1080.,
    );
    let bounds = fixture.bounds();
    let app_slot = usize::from(state.prefs.show_launcher);
    let slots = metrics.samples(None, 0.);
    let slot = &slots[app_slot];
    assert_eq!(f32::from(bounds.left()), slot.origin_main);
    assert_eq!(f32::from(bounds.top()), metrics.cross_padding + metrics.inset);
    assert_eq!(f32::from(bounds.size.width), metrics.icon_size);
    // The cross-axis clearance is the same above and below the icon.
    let above = f32::from(bounds.top()) - metrics.cross_padding;
    let below = (metrics.cross_padding + metrics.thickness) - f32::from(bounds.bottom());
    assert!((above - below).abs() < 0.01);
    assert!(above >= 6.);
    // The end clearance matches apart from the separator clearance inserted
    // between the app group and the trash.
    let left = f32::from(bounds.left()) - metrics.main_padding;
    let right = (metrics.main_padding + metrics.length) - f32::from(bounds.right());
    let separators_after = metrics
        .separator_before
        .iter()
        .filter(|index| **index > app_slot)
        .count() as f32;
    assert!(
        (right - left - separators_after * metrics.separator_extra).abs() < 0.01,
        "left={left} right={right} separators={separators_after}"
    );
    assert!(left >= 6.);
}

#[gpui::test]
fn the_hover_tooltip_grows_with_a_longer_name(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx, false);
    let width_for = |fixture: &mut Fixture, name: &str| -> Pixels {
        fixture.view.update(&mut fixture.visual, |view, cx| {
            view.dock_motion.tooltip_name = Some(name.to_owned());
            cx.notify();
        });
        fixture.visual.update(|window, _| window.refresh());
        fixture.visual.run_until_parked();
        fixture
            .visual
            .debug_bounds("dock-tooltip")
            .expect("the tooltip is painted")
            .size
            .width
    };
    let short = width_for(&mut fixture, "A");
    let long = width_for(&mut fixture, "A very long application name indeed");
    assert!(
        long > short,
        "the tooltip grows with the name: {short:?} vs {long:?}"
    );
    // Both ends are clamped so neither a glyph nor a paragraph is cut off.
    assert!(short >= px(64.) && short <= px(220.));
    assert!(long <= px(220.));
}

#[gpui::test]
fn hovering_a_tile_reveals_its_name_and_leaving_hides_it(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx, false);
    // No tooltip before the pointer arrives.
    assert!(fixture.visual.debug_bounds("dock-tooltip").is_none());
    let icon = fixture.bounds();
    fixture
        .visual
        .simulate_mouse_move(icon.center(), None, Modifiers::default());
    fixture.visual.run_until_parked();
    // The label is armed but waits out its delay before it appears.
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert_eq!(
            view.dock_motion.tooltip_armed.as_ref().map(|(name, _)| name.as_str()),
            Some(APP_ID),
            "the hovered application name is armed"
        );
        assert!(
            view.dock_motion.tooltip_name.is_none(),
            "the label must not appear the instant the pointer arrives"
        );
    });
    assert!(fixture.visual.debug_bounds("dock-tooltip").is_none());
    // Let the one-second delay elapse, then render again.
    fixture.advance_motion(std::time::Duration::from_secs(1));
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert_eq!(
            view.dock_motion.tooltip_name.as_deref(),
            Some(APP_ID),
            "the rested application name is tracked"
        );
    });
    let tooltip = fixture
        .visual
        .debug_bounds("dock-tooltip")
        .expect("the tooltip is painted above the tile");
    assert!(tooltip.size.width > px(0.) && tooltip.size.height > px(0.));
    // The label stays inside the reserved band above the tray.
    assert!(tooltip.bottom() <= icon.top() + px(1.));
    fixture
        .visual
        .simulate_mouse_move(point(px(-20.), px(-20.)), None, Modifiers::default());
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(view.dock_motion.tooltip_name.is_none());
        assert!(view.dock_motion.tooltip_armed.is_none());
    });
}

#[gpui::test]
fn the_drag_leaves_a_shadowed_placeholder_and_lifts_the_icon(cx: &mut TestAppContext) {
    let mut fixture = Fixture::with_apps(cx, false, &[APP_A, APP_B, APP_C]);
    let a = fixture
        .visual
        .debug_bounds("dock-org.clarion.DockA.desktop")
        .expect("pinned application A");
    let c = fixture
        .visual
        .debug_bounds("dock-org.clarion.DockC.desktop")
        .expect("pinned application C");
    fixture.visual.simulate_mouse_down(a.center(), MouseButton::Left, Modifiers::default());
    fixture.visual.simulate_mouse_move(
        c.center(),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    fixture.visual.run_until_parked();
    // The dragged icon floats on its own selector and keeps the running order
    // hole; the placeholder occupies the landing slot instead of nothing.
    let dragged = fixture
        .visual
        .debug_bounds("dock-drag-org.clarion.DockA.desktop")
        .expect("the dragged icon floats above the row");
    assert!(dragged.size.width > px(0.));
    // The placeholder is a plain div in the hole, so the row keeps its span.
    let row_left = fixture
        .visual
        .debug_bounds("dock-org.clarion.DockB.desktop")
        .expect("application B stays in the row");
    assert!(row_left.left() >= px(0.));
}

#[gpui::test]
fn a_reorder_slides_the_untouched_icons_towards_their_new_slots(cx: &mut TestAppContext) {
    let mut fixture = Fixture::with_apps(cx, false, &[APP_A, APP_B, APP_C]);
    let a = fixture
        .visual
        .debug_bounds("dock-org.clarion.DockA.desktop")
        .expect("pinned application A");
    let c = fixture
        .visual
        .debug_bounds("dock-org.clarion.DockC.desktop")
        .expect("pinned application C");
    // Lift A and drag it past C, so B and C move a slot towards the leading edge.
    fixture.visual.simulate_mouse_down(a.center(), MouseButton::Left, Modifiers::default());
    let drop = c.center() + point(px(64.), px(0.));
    fixture.visual.simulate_mouse_move(drop, Some(MouseButton::Left), Modifiers::default());
    fixture.visual.run_until_parked();
    // The in-flight arrangement tracks each sliding icon with a motion that is
    // still settling, so the Dock keeps asking for frames.
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(view.dock_motion.drag.is_some());
        assert!(
            !view.dock_motion.reorder.is_empty(),
            "the untouched pinned icons own a reorder motion"
        );
        assert!(
            view.dock_motion.reorder.values().any(|m| m.target() > 0.),
            "the sliding icons retarget away from the resting origin"
        );
    });
    // A released drag snaps the icons back onto their slots and drains the map.
    fixture.visual.simulate_mouse_up(drop, MouseButton::Left, Modifiers::default());
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(view.dock_motion.drag.is_none());
        assert!(
            view.dock_motion.reorder.is_empty(),
            "a settled row clears the reorder motions"
        );
    });
}

#[gpui::test]
fn dragging_a_pinned_icon_reorders_it_and_never_launches(cx: &mut TestAppContext) {
    let mut fixture = Fixture::with_apps(cx, false, &[APP_A, APP_B, APP_C]);
    let a = fixture
        .visual
        .debug_bounds("dock-org.clarion.DockA.desktop")
        .expect("pinned application A");
    let c = fixture
        .visual
        .debug_bounds("dock-org.clarion.DockC.desktop")
        .expect("pinned application C");
    fixture.visual.simulate_mouse_down(a.center(), MouseButton::Left, Modifiers::default());
    fixture.visual.simulate_mouse_move(
        c.center(),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        let drag = view
            .dock_motion
            .drag
            .as_ref()
            .expect("the press turns into a drag");
        assert_eq!(drag.id, APP_A);
    });
    // Release past the last pinned slot: A lands at the end of the group.
    let drop = c.center() + point(px(64.), px(0.));
    fixture.visual.simulate_mouse_move(drop, Some(MouseButton::Left), Modifiers::default());
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(view.dock_motion.drag.is_some());
    });
    fixture.visual.simulate_mouse_up(drop, MouseButton::Left, Modifiers::default());
    fixture.visual.run_until_parked();
    fixture.host.read_with(&fixture.visual, |host, _| {
        assert_eq!(
            host.prefs.dock_order,
            Some(vec![APP_B.to_owned(), APP_C.to_owned(), APP_A.to_owned()]),
            "the pinned group carries the new order"
        );
    });
    // Persisting the new order saves the preferences; it never launches.
    while let Ok(command) = fixture.commands.try_recv() {
        assert!(
            !matches!(command, backend::Command::LaunchDesktop(_)),
            "a reorder never launches the application"
        );
    }
}

#[gpui::test]
fn a_running_application_is_not_reordered(cx: &mut TestAppContext) {
    // C is installed and running with a window, but not pinned.
    let mut fixture = Fixture::with_mixed_apps(cx, false, &[APP_A, APP_B], &[APP_C]);
    fixture.visual.cx.update(|cx| {
        fixture.host.update(cx, |host, cx| {
            // C runs with a window but is not pinned.
            host.desktop.windows = vec![clarion_ipc::Window {
                id: 7,
                title: Some("Running".into()),
                app_id: Some("org.clarion.DockC".into()),
                pid: None,
                gtk_app_menu: None,
                app_menu: None,
                workspace_id: Some(1),
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
            }];
            host.changed(cx);
        });
    });
    fixture.visual.run_until_parked();
    let c = fixture
        .visual
        .debug_bounds("dock-org.clarion.DockC.desktop")
        .expect("the running application");
    fixture.visual.simulate_mouse_down(c.center(), MouseButton::Left, Modifiers::default());
    fixture.visual.simulate_mouse_move(
        c.center() + point(px(200.), px(0.)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(
            view.dock_motion.drag.is_none(),
            "running applications cannot be rearranged"
        );
    });
    fixture.visual.simulate_mouse_up(
        c.center() + point(px(200.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    fixture.visual.run_until_parked();
    fixture.host.read_with(&fixture.visual, |host, _| {
        assert_eq!(host.prefs.dock_order, None);
    });
}

#[gpui::test]
fn running_and_recent_applications_keep_slots_when_the_pinned_group_overflows(
    cx: &mut TestAppContext,
) {
    let recent = [
        "org.clarion.RecentA.desktop",
        "org.clarion.RecentB.desktop",
        "org.clarion.RecentC.desktop",
    ];
    let running = "org.clarion.Running.desktop";
    let mut fixture = Fixture::with_mixed_apps(cx, false, APP_OVERFLOW, &[
        recent[0], recent[1], recent[2], running,
    ]);
    let name = fixture
        .visual
        .cx
        .update(|cx| testing::output_name(&fixture.host, cx));
    fixture.visual.cx.update(|cx| {
        fixture.host.update(cx, |host, cx| {
            host.prefs.show_recent_applications = true;
            host.prefs.recent_app_ids = recent.iter().map(|id| (*id).to_owned()).collect();
            host.desktop.windows = vec![clarion_ipc::Window {
                id: 7,
                title: Some("Running".into()),
                app_id: Some("org.clarion.Running".into()),
                pid: None,
                gtk_app_menu: None,
                app_menu: None,
                workspace_id: Some(1),
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
            }];
            host.desktop.outputs.insert(
                name,
                clarion_ipc::Output {
                    name: "recent-overflow-test".into(),
                    make: "clarion".into(),
                    model: "recent-overflow-test".into(),
                    serial: None,
                    physical_size: None,
                    modes: Vec::new(),
                    current_mode: None,
                    is_custom_mode: false,
                    vrr_supported: false,
                    vrr_enabled: false,
                    logical: Some(clarion_ipc::LogicalOutput {
                        x: 0,
                        y: 0,
                        width: 400,
                        height: 800,
                        scale: 1.,
                        transform: clarion_ipc::Transform::Normal,
                    }),
                    max_bpc: None,
                    hdr_active: false,
                    hdr_supported: false,
                    hdr_status: None,
                    hdr_peak_luminance: None,
                    sdr_white_level: None,
                    hdr_brightness: None,
                },
            );
            host.changed(cx);
        });
    });
    fixture.visual.run_until_parked();
    assert!(
        fixture.visual.debug_bounds("dock-overflow").is_some(),
        "the pinned group still overflows"
    );
    for id in recent {
        let selector: &'static str = Box::leak(format!("dock-{id}").into_boxed_str());
        fixture
            .visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("recent application {id} was crowded out by pinned apps"));
    }
    fixture
        .visual
        .debug_bounds("dock-org.clarion.Running.desktop")
        .expect("a running application keeps its slot before the recent group");
}

#[gpui::test]
fn a_crowded_dock_shows_the_overflow_tile_and_the_hidden_apps_panel(
    cx: &mut TestAppContext,
) {
    let mut fixture = Fixture::with_apps(cx, false, APP_OVERFLOW);
    // Shrink the logical output so twelve icons no longer fit the row.
    let name = fixture
        .visual
        .cx
        .update(|cx| testing::output_name(&fixture.host, cx));
    fixture.visual.cx.update(|cx| {
        fixture.host.update(cx, |host, cx| {
            host.desktop.outputs.insert(
                name,
                clarion_ipc::Output {
                    name: "overflow-test".into(),
                    make: "clarion".into(),
                    model: "overflow-test".into(),
                    serial: None,
                    physical_size: None,
                    modes: Vec::new(),
                    current_mode: None,
                    is_custom_mode: false,
                    vrr_supported: false,
                    vrr_enabled: false,
                    logical: Some(clarion_ipc::LogicalOutput {
                        x: 0,
                        y: 0,
                        width: 400,
                        height: 800,
                        scale: 1.,
                        transform: clarion_ipc::Transform::Normal,
                    }),
                    max_bpc: None,
                    hdr_active: false,
                    hdr_supported: false,
                    hdr_status: None,
                    hdr_peak_luminance: None,
                    sdr_white_level: None,
                    hdr_brightness: None,
                },
            );
            host.changed(cx);
        });
    });
    fixture.visual.run_until_parked();
    let tile = fixture
        .visual
        .debug_bounds("dock-overflow")
        .expect("the overflow tile appears");
    let ellipsis = fixture
        .visual
        .debug_bounds("dock-overflow-ellipsis")
        .expect("the overflow tile shows an ellipsis inside its rounded rectangle");
    assert_eq!(
        ellipsis.size, tile.size,
        "the drawer glyph must fill the same slot as a normal app icon"
    );
    let badge = fixture
        .visual
        .debug_bounds("dock-overflow-badge")
        .expect("the hidden-app count badge is painted");
    let badge_ratio = |icon: gpui::Bounds<gpui::Pixels>,
                       badge: gpui::Bounds<gpui::Pixels>| {
        (
            f32::from(badge.center().x - icon.center().x) / f32::from(icon.size.width),
            f32::from(badge.center().y - icon.center().y) / f32::from(icon.size.height),
        )
    };
    let resting_ratio = badge_ratio(ellipsis, badge);
    fixture
        .visual
        .simulate_mouse_move(tile.center(), None, Modifiers::default());
    fixture.advance_motion(Duration::from_millis(300));
    let hovered_tile = fixture
        .visual
        .debug_bounds("dock-overflow")
        .expect("the hovered overflow tile is painted");
    let hovered_ellipsis = fixture
        .visual
        .debug_bounds("dock-overflow-ellipsis")
        .expect("the hovered drawer glyph is painted");
    let hovered_badge = fixture
        .visual
        .debug_bounds("dock-overflow-badge")
        .expect("the hovered count badge is painted");
    assert!(
        hovered_ellipsis.size.width > ellipsis.size.width + px(10.),
        "the drawer glyph and badge should enlarge with the Dock: {:?} vs {:?}",
        ellipsis.size,
        hovered_ellipsis.size
    );
    let hovered_ratio = badge_ratio(hovered_ellipsis, hovered_badge);
    assert!(
        (hovered_ratio.0 - resting_ratio.0).abs() < 0.02
            && (hovered_ratio.1 - resting_ratio.1).abs() < 0.02,
        "the badge must keep its position on the drawer glyph: {resting_ratio:?} vs {hovered_ratio:?}"
    );
    // The applications that fit stay visible; the rest fold into the tile.
    let (hidden, visible_apps) = fixture.view.read_with(&fixture.visual, |view, _| {
        let metrics = crate::dock_motion::DockMetrics::new(
            view.state.dock_apps().len(),
            view.state.prefs.dock_size as f32,
            view.state.prefs.magnification_scale as f32,
            view.state.prefs.magnification,
            view.state.prefs.show_launcher,
            false,
            400.,
            800.,
        );
        (metrics.hidden_apps, metrics.visible_apps)
    });
    assert!(hidden > 0);
    fixture
        .visual
        .debug_bounds("dock-org.clarion.OverflowA.desktop")
        .expect("the first application stays visible");
    let last_id = APP_OVERFLOW[visible_apps - 1];
    let last_selector: &'static str = Box::leak(format!("dock-{last_id}").into_boxed_str());
    let last_visible = fixture
        .visual
        .debug_bounds(last_selector)
        .expect("the last visible application is painted");
    // The overflow tile occupies the slot right after the last visible
    // application. (The recorded debug-bounds map keeps stale entries across
    // frames, so folding is proven by the slot order instead of a missing
    // selector.)
    let gap = f32::from(tile.left()) - f32::from(last_visible.right());
    assert!(gap > 0. && gap <= 16., "overflow tile follows the last app");
    fixture
        .visual
        .simulate_click(hovered_tile.center(), Modifiers::default());
    fixture.visual.run_until_parked();
    let panel = fixture
        .visual
        .cx
        .update(|cx| testing::popup(&fixture.host, cx));
    let mut panel_visual = VisualTestContext::from_window(*panel, cx);
    panel_visual.update(|window, _| {
        window.activate_window();
        window.refresh();
    });
    panel_visual.run_until_parked();
    let entry = panel_visual
        .debug_bounds("overflow-org.clarion.OverflowL.desktop")
        .expect("the hidden application is offered in the panel");
    assert!(entry.size.width > px(0.));
    assert!(
        (f32::from(entry.size.width) - f32::from(hovered_tile.size.width)).abs() <= 1.,
        "the drawer must open at the current Dock icon size: {:?} vs {:?}",
        entry.size,
        hovered_tile.size
    );
    // The container is a three-column grid: the first three drawer cells differ
    // only in x, and the fourth starts a new row below the first. Only the hidden
    // applications appear here, so use the ids the Dock folded away.
    let hidden_ids: Vec<String> = fixture.view.read_with(&fixture.visual, |view, _| {
        view.state
            .dock_apps()
            .iter()
            .skip(visible_apps)
            .map(|app| app.id.clone())
            .collect()
    });
    assert!(hidden_ids.len() >= 4);
    let mut cell = |index: usize| {
        let selector: &'static str =
            Box::leak(format!("overflow-{}", hidden_ids[index]).into_boxed_str());
        panel_visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("drawer cell {index} = {}", hidden_ids[index]))
    };
    let (a, b, c, d) = (cell(0), cell(1), cell(2), cell(3));
    assert_eq!(a.top(), b.top());
    assert_eq!(b.top(), c.top());
    assert!(a.left() < b.left() && b.left() < c.left());
    assert!(d.top() > a.top(), "the fourth cell starts a new row");
    assert_eq!(d.left(), a.left(), "every row restarts at the first column");
    // The drawer tiles use the size the clicked overflow tile is currently
    // drawing, including its hover magnification, so the drawer icons match
    // the rest of the enlarged Dock row.
    assert_eq!(a.size.width, hovered_tile.size.width);
    assert_eq!(a.size.height, hovered_tile.size.height);
}

#[gpui::test]
fn dragging_a_pinned_drawer_tile_reorders_the_pinned_group(cx: &mut TestAppContext) {
    let mut fixture = Fixture::with_apps(cx, false, APP_OVERFLOW);
    // A compact output forces a real overflow so the drawer has content.
    let name = fixture
        .visual
        .cx
        .update(|cx| testing::output_name(&fixture.host, cx));
    fixture.visual.cx.update(|cx| {
        fixture.host.update(cx, |host, cx| {
            host.desktop.outputs.insert(
                name,
                clarion_ipc::Output {
                    name: "overflow-test".into(),
                    make: "clarion".into(),
                    model: "overflow-test".into(),
                    serial: None,
                    physical_size: None,
                    modes: Vec::new(),
                    current_mode: None,
                    is_custom_mode: false,
                    vrr_supported: false,
                    vrr_enabled: false,
                    logical: Some(clarion_ipc::LogicalOutput {
                        x: 0,
                        y: 0,
                        width: 400,
                        height: 800,
                        scale: 1.,
                        transform: clarion_ipc::Transform::Normal,
                    }),
                    max_bpc: None,
                    hdr_active: false,
                    hdr_supported: false,
                    hdr_status: None,
                    hdr_peak_luminance: None,
                    sdr_white_level: None,
                    hdr_brightness: None,
                },
            );
            host.changed(cx);
        });
    });
    fixture.visual.run_until_parked();
    // The first hidden application id, and the ones after it.
    let hidden: Vec<String> = fixture.view.read_with(&fixture.visual, |view, _| {
        let metrics = crate::dock_motion::DockMetrics::new(
            view.state.dock_apps().len(),
            view.state.prefs.dock_size as f32,
            view.state.prefs.magnification_scale as f32,
            view.state.prefs.magnification,
            view.state.prefs.show_launcher,
            false,
            400.,
            800.,
        );
        view.state
            .dock_apps()
            .iter()
            .skip(metrics.visible_apps)
            .map(|app| app.id.clone())
            .collect()
    });
    assert!(hidden.len() >= 3, "the drawer has at least three tiles");
    let tile = fixture
        .visual
        .debug_bounds("dock-overflow")
        .expect("the overflow tile appears");
    assert!(
        fixture
            .visual
            .debug_bounds("dock-overflow-ellipsis")
            .is_some(),
        "the overflow tile shows an ellipsis inside its rounded rectangle"
    );
    fixture
        .visual
        .simulate_click(tile.center(), Modifiers::default());
    fixture.visual.run_until_parked();
    let panel = fixture
        .visual
        .cx
        .update(|cx| testing::popup(&fixture.host, cx));
    let mut panel_visual = VisualTestContext::from_window(*panel, cx);
    panel_visual.update(|window, _| {
        window.activate_window();
        window.refresh();
    });
    panel_visual.run_until_parked();
    let first_selector: &'static str = Box::leak(format!("overflow-{}", hidden[0]).into_boxed_str());
    let third_selector: &'static str = Box::leak(format!("overflow-{}", hidden[2]).into_boxed_str());
    let first = panel_visual
        .debug_bounds(first_selector)
        .expect("the first drawer tile");
    let third = panel_visual
        .debug_bounds(third_selector)
        .expect("the third drawer tile");
    // Drag the first tile onto the third cell and release.
    panel_visual.simulate_mouse_down(first.center(), MouseButton::Left, Modifiers::default());
    // The first move starts the drag; the second one tracks it onto the target
    // cell, matching how a pointer actually crosses the grid.
    panel_visual.simulate_mouse_move(
        first.center() + point(px(20.), px(0.)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    panel_visual.simulate_mouse_move(third.center(), Some(MouseButton::Left), Modifiers::default());
    panel_visual.run_until_parked();
    let dragging = panel_visual
        .debug_bounds(Box::leak(
            format!("drawer-drag-{}", hidden[0]).into_boxed_str(),
        ))
        .is_some();
    assert!(dragging, "the dragged tile floats above the grid");
    panel_visual.simulate_mouse_up(third.center(), MouseButton::Left, Modifiers::default());
    panel_visual.run_until_parked();
    // The drag is over once the pointer releases.
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(view.drawer.drag.is_none(), "the release ends the drawer drag");
    });
    // The dragged application left the first cell for the third, so it now sits
    // behind the application that used to be third. The full pinned order
    // reflects it.
    fixture.host.read_with(&fixture.visual, |host, _| {
        let order = host.prefs.dock_order.clone().unwrap_or_default();
        let first = order.iter().position(|id| id == &hidden[0]);
        let third = order.iter().position(|id| id == &hidden[2]);
        assert!(
            first.is_some() && third.is_some(),
            "the reordered applications stay in the pinned group"
        );
        assert!(
            first > third,
            "the dragged application lands after the third tile: order={order:?}"
        );
    });
}

#[gpui::test]
fn clicking_a_drawer_tile_activates_the_app_and_dismisses_the_drawer(cx: &mut TestAppContext) {
    let mut fixture = Fixture::with_apps(cx, false, APP_OVERFLOW);
    // A compact output forces a real overflow so the drawer has content.
    let name = fixture
        .visual
        .cx
        .update(|cx| testing::output_name(&fixture.host, cx));
    fixture.visual.cx.update(|cx| {
        fixture.host.update(cx, |host, cx| {
            host.desktop.outputs.insert(
                name,
                clarion_ipc::Output {
                    name: "overflow-test".into(),
                    make: "clarion".into(),
                    model: "overflow-test".into(),
                    serial: None,
                    physical_size: None,
                    modes: Vec::new(),
                    current_mode: None,
                    is_custom_mode: false,
                    vrr_supported: false,
                    vrr_enabled: false,
                    logical: Some(clarion_ipc::LogicalOutput {
                        x: 0,
                        y: 0,
                        width: 400,
                        height: 800,
                        scale: 1.,
                        transform: clarion_ipc::Transform::Normal,
                    }),
                    max_bpc: None,
                    hdr_active: false,
                    hdr_supported: false,
                    hdr_status: None,
                    hdr_peak_luminance: None,
                    sdr_white_level: None,
                    hdr_brightness: None,
                },
            );
            host.changed(cx);
        });
    });
    fixture.visual.run_until_parked();
    let hidden: Vec<String> = fixture.view.read_with(&fixture.visual, |view, _| {
        let metrics = crate::dock_motion::DockMetrics::new(
            view.state.dock_apps().len(),
            view.state.prefs.dock_size as f32,
            view.state.prefs.magnification_scale as f32,
            view.state.prefs.magnification,
            view.state.prefs.show_launcher,
            false,
            400.,
            800.,
        );
        view.state
            .dock_apps()
            .iter()
            .skip(metrics.visible_apps)
            .map(|app| app.id.clone())
            .collect()
    });
    assert!(!hidden.is_empty(), "the overflow holds applications");
    let tile = fixture
        .visual
        .debug_bounds("dock-overflow")
        .expect("the overflow tile appears");
    assert!(
        fixture
            .visual
            .debug_bounds("dock-overflow-ellipsis")
            .is_some(),
        "the overflow tile shows an ellipsis inside its rounded rectangle"
    );
    fixture
        .visual
        .simulate_click(tile.center(), Modifiers::default());
    fixture.visual.run_until_parked();
    let panel = fixture
        .visual
        .cx
        .update(|cx| testing::popup(&fixture.host, cx));
    let mut panel_visual = VisualTestContext::from_window(*panel, cx);
    panel_visual.update(|window, _| {
        window.activate_window();
        window.refresh();
    });
    panel_visual.run_until_parked();
    let cell_selector: &'static str =
        Box::leak(format!("overflow-{}", hidden[0]).into_boxed_str());
    let cell = panel_visual
        .debug_bounds(cell_selector)
        .expect("the drawer shows the folded application");
    // Clicking the tile activates the application and folds the drawer away.
    panel_visual.simulate_click(cell.center(), Modifiers::default());
    fixture.visual.run_until_parked();
    fixture.host.read_with(&fixture.visual, |host, _| {
        assert!(
            host.panel.is_none(),
            "clicking a drawer tile must dismiss the drawer"
        );
    });
}


#[gpui::test]
fn pinning_and_unpinning_a_folder_grows_the_icon_in_and_rides_it_out(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let folder_path = directory.path().join("Projects");
    std::fs::create_dir(&folder_path).unwrap();
    let mut fixture = Fixture::with_apps(cx, false, &[APP_A]);
    let host = fixture.host.clone();
    let path = folder_path.to_string_lossy().into_owned();
    let pin = |host: &Entity<crate::app::Host>,
               folders: Vec<crate::preferences::DockFolderPreference>,
               fixture: &mut Fixture| {
        fixture.visual.cx.update(|cx| {
            host.update(cx, |host, cx| {
                host.prefs.dock_folders = folders;
                host.changed(cx);
            });
        });
        fixture.visual.update(|window, _| window.refresh());
    };
    let folder_prefs = vec![crate::preferences::DockFolderPreference {
        path: path.clone(),
        display: crate::preferences::FolderDisplay::Folder,
        layout: crate::preferences::FolderLayout::Auto,
        sort: crate::preferences::FolderSort::Name,
        descending: false,
    }];

    // The row before any folder is pinned.
    let rest_trash = fixture.visual.debug_bounds("dock-trash").unwrap();

    // Pin: the arrival slide keys straight to the folder path, the tile
    // paints on its slot, and the resting layout still grows to make room.
    pin(&host, folder_prefs.clone(), &mut fixture);
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(
            view.dock_motion.folders_seeded,
            "the pin diff seeds itself on the first render"
        );
        assert!(
            view.dock_motion
                .folder_appear
                .get(&path)
                .is_some_and(|slide| slide.target() == 1.),
            "the pin keys a grow-in slide for the new folder"
        );
        assert!(
            view.dock_motion.folder_exiting.is_empty(),
            "pinning must not seed an exit ride"
        );
    });
    let pinned_trash = fixture.visual.debug_bounds("dock-trash").unwrap();
    assert!(
        pinned_trash.right() > rest_trash.right(),
        "the pinned slot reserves room for the incoming folder"
    );
    assert!(
        fixture.visual.debug_bounds("dock-folder-0").is_some(),
        "the pinned folder tile paints during the grow-in"
    );

    // Finish the grow-in deterministically (the slide clock is real time in
    // tests, not the mocked test clock) so the fade below starts from full
    // size, the way a folder pinned since an earlier frame would.
    fixture.view.update(&mut fixture.visual, |view, cx| {
        if let Some(slide) = view.dock_motion.folder_appear.get_mut(&path) {
            slide.snap(1.);
        }
        cx.notify();
    });
    fixture.visual.update(|window, _| window.refresh());

    // Unpin: on the first frame the tile rides out on a reserved slot and
    // the row must NOT reflow yet.
    pin(&host, Vec::new(), &mut fixture);
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert_eq!(
            view.dock_motion.folder_exiting.len(),
            1,
            "the unpinned folder rides an exit slide on a reserved slot"
        );
        assert_eq!(
            view.dock_motion.folder_exiting[0]
                .0
                .path
                .to_string_lossy(),
            path.as_str()
        );
    });
    let riding_trash = fixture.visual.debug_bounds("dock-trash").unwrap();
    assert_eq!(
        riding_trash, pinned_trash,
        "the row keeps the slot reserved while the fade runs"
    );
    assert!(
        fixture.visual.debug_bounds("dock-folder-exit").is_some(),
        "the fading folder tile paints during the exit ride"
    );

    // Settle the ride directly; the next render retires it and reflows.
    fixture.view.update(&mut fixture.visual, |view, cx| {
        if let Some((_, slide)) = view.dock_motion.folder_exiting.first_mut() {
            slide.snap(0.);
        }
        cx.notify();
    });
    fixture.visual.update(|window, _| window.refresh());
    fixture.visual.run_until_parked();
    fixture.view.read_with(&fixture.visual, |view, _| {
        assert!(
            view.dock_motion.folder_exiting.is_empty(),
            "a finished ride retires on the next render"
        );
    });
    // The test platform's debug_bounds accumulate across frames, so the row
    // reflow is asserted on geometry rather than on element absence.
    let settled_trash = fixture.visual.debug_bounds("dock-trash").unwrap();
    assert_eq!(settled_trash, rest_trash, "the row reflows after the fade");
}
