// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use ::core::prelude::v1::test;
use clarion_launcher::catalog::AppCatalog;

use super::*;
use crate::app::testing;

fn fixture(cx: &mut TestAppContext) -> ViewState {
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    let window = cx.update(|cx| testing::open_panel(&host, Panel::Apps, cx));
    window
        .root(cx)
        .unwrap()
        .read_with(cx, |view, _| view.state.as_ref().clone())
}

fn covers(regions: &[Bounds<Pixels>], x: f32, y: f32) -> bool {
    regions.iter().any(|region| {
        x >= f32::from(region.left())
            && x < f32::from(region.right())
            && y >= f32::from(region.top())
            && y < f32::from(region.bottom())
    })
}

fn assert_bounded(regions: &[Bounds<Pixels>], width: f32, height: f32) {
    assert!(!regions.is_empty() && regions.len() <= 4);
    for region in regions {
        assert!(region.left() >= px(0.) && region.top() >= px(0.));
        assert!(region.right() <= px(width) && region.bottom() <= px(height));
        assert!(region.size.width > px(0.) && region.size.height > px(0.));
    }
}

#[gpui::test]
fn dock_cutout_keeps_the_rest_of_the_bottom_or_side_wallpaper_blurred(cx: &mut TestAppContext) {
    let mut state = fixture(cx);
    state.prefs.dock_size = 48.;
    state.prefs.show_launcher = true;
    for position in [
        DockPosition::Bottom,
        DockPosition::Left,
        DockPosition::Right,
    ] {
        state.prefs.dock_position = position;
        for auto_hide in [false, true] {
            state.prefs.dock_auto_hide = auto_hide;
            let regions = backdrop_regions(&state, 1440., 900.);
            assert_bounded(&regions, 1440., 900.);
            // The cutout is the resting tray, sampled at its center.
            let (tx, ty, tw, th) =
                dock_cutout(&state, 1440., 900.).expect("visible dock has a cutout");
            let dock_x = tx + tw / 2.;
            let dock_y = ty + th / 2.;
            // A wallpaper point far from the tray stays dimmed.
            let (wallpaper_x, wallpaper_y) = match position {
                DockPosition::Bottom => (16., 64.),
                DockPosition::Left => (54., 64.),
                DockPosition::Right => (1386., 64.),
            };
            assert!(
                !covers(&regions, dock_x, dock_y),
                "Dock covered: {position:?}"
            );
            assert!(
                covers(&regions, wallpaper_x, wallpaper_y),
                "Old full-width/height transparent strip returned: {position:?}"
            );
            // The transparent band the surface reserves for the tooltip and
            // magnification is dimmed like the rest of the wallpaper, so no
            // rectangle shows around the tray.
            let band = match position {
                DockPosition::Bottom => (dock_x, ty - 14.),
                DockPosition::Left => (tx + tw + 14., dock_y),
                DockPosition::Right => (tx - 14., dock_y),
            };
            assert!(
                covers(&regions, band.0, band.1),
                "Surface band not dimmed: {position:?}"
            );
        }
    }
}

#[gpui::test]
fn a_hidden_dock_restores_the_full_launchpad_backdrop(cx: &mut TestAppContext) {
    let mut state = fixture(cx);
    state.prefs.dock_hidden = true;
    for position in [
        DockPosition::Bottom,
        DockPosition::Left,
        DockPosition::Right,
    ] {
        state.prefs.dock_position = position;
        assert_eq!(launchpad_insets(&state), (0., 0., 0.));
        let regions = backdrop_regions(&state, 1440., 900.);
        assert_bounded(&regions, 1440., 900.);
        assert_eq!(regions.len(), 1);
        for point in [
            (1., 1.),
            (1439., 1.),
            (1., 899.),
            (1439., 899.),
            (720., 450.),
        ] {
            assert!(covers(&regions, point.0, point.1));
        }
    }
}

#[gpui::test]
fn dock_backdrop_rectangles_stay_in_bounds_across_output_sizes(cx: &mut TestAppContext) {
    let mut state = fixture(cx);
    for (width, height) in [(640., 480.), (1440., 900.), (3840., 2160.)] {
        for position in [
            DockPosition::Bottom,
            DockPosition::Left,
            DockPosition::Right,
        ] {
            state.prefs.dock_position = position;
            for icon_size in [32., 64.] {
                state.prefs.dock_size = icon_size;
                assert_bounded(&backdrop_regions(&state, width, height), width, height);
            }
        }
    }
}

