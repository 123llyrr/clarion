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
fn searching_a_window_title_does_not_push_the_grid_down(cx: &mut TestAppContext) {
    let apps = [("alpha", "Alpha Browser", "")];
    let (_fixture, _host, _view, mut visual, _window) =
        crate::view::ui_tests::panel_with_apps_and_windows(
            cx,
            Panel::Apps,
            &apps,
            vec![crate::view::ui_tests::dock_window(
                4242,
                "org.example.alpha",
                "Alpha Browser Window",
            )],
        );

    visual.simulate_input("Alpha");
    visual.run_until_parked();

    let first = visual
        .debug_bounds("launchpad-app-alpha.desktop")
        .expect("the matching application stays in the grid");
    let metrics = visual.update(|window, _| super::grid_metrics(window));
    assert!(
        (f32::from(first.origin.y) - metrics.grid_top).abs() <= 1.,
        "a matching current window must not add a row under the search box: first {:?}, grid top {}",
        first,
        metrics.grid_top
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
fn the_bottom_fifth_stays_empty_and_the_dots_follow_the_grid(cx: &mut TestAppContext) {
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
    let last = visual
        .debug_bounds(leaked(format!("launchpad-app-{}", order[per_page - 1])))
        .expect("the last app on the first page is painted");
    let viewport = visual.update(|window, _| window.viewport_size());
    let height = f32::from(viewport.height);
    let dock_top = px(height * (1. - LAUNCHPAD_DOCK_SHARE));

    assert!(
        (f32::from(dots.top()) - f32::from(last.bottom()) - LAUNCHPAD_DOTS_GAP).abs() <= 2.,
        "the dots keep the widened gap under the grid: grid bottom {:?}, dots top {:?}",
        last.bottom(),
        dots.top()
    );
    assert!(
        dots.bottom() <= dock_top - px(LAUNCHPAD_DOTS_BOTTOM) + px(1.),
        "the dots leave the bottom margin above the reserved dock: dots bottom {:?}, dock top {:?}",
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
fn a_full_page_fills_the_screen_between_equal_side_bands(cx: &mut TestAppContext) {
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
    let viewport = visual.update(|window, _| window.viewport_size());
    let viewport_width = f32::from(viewport.width);
    let first = visual
        .debug_bounds(leaked(format!("launchpad-app-{}", order[0])))
        .expect("the first tile is painted");
    let last_column = visual
        .debug_bounds(leaked(format!(
            "launchpad-app-{}",
            order[metrics.columns - 1]
        )))
        .expect("the last column of the first row is painted");
    let last_row = visual
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
        (f32::from(first.origin.x) - metrics.side).abs() <= 1.,
        "the grid starts at the left edge of the paging band: first {:?}, side {}",
        first,
        metrics.side
    );
    assert!(
        (viewport_width - f32::from(last_column.right()) - metrics.side).abs() <= 1.,
        "a full page fills the width with equal side bands: right {:?}, side {}, viewport {}",
        last_column.right(),
        metrics.side,
        viewport_width
    );
    assert!(
        (f32::from(last_row.bottom()) - (metrics.grid_top + metrics.grid_height)).abs() <= 1.,
        "a full page fills the middle band: last bottom {:?}, grid bottom {}",
        last_row.bottom(),
        metrics.grid_top + metrics.grid_height
    );

    let second_row = visual
        .debug_bounds(leaked(format!("launchpad-app-{}", order[metrics.columns])))
        .expect("the second row is painted");
    let row_gap = f32::from(second_row.origin.y) - f32::from(first.bottom());
    assert!(
        (row_gap - metrics.row_gap).abs() <= 1.,
        "rows share the page's even gap: {row_gap}, expected {}",
        metrics.row_gap
    );
    let second_column = visual
        .debug_bounds(leaked(format!("launchpad-app-{}", order[1])))
        .expect("the second column is painted");
    let column_pitch = f32::from(second_column.origin.x) - f32::from(first.origin.x);
    assert!(
        (column_pitch - metrics.cell_width).abs() <= 1.,
        "columns keep one steady pitch: {column_pitch}, expected {}",
        metrics.cell_width
    );

    let prev = visual
        .debug_bounds("launchpad-prev")
        .expect("the previous arrow is painted");
    let next = visual
        .debug_bounds("launchpad-next")
        .expect("the next arrow is painted");
    assert!(
        f32::from(prev.right()) + metrics.arrow_inset() <= f32::from(first.origin.x) + 1.,
        "the previous arrow keeps its doubled band clear of the grid: prev {:?}",
        prev
    );
    let grid_right = metrics.side + metrics.width;
    assert!(
        f32::from(next.origin.x) >= grid_right + metrics.arrow_inset() - 1.,
        "the next arrow keeps its doubled band clear of the grid: next {:?}, grid right {grid_right}",
        next
    );
}

#[gpui::test]
fn a_partial_page_keeps_the_full_page_pitch(cx: &mut TestAppContext) {
    let names: Vec<String> = (0..43).map(|index| format!("App {index:02}")).collect();
    let apps: Vec<(&str, &str, &str)> = names
        .iter()
        .map(|name| (name.as_str(), name.as_str(), ""))
        .collect();
    let (_fixture, host, view, mut visual, _window) =
        crate::view::ui_tests::panel_with_apps(cx, Panel::Apps, &apps);
    let state = cx.update(|cx| crate::app::testing::snapshot(&host, cx));
    let order: Vec<String> = state
        .catalog
        .search("")
        .iter()
        .map(|app| app.id.clone())
        .collect();
    let (_, pages, per_page) = visual.update(|window, _| {
        super::launchpad_page_count(window, order.len(), 0)
    });
    let metrics = visual.update(|window, _| super::grid_metrics(window));
    let last_dot = visual
        .debug_bounds(leaked(format!("launchpad-page-{}", pages - 1)))
        .expect("the last page dot is painted");
    visual.simulate_click(last_dot.center(), Modifiers::default());
    visual.run_until_parked();
    settle_slide(&mut visual, &view);

    let start = (pages - 1) * per_page;
    let remaining = order.len() - start;
    let page_rows = remaining.div_ceil(metrics.columns);
    let last_row_start = start + (page_rows - 1) * metrics.columns;
    let first = visual
        .debug_bounds(leaked(format!("launchpad-app-{}", order[start])))
        .expect("the first tile of the last page is painted");
    let second_column = visual
        .debug_bounds(leaked(format!("launchpad-app-{}", order[start + 1])))
        .expect("the second column of the last page is painted");
    let last_row_first = visual
        .debug_bounds(leaked(format!("launchpad-app-{}", order[last_row_start])))
        .expect("the last row of the last page is painted");
    let last = visual
        .debug_bounds(leaked(format!("launchpad-app-{}", order[order.len() - 1])))
        .expect("the final tile is painted");

    assert!(
        (f32::from(first.origin.x) - metrics.side).abs() <= 1.,
        "a partial page still starts at the left column: first {:?}, side {}",
        first,
        metrics.side
    );
    assert!(
        (f32::from(first.origin.y) - metrics.grid_top).abs() <= 1.,
        "a partial page starts at the top of the middle band: first {:?}",
        first
    );
    let column_pitch = f32::from(second_column.origin.x) - f32::from(first.origin.x);
    assert!(
        (column_pitch - metrics.cell_width).abs() <= 1.,
        "missing columns do not stretch the pitch: {column_pitch}, expected {}",
        metrics.cell_width
    );
    if page_rows > 1 {
        let row_gap = f32::from(last_row_first.origin.y) - f32::from(first.bottom());
        assert!(
            (row_gap - metrics.row_gap).abs() <= 1.,
            "missing rows do not stretch the gap: {row_gap}, expected {}",
            metrics.row_gap
        );
    }
    let page_height = page_rows as f32 * LAUNCHPAD_ROW_H
        + (page_rows.saturating_sub(1)) as f32 * metrics.row_gap;
    assert!(
        (f32::from(last.bottom()) - (metrics.grid_top + page_height)).abs() <= 1.,
        "a partial page keeps its natural height: last bottom {:?}, expected {}",
        last.bottom(),
        metrics.grid_top + page_height
    );
}

#[gpui::test]
fn a_short_last_page_keeps_the_dots_at_their_fixed_place(cx: &mut TestAppContext) {
    let names: Vec<String> = (0..41).map(|index| format!("App {index:02}")).collect();
    let apps: Vec<(&str, &str, &str)> = names
        .iter()
        .map(|name| (name.as_str(), name.as_str(), ""))
        .collect();
    let (_fixture, host, view, mut visual, _window) =
        crate::view::ui_tests::panel_with_apps(cx, Panel::Apps, &apps);
    let state = cx.update(|cx| crate::app::testing::snapshot(&host, cx));
    let order: Vec<String> = state
        .catalog
        .search("")
        .iter()
        .map(|app| app.id.clone())
        .collect();
    let (_, pages, per_page) =
        visual.update(|window, _| super::launchpad_page_count(window, order.len(), 0));
    let metrics = visual.update(|window, _| super::grid_metrics(window));
    let last_dot = visual
        .debug_bounds(leaked(format!("launchpad-page-{}", pages - 1)))
        .expect("the last page dot is painted");
    visual.simulate_click(last_dot.center(), Modifiers::default());
    visual.run_until_parked();
    settle_slide(&mut visual, &view);

    let start = (pages - 1) * per_page;
    let remaining = order.len() - start;
    let page_rows = remaining.div_ceil(metrics.columns);
    assert!(
        page_rows < metrics.rows,
        "the last page must show fewer rows than a full page: {page_rows} vs {}",
        metrics.rows
    );
    let last = visual
        .debug_bounds(leaked(format!("launchpad-app-{}", order[order.len() - 1])))
        .expect("the final tile is painted");
    let dots = visual
        .debug_bounds(leaked(format!("launchpad-page-{}", pages - 1)))
        .expect("the active page dot is painted");

    assert!(
        (f32::from(last.bottom()) - (metrics.grid_top + LAUNCHPAD_ROW_H)).abs() <= 1.,
        "the single row keeps its height at the top of the band: last bottom {:?}",
        last.bottom()
    );
    assert!(
        (f32::from(dots.top()) - metrics.dots_top).abs() <= 1.,
        "the dots keep their fixed place above the Dock band on a short page: dots {:?}, fixed {}",
        dots.top(),
        metrics.dots_top
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
    assert_eq!(wide.columns, 11, "fixed columns fill the roomy output");
    assert_eq!(wide.rows, 3, "the middle band fits three rows");
    assert_eq!(wide.per_page(), 33);
    assert_eq!(wide.side, LAUNCHPAD_ARROW_BAND);
    assert!(
        (wide.side + wide.width + wide.side - 1920.).abs() < 0.01,
        "a full page fills the width between equal paging bands: side {}, width {}",
        wide.side,
        wide.width
    );
    assert!(
        (wide.cell_width - wide.width / wide.columns as f32).abs() < 0.01,
        "the columns share the page width evenly instead of keeping a left-packed pitch"
    );
    assert!((wide.grid_top - 1080. * LAUNCHPAD_SEARCH_SHARE).abs() < 0.01);
    assert!((wide.dock_top - 1080. * (1. - LAUNCHPAD_DOCK_SHARE)).abs() < 0.01);
    assert_eq!(
        wide.grid_height,
        wide.rows as f32 * LAUNCHPAD_ROW_H + (wide.rows - 1) as f32 * wide.row_gap
    );
    assert!(
        wide.row_gap > LAUNCHPAD_ROW_GAP,
        "a full page spends the spare middle height on even row gaps: {}",
        wide.row_gap
    );
    assert!(
        (wide.dots_top - (wide.grid_top + wide.grid_height) - LAUNCHPAD_DOTS_GAP).abs() < 0.01,
        "the dots keep the widened gap under the grid"
    );
    assert!(
        wide.dots_top + LAUNCHPAD_DOTS_H + LAUNCHPAD_DOTS_BOTTOM <= wide.dock_top + 0.01,
        "the dots stay above the reserved dock band"
    );

    // A wide output centres the same full page instead of pinning it left.
    let ultra = launchpad_metrics(2560., 1440.);
    assert_eq!(ultra.columns, LAUNCHPAD_MAX_COLUMNS);
    assert!(
        (ultra.side + ultra.width + ultra.side - 2560.).abs() < 0.01,
        "an ultrawide output keeps equal side bands: side {}, width {}",
        ultra.side,
        ultra.width
    );
    assert!(
        (ultra.cell_width - ultra.width / ultra.columns as f32).abs() < 0.01,
        "the ultrawide page spreads its columns instead of leaving the right side empty"
    );

    // A small output keeps the minimum shape instead of dropping to one tile.
    let small = launchpad_metrics(640., 480.);
    assert_eq!(small.columns, LAUNCHPAD_MIN_COLUMNS);
    assert!(small.rows >= LAUNCHPAD_MIN_ROWS);
    assert!(
        small.per_page() < wide.per_page(),
        "a small screen pages more often: {} vs {}",
        small.per_page(),
        wide.per_page()
    );
    assert!(small.side < wide.side);
    assert!(small.width <= 640.);
    assert!(
        (small.side + small.width + small.side - 640.).abs() < 0.01,
        "even a small output keeps the two side bands equal"
    );

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

/// A page step rides a real-time spring, and the test clock does not advance
/// it. Geometry tests jump the ride to its end so they read the settled page,
/// exactly what a user sees once the slide lands.
fn settle_slide(visual: &mut VisualTestContext, view: &gpui::Entity<super::ShellView>) {
    visual.update(|_, cx| {
        view.update(cx, |view, cx| {
            if let Some(slide) = view.launchpad_slide.as_mut() {
                slide.started = std::time::Instant::now() - std::time::Duration::from_secs(1);
            }
            cx.notify();
        });
    });
    visual.run_until_parked();
}

#[gpui::test]
fn a_page_step_slides_the_pages_past_each_other(cx: &mut TestAppContext) {
    let names: Vec<String> = (0..48).map(|index| format!("App {index:02}")).collect();
    let apps: Vec<(&str, &str, &str)> = names
        .iter()
        .map(|name| (name.as_str(), name.as_str(), ""))
        .collect();
    let (_fixture, host, view, mut visual, _window) =
        crate::view::ui_tests::panel_with_apps(cx, Panel::Apps, &apps);
    let state = cx.update(|cx| crate::app::testing::snapshot(&host, cx));
    let order: Vec<String> = state
        .catalog
        .search("")
        .iter()
        .map(|app| app.id.clone())
        .collect();
    let count = order.len();
    let (_, pages, per_page) =
        visual.update(|window, _| super::launchpad_page_count(window, count, 0));
    assert!(pages > 1, "the fixture needs more than one page");

    // A page step remembers both pages, so the renderer can carry one out
    // while the other follows it in.
    visual.update(|_, cx| {
        view.update(cx, |view, cx| {
            // The selection has already landed on the new page; the ride is
            // what the user still sees.
            view.selection = per_page;
            view.launchpad_start_slide(0, 1);
            assert_eq!(
                view.launchpad_slide
                    .as_ref()
                    .map(|slide| (slide.from, slide.to)),
                Some((0, 1))
            );
            cx.notify();
        })
    });
    visual.run_until_parked();
    let first_page = leaked(format!("launchpad-app-{}", order[0]));
    let second_page = leaked(format!("launchpad-app-{}", order[per_page]));
    assert!(
        visual.debug_bounds(first_page).is_some() && visual.debug_bounds(second_page).is_some(),
        "both pages are painted while they trade places"
    );

    // Once the spring settles the outgoing page is gone and the ride releases.
    settle_slide(&mut visual, &view);
    assert!(
        view.read_with(&visual, |view, _| view.launchpad_slide.is_none()),
        "the slide releases once the pages have traded places"
    );
    assert!(
        visual.debug_bounds(first_page).is_none() && visual.debug_bounds(second_page).is_some(),
        "only the new page remains once the slide lands"
    );

    // Reduced motion changes pages without any ride at all.
    visual.update(|_, cx| {
        view.update(cx, |view, cx| {
            let mut state = (*view.state).clone();
            state.prefs.reduce_motion = true;
            view.state = std::sync::Arc::new(state);
            view.launchpad_start_slide(1, 0);
            assert!(view.launchpad_slide.is_none());
            cx.notify();
        })
    });
}

#[gpui::test]
fn the_launchpad_pages_a_wide_grid_with_arrows_and_dots(cx: &mut TestAppContext) {
    let names: Vec<String> = (0..48).map(|index| format!("App {index:02}")).collect();
    let apps: Vec<(&str, &str, &str)> = names
        .iter()
        .map(|name| (name.as_str(), name.as_str(), ""))
        .collect();
    let (_fixture, host, view, mut visual, _window) =
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
    settle_slide(&mut visual, &view);
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
    settle_slide(&mut visual, &view);
    let last_page_first = leaked(format!(
        "launchpad-app-{}",
        order[((pages - 1) * per_page).min(order.len() - 1)]
    ));
    assert!(
        visual.debug_bounds(last_page_first).is_some(),
        "the last dot shows the last page"
    );
}
