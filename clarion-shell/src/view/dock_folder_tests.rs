//! Dock folder popup rendering tests on GPUI's isolated test platform.

use ::core::prelude::v1::test;
use std::collections::HashMap;

use gpui::{px, TestAppContext, VisualTestContext};

use super::dock_folder::{
    fan_capacity, fan_close_progress, fan_extension, fan_extension_center, fan_geometry,
    fan_open_progress, fan_stack_layers,
};
use super::*;
use crate::app::testing;
use crate::preferences::{DockPosition, FolderLayout};
use clarion_launcher::catalog::AppCatalog;

fn viewer_host(cx: &mut TestAppContext) -> (tempfile::TempDir, gpui::Entity<crate::app::Host>) {
    let root = tempfile::tempdir().unwrap();
    let applications = root.path().join("data/applications");
    std::fs::create_dir_all(&applications).unwrap();
    std::fs::write(
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
    let host = cx.update(|cx| testing::test_host(cx, catalog, environment));
    (root, host)
}

#[test]
fn fan_items_stay_inside_every_output_edge() {
    let viewport = size(px(1440.), px(900.));
    for position in [
        DockPosition::Bottom,
        DockPosition::Left,
        DockPosition::Right,
    ] {
        let anchor = match position {
            DockPosition::Bottom => point(px(720.), px(860.)),
            DockPosition::Left => point(px(40.), px(450.)),
            DockPosition::Right => point(px(1400.), px(450.)),
        };
        let visible = fan_capacity(anchor, position, 48., viewport).min(8);
        let (items, bounds) =
            fan_geometry(anchor, position, visible, visible, 0, 48., viewport);
        assert_eq!(items.len(), visible);
        assert!(bounds.left() >= px(0.) && bounds.top() >= px(0.));
        assert!(bounds.right() <= viewport.width && bounds.bottom() <= viewport.height);
        for item in items {
            let icon = item.icon_extent();
            let label = item.label_extent();
            assert!(icon.left() >= px(0.) && icon.right() <= viewport.width);
            assert!(label.left() >= px(0.) && label.right() <= viewport.width);
            assert!(icon.top() >= px(0.) && icon.bottom() <= viewport.height);
            assert!(label.top() >= px(0.) && label.bottom() <= viewport.height);
            match position {
                DockPosition::Bottom => assert!(item.icon.bottom() <= anchor.y),
                DockPosition::Left => assert!(item.icon.center().y <= anchor.y),
                DockPosition::Right => assert!(item.icon.center().y <= anchor.y),
            }
        }
        assert!(visible >= 1);
    }
}

#[test]
fn crowded_fan_keeps_even_spacing_and_a_gentle_arc() {
    let viewport = size(px(1440.), px(900.));
    let (items, _) = fan_geometry(
        point(px(720.), px(860.)),
        DockPosition::Bottom,
        12,
        10,
        0,
        48.,
        viewport,
    );
    assert_eq!(items.len(), 10);
    let gaps = items
        .windows(2)
        .map(|pair| (pair[1].icon.top() - pair[0].icon.top()).abs())
        .collect::<Vec<_>>();
    for gap in &gaps {
        assert!(
            (f32::from(*gap) - f32::from(gaps[0])).abs() < 0.01,
            "the fan step must stay uniform, got {gaps:?}"
        );
    }
    assert!(
        gaps[0] >= px(52.),
        "equal rows must leave room for the enlarged labels, got {:?}",
        gaps[0]
    );
    let curve = (items[9].icon.left() - items[0].icon.left()).abs();
    assert!(
        curve > px(30.) && curve < px(110.),
        "the bottom fan should keep a gentle arc, got {curve:?}"
    );
    let angle_steps = items
        .windows(2)
        .map(|pair| pair[1].angle - pair[0].angle)
        .collect::<Vec<_>>();
    for step in &angle_steps {
        assert!(
            (step - angle_steps[0]).abs() < 0.01,
            "the tilt must change evenly, got {angle_steps:?}"
        );
    }
    assert!(items.last().unwrap().angle <= 18.5);
}

#[test]
fn bottom_fan_rows_leave_room_for_rotated_labels() {
    let viewport = size(px(1440.), px(900.));
    let (items, _) = fan_geometry(
        point(px(720.), px(860.)),
        DockPosition::Bottom,
        10,
        10,
        0,
        48.,
        viewport,
    );
    for pair in items.windows(2) {
        let lower = pair[0].label_extent();
        let upper = pair[1].label_extent();
        assert!(
            upper.bottom() <= lower.top() + px(1.),
            "bottom labels must not overlap: {lower:?} / {upper:?}"
        );
    }
}

#[test]
fn side_fan_labels_stay_compact_without_overlapping() {
    let viewport = size(px(1440.), px(900.));
    for (position, anchor) in [
        (DockPosition::Left, point(px(40.), px(450.))),
        (DockPosition::Right, point(px(1400.), px(450.))),
    ] {
        let visible = fan_capacity(anchor, position, 48., viewport);
        let (items, _) =
            fan_geometry(anchor, position, visible, visible, 0, 48., viewport);
        assert_eq!(items.len(), visible);
        for item in &items {
            assert!(
                (item.label_angle - item.angle).abs() < 0.01,
                "{position:?} labels must rotate with their icons"
            );
        }
        let direction = if position == DockPosition::Left { 1. } else { -1. };
        for pair in items.windows(2) {
            let gap = f32::from(pair[1].icon.center().x - pair[0].icon.center().x)
                * direction;
            assert!(
                gap >= 110.,
                "side icons need horizontal spacing for their labels: {gap}"
            );
            let first = pair[0].label_extent();
            let second = pair[1].label_extent();
            if direction > 0. {
                assert!(
                    second.left() >= first.right() - px(1.),
                    "side labels must not overlap horizontally: {first:?} / {second:?}"
                );
            } else {
                assert!(
                    second.right() <= first.left() + px(1.),
                    "side labels must not overlap horizontally: {first:?} / {second:?}"
                );
            }
        }
    }
}

#[test]
fn small_side_icons_keep_enough_room_for_tilted_labels() {
    let viewport = size(px(1440.), px(900.));
    for position in [DockPosition::Left, DockPosition::Right] {
        let anchor = match position {
            DockPosition::Left => point(px(40.), px(450.)),
            DockPosition::Right => point(px(1400.), px(450.)),
            DockPosition::Bottom => unreachable!(),
        };
        let visible = fan_capacity(anchor, position, 32., viewport);
        let (items, _) =
            fan_geometry(anchor, position, visible, visible, 0, 32., viewport);
        let direction = if position == DockPosition::Left { 1. } else { -1. };
        for pair in items.windows(2) {
            let gap = f32::from(pair[1].icon.center().x - pair[0].icon.center().x)
                * direction;
            assert!(
                gap >= 110.,
                "{position:?} labels would collide at small icon sizes: {gap}"
            );
            let first = pair[0].label_extent();
            let second = pair[1].label_extent();
            if direction > 0. {
                assert!(
                    second.left() >= first.right() - px(1.),
                    "{position:?} labels collide: {first:?} / {second:?}"
                );
            } else {
                assert!(
                    second.right() <= first.left() + px(1.),
                    "{position:?} labels collide: {first:?} / {second:?}"
                );
            }
        }
    }
}

#[test]
fn side_fan_capacity_accounts_for_roomier_rows() {
    let viewport = size(px(1440.), px(900.));
    for position in [DockPosition::Left, DockPosition::Right] {
        let capacity = fan_capacity(point(px(40.), px(450.)), position, 80., viewport);
        assert!(
            capacity <= 9,
            "80px side icons need more than nine rows in {position:?}: {capacity}"
        );
        assert!(capacity >= 1);
    }
}

#[test]
fn fan_items_rotate_with_the_curve_from_the_dock() {
    let viewport = size(px(1440.), px(900.));
    let (bottom, _) = fan_geometry(
        point(px(720.), px(860.)),
        DockPosition::Bottom,
        8,
        8,
        0,
        48.,
        viewport,
    );
    assert!(bottom[0].angle.abs() <= 0.01);
    assert!(
        bottom[7].angle > 15.,
        "the outer bottom item should follow the arc, got {}",
        bottom[7].angle
    );
    assert!(bottom[7].angle <= 18.5);

    for (position, expected) in [
        (DockPosition::Left, 1.0),
        (DockPosition::Right, -1.0),
    ] {
        let anchor = match position {
            DockPosition::Left => point(px(40.), px(450.)),
            DockPosition::Right => point(px(1400.), px(450.)),
            DockPosition::Bottom => unreachable!(),
        };
        let visible = fan_capacity(anchor, position, 48., viewport);
        let (items, _) =
            fan_geometry(anchor, position, visible, visible, 0, 48., viewport);
        assert!(
            items[0].angle.abs() < 0.01,
            "{position:?} the item at the dock must start upright, got {}",
            items[0].angle
        );
        assert!(
            (items.last().unwrap().angle.abs() - 18.).abs() < 0.01,
            "{position:?} outer item should lean into the curve, got {}",
            items.last().unwrap().angle
        );
        for pair in items.windows(2) {
            assert!(
                pair[1].angle * expected > pair[0].angle * expected,
                "{position:?} cards must rotate gradually with the curve"
            );
        }
        assert!(
            items[0].icon.center().y > items.last().unwrap().icon.center().y,
            "{position:?} fan must flip upward from the dock tile"
        );
    }
}

#[test]
fn side_fan_starts_upright_and_curves_upward() {
    let viewport = size(px(1440.), px(900.));
    for (position, direction) in [
        (DockPosition::Left, 1.),
        (DockPosition::Right, -1.),
    ] {
        let anchor = match position {
            DockPosition::Left => point(px(40.), px(450.)),
            DockPosition::Right => point(px(1400.), px(450.)),
            DockPosition::Bottom => unreachable!(),
        };
        let visible = fan_capacity(anchor, position, 48., viewport);
        let (items, _) =
            fan_geometry(anchor, position, visible, visible, 0, 48., viewport);
        assert!(visible >= 3, "{position:?} needs a curve to test");
        let first = items.first().unwrap();
        let last = items.last().unwrap();
        assert!(
            (f32::from(first.icon.center().y) - 450.).abs() < 0.01,
            "{position:?} the first icon must leave the tile horizontally"
        );
        assert!(
            last.icon.center().y < first.icon.center().y - px(20.),
            "{position:?} the row must curve upward"
        );
        assert!(
            first.angle.abs() < 0.01,
            "{position:?} the first icon must start upright"
        );
        assert!(
            (last.angle.abs() - 18.).abs() < 0.01,
            "{position:?} the outer icon should lean into the curve, got {}",
            last.angle
        );
        for pair in items.windows(2) {
            let lower = pair[0].angle * direction;
            let upper = pair[1].angle * direction;
            assert!(
                upper > lower,
                "{position:?} cards must rotate gradually with the curve"
            );
            assert!(
                upper - lower < 30.,
                "{position:?} a card jumped instead of rotating gradually: {lower} -> {upper}"
            );
        }
        let x_steps = items
            .windows(2)
            .map(|pair| {
                f32::from(pair[1].icon.center().x - pair[0].icon.center().x) * direction
            })
            .collect::<Vec<_>>();
        for step in &x_steps {
            assert!(
                *step > 100.,
                "{position:?} cards should extend horizontally: {x_steps:?}"
            );
            assert!(
                (*step - x_steps[0]).abs() < 0.01,
                "{position:?} horizontal spacing must stay even: {x_steps:?}"
            );
        }
        let first_rise = first.icon.center().y - items[1].icon.center().y;
        let last_rise =
            items[visible - 2].icon.center().y - last.icon.center().y;
        assert!(
            last_rise > first_rise,
            "{position:?} the upward curve must accelerate: {first_rise:?} vs {last_rise:?}"
        );
    }
}

#[test]
fn bottom_icon_and_label_rotate_as_one_rigid_group() {
    let viewport = size(px(1440.), px(900.));
    let anchor = point(px(720.), px(860.));
    let visible = fan_capacity(anchor, DockPosition::Bottom, 48., viewport).min(6);
    let (items, _) = fan_geometry(
        anchor,
        DockPosition::Bottom,
        visible,
        visible,
        0,
        48.,
        viewport,
    );
    for item in items {
        let dx = f32::from(item.label.center().x - item.icon.center().x);
        let dy = f32::from(item.label.center().y - item.icon.center().y);
        let distance = (dx * dx + dy * dy).sqrt();
        assert!(
            (distance - 92.).abs() < 0.01,
            "label offset must be rigid, got {distance}"
        );
        let offset_angle = dy.atan2(dx).to_degrees();
        let delta = (offset_angle - item.angle).rem_euclid(180.);
        let delta = delta.min(180. - delta);
        assert!(
            delta < 0.01,
            "label and icon angle drifted: {offset_angle} vs {}",
            item.angle
        );
    }
}

#[test]
fn side_labels_rotate_with_their_icon_below_the_card() {
    let viewport = size(px(1440.), px(900.));
    for (position, anchor) in [
        (DockPosition::Left, point(px(40.), px(450.))),
        (DockPosition::Right, point(px(1400.), px(450.))),
    ] {
        let visible = fan_capacity(anchor, position, 48., viewport);
        let (items, _) =
            fan_geometry(anchor, position, visible, visible, 0, 48., viewport);
        let first = items.first().unwrap();
        assert!(
            first.label.center().y > first.icon.center().y + px(20.),
            "{position:?} the first side label must sit below its icon"
        );
        for item in items {
            assert!(
                (item.label_angle - item.angle).abs() < 0.01,
                "{position:?} the label must share the icon's angle"
            );
            let dx = f32::from(item.label.center().x - item.icon.center().x);
            let dy = f32::from(item.label.center().y - item.icon.center().y);
            let distance = (dx * dx + dy * dy).sqrt();
            assert!(
                (distance - 40.).abs() < 0.01,
                "{position:?} the label must hug the icon's local bottom: {distance}"
            );
            let offset = dy.atan2(dx).to_degrees();
            let expected = (item.angle + 90.).rem_euclid(360.);
            let delta = (offset - expected).rem_euclid(360.);
            let delta = delta.min(360. - delta);
            assert!(
                delta < 0.01,
                "{position:?} label offset drifted: {offset} vs {expected}"
            );
        }
    }
}

#[test]
fn fan_open_curve_overshoots_then_settles() {
    let (start, active) = fan_open_progress(std::time::Duration::ZERO);
    assert!(start.abs() < 1e-5, "the fan must start collapsed: {start}");
    assert!(active);
    let (middle, active) = fan_open_progress(std::time::Duration::from_millis(300));
    assert!(active);
    assert!(
        middle > 1.,
        "the fan should spring past its resting size, got {middle}"
    );
    let (end, active) = fan_open_progress(std::time::Duration::from_millis(500));
    assert_eq!(end, 1.);
    assert!(!active);
}

#[test]
fn fan_close_curve_anticipates_then_fades_smoothly() {
    let (start, opacity, active) = fan_close_progress(std::time::Duration::ZERO);
    assert!((start - 1.).abs() < 1e-5, "the fan begins open: {start}");
    assert!((opacity - 1.).abs() < 1e-5, "the fan begins opaque: {opacity}");
    assert!(active);
    // The early anticipation is still visible, but it is now short and the
    // last frames approach zero with no velocity.
    let (early, opacity, active) =
        fan_close_progress(std::time::Duration::from_millis(40));
    assert!(active);
    assert!(
        early > 1.,
        "the retract should ease outwards before collapsing, got {early}"
    );
    assert!((opacity - 1.).abs() < 1e-5, "the anticipation stays opaque");
    let (late, late_opacity, active) =
        fan_close_progress(std::time::Duration::from_millis(220));
    assert!(active);
    assert!(
        late < 1. && late > 0.,
        "the fan should be collapsing, got {late}"
    );
    assert!(
        late_opacity < 1. && late_opacity > 0.,
        "the fan should fade while collapsing, got {late_opacity}"
    );
    assert!((late_opacity - late).abs() < 1e-5);
    // The frame before the window closes is already nearly transparent, so the
    // folder tile underneath is revealed instead of a final snap.
    let (last, last_opacity, active) =
        fan_close_progress(std::time::Duration::from_millis(350));
    assert!(active);
    assert!(last < 0.02, "the last frame is almost collapsed: {last}");
    assert!(
        last_opacity < 0.02,
        "the last frame is almost transparent: {last_opacity}"
    );
    let (end, end_opacity, active) =
        fan_close_progress(std::time::Duration::from_millis(400));
    assert_eq!(end, 0.);
    assert_eq!(end_opacity, 0.);
    assert!(!active);
}

#[test]
fn fan_stack_layers_fade_out_after_three_entries() {
    let layers = fan_stack_layers(12, 0);
    assert_eq!(layers.len(), 3);
    assert_eq!(
        layers.iter().map(|layer| layer.index).collect::<Vec<_>>(),
        vec![2, 1, 0],
        "far layers are painted first"
    );
    assert!(
        layers[0].opacity < layers[1].opacity && layers[1].opacity < layers[2].opacity,
        "opacity rises towards the visible end: {:?}",
        layers.iter().map(|layer| layer.opacity).collect::<Vec<_>>()
    );
    assert!(layers[0].opacity <= 0.3);
    assert!(fan_stack_layers(12, 10).len() == 2);
    assert!(fan_stack_layers(12, 11).len() == 1);
    assert!(fan_stack_layers(12, 12).is_empty());
}

#[test]
fn fan_extension_continues_the_last_tangent() {
    let viewport = size(px(1440.), px(900.));
    let anchor = point(px(720.), px(860.));
    let (items, _) = fan_geometry(anchor, DockPosition::Bottom, 12, 10, 0, 48., viewport);
    let last = items.last().unwrap();
    let (beyond, angle) =
        fan_extension(anchor, DockPosition::Bottom, 10, 48., viewport, 1.);
    assert!(
        (angle - last.angle).abs() < 0.01,
        "the terminal group must share the last item's angle: {angle} vs {}",
        last.angle
    );
    assert!(
        (beyond.x - last.icon.center().x).abs() > px(5.),
        "the extension keeps moving along the tangent: {beyond:?} vs {:?}",
        last.icon.center()
    );
    assert!(beyond.y < last.icon.center().y);
    assert_eq!(
        fan_extension_center(anchor, DockPosition::Bottom, 10, 48., viewport, 1.),
        beyond
    );
}

#[test]
fn side_fan_extension_continues_upward_from_the_dock_tile() {
    let viewport = size(px(1440.), px(900.));
    for (position, anchor) in [
        (DockPosition::Left, point(px(40.), px(450.))),
        (DockPosition::Right, point(px(1400.), px(450.))),
    ] {
        let visible = fan_capacity(anchor, position, 48., viewport);
        let (items, _) =
            fan_geometry(anchor, position, visible, visible, 0, 48., viewport);
        let last = items.last().unwrap();
        let (beyond, angle) = fan_extension(anchor, position, visible, 48., viewport, 1.);
        assert!(
            beyond.y < last.icon.center().y,
            "{position:?} extension must continue upward: {beyond:?}"
        );
        assert!(
            (angle - last.angle).abs() < 0.01,
            "{position:?} terminal angle must match the last item: {angle} vs {}",
            last.angle
        );
        match position {
            DockPosition::Left => assert!(
                beyond.x > last.icon.center().x + px(100.),
                "{position:?} the terminal card should continue horizontally"
            ),
            DockPosition::Right => assert!(
                beyond.x < last.icon.center().x - px(100.),
                "{position:?} the terminal card should continue horizontally"
            ),
            DockPosition::Bottom => unreachable!(),
        }
    }
}

#[test]
fn fan_window_scrolls_only_when_there_are_more_entries() {
    let viewport = size(px(1440.), px(900.));
    let (items, _) = fan_geometry(
        point(px(720.), px(860.)),
        DockPosition::Bottom,
        24,
        8,
        12,
        48.,
        viewport,
    );
    assert_eq!(items.first().unwrap().index, 12);
    assert_eq!(items.last().unwrap().index, 19);
}

fn drag_folder_entry_to_dock(
    cx: &mut TestAppContext,
    layout: FolderLayout,
    entry_selector: &'static str,
    expect_accept: bool,
) -> Option<(std::path::PathBuf, Vec<std::path::PathBuf>)> {
    let (root, host) = viewer_host(cx);
    let folder = root.path().join("pinned");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("note.txt"), b"x").unwrap();
    std::fs::write(folder.join("photo.png"), b"x").unwrap();
    let folder = folder.canonicalize().unwrap();
    host.update(cx, |host, cx| {
        host.prefs.pinned_app_ids = Some(vec!["viewer.desktop".into()]);
        host.handle(Action::PinFolder(folder.clone()), cx);
        host.handle(Action::SetFolderLayout(folder.clone(), layout), cx);
    });
    cx.run_until_parked();
    let commands = cx.update(|cx| testing::capture_backend(&host, cx));
    let dock_window = cx.update(|cx| testing::open_dock(&host, cx));
    let mut dock_visual = VisualTestContext::from_window(*dock_window, cx);
    dock_visual.update(|window, _| {
        window.activate_window();
        window.refresh();
    });
    dock_visual.run_until_parked();
    let idle_app = dock_visual
        .debug_bounds("dock-viewer.desktop")
        .expect("the target app is painted in the Dock");
    let window = cx.update(|cx| testing::open_folder(&host, folder, cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    let entry = visual
        .debug_bounds(entry_selector)
        .expect("the folder entry is painted");
    let start = entry.center();
    visual.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
    // Cross GPUI's drag threshold. The popup becomes an invisible fullscreen
    // drag layer that forwards pointer events to the Dock.
    visual.simulate_mouse_move(
        point(start.x + px(24.), start.y),
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("folder-drag-layer").is_some(),
        "grabbing a folder entry leaves an invisible drag layer"
    );
    assert!(
        visual.debug_bounds("folder-drag-preview").is_some(),
        "the drag preview is painted by the fullscreen drag layer"
    );
    let output = visual.update(|window, _| window.viewport_size());
    let dock_size = dock_visual.update(|window, _| window.viewport_size());
    let origin = point(
        px((f32::from(output.width) - f32::from(dock_size.width)) / 2.),
        px(f32::from(output.height) - 8. - f32::from(dock_size.height)),
    );
    let target = point(idle_app.center().x + origin.x, idle_app.center().y + origin.y);
    visual.simulate_mouse_move(
        target,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    if expect_accept {
        assert!(
            dock_visual.debug_bounds("dock-drag-accept").is_some(),
            "the accepting app shows a + badge on its icon"
        );
        let lifted = dock_visual
            .debug_bounds("dock-drag-accept-icon")
            .expect("the accepting icon is lifted");
        assert!(
            lifted.size.width > idle_app.size.width,
            "the accepting icon must grow: {:?} vs {:?}",
            lifted.size,
            idle_app.size
        );
        assert!(
            dock_visual.debug_bounds("dock-drag-accept-wash").is_some(),
            "the accepting icon gets a translucent wash"
        );
    } else {
        assert!(
            dock_visual.debug_bounds("dock-drag-accept").is_none(),
            "an app that rejects the path must not show a + badge"
        );
        assert!(
            dock_visual
                .debug_bounds("dock-drag-accept-wash")
                .is_none(),
            "an app that rejects the path must not be washed"
        );
    }
    visual.simulate_mouse_up(
        target,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    let mut launched = None;
    while let Ok(command) = commands.try_recv() {
        if let crate::backend::Command::LaunchDesktopWithPaths(path, files) = command {
            launched = Some((path, files));
        }
    }
    launched
}

#[gpui::test]
fn dragging_a_file_over_an_open_folder_magnifies_the_app_under_it(
    cx: &mut TestAppContext,
) {
    let (root, host) = viewer_host(cx);
    let folder = root.path().join("pinned");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("note.txt"), b"x").unwrap();
    let folder = folder.canonicalize().unwrap();
    host.update(cx, |host, cx| {
        host.prefs.pinned_app_ids = Some(vec!["viewer.desktop".into()]);
        host.handle(Action::PinFolder(folder.clone()), cx);
        host.handle(
            Action::SetFolderLayout(folder.clone(), FolderLayout::Fan),
            cx,
        );
    });
    cx.run_until_parked();
    let dock_window = cx.update(|cx| testing::open_dock(&host, cx));
    let dock_view = dock_window.root(cx).unwrap();
    let mut dock_visual = VisualTestContext::from_window(*dock_window, cx);
    dock_visual.update(|window, _| {
        window.activate_window();
        window.refresh();
    });
    dock_visual.run_until_parked();
    let idle_app = dock_visual
        .debug_bounds("dock-viewer.desktop")
        .expect("the target app is painted in the Dock");
    let window = cx.update(|cx| testing::open_folder(&host, folder, cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    let item = visual.debug_bounds("folder-fan-0").unwrap();
    let start = item.center();
    visual.simulate_mouse_down(
        start,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    // Cross GPUI's drag threshold. Grabbing the file retracts the fan right
    // away, so the fullscreen overlay stops owning the pointer.
    visual.simulate_mouse_move(
        point(start.x + px(24.), start.y),
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("folder-drag-layer").is_some(),
        "grabbing a fan item leaves an invisible drag layer that follows the pointer"
    );
    // The fan window stays mapped as an invisible drag layer, so drive the
    // pointer through it and let it forward the move to the Dock.
    let output = visual.update(|window, _| window.viewport_size());
    let dock_size = dock_visual.update(|window, _| window.viewport_size());
    let origin = point(
        px((f32::from(output.width) - f32::from(dock_size.width)) / 2.),
        px(f32::from(output.height) - 8. - f32::from(dock_size.height)),
    );
    let target = point(idle_app.center().x + origin.x, idle_app.center().y + origin.y);
    visual.simulate_mouse_move(
        target,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        dock_visual.debug_bounds("dock-drag-accept").is_some(),
        "the accepting app shows a + badge on its icon"
    );
    dock_visual.update(|window, cx| {
        dock_view.update(cx, |view, cx| {
            view.dock_motion.epoch -= std::time::Duration::from_millis(300);
            cx.notify();
        });
        window.refresh();
    });
    dock_visual.run_until_parked();
    let app_after = dock_visual
        .debug_bounds("dock-viewer.desktop")
        .expect("the target app is still painted");
    let folder_after = dock_visual
        .debug_bounds("dock-folder-0")
        .expect("the folder tile is still painted");
    assert!(
        app_after.size.width > idle_app.size.width + px(10.),
        "the app under the dragged file must magnify: {:?} vs {:?}",
        idle_app.size,
        app_after.size
    );
    assert!(
        app_after.size.width > folder_after.size.width + px(5.),
        "the app must be the primary magnified icon, not the folder tile: {:?} vs {:?}",
        app_after.size,
        folder_after.size
    );
}

#[gpui::test]
fn folder_entry_drag_to_a_compatible_app_launches_it(cx: &mut TestAppContext) {
    let launched = drag_folder_entry_to_dock(
        cx,
        FolderLayout::List,
        "folder-entry-0",
        true,
    );
    let (desktop, files) = launched.expect("the folder entry opened in the target app");
    assert_eq!(desktop.file_name().unwrap(), "viewer.desktop");
    assert_eq!(files[0].file_name().unwrap(), "note.txt");
}

#[gpui::test]
fn folder_entry_drag_to_an_incompatible_app_does_nothing(cx: &mut TestAppContext) {
    let launched = drag_folder_entry_to_dock(
        cx,
        FolderLayout::List,
        "folder-entry-1",
        false,
    );
    assert!(launched.is_none());
}

#[gpui::test]
fn folder_grid_entry_drag_uses_the_same_drag_layer(cx: &mut TestAppContext) {
    let launched = drag_folder_entry_to_dock(
        cx,
        FolderLayout::Grid,
        "folder-entry-0",
        true,
    );
    let (desktop, files) = launched.expect("the grid entry opened in the target app");
    assert_eq!(desktop.file_name().unwrap(), "viewer.desktop");
    assert_eq!(files[0].file_name().unwrap(), "note.txt");
}

#[gpui::test]
fn entering_a_subfolder_shows_its_entries(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let child = root.path().join("child");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(child.join("inside.txt"), b"x").unwrap();
    let root = root.path().canonicalize().unwrap();
    let child = child.canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.clone()), cx);
        host.handle(
            Action::SetFolderLayout(root.clone(), FolderLayout::List),
            cx,
        );
    });
    cx.run_until_parked();
    let window = cx.update(|cx| testing::open_folder(&host, root.clone(), cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    let child_row = visual.debug_bounds("folder-entry-0").unwrap();
    visual.simulate_click(child_row.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    view.read_with(&visual, |view, _| {
        assert_eq!(
            view.state.folder_navigation.as_ref().unwrap().current,
            child,
            "the subfolder was entered"
        );
    });
    assert!(
        visual.debug_bounds("folder-entry-0").is_some(),
        "the subfolder's contents are painted"
    );
}

#[gpui::test]
fn terminal_reveal_button_and_label_rotate_as_one_group(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"x").unwrap();
    std::fs::write(root.path().join("b.txt"), b"x").unwrap();
    let root = root.path().canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.clone()), cx);
        host.handle(
            Action::SetFolderLayout(root.clone(), FolderLayout::Fan),
            cx,
        );
    });
    cx.run_until_parked();
    let window = cx.update(|cx| testing::open_folder(&host, root, cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    let button = visual
        .debug_bounds("folder-fan-reveal")
        .expect("the reveal button is painted");
    let label = visual
        .debug_bounds("folder-fan-reveal-label")
        .expect("the reveal label is painted");
    let dx = f32::from(label.center().x - button.center().x);
    let dy = f32::from(label.center().y - button.center().y);
    let distance = (dx * dx + dy * dy).sqrt();
    assert!(
        (distance - 99.28).abs() < 1.,
        "the terminal pair must stay rigid, got {distance}"
    );
    assert!(
        dy.abs() > 5.,
        "the label must rotate with the terminal button, got dy {dy}"
    );
}

#[gpui::test]
fn clicking_a_fan_file_opens_it(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("note.txt"), b"x").unwrap();
    let root = root.path().canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.clone()), cx);
        host.handle(
            Action::SetFolderLayout(root.clone(), FolderLayout::Fan),
            cx,
        );
    });
    cx.run_until_parked();
    let commands = cx.update(|cx| testing::capture_backend(&host, cx));
    let window = cx.update(|cx| testing::open_folder(&host, root, cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    let item = visual.debug_bounds("folder-fan-0").unwrap();
    visual.simulate_mouse_move(item.center(), None, gpui::Modifiers::default());
    visual.simulate_mouse_down(item.center(), gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.run_until_parked();
    assert!(
        host.read_with(&visual, |host, _| host.panel.is_some()),
        "mouse down on a fan item must not hit the dismiss backdrop"
    );
    visual.simulate_mouse_up(item.center(), gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.run_until_parked();
    let mut opened = false;
    while let Ok(command) = commands.try_recv() {
        opened |= matches!(command, crate::backend::Command::OpenPath(_));
    }
    assert!(opened, "clicking the fan item opens its file");
}

#[gpui::test]
fn dragging_a_fan_item_to_a_compatible_app_launches_it(cx: &mut TestAppContext) {
    let (root, host) = viewer_host(cx);
    let folder = root.path().join("pinned");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("note.txt"), b"x").unwrap();
    let folder = folder.canonicalize().unwrap();
    host.update(cx, |host, cx| {
        host.prefs.pinned_app_ids = Some(vec!["viewer.desktop".into()]);
        host.handle(Action::PinFolder(folder.clone()), cx);
        host.handle(
            Action::SetFolderLayout(folder.clone(), FolderLayout::Fan),
            cx,
        );
    });
    cx.run_until_parked();
    let commands = cx.update(|cx| testing::capture_backend(&host, cx));
    let dock_window = cx.update(|cx| testing::open_dock(&host, cx));
    let mut dock_visual = VisualTestContext::from_window(*dock_window, cx);
    dock_visual.update(|window, _| {
        window.activate_window();
        window.refresh();
    });
    dock_visual.run_until_parked();
    let target = dock_visual
        .debug_bounds("dock-viewer.desktop")
        .expect("the target app is painted in the Dock");
    let window = cx.update(|cx| testing::open_folder(&host, folder, cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    let item = visual.debug_bounds("folder-fan-0").unwrap();
    let start = item.center();
    visual.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.simulate_mouse_move(
        point(start.x + px(24.), start.y),
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    let layer = view.read_with(&visual, |view, _| view.folder_drag_layer);
    let panel_open = host.read_with(&visual, |host, _| host.panel.is_some());
    assert!(
        visual.debug_bounds("folder-drag-layer").is_some() && layer && panel_open,
        "grabbing a fan item leaves an invisible drag layer (layer={layer}, panel={panel_open})"
    );
    let output = visual.update(|window, _| window.viewport_size());
    let dock_size = dock_visual.update(|window, _| window.viewport_size());
    let origin = point(
        px((f32::from(output.width) - f32::from(dock_size.width)) / 2.),
        px(f32::from(output.height) - 8. - f32::from(dock_size.height)),
    );
    let drop = point(target.center().x + origin.x, target.center().y + origin.y);
    visual.simulate_mouse_move(
        drop,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    visual.simulate_mouse_up(
        drop,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        host.read_with(&dock_visual, |host, _| host.panel.is_none()),
        "releasing the drag removes the overlay"
    );
    let mut launched = None;
    while let Ok(command) = commands.try_recv() {
        if let crate::backend::Command::LaunchDesktopWithPaths(path, files) = command {
            launched = Some((path, files));
        }
    }
    let (desktop, files) = launched.expect("the fan item opened in the target app");
    assert_eq!(desktop.file_name().unwrap(), "viewer.desktop");
    assert_eq!(files[0].file_name().unwrap(), "note.txt");
}

#[gpui::test]
fn a_crowded_fan_shows_stacks_at_the_unexpanded_ends(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    for index in 0..12 {
        std::fs::write(root.path().join(format!("{index:02}.txt")), b"x").unwrap();
    }
    let root = root.path().canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.clone()), cx);
        host.handle(
            Action::SetFolderLayout(root.clone(), FolderLayout::Fan),
            cx,
        );
    });
    cx.run_until_parked();
    let window = cx.update(|cx| testing::open_folder(&host, root, cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    let far_stack = visual
        .debug_bounds("folder-fan-stack-far")
        .expect("more content is folded at the outer end");
    assert!(visual.debug_bounds("folder-fan-fold-near").is_none());
    let reveal = visual.debug_bounds("folder-fan-reveal").unwrap();
    let last = visual.debug_bounds("folder-fan-9").unwrap();
    assert!(
        (reveal.center().x - last.center().x).abs() > px(1.),
        "the reveal control continues the arc instead of a straight extension"
    );
    assert!(
        visual.debug_bounds("folder-fan-reveal-label").is_some(),
        "the reveal control carries a side label"
    );
    assert!(far_stack.size.width >= px(40.), "the far stack keeps icon size");
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.folder_fan_offset = 1;
            cx.notify();
        });
        window.refresh();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("folder-fan-fold-near").is_none());
    assert!(visual.debug_bounds("folder-fan-stack-far").is_some());
}

#[gpui::test]
fn side_dock_fan_starts_upright_and_curves_upward_from_the_folder_tile(
    cx: &mut TestAppContext,
) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"x").unwrap();
    std::fs::write(root.path().join("b.txt"), b"x").unwrap();
    let root = root.path().canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.prefs.dock_position = DockPosition::Left;
        host.handle(Action::PinFolder(root.clone()), cx);
        host.handle(
            Action::SetFolderLayout(root.clone(), FolderLayout::Fan),
            cx,
        );
    });
    cx.run_until_parked();
    let window = cx.update(|cx| testing::open_folder(&host, root.clone(), cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    let anchor = view
        .read_with(&visual, |view, _| {
            crate::app::panels::folder_tile_center(view.state.as_ref(), &root, 1440., 900.)
        })
        .unwrap();
    let first = visual
        .debug_bounds("folder-fan-0")
        .expect("the side fan is painted");
    let second = visual
        .debug_bounds("folder-fan-1")
        .expect("the side fan has a second card");
    assert!(
        first.center().x > px(anchor.0),
        "the left-dock fan unfolds inward: {first:?} vs {anchor:?}"
    );
    assert!(
        second.center().x > first.center().x,
        "the side fan extends horizontally: {first:?} / {second:?}"
    );
    assert!(
        second.center().y < first.center().y,
        "the side fan curves upward: {first:?} / {second:?}"
    );
    assert!(
        first.center().y - second.center().y < second.center().x - first.center().x,
        "the first step must stay nearly horizontal: {first:?} / {second:?}"
    );
}

#[gpui::test]
fn folder_menu_uses_vertical_choice_lists(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.clone()), cx);
    });
    cx.run_until_parked();
    let window = cx.update(|cx| {
        testing::open_panel(
            &host,
            Panel::DockFolderMenu {
                path: root.clone(),
            },
            cx,
        )
    });
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    let popup = view
        .read_with(&visual, |view, _| {
            crate::app::panels::placement(
                view.state.as_ref(),
                &Panel::DockFolderMenu {
                    path: root.clone(),
                },
                1440.,
                900.,
            )
            .popup
        })
        .unwrap();
    let height = f32::from(popup.size.height);
    assert!(
        (480. ..=560.).contains(&height),
        "the compact choice menu should not leave a tall empty band, got {height}"
    );
    assert!(
        f32::from(popup.size.width) <= 260.,
        "the folder menu is too wide: {}",
        f32::from(popup.size.width)
    );
    let display = visual.debug_bounds("folder-display-stack").unwrap();
    let open = visual.debug_bounds("folder-open").unwrap();
    assert!(
        display.top() < open.top(),
        "choice controls sit above the actions: {display:?} vs {open:?}"
    );
    for id in [
        "folder-display-stack",
        "folder-layout-grid",
        "folder-sort-size",
        "folder-ascending",
        "folder-descending",
    ] {
        assert!(visual.debug_bounds(id).is_some(), "missing {id}");
    }
    let display = visual.debug_bounds("folder-display-stack").unwrap();
    let display_next = visual.debug_bounds("folder-display-folder").unwrap();
    let layout = visual.debug_bounds("folder-layout-list").unwrap();
    let layout_next = visual.debug_bounds("folder-layout-grid").unwrap();
    let sort = visual.debug_bounds("folder-sort-name").unwrap();
    let direction = visual.debug_bounds("folder-ascending").unwrap();
    let row_pitch = display_next.top() - display.top();
    assert!(
        row_pitch >= px(22.) && row_pitch <= px(24.),
        "choice rows should be packed tightly, got {row_pitch}"
    );
    assert_eq!(
        layout_next.top() - layout.top(),
        row_pitch,
        "every choice list should share the compact row rhythm"
    );
    assert!(display.size.height <= px(22.1));
    assert!(open.size.height <= px(22.1));
    let remove = visual.debug_bounds("folder-remove").unwrap();
    let scroll = visual.debug_bounds("folder-menu-scroll").unwrap();
    let bottom_gap = scroll.bottom() - remove.bottom();
    assert!(
        bottom_gap >= px(0.) && bottom_gap <= px(3.),
        "the last action should sit at the end of the compact column: {remove:?} / {scroll:?}"
    );
    let separators = (0..5)
        .map(|index| {
            let id: &'static str =
                Box::leak(format!("folder-menu-separator-{index}").into_boxed_str());
            visual
                .debug_bounds(id)
                .unwrap_or_else(|| panic!("missing menu separator {index}"))
        })
        .collect::<Vec<_>>();
    assert!(
        separators[1].top() > display.bottom() && separators[1].bottom() < layout.top(),
        "the display group is separated from layout: {display:?} / {:?} / {layout:?}",
        separators[1]
    );
    assert!(
        separators[2].top() > layout.bottom() && separators[2].bottom() < sort.top(),
        "the layout group is separated from sort"
    );
    assert!(
        separators[3].top() > sort.bottom() && separators[3].bottom() < direction.top(),
        "the sort group is separated from direction"
    );
}