#[test]
fn the_grid_spends_the_screen_on_columns_and_pages_sideways() {
    let wide = launchpad_metrics(1440., 640.);
    assert_eq!(wide.columns, 9, "a wide output gets the full column count");
    assert!(
        (3..=5).contains(&wide.rows),
        "few rows: {} for 640px of grid room",
        wide.rows
    );
    assert!(wide.per_page() >= 27);
    assert!((wide.cell_width - wide.width / 9.).abs() < f32::EPSILON);

    // A small output keeps the minimum shape instead of dropping to one tile.
    let small = launchpad_metrics(640., 300.);
    assert!(small.columns >= LAUNCHPAD_MIN_COLUMNS);
    assert!(small.rows >= LAUNCHPAD_MIN_ROWS);
    assert!(small.width <= 640.);

    // Pages clamp both ends.
    assert_eq!(launchpad_page(0, 100, 20), (0, 5));
    assert_eq!(launchpad_page(41, 100, 20), (2, 5));
    assert_eq!(launchpad_page(99, 1, 20), (0, 1));
    assert_eq!(launchpad_page(0, 0, 20), (0, 1));
}

/// `debug_bounds` keys on `&'static str`, and the fixture's app ids are built
/// at runtime; leaking them in a test process is fine.
fn leaked(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

#[gpui::test]
fn the_launchpad_pages_a_wide_grid_with_arrows_and_dots(cx: &mut TestAppContext) {
    let names: Vec<String> = (0..48).map(|index| format!("App {index:02}")).collect();
    let apps: Vec<(&str, &str, &str)> = names
        .iter()
        .map(|name| (name.as_str(), name.as_str(), ""))
        .collect();
    let (_fixture, host, _view, mut visual, _window) =
        crate::view::ui_tests::panel_with_apps(cx, Panel::Apps, &apps);
    let state = cx.update(|cx| crate::app::testing::snapshot(&host, cx));
    let order: Vec<String> = state
        .catalog
        .search("")
        .iter()
        .map(|app| app.id.clone())
        .collect();
    let (page, pages, per_page) = visual.update(|window, _| {
        super::launchpad_page_count(&state, window, order.len(), 0)
    });
    assert_eq!(page, 0);
    assert!(pages > 1, "48 apps do not fit one wide page");

    let search = visual
        .debug_bounds("launchpad-search")
        .expect("the capsule is rendered");
    assert_eq!(f32::from(search.size.height), LAUNCHPAD_CAPSULE_H);
    assert!(f32::from(search.size.width) > 400., "the capsule is wide");
    assert!(visual.debug_bounds("launchpad-prev").is_some());
    assert!(visual.debug_bounds("launchpad-next").is_some());
    assert!(
        visual
            .debug_bounds(leaked(format!("launchpad-page-{}", pages - 1)))
            .is_some()
    );

    assert!(
        visual
            .debug_bounds(leaked(format!("launchpad-app-{}", order[0])))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(leaked(format!("launchpad-app-{}", order[per_page])))
            .is_none(),
        "the next page is not painted on this one"
    );

    let next = visual.debug_bounds("launchpad-next").unwrap();
    visual.simulate_click(next.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(leaked(format!("launchpad-app-{}", order[0])))
            .is_none(),
        "the arrows page sideways instead of scrolling"
    );
    assert!(
        visual
            .debug_bounds(leaked(format!("launchpad-app-{}", order[per_page])))
            .is_some()
    );

    // The dots jump straight to a page too.
    let last = visual
        .debug_bounds(leaked(format!("launchpad-page-{}", pages - 1)))
        .unwrap();
    visual.simulate_click(last.center(), Modifiers::default());
    visual.run_until_parked();
    let last_page_first = leaked(format!(
        "launchpad-app-{}",
        order[((pages - 1) * per_page).min(order.len() - 1)]
    ));
    assert!(
        visual.debug_bounds(last_page_first).is_some(),
        "the last dot shows the last page"
    );
}
