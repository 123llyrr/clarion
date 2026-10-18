//! Dispatch actual Dock widgets on GPUI's isolated test platform. The recorded
//! masks are the exact rectangles sent by rendering, since TestWindow itself
//! does not implement compositor-side input-region routing.
use super::*;
use crate::app::testing;
use ::core::prelude::v1::test;
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, WindowHandle};
use std::collections::HashMap;
use std::time::Duration;

const APP_ID: &str = "org.clarion.DockFixture.desktop";
const APP_SELECTOR: &str = "dock-org.clarion.DockFixture.desktop";

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
        let directory = tempfile::tempdir().unwrap();
        let applications = directory.path().join("data/applications");
        std::fs::create_dir_all(&applications).unwrap();
        std::fs::write(
            applications.join(APP_ID),
            "[Desktop Entry]\nType=Application\nDBusActivatable=true\nName=Dock Fixture\nCategories=Utility;\n",
        )
        .unwrap();
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
        assert_eq!(catalog.apps().len(), 1);
        let host = cx.update(|cx| testing::test_host(cx, catalog, environment));
        host.update(cx, |host, _| {
            host.prefs.pinned_app_ids = Some(vec![APP_ID.into()]);
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