#[gpui::test]
fn folder_menu_changes_display_layout_and_sort(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"x").unwrap();
    let root = root.path().canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.clone()), cx);
    });
    cx.run_until_parked();
    let window = cx.update(|cx| {
        testing::open_panel(
            &host,
            Panel::DockFolderMenu {
                path: root.clone(),
            },
            cx,
        )
    });
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    for id in ["folder-display-folder", "folder-layout-grid", "folder-sort-size"] {
        let bounds = visual.debug_bounds(id).expect("menu choice is painted");
        visual.simulate_click(bounds.center(), gpui::Modifiers::default());
        visual.run_until_parked();
    }
    host.read_with(&visual, |host, _| {
        let folder = &host.prefs.dock_folders[0];
        assert_eq!(folder.display, crate::preferences::FolderDisplay::Folder);
        assert_eq!(folder.layout, FolderLayout::Grid);
        assert_eq!(folder.sort, crate::preferences::FolderSort::Size);
    });
    let bounds = visual.debug_bounds("folder-remove").unwrap();
    visual.simulate_click(bounds.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    host.read_with(&visual, |host, _| {
        assert!(host.prefs.dock_folders.is_empty());
        assert!(host.panel.is_none(), "removing the folder closes its menu");
    });
    assert!(root.is_dir());
}

