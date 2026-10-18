// SPDX-License-Identifier: GPL-3.0-or-later
//! The right-edge notification column has separate cards and transparent gaps.
use super::*;

pub(super) const DATE_HEIGHT: f32 = 56.;
pub(super) const CARD_HEIGHT: f32 = 132.;
pub(super) const CARD_GAP: f32 = 12.;

impl ShellView {
    pub(super) fn macos_notifications(
        &self,
        host: &ViewState,
        p: Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut cards = div().flex().flex_col().gap(px(CARD_GAP));
        if host.notifications.is_empty() {
            cards = cards.child(
                div()
                    .h(px(72.))
                    .w_full()
                    .rounded(px(14.))
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
                    )),
            );
        }
        for item in host.notifications.iter().rev() {
            let mut actions = div().flex().gap(px(6.));
            for (key, label) in item.actions.iter().take(2) {
                actions = actions.child(self.button(
                    format!("notification-{}-{key}", item.id),
                    label.clone(),
                    Action::Service(services::Command::NotificationAction {
                        id: item.id,
                        action: key.clone(),
                    }),
                    p,
                ));
            }
            cards = cards.child(
                div()
                    .h(px(CARD_HEIGHT))
                    .flex_shrink_0()
                    .w_full()
                    .rounded(px(14.))
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
                            .items_center()
                            .justify_between()
                            .h(px(22.))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(p.muted)
                                    .overflow_hidden()
                                    .child(menu_text(
                                        item.app.clone(),
                                        FontWeight::MEDIUM,
                                        p.muted,
                                        cx,
                                    )),
                            )
                            .child(self.glyph_button(
                                &format!("dismiss-{}", item.id),
                                "x",
                                Action::Service(services::Command::DismissNotification(item.id)),
                                p,
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(menu_text(
                                item.title.clone(),
                                FontWeight::SEMIBOLD,
                                p.fg,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .child(menu_text(
                                item.body.chars().take(2000).collect(),
                                FontWeight::NORMAL,
                                p.fg,
                                cx,
                            )),
                    )
                    .child(actions),
            );
        }
        div()
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
            .into_any_element()
    }
}

pub(super) fn card_regions(
    width: f32,
    height: f32,
    count: usize,
    scroll_y: f32,
) -> Vec<Bounds<Pixels>> {
    let mut regions = vec![];
    // A narrow date hit target also makes the whole column dismiss on Escape.
    regions.push(Bounds::new(
        point(px(12.), px(8.)),
        size(px((width - 24.).max(0.)), px(40.)),
    ));
    let cards = count.max(1);
    let card_height = if count == 0 { 72. } else { CARD_HEIGHT };
    let step = card_height + CARD_GAP;
    let content_height = cards as f32 * step - CARD_GAP;
    let viewport_height = (height - DATE_HEIGHT).max(0.);
    // Dismissing notifications may shrink the content before GPUI's next
    // prepaint clamps its scroll handle. Use the same physical scroll range.
    let scroll_y = if scroll_y.is_finite() {
        scroll_y.clamp(-(content_height - viewport_height).max(0.), 0.)
    } else {
        0.
    };
    let first = ((-scroll_y / step).floor() as usize).min(cards);
    let last = (((height - DATE_HEIGHT - scroll_y) / step).ceil().max(0.) as usize).min(cards);
    for index in first..last {
        let y = DATE_HEIGHT + index as f32 * step + scroll_y;
        // Round the actual card before clipping. A card whose rounded top has
        // scrolled offscreen has a square cross-section at the viewport edge.
        let mut card = rounded_region(width, card_height, 14.);
        for region in &mut card {
            region.origin.y += px(y);
        }
        card.sort_by(|a, b| f32::from(a.origin.y).total_cmp(&f32::from(b.origin.y)));
        for mut region in card {
            let top = region.top().max(px(DATE_HEIGHT));
            let bottom = region.bottom().min(px(height));
            if bottom <= top {
                continue;
            }
            region.origin.y = top;
            region.size.height = bottom - top;
            // Equal-inset scanlines can be represented exactly by one taller
            // rectangle. Keep transparent gaps between distinct cards intact.
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
    use super::*;

    fn contains(regions: &[Bounds<Pixels>], x: f32, y: f32) -> bool {
        regions.iter().any(|region| {
            x >= f32::from(region.left())
                && x < f32::from(region.right())
                && y >= f32::from(region.top())
                && y < f32::from(region.bottom())
        })
    }

    #[::core::prelude::v1::test]
    fn bounded_notification_viewports_keep_every_visible_card_interactive() {
        // Placement caps the notification surface at 1800 logical pixels even
        // on 2160/4320-high outputs, keeping the exact shape below 256 regions.
        for height in [1080., 1800.] {
            let regions = card_regions(368., height, 40, 0.);
            assert!(regions.len() <= 256);
            for index in 0..40 {
                let center =
                    DATE_HEIGHT + index as f32 * (CARD_HEIGHT + CARD_GAP) + CARD_HEIGHT / 2.;
                if center < height {
                    assert!(contains(&regions, 1., center), "missing card {index}");
                }
            }
        }
    }

    #[::core::prelude::v1::test]
    fn scrolling_clips_the_original_rounding_instead_of_rounding_the_cut() {
        let regions = card_regions(368., 400., 10, -50.);
        assert!(contains(&regions, 0.5, DATE_HEIGHT + 0.5));
        assert!(!contains(
            &regions,
            184.,
            DATE_HEIGHT - 50. + CARD_HEIGHT + 6.
        ));
    }

    #[::core::prelude::v1::test]
    fn card_corners_and_gaps_remain_transparent() {
        let regions = card_regions(368., 600., 3, 0.);
        assert!(!contains(&regions, 0.5, DATE_HEIGHT + 0.5));
        assert!(contains(&regions, 184., DATE_HEIGHT + 0.5));
        assert!(!contains(&regions, 184., DATE_HEIGHT + CARD_HEIGHT + 6.));
    }

    #[::core::prelude::v1::test]
    fn dismissing_cards_clamps_a_stale_scroll_before_the_next_prepaint() {
        let regions = card_regions(368., 300., 1, -900.);
        assert!(contains(&regions, 184., DATE_HEIGHT + 30.));
        assert!(!contains(&regions, 184., DATE_HEIGHT + CARD_HEIGHT + 6.));
    }
}
