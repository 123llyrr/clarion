// SPDX-License-Identifier: GPL-3.0-or-later
//! Notification text wraps naturally; hit regions follow the painted cards.
use std::cell::RefCell;
use std::rc::Rc;

use super::*;

pub(super) const DATE_HEIGHT: f32 = 56.;
const MIN_CARD_HEIGHT: f32 = 132.;
pub(super) const CARD_GAP: f32 = 12.;
const CARD_RADIUS: f32 = 14.;
const MAX_TEXT_HEIGHT: f32 = 280.;
type CardBounds = Rc<RefCell<Vec<Bounds<Pixels>>>>;

fn record_bounds(bounds: CardBounds) -> impl IntoElement {
    canvas(
        move |rect, _, _| bounds.borrow_mut().push(rect),
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

impl ShellView {
    fn notification_card(
        &self,
        item: &services::Notification,
        p: Palette,
        text_height: f32,
        toast: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = item.id;
        let host = self.host.clone();
        let mut content = div()
            .flex()
            .flex_col()
            .gap(px(5.))
            .min_w_0()
            .w_full()
            .child(
                div()
                    .id(("notification-title", id as usize))
                    .debug_selector(move || format!("notification-title-{id}"))
                    .flex_shrink_0()
                    .w_full()
                    .whitespace_normal()
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .child(menu_text(
                        item.title.clone(),
                        FontWeight::SEMIBOLD,
                        p.fg,
                        cx,
                    )),
            );
        if !item.body.is_empty() {
            content = content.child(
                div()
                    .id(("notification-body", id as usize))
                    .debug_selector(move || format!("notification-body-{id}"))
                    .flex_shrink_0()
                    .w_full()
                    .whitespace_normal()
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .child(menu_text(item.body.clone(), FontWeight::NORMAL, p.fg, cx)),
            );
        }
        // Keep every action reachable, including long translated labels. The
        // entire text/action area scrolls, so even a very long title cannot push
        // the body or action buttons out of an unscrollable fixed-height card.
        for (index, (key, label)) in item.actions.iter().enumerate() {
            let host = self.host.clone();
            let key = key.clone();
            content = content.child(
                div()
                    .id(SharedString::from(format!("notification-{id}-{key}")))
                    .debug_selector(move || format!("notification-action-{id}-{index}"))
                    .w_full()
                    .flex_shrink_0()
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .bg(p.card)
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .whitespace_normal()
                    .cursor_pointer()
                    .hover(move |s| s.bg(p.line))
                    .child(menu_text(label.clone(), FontWeight::MEDIUM, p.fg, cx))
                    .on_click(move |_, _, cx| {
                        host.update(cx, |host, cx| {
                            host.handle(
                                Action::Service(services::Command::NotificationAction {
                                    id,
                                    action: key.clone(),
                                }),
                                cx,
                            )
                        })
                    }),
            );
        }
        div()
            .id(("notification-card", id as usize))
            .debug_selector(move || format!("notification-card-{id}"))
            .min_h(px(MIN_CARD_HEIGHT))
            .flex_shrink_0()
            .w_full()
            .rounded(px(CARD_RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.line)
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .flex_shrink_0()
                    .min_h(px(30.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .whitespace_normal()
                            .child(menu_text(item.app.clone(), FontWeight::MEDIUM, p.muted, cx)),
                    )
                    .child(self.glyph_button(
                        &format!("dismiss-{id}"),
                        "x",
                        Action::Service(services::Command::DismissNotification(id)),
                        p,
                    )),
            )
            .child(
                div()
                    .id(("notification-text-scroll", id as usize))
                    .debug_selector(move || format!("notification-text-scroll-{id}"))
                    .w_full()
                    .min_h_0()
                    .max_h(px(text_height))
                    .overflow_y_scroll()
                    .child(content),
            )
            .when(toast, |card| {
                card.child(
                    div()
                        .id(("notification-open", id as usize))
                        .flex_shrink_0()
                        .pt(px(4.))
                        .text_size(px(11.))
                        .text_color(p.accent)
                        .cursor_pointer()
                        .child(menu_text(
                            "在通知中心查看".into(),
                            FontWeight::MEDIUM,
                            p.accent,
                            cx,
                        ))
                        .on_click(move |_, _, cx| {
                            host.update(cx, |host, cx| {
                                host.handle(Action::Panel(Panel::Notifications), cx)
                            })
                        }),
                )
            })
            .into_any_element()
    }

    pub(super) fn macos_notifications(
        &self,
        host: &ViewState,
        p: Palette,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let bounds: CardBounds = Default::default();
        #[cfg(test)]
        let recorded_regions = self.notification_regions.clone();
        let mut cards = div().flex().flex_col().gap(px(CARD_GAP));
        if host.notifications.is_empty() {
            cards = cards.child(
                div()
                    .relative()
                    .h(px(72.))
                    .flex_shrink_0()
                    .w_full()
                    .rounded(px(CARD_RADIUS))
                    .bg(p.panel)
                    .border_1()
                    .border_color(p.line)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.))
                    .child(menu_text(
                        "无新通知".into(),
                        FontWeight::NORMAL,
                        p.muted,
                        cx,
                    ))
                    .child(record_bounds(bounds.clone())),
            );
        }
        let text_height = (f32::from(window.viewport_size().height) - DATE_HEIGHT - 96.)
            .clamp(60., MAX_TEXT_HEIGHT);
        for item in host.notifications.iter().rev() {
            cards = cards.child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .child(self.notification_card(item, p, text_height, false, cx))
                    .child(record_bounds(bounds.clone())),
            );
        }
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .text_color(p.fg)
            .child(
                div()
                    .h(px(DATE_HEIGHT))
                    .flex_shrink_0()
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .text_size(px(26.))
                    .line_height(px(34.))
                    .child(menu_text(
                        host.system.date.clone(),
                        FontWeight::LIGHT,
                        p.fg,
                        cx,
                    )),
            )
            .child(
                div()
                    .id("notifications-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(cards),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |viewport, _, window, _| {
                        // Read actual prepaint bounds after GPUI has wrapped text
                        // and clamped scrolling, rather than predicting row sizes
                        // from a stale render-time ScrollHandle offset.
                        let regions = card_regions(viewport, &bounds.borrow());
                        #[cfg(test)]
                        recorded_regions.replace(regions.clone());
                        if p.frosted() {
                            window.set_glass_region(Some(&regions));
                        } else {
                            window.set_glass_region(Some(&[]));
                        }
                        window.set_input_region(Some(&regions));
                    },
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }

    pub(super) fn macos_toast(
        &self,
        item: &services::Notification,
        host: &ViewState,
        p: Palette,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let output_height = window
            .display(cx)
            .map(|display| f32::from(display.bounds().size.height))
            .unwrap_or(800.);
        let height_limit =
            (output_height - host.prefs.bar_height as f32 - 40.).clamp(MIN_CARD_HEIGHT, 400.);
        div()
            .relative()
            .id("notification-toast-scroll")
            .w_full()
            .max_h(px(height_limit))
            .overflow_y_scroll()
            .child(self.notification_card(item, p, (height_limit - 104.).max(40.), true, cx))
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |_, bounds, window, _| {
                        let size = size(bounds.size.width, bounds.size.height.ceil());
                        if window.viewport_size() != size {
                            window.resize(size);
                        }
                        let regions = rounded_region(
                            f32::from(size.width),
                            f32::from(size.height),
                            CARD_RADIUS,
                        );
                        if p.frosted() {
                            window.set_glass_region(Some(&regions));
                        } else {
                            window.set_glass_region(Some(&[]));
                        }
                        window.set_input_region(Some(&regions));
                    },
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }
}

fn card_regions(viewport: Bounds<Pixels>, cards: &[Bounds<Pixels>]) -> Vec<Bounds<Pixels>> {
    let width = f32::from(viewport.size.width);
    let mut regions = vec![Bounds::new(
        viewport.origin + point(px(12.), px(8.)),
        size(px((width - 24.).max(0.)), px(40.)),
    )];
    let clip_top = viewport.top() + px(DATE_HEIGHT);
    for bounds in cards {
        if bounds.bottom() <= clip_top || bounds.top() >= viewport.bottom() {
            continue;
        }
        let mut card = rounded_region(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            CARD_RADIUS,
        );
        for region in &mut card {
            region.origin += bounds.origin;
        }
        card.sort_by(|a, b| f32::from(a.top()).total_cmp(&f32::from(b.top())));
        for mut region in card {
            let top = region.top().max(clip_top);
            let bottom = region.bottom().min(viewport.bottom());
            if bottom <= top {
                continue;
            }
            region.origin.y = top;
            region.size.height = bottom - top;
            if let Some(previous) = regions.last_mut() {
                if previous.origin.x == region.origin.x
                    && previous.size.width == region.size.width
                    && f32::from(previous.bottom() - region.top()).abs() < 0.001
                {
                    previous.size.height += region.size.height;
                    continue;
                }
            }
            regions.push(region);
        }
    }
    regions
}

#[cfg(test)]
mod tests {
    use ::core::prelude::v1::test;

    use super::*;

    fn contains(regions: &[Bounds<Pixels>], x: f32, y: f32) -> bool {
        regions
            .iter()
            .any(|region| region.contains(&point(px(x), px(y))))
    }
    fn viewport(height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(0.)), size(px(368.), px(height)))
    }
    fn cards(heights: &[f32], scroll_y: f32) -> Vec<Bounds<Pixels>> {
        let mut y = DATE_HEIGHT + scroll_y;
        heights
            .iter()
            .map(|height| {
                let bounds = Bounds::new(point(px(0.), px(y)), size(px(368.), px(*height)));
                y += height + CARD_GAP;
                bounds
            })
            .collect()
    }

    #[test]
    fn variable_height_card_hits_follow_layout_and_keep_gaps_transparent() {
        let cards = cards(&[190., 340., 152.], 0.);
        let regions = card_regions(viewport(900.), &cards);
        for card in &cards {
            assert!(contains(&regions, 1., f32::from(card.center().y)));
            assert!(!contains(&regions, 184., f32::from(card.bottom()) + 6.));
            assert!(!contains(&regions, 0.5, f32::from(card.top()) + 0.5));
        }
    }

    #[test]
    fn scrolled_cards_clip_the_original_rounding_at_the_viewport() {
        let regions = card_regions(viewport(400.), &cards(&[210., 340., 132.], -50.));
        assert!(contains(&regions, 0.5, DATE_HEIGHT + 0.5));
        assert!(!contains(&regions, 184., DATE_HEIGHT - 50. + 210. + 6.));
        assert!(!contains(&regions, 184., 401.));
    }

    #[test]
    fn every_scroll_position_stays_within_the_wayland_region_limit() {
        for height in [300., 1080., 1800.] {
            for scroll in 0..144 {
                let cards = cards(&[MIN_CARD_HEIGHT; 40], -(scroll as f32));
                let regions = card_regions(viewport(height), &cards);
                assert!(regions.len() <= 256, "{height} {scroll}: {}", regions.len());
                for card in &cards {
                    let center = f32::from(card.center().y);
                    if center > DATE_HEIGHT && center < height {
                        assert!(contains(&regions, 1., center));
                    }
                }
            }
        }
    }

    #[gpui::test]
    fn complete_multiline_text_and_all_actions_are_reachable_by_scrolling(cx: &mut TestAppContext) {
        use std::collections::HashMap;

        use crate::app::testing;
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().to_string_lossy().into_owned();
        let environment: HashMap<String, String> = [
            ("HOME".into(), private.clone()),
            ("XDG_CONFIG_HOME".into(), private.clone()),
            ("XDG_CONFIG_DIRS".into(), private.clone()),
            ("XDG_DATA_HOME".into(), private.clone()),
            ("XDG_DATA_DIRS".into(), private),
        ]
        .into();
        let catalog = clarion_launcher::catalog::AppCatalog::load(&environment).unwrap();
        let host = cx.update(|cx| testing::test_host(cx, catalog, environment));
        host.update(cx, |host, _| {
            host.notifications = vec![services::Notification {
                id: 1,
                app: "中文通知测试".into(),
                title: format!(
                    "{}\n第二行标题\n第三行标题\n第四行标题",
                    "没有空格的中文长标题".repeat(20)
                ),
                body: format!(
                    "{}\n{}",
                    "UnbrokenLongNotificationLink".repeat(80),
                    "中文正文与 English text\n".repeat(600)
                ),
                actions: (0..3)
                    .map(|index| (format!("action-{index}"), format!("通知操作 {index}")))
                    .collect(),
                created_at: 0,
                unread: true,
            }];
        });
        let window = cx.update(|cx| testing::open_panel(&host, Panel::Notifications, cx));
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.update(|window, _| {
            window.resize(size(px(368.), px(600.)));
            window.activate_window();
            window.refresh();
        });
        visual.run_until_parked();
        let title = visual
            .debug_bounds("notification-title-1")
            .expect("full title");
        let body = visual
            .debug_bounds("notification-body-1")
            .expect("full body");
        let scroll = visual
            .debug_bounds("notification-text-scroll-1")
            .expect("scrollable content");
        assert!(
            title.size.height > px(4. * 18.),
            "all explicit title lines are retained: {title:?}"
        );
        assert!(
            body.size.height > px(603. * 17.),
            "body must retain text beyond 2000 characters: {body:?}"
        );
        assert!(scroll.size.height <= px(MAX_TEXT_HEIGHT));
        let card = visual
            .debug_bounds("notification-card-1")
            .expect("auto-height card");
        let regions = view.read_with(&visual, |view, _| {
            view.notification_regions.borrow().clone()
        });
        assert!(card.size.height > px(MIN_CARD_HEIGHT));
        assert!(
            contains(&regions, 1., f32::from(card.center().y)),
            "actual card mask follows its measured height"
        );
        assert!(
            !contains(&regions, 184., f32::from(card.bottom()) + 6.),
            "transparent gap below the actual card"
        );
        assert!(
            !contains(&regions, 0.5, f32::from(card.top()) + 0.5),
            "transparent actual card corner"
        );
        visual.simulate_event(ScrollWheelEvent {
            position: scroll.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-100000.))),
            ..Default::default()
        });
        visual.run_until_parked();
        let final_action = visual
            .debug_bounds("notification-action-1-2")
            .expect("third action");
        assert!(
            final_action.center().y > scroll.top() && final_action.center().y < scroll.bottom(),
            "the final action is visible after scrolling: {final_action:?} in {scroll:?}"
        );
        let body_after = visual.debug_bounds("notification-body-1").unwrap();
        assert!(body_after.top() < body.top());

        // The banner uses the same complete content and resizes to the measured
        // card, with a bounded scroll viewport and an explicit persistent view.
        view.update(&mut visual, |view, cx| {
            view.kind = Surface::Toast(1);
            cx.notify();
        });
        visual.update(|window, _| {
            window.resize(size(px(360.), px(124.)));
            window.refresh();
        });
        visual.run_until_parked();
        let toast = visual
            .debug_bounds("notification-card-1")
            .expect("toast card");
        assert!(
            toast.size.height > px(124.),
            "the old fixed banner height must not clip content"
        );
        visual.update(|window, _| {
            assert!(window.bounds().size.height > px(124.));
            assert!(window.bounds().size.height <= px(400.));
        });
    }
}