#[gpui::test]
fn auto_layout_opens_a_fan_from_the_folder_tile(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"x").unwrap();
    std::fs::write(root.path().join("b.txt"), b"x").unwrap();
    let root = root.path().canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.clone()), cx);
    });
    cx.run_until_parked();
    let window = cx.update(|cx| testing::open_folder(&host, root, cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("folder-fan-0").is_some());
    assert!(visual.debug_bounds("folder-fan-1").is_some());
    assert!(visual.debug_bounds("folder-fan-back").is_none());
    let reveal = visual
        .debug_bounds("folder-fan-reveal")
        .expect("the fan exposes the current folder");
    let viewport = visual.update(|window, _| window.viewport_size());
    assert!(reveal.left() >= px(0.) && reveal.top() >= px(0.));
    assert!(reveal.right() <= viewport.width && reveal.bottom() <= viewport.height);
}

#[gpui::test]
fn clicking_the_fan_backdrop_dismisses_the_folder(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"x").unwrap();
    let root = root.path().canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.clone()), cx);
    });
    cx.run_until_parked();
    let window = cx.update(|cx| testing::open_folder(&host, root, cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("folder-fan-0").is_some());
    visual.simulate_mouse_down(
        point(px(4.), px(4.)),
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    host.read_with(&visual, |host, _| {
        assert!(host.panel.is_none(), "the backdrop dismisses the fan");
    });
}

#[gpui::test]
fn escape_dismisses_an_open_fan(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"x").unwrap();
    let root = root.path().canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    cx.update(super::bind_keys);
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.clone()), cx);
    });
    cx.run_until_parked();
    let window = cx.update(|cx| testing::open_folder(&host, root, cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("folder-fan-0").is_some());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    host.read_with(&visual, |host, _| {
        assert!(host.panel.is_none(), "Escape dismisses the fan");
    });
}

