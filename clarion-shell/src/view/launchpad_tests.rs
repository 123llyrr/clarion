// SPDX-License-Identifier: GPL-3.0-or-later
use ::core::prelude::v1::test;

use super::*;

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
fn the_search_capsule_is_centred_in_the_top_quarter(cx: &mut TestAppContext) {
    let apps = [("alpha", "Alpha Browser", "")];
    let (_fixture, _host, _view, mut visual, _window) =
        crate::view::ui_tests::panel_with_apps(cx, Panel::Apps, &apps);

    let search = visual
        .debug_bounds("launchpad-search")
        .expect("the search capsule is painted");
    let viewport = visual.update(|window, _| window.viewport_size());
    let expected = f32::from(viewport.height) * LAUNCHPAD_SEARCH_SHARE / 2.;
    let centre = f32::from(search.origin.y) + LAUNCHPAD_CAPSULE_H / 2.;
    assert!(
        (centre - expected).abs() <= 1.,
        "the search capsule should centre in the top quarter: centre {centre}, expected {expected}"
    );
}

#[gpui::test]
fn the_launcher_grid_icons_grow_by_a_fifth(cx: &mut TestAppContext) {
    let apps = [("alpha", "Alpha Browser", "")];
    let (_fixture, _host, _view, mut visual, _window) =
        crate::view::ui_tests::panel_with_apps(cx, Panel::Apps, &apps);

    let tile = visual
        .debug_bounds("launchpad-app-alpha.desktop")
        .expect("the application tile is painted");
    assert_eq!(
        f32::from(tile.size.height),
        LAUNCHPAD_ROW_H,
        "the tile grows with the icon"
    );

    let icon = visual
        .debug_bounds("launchpad-icon-alpha.desktop")
        .expect("the icon plate is painted");
    assert_eq!(
        f32::from(icon.size.width),
        98.,
        "an 86px icon plus its 12px plate should be 98px wide"
    );
}

#[gpui::test]
fn the_bottom_fifth_stays_empty_and_only_dots_close_the_middle_band(cx: &mut TestAppContext) {
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
    let (_, _, per_page) =
        visual.update(|window, _| super::launchpad_page_count(window, order.len(), 0));
    let dots = visual
        .debug_bounds("launchpad-page-0")
        .expect("the page dot row is painted");
    let viewport = visual.update(|window, _| window.viewport_size());
    let height = f32::from(viewport.height);
    let dock_top = height * (1. - LAUNCHPAD_DOCK_SHARE);
    let dock_top = px(dock_top);

    assert!(
        dots.bottom() <= dock_top,
        "the dots must stay inside the middle band: dots bottom {:?}, dock top {:?}",
        dots.bottom(),
        dock_top
    );
    assert!(
        (dots.bottom() - (dock_top - px(LAUNCHPAD_DOTS_BOTTOM))).abs() <= px(1.),
        "the dots close the middle band just above the reserved dock: dots bottom {:?}, dock top {:?}",
        dots.bottom(),
        dock_top
    );
    for app in &order[..per_page] {
        let tile = visual
            .debug_bounds(leaked(format!("launchpad-app-{app}")))
            .expect("every app on the first page is painted");
        assert!(
            tile.bottom() <= dock_top,
            "an app tile intrudes into the reserved dock band: {:?} on {:?}",
            tile,
            app
        );
    }
    let next = visual
        .debug_bounds("launchpad-next")
        .expect("the next arrow is painted");
    assert!(
        next.bottom() <= dock_top,
        "the paging arrow intrudes into the reserved dock band: {:?}",
        next
    );
}

#[gpui::test]
fn the_grid_fills_the_middle_band_with_spacing(cx: &mut TestAppContext) {
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
    let metrics = visual.update(|window, _| super::grid_metrics(window));
    let first = visual
        .debug_bounds(leaked(format!("launchpad-app-{}", order[0])))
        .expect("the first tile is painted");
    let last = visual
        .debug_bounds(leaked(format!(
            "launchpad-app-{}",
            order[(metrics.rows - 1) * metrics.columns]
        )))
        .expect("the last row of the first page is painted");

    assert!(
        (f32::from(first.origin.y) - metrics.grid_top).abs() <= 1.,
        "the grid starts at the top of the middle band: first {:?}, top {}",
        first,
        metrics.grid_top
    );
    assert!(
        (f32::from(last.bottom()) - (metrics.grid_top + metrics.grid_height)).abs() <= 1.,
        "the grid fills the middle band down to the dots: last bottom {:?}, band bottom {}",
        last.bottom(),
        metrics.grid_top + metrics.grid_height
    );

    let second = visual
        .debug_bounds(leaked(format!("launchpad-app-{}", order[metrics.columns])))
        .expect("the second row is painted");
    let row_gap = f32::from(second.origin.y) - f32::from(first.bottom());
    assert!(
        row_gap > LAUNCHPAD_MIN_ROW_GAP,
        "the spare middle height becomes row spacing instead of a block at the top: gap {row_gap}"
    );

    let prev = visual
        .debug_bounds("launchpad-prev")
        .expect("the previous arrow is painted");
    let next = visual
        .debug_bounds("launchpad-next")
        .expect("the next arrow is painted");
    let grid_left = f32::from(first.origin.x);
    let grid_right = f32::from(first.origin.x) + metrics.width;
    assert!(
        grid_left - f32::from(prev.right()) >= LAUNCHPAD_ARROW_GAP - 1.,
        "the arrows keep a roomy gap from the grid: prev {:?}, grid left {grid_left}",
        prev
    );
    assert!(
        f32::from(next.origin.x) - grid_right >= LAUNCHPAD_ARROW_GAP - 1.,
        "the arrows keep a roomy gap from the grid: next {:?}, grid right {grid_right}",
        next
    );
}

#[test]
fn the_launcher_backdrop_is_one_full_output_without_a_dock_hole() {
    for (width, height) in [(640., 480.), (1440., 900.), (3840., 2160.)] {
        let regions = backdrop_regions(width, height);
        assert_bounded(&regions, width, height);
        assert_eq!(regions.len(), 1);
        for (x, y) in [
            (1., 1.),
            (width - 1., 1.),
            (1., height - 1.),
            (width - 1., height - 1.),
            (width / 2., height / 2.),
        ] {
            assert!(
                covers(&regions, x, y),
                "the backdrop leaves ({x}, {y}) undimmed on {width}x{height}"
            );
        }
    }
}

#[test]
fn the_grid_spends_the_screen_on_columns_and_pages_sideways() {
    let wide = launchpad_metrics(1920., 1080.);
    assert_eq!(wide.columns, 9, "a wide output gets the full column count");
    assert_eq!(wide.rows, 3, "the middle band fits three rows");
    assert_eq!(wide.per_page(), 27);
    assert!((wide.grid_top - 1080. * LAUNCHPAD_SEARCH_SHARE).abs() < 0.01);
    assert!((wide.dock_top - 1080. * (1. - LAUNCHPAD_DOCK_SHARE)).abs() < 0.01);
    assert!((wide.cell_width - wide.width / 9.).abs() < f32::EPSILON);

    // A small output keeps the minimum shape instead of dropping to one tile.
    let small = launchpad_metrics(640., 480.);
    assert!(small.columns >= LAUNCHPAD_MIN_COLUMNS);
    assert!(small.rows >= LAUNCHPAD_MIN_ROWS);
    assert!(
        small.per_page() < wide.per_page(),
        "a small screen pages more often: {} vs {}",
        small.per_page(),
        wide.per_page()
    );
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
        super::launchpad_page_count(window, order.len(), 0)
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
