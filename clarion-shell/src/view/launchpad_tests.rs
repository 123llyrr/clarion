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