#[gpui::test]
fn list_and_grid_render_entries_without_wrapping_long_names(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path()
            .join("a-very-long-file-name-that-must-not-wrap-or-grow-the-row.txt"),
        b"x",
    )
    .unwrap();
    std::fs::write(root.path().join("z.txt"), b"x").unwrap();
    let root = root.path().canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.clone()), cx);
        host.handle(
            Action::SetFolderLayout(root.clone(), FolderLayout::List),
            cx,
        );
    });
    cx.run_until_parked();
    let window = cx.update(|cx| testing::open_folder(&host, root.clone(), cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    let long = visual
        .debug_bounds("folder-entry-0")
        .expect("the long entry is painted");
    assert!(long.size.height <= px(34.01));
    assert!(
        long.size.width <= px(280.01),
        "long entry bounds: {long:?}"
    );
}

#[gpui::test]
fn the_fan_back_button_holds_its_spot_while_navigating_deeper(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let sub = root.path().join("Sub");
    let deep = sub.join("Deep");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(sub.join("note.txt"), b"x").unwrap();
    std::fs::write(deep.join("marker.txt"), b"x").unwrap();
    let root = root.path().canonicalize().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.clone()), cx);
        host.handle(
            Action::SetFolderLayout(root.clone(), FolderLayout::Fan),
            cx,
        );
    });
    cx.run_until_parked();
    let window = cx.update(|cx| testing::open_folder(&host, root, cx));
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.update(|window, cx| {
        view.read(cx).focus(window, cx);
        window.refresh();
    });
    visual.run_until_parked();
    // The root owns only the Sub directory, so fan item 0 enters the level
    // below and the back button appears.
    let item = visual.debug_bounds("folder-fan-0").unwrap();
    visual.simulate_mouse_move(item.center(), None, gpui::Modifiers::default());
    visual.simulate_mouse_down(item.center(), gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(item.center(), gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.run_until_parked();
    let back = visual
        .debug_bounds("folder-fan-back")
        .expect("the fan shows the back button after entering a folder");
    host.update(cx, |host, cx| {
        host.handle(Action::NavigateFolder(deep.clone()), cx);
    });
    visual.run_until_parked();
    let back_deeper = visual
        .debug_bounds("folder-fan-back")
        .expect("the back button survives deeper navigation");
    assert_eq!(
        back_deeper, back,
        "the back button holds a fixed spot beside the Dock icon"
    );
}
