// SPDX-License-Identifier: GPL-3.0-or-later
//! The notification center: one frosted column with a calendar header and
//! compact cards below. Cards from one application fold into a stack.
use chrono::{Datelike, NaiveDate};

use super::*;

/// The smallest viewport the toast gives its scrolling card.
const MIN_TOAST_HEIGHT: f32 = 84.;
pub(super) const CARD_GAP: f32 = 6.;
const CARD_RADIUS: f32 = 20.;
const CONTAINER_RADIUS: f32 = 26.;
/// How far each folded sheet drops below the one above it, in points.
const STACK_STEP: f32 = 10.;
/// The deepest folded stack paints at most this many sheets behind the top
/// card, so a long run of notifications stays one compact pile.
const STACK_SHEETS: usize = 3;
const CARD_PADDING: f32 = 10.;
const CARD_BORDER: f32 = 2.;
const CARD_HEADER: f32 = 20.;
const APP_ICON: f32 = 36.;
/// The title folds at two lines and the body at three; these are the heights
/// of those full blocks and therefore the caps a card can reach.
const TITLE_HEIGHT: f32 = 34.;
const BODY_HEIGHT: f32 = 48.;
const ACTION_GAP: f32 = 8.;
const ACTION_HEIGHT: f32 = 24.;
const TOAST_LINK_HEIGHT: f32 = 14.;
/// A drag past this distance dismisses the card on release.
pub(super) const DISMISS_DRAG: f32 = 120.;

/// How a card is being painted. `stacked` rises to one while it is buried in
/// a folded pile, which thickens the tile so the card behind it cannot read
/// through; a toast or a lone card keeps the airy fill.
#[derive(Clone, Copy)]
struct CardSurface {
    toast: bool,
    stacked: f32,
}

impl CardSurface {
    const LONE: Self = Self {
        toast: false,
        stacked: 0.,
    };
    const TOAST: Self = Self {
        toast: true,
        stacked: 0.,
    };
    fn stacked_at(self, stacked: f32) -> Self {
        Self { stacked, ..self }
    }
}

impl ShellView {
    /// The artwork for a notification card's leading tile. The application's
    /// own icon wins; a catalog match by name covers notifications that send
    /// no icon at all, and the generic bell is the last resort.
    fn notification_icon_path(
        &self,
        host: &ViewState,
        item: &services::Notification,
    ) -> Option<PathBuf> {
        if let Some(name) = item.icon.as_deref().filter(|name| !name.trim().is_empty()) {
            let path = PathBuf::from(name);
            if path.is_absolute() {
                if path.is_file() {
                    return Some(path);
                }
            } else if let Some(path) = theme_icon(name, &mut self.theme_icons.borrow_mut()) {
                return Some(path);
            }
        }
        let app = item.app.trim();
        host.catalog
            .apps()
            .iter()
            .find(|candidate| {
                candidate.name.eq_ignore_ascii_case(app)
                    || candidate
                        .id
                        .trim_end_matches(".desktop")
                        .eq_ignore_ascii_case(app)
            })
            .and_then(|candidate| candidate.icon_path.clone())
    }

    fn notification_app_icon(
        &self,
        host: &ViewState,
        item: &services::Notification,
        p: Palette,
    ) -> AnyElement {
        let tile = div()
            .id(("notification-app", item.id))
            .debug_selector(move || format!("notification-app-{}", item.id))
            .size(px(APP_ICON))
            .flex_shrink_0()
            .rounded(px(10.))
            .bg(p.panel)
            .border_1()
            .border_color(p.line.opacity(0.55))
            .flex()
            .items_center()
            .justify_center();
        match self.notification_icon_path(host, item) {
            Some(path) => tile
                .child(img(path).size(px(26.)).object_fit(ObjectFit::Contain))
                .into_any_element(),
            None => tile.child(self.symbol("bell", 18., p)).into_any_element(),
        }
    }

    /// A compact pin toggle in the card header. Text reads faster than a
    /// theme-dependent pin glyph at this size and carries the state directly.
    fn notification_pin(
        &self,
        item: &services::Notification,
        p: Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let host = self.host.clone();
        let id = item.id;
        let pinned = item.pinned;
        let color = if pinned { p.accent } else { p.muted };
        div()
            .id(("notification-pin", id as usize))
            .debug_selector(move || format!("notification-pin-{id}"))
            .h(px(CARD_HEADER))
            .px(px(6.))
            .rounded(px(5.))
            .flex()
            .items_center()
            .text_size(px(10.))
            .line_height(px(13.))
            .cursor_pointer()
            .when(pinned, move |button| button.bg(p.accent.opacity(0.14)))
            .when(!pinned, move |button| button.hover(move |s| s.bg(p.line)))
            .child(menu_text(
                if pinned { "已固定" } else { "固定" }.into(),
                FontWeight::MEDIUM,
                color,
                cx,
            ))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                host.update(cx, |host, cx| {
                    host.handle(
                        Action::Service(services::Command::PinNotification {
                            id,
                            pinned: !pinned,
                        }),
                        cx,
                    )
                })
            })
            .into_any_element()
    }

    fn notification_close(&self, id: u32, p: Palette) -> AnyElement {
        let host = self.host.clone();
        div()
            .id(("notification-close", id as usize))
            .debug_selector(move || format!("notification-close-{id}"))
            .size(px(CARD_HEADER))
            .rounded(px(5.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(move |s| s.bg(p.line))
            .child(self.symbol(
                "x",
                11.,
                Palette {
                    fg: p.muted,
                    ..p
                },
            ))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                host.update(cx, |host, cx| {
                    host.handle(
                        Action::Service(services::Command::DismissNotification(id)),
                        cx,
                    )
                })
            })
            .into_any_element()
    }

    /// Fold the cards of one application into a stack. The top card is always
    /// visible; the rest ride under it and slide open when the stack is
    /// clicked. `stack` is the application key for stacks with more than one
    /// card, `None` for a lone card that has nothing to expand.
    fn notification_stack(
        &self,
        host: &ViewState,
        app: &str,
        items: &[&services::Notification],
        p: Palette,
        now: std::time::Duration,
        cx: &Context<Self>,
    ) -> (AnyElement, bool) {
        let expanded = self.notification_expanded.borrow().contains(app);
        let (progress, active) =
            self.notification_stack_progress(app, expanded, host.prefs.reduce_motion, now);
        if items.len() == 1 {
            // A lone card has nothing to fold, so let it size to its content
            // instead of reserving a fixed pile.
            return (
                self.notification_card(host, items[0], p, CardSurface::LONE, None, cx),
                active,
            );
        }

        // Cards size to their content. Use the measured height from the last
        // frame when it is known, and the clamped maximum otherwise, so the
        // first frame can never clip a card.
        let measured = self.notification_heights.borrow();
        let heights: Vec<f32> = items
            .iter()
            .map(|item| {
                measured
                    .get(&item.id)
                    .map(|height| height.ceil())
                    .unwrap_or_else(|| notification_card_height(item, false))
            })
            .collect();
        drop(measured);
        let first_height = heights[0];
        let sheets = if items.len() > 1 {
            items.len().saturating_sub(1).min(STACK_SHEETS)
        } else {
            0
        };
        let collapsed = first_height + sheets as f32 * STACK_STEP;
        let open_height = heights.iter().sum::<f32>()
            + CARD_GAP * items.len().saturating_sub(1) as f32;
        // The elastic sample may overshoot its endpoint; let the container
        // ride the bounce, but never clip the top card smaller than itself.
        let height =
            (collapsed + (open_height - collapsed) * progress).max(first_height);

        // Where each real card sits once the pile is open.
        let mut expanded = Vec::with_capacity(heights.len());
        let mut offset = 0.;
        for card_height in &heights {
            expanded.push(offset);
            offset += card_height + CARD_GAP;
        }

        // Layout is the only place that knows how tall wrapped text ended up.
        // Record each card's natural height and repaint once so the container
        // and the open slots use the real values instead of the caps.
        let measure_heights = std::rc::Rc::clone(&self.notification_heights);
        let ids: Vec<u32> = items.iter().map(|item| item.id).collect();
        let entity = cx.entity().entity_id();
        let mut stack = div()
            .on_children_prepainted(move |bounds, _, cx| {
                let mut measured = measure_heights.borrow_mut();
                let mut changed = false;
                // Children paint back to front: the last child is the top card.
                for (slot, id) in ids.iter().rev().enumerate() {
                    let Some(bound) = bounds.get(slot) else {
                        continue;
                    };
                    let height = f32::from(bound.size.height);
                    if height <= 0. {
                        continue;
                    }
                    if (measured.get(id).copied().unwrap_or(0.) - height).abs() > 0.25 {
                        measured.insert(*id, height);
                        changed = true;
                    }
                }
                drop(measured);
                if changed {
                    cx.notify(entity);
                }
            })
            .id(SharedString::from(format!("notification-stack-{app}")))
            .debug_selector({
                let app = app.to_owned();
                move || format!("notification-stack-{app}")
            })
            .relative()
            .w_full()
            .h(px(height))
            .flex_shrink_0()
            .overflow_hidden();
        // The real cards layer behind the top card while folded: each one
        // drops a step lower and narrows a touch, then slides and widens into
        // its open slot. Painting back to front keeps the newest card on top.
        for (index, item) in items.iter().enumerate().rev() {
            let card_height = heights[index];
            let depth = index.min(sheets);
            let folded_y = first_height - card_height + depth as f32 * STACK_STEP;
            let folded_inset = if index == 0 {
                0.
            } else {
                4. + depth as f32 * 4.
            };
            let y = folded_y + (expanded[index] - folded_y) * progress;
            let inset =
                folded_inset * (1. - progress.clamp(0., 1.));
            let opacity = if index <= sheets {
                1.
            } else {
                progress.clamp(0., 1.)
            };
            let toggles = (index == 0 && items.len() > 1).then(|| app.to_owned());
            // Folded cards overlap, so their fill densifies as the pile
            // closes; an open stack returns to the airy lone-card fill.
            let stacked = (1. - progress.clamp(0., 1.)).clamp(0., 1.);
            let card = self.notification_card(
                host,
                item,
                p,
                CardSurface::LONE.stacked_at(stacked),
                toggles,
                cx,
            );
            stack = stack.child(
                div()
                    .absolute()
                    .left(px(inset))
                    .right(px(inset))
                    .top(px(y))
                    .opacity(opacity)
                    .child(card),
            );
        }
        (stack.into_any_element(), active)
    }

    fn notification_card(
        &self,
        host: &ViewState,
        item: &services::Notification,
        p: Palette,
        surface: CardSurface,
        stack: Option<String>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = item.id;
        let drag = self
            .notification_drag
            .as_ref()
            .filter(|drag| drag.id == id);
        let (drag_dx, drag_dy) = drag.map(|drag| (drag.dx, drag.dy)).unwrap_or((0., 0.));
        // The column clips its contents, so the card only nudges while the
        // pointer travels; the fade carries the rest of the dismissal signal.
        let dx = drag_dx.clamp(-28., 28.);
        let dy = drag_dy.clamp(-28., 28.);
        let fade = 1. - (drag_dx.abs().max(drag_dy.abs()) / 220.).clamp(0., 1.) * 0.6;

        let header = div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(px(11.))
                    .line_height(px(14.))
                    .child(menu_text(
                        item.app.clone(),
                        FontWeight::MEDIUM,
                        p.muted,
                        cx,
                    )),
            )
            .child(self.notification_pin(item, p, cx))
            .child(self.notification_close(id, p));

        let mut text = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .min_w_0()
            .child(
                div()
                    .id(("notification-title", id as usize))
                    .debug_selector(move || format!("notification-title-{id}"))
                    .w_full()
                    .max_h(px(TITLE_HEIGHT))
                    .whitespace_normal()
                    .text_size(px(13.))
                    .line_height(px(17.))
                    .line_clamp(2)
                    .overflow_hidden()
                    .child(menu_text(
                        item.title.clone(),
                        FontWeight::SEMIBOLD,
                        p.fg,
                        cx,
                    )),
            );
        if !item.body.is_empty() {
            text = text.child(
                div()
                    .id(("notification-body", id as usize))
                    .debug_selector(move || format!("notification-body-{id}"))
                    .w_full()
                    .max_h(px(BODY_HEIGHT))
                    .whitespace_normal()
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .line_clamp(3)
                    .overflow_hidden()
                    .child(menu_text(item.body.clone(), FontWeight::NORMAL, p.fg, cx)),
            );
        }

        let mut card = div()
            .id(("notification-card", id as usize))
            .debug_selector(move || format!("notification-card-{id}"))
            .relative()
            .left(px(dx))
            .top(px(dy))
            .opacity(fade)
            .flex_shrink_0()
            .w_full()
            .rounded(px(CARD_RADIUS))
            .bg(notification_card_fill(p, surface.stacked))
            .border_1()
            .border_color(p.line.opacity(0.7))
            .p(px(CARD_PADDING))
            .flex()
            .flex_col()
            .gap(px(ACTION_GAP))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .child(self.notification_app_icon(host, item, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(header)
                            .child(text),
                    ),
            );

        if !item.actions.is_empty() {
            // One fixed action row: the height table counts exactly one row,
            // and each button ellipsizes rather than wrapping the card taller.
            let mut actions = div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(6.))
                .overflow_hidden();
            for (index, (key, label)) in item.actions.iter().enumerate() {
                let host = self.host.clone();
                let key = key.clone();
                let label = action_label(key.as_str(), label);
                actions = actions.child(
                    div()
                        .id(SharedString::from(format!("notification-{id}-{key}")))
                        .debug_selector(move || format!("notification-action-{id}-{index}"))
                        .h(px(ACTION_HEIGHT))
                        .px(px(9.))
                        .rounded(px(6.))
                        .bg(p.panel)
                        .border_1()
                        .border_color(p.line.opacity(0.5))
                        .flex()
                        .flex_shrink()
                        .min_w(px(0.))
                        .items_center()
                        .max_w(px(150.))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(11.))
                        .line_height(px(14.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(p.line))
                        .child(menu_text(label, FontWeight::MEDIUM, p.fg, cx))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
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
            card = card.child(actions);
        }

        if surface.toast {
            let host = self.host.clone();
            card = card.child(
                div()
                    .id(("notification-open", id as usize))
                    .flex_shrink_0()
                    .h(px(TOAST_LINK_HEIGHT))
                    .text_size(px(11.))
                    .line_height(px(TOAST_LINK_HEIGHT))
                    .text_color(p.accent)
                    .cursor_pointer()
                    .child(menu_text(
                        "在通知中心查看".into(),
                        FontWeight::MEDIUM,
                        p.accent,
                        cx,
                    ))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        host.update(cx, |host, cx| {
                            host.handle(Action::Panel(Panel::Notifications), cx)
                        })
                    }),
            );
        } else {
            card = card.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.notification_drag_dismissed.set(false);
                    this.notification_drag = Some(NotificationDrag {
                        id,
                        start: event.position,
                        dx: 0.,
                        dy: 0.,
                    });
                    cx.notify();
                }),
            );
        }

        if let Some(app) = stack {
            let reduce = host.prefs.reduce_motion;
            card = card.on_click(cx.listener(move |this, _, _, cx| {
                if this.notification_drag_dismissed.replace(false) {
                    return;
                }
                let expanded = {
                    let mut open = this.notification_expanded.borrow_mut();
                    if open.contains(&app) {
                        open.remove(&app);
                        false
                    } else {
                        open.insert(app.clone());
                        true
                    }
                };
                let target = if expanded { 1. } else { 0. };
                let now = this.popup_epoch.elapsed();
                let mut stacks = this.notification_stacks.borrow_mut();
                let motion = stacks
                    .entry(app.clone())
                    .or_insert_with(|| crate::dock_motion::Elastic::new(0.));
                if reduce {
                    motion.snap(target);
                } else {
                    motion.retarget(target, now);
                }
                cx.notify();
            }))
            .cursor_pointer();
        }
        card.into_any_element()
    }

    /// The calendar's displayed year and month. The first render follows the
    /// snapshot's date and selects it.
    fn calendar_display(&self, today: NaiveDate) -> (i32, u32) {
        if let Some(display) = self.calendar_month.get() {
            return display;
        }
        self.calendar_month.set(Some((today.year(), today.month())));
        self.calendar_day.set(Some(today));
        (today.year(), today.month())
    }

    fn calendar_shift_month(&self, delta: i32, today: NaiveDate, cx: &mut Context<Self>) {
        let (year, month) = self.calendar_display(today);
        self.calendar_month
            .set(Some(shifted_month(year, month, delta)));
        self.calendar_day.set(None);
        cx.notify();
    }

    fn calendar_shift_year(&self, delta: i32, today: NaiveDate, cx: &mut Context<Self>) {
        let (year, month) = self.calendar_display(today);
        self.calendar_month
            .set(Some(((year + delta).clamp(1, 9999), month)));
        self.calendar_day.set(None);
        cx.notify();
    }

    /// One step of the month/year navigator.
    fn calendar_arrow(
        &self,
        selector: &'static str,
        left: bool,
        step: CalendarStep,
        p: Palette,
        cx: &Context<Self>,
        today: NaiveDate,
    ) -> AnyElement {
        let asset = if left {
            "chevron-left.svg"
        } else {
            "chevron-right.svg"
        };
        let path = icon_path(asset);
        let glyph = if path.is_file() {
            svg()
                .path(path.to_string_lossy().into_owned())
                .size(px(12.))
                .text_color(p.muted)
                .into_any_element()
        } else {
            div()
                .text_size(px(14.))
                .line_height(px(18.))
                .text_color(p.muted)
                .child(if left { "‹" } else { "›" })
                .into_any_element()
        };
        div()
            .id(selector)
            .debug_selector(move || selector.to_owned())
            .size(px(22.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(move |s| s.bg(p.line))
            .child(glyph)
            .on_click(cx.listener(move |this, _, _, cx| match step {
                CalendarStep::Month(delta) => this.calendar_shift_month(delta, today, cx),
                CalendarStep::Year(delta) => this.calendar_shift_year(delta, today, cx),
            }))
            .into_any_element()
    }

    fn calendar_today_button(
        &self,
        today: NaiveDate,
        p: Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .id("notification-calendar-today-button")
            .debug_selector(|| "notification-calendar-today-button".into())
            .h(px(22.))
            .px(px(9.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .cursor_pointer()
            .hover(move |s| s.bg(p.line))
            .child(
                div()
                    .text_size(px(11.))
                    .line_height(px(14.))
                    .child(menu_text(
                        "今天".into(),
                        FontWeight::MEDIUM,
                        p.accent,
                        cx,
                    )),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.calendar_month
                    .set(Some((today.year(), today.month())));
                this.calendar_day.set(Some(today));
                cx.notify();
            }))
            .into_any_element()
    }

    /// The calendar at the top of the center: the localized date, a divider,
    /// a year/month/day selector, and the notification count below it.
    fn notification_calendar(
        &self,
        host: &ViewState,
        p: Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let today = host
            .system
            .today
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        let (year, month) = self.calendar_display(today);
        let selected = self.calendar_day.get();
        let calendar = calendar_month(year, month);
        let date = if host.system.date.trim().is_empty() {
            format!("{}月{}日", month, today.day())
        } else {
            host.system.date.clone()
        };

        let year_group = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(self.calendar_arrow(
                "notification-calendar-prev-year",
                true,
                CalendarStep::Year(-1),
                p,
                cx,
                today,
            ))
            .child(
                div()
                    .debug_selector(|| "notification-calendar-year".into())
                    .min_w(px(58.))
                    .flex()
                    .justify_center()
                    .text_size(px(12.))
                    .line_height(px(15.))
                    .child(menu_text(
                        format!("{year}年"),
                        FontWeight::SEMIBOLD,
                        p.fg,
                        cx,
                    )),
            )
            .child(self.calendar_arrow(
                "notification-calendar-next-year",
                false,
                CalendarStep::Year(1),
                p,
                cx,
                today,
            ));
        let month_group = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(self.calendar_arrow(
                "notification-calendar-prev-month",
                true,
                CalendarStep::Month(-1),
                p,
                cx,
                today,
            ))
            .child(
                div()
                    .debug_selector(|| "notification-calendar-month".into())
                    .min_w(px(38.))
                    .flex()
                    .justify_center()
                    .text_size(px(12.))
                    .line_height(px(15.))
                    .child(menu_text(
                        format!("{month}月"),
                        FontWeight::SEMIBOLD,
                        p.fg,
                        cx,
                    )),
            )
            .child(self.calendar_arrow(
                "notification-calendar-next-month",
                false,
                CalendarStep::Month(1),
                p,
                cx,
                today,
            ));
        let mut tools = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(24.))
            .child(year_group)
            .child(month_group);
        if (year, month) != (today.year(), today.month()) || selected != Some(today) {
            tools = tools.child(self.calendar_today_button(today, p, cx));
        }

        let mut grid = div().flex().flex_col().gap(px(2.)).child(
            div()
                .flex()
                .w_full()
                .h(px(18.))
                .children(
                    ["一", "二", "三", "四", "五", "六", "日"]
                        .into_iter()
                        .map(|label| {
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(11.))
                                .line_height(px(14.))
                                .child(menu_text(
                                    label.into(),
                                    FontWeight::MEDIUM,
                                    p.muted,
                                    cx,
                                ))
                        }),
                ),
        );
        for week in calendar_weeks(calendar) {
            let mut row = div().flex().w_full();
            for day in week {
                let wrapper = div()
                    .flex_1()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center();
                row = row.child(match day {
                    Some(day) => {
                        let date = NaiveDate::from_ymd_opt(year, month, day).unwrap_or(today);
                        let is_today = date == today;
                        let is_selected = selected == Some(date);
                        let circle = div()
                            .size(px(28.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(13.))
                            .line_height(px(16.));
                        let (circle, label) = if is_selected {
                            (
                                circle.bg(p.accent),
                                div()
                                    .debug_selector(|| "notification-calendar-selected".into())
                                    .child(menu_text(
                                        day.to_string(),
                                        FontWeight::MEDIUM,
                                        rgb(0xffffff).into(),
                                        cx,
                                    )),
                            )
                        } else if is_today {
                            (
                                circle.bg(p.accent.opacity(0.18)),
                                div().child(menu_text(
                                    day.to_string(),
                                    FontWeight::MEDIUM,
                                    p.accent,
                                    cx,
                                )),
                            )
                        } else {
                            (
                                circle.hover(move |s| s.bg(p.line)),
                                div().child(menu_text(
                                    day.to_string(),
                                    FontWeight::NORMAL,
                                    p.fg,
                                    cx,
                                )),
                            )
                        };
                        let circle = if is_today {
                            circle.debug_selector(|| "notification-calendar-today".into())
                        } else {
                            circle
                        };
                        wrapper
                            .id(SharedString::from(format!(
                                "notification-calendar-day-{day}"
                            )))
                            .cursor_pointer()
                            .debug_selector(move || format!("notification-calendar-day-{day}"))
                            .child(circle.child(label))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.calendar_day.set(Some(date));
                                cx.notify();
                            }))
                            .into_any_element()
                    }
                    None => wrapper.into_any_element(),
                });
            }
            grid = grid.child(row);
        }

        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .text_size(px(28.))
                    .line_height(px(36.))
                    .child(menu_text(date, FontWeight::LIGHT, p.fg, cx)),
            )
            .child(
                div()
                    .h(px(1.))
                    .bg(p.line.opacity(0.6)),
            )
            .child(tools)
            .child(grid)
            .child(
                div()
                    .h(px(1.))
                    .bg(p.line.opacity(0.6)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(15.))
                            .child(menu_text(
                                "通知".into(),
                                FontWeight::MEDIUM,
                                p.muted,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(15.))
                            .child(menu_text(
                                format!("{} 条", host.notifications.len()),
                                FontWeight::NORMAL,
                                p.muted,
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn macos_notifications(
        &mut self,
        host: &ViewState,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
        let placement = crate::app::panels::placement(
            host,
            &Panel::Notifications,
            f32::from(viewport.size.width),
            f32::from(viewport.size.height),
        );
        let popup = placement.popup.unwrap_or(viewport);
        let pw = f32::from(popup.size.width);
        let ph = f32::from(popup.size.height);

        // The whole column slides in from the output's right edge and retreats
        // the same way before the Host releases the surface.
        let now = self.popup_epoch.elapsed();
        let motion = self.popup_motion.get_or_insert_with(|| {
            let mut motion = crate::dock_motion::Motion::new(0.);
            if host.prefs.reduce_motion {
                motion.snap(1.);
            } else {
                motion.retarget(1., now);
            }
            motion
        });
        if host.panel_exit {
            if host.prefs.reduce_motion {
                motion.snap(0.);
            } else if motion.target() != 0. {
                motion.retarget(0., now);
            }
        }
        let sample = motion.sample(now);
        if sample.active {
            window.request_animation_frame();
        }
        let ride = sample.value.clamp(0., 1.);
        let slide = slide_offset(ride, pw);

        let mut animating = false;
        let mut cards = div().flex().flex_col().gap(px(CARD_GAP)).pb(px(2.));
        if host.notifications.is_empty() {
            cards = cards.child(
                div()
                    .w_full()
                    .h(px(140.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .child(self.symbol(
                        "bell",
                        24.,
                        Palette {
                            fg: p.muted,
                            ..p
                        },
                    ))
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(px(16.))
                            .child(menu_text(
                                "无新通知".into(),
                                FontWeight::NORMAL,
                                p.muted,
                                cx,
                            )),
                    ),
            );
        }
        // The service sorts pinned entries first and newest first within each
        // group; the stacks inherit that order so the most important card tops
        // the column.
        for (app, items) in notification_groups(&host.notifications) {
            let (stack, active) =
                self.notification_stack(host, &app, &items, p, now, cx);
            animating |= active;
            cards = cards.child(stack);
        }
        if animating {
            window.request_animation_frame();
        }

        let column = div()
            .id("notification-center")
            .debug_selector(|| "notification-center".into())
            .absolute()
            .left(px(f32::from(popup.origin.x) + slide))
            .top(popup.origin.y)
            .w(px(pw))
            .h(px(ph))
            .rounded(px(CONTAINER_RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.line)
            .overflow_hidden()
            .pt(px(14.))
            .px(px(14.))
            .pb(px(12.))
            .flex()
            .flex_col()
            // Keep clicks inside the column from also reaching the sibling
            // click-away catcher behind it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(self.notification_calendar(host, p, cx))
            .child(
                div()
                    .id("notifications-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .pt(px(6.))
                    .child(cards),
            );

        let close_host = self.host.clone();
        let frost = container_regions(
            point(popup.origin.x + px(slide), popup.origin.y),
            pw,
            ph,
        );
        #[cfg(test)]
        let recorded_regions = self.notification_regions.clone();
        div()
            .relative()
            .size_full()
            .text_color(p.fg)
            .child(
                div()
                    .id("notifications-backdrop")
                    .debug_selector(|| "notifications-backdrop".into())
                    .absolute()
                    .inset_0()
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        close_host.update(cx, |host, cx| host.handle(Action::Close, cx))
                    }),
            )
            .child(column)
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        #[cfg(test)]
                        recorded_regions.replace(frost.clone());
                        if p.frosted() {
                            window.set_glass_region(Some(&frost));
                        } else {
                            window.set_glass_region(Some(&[]));
                        }
                        // The whole output is the click-away catcher; only the
                        // column carries visible frost.
                        window.set_input_region(Some(&[viewport]));
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
            (output_height - host.prefs.bar_height as f32 - 40.).clamp(MIN_TOAST_HEIGHT, 400.);
        div()
            .relative()
            .id("notification-toast-scroll")
            .w_full()
            .max_h(px(height_limit))
            .overflow_y_scroll()
            .child(self.notification_card(host, item, p, CardSurface::TOAST, None, cx))
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |_, bounds, window, _| {
                        let size = size(bounds.size.width, bounds.size.height.ceil());
                        if window.viewport_size() != size {
                            window.resize(size);
                        }
                        let regions = card_frost_shape(
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

    /// The stack's expansion progress, sampled at the popup clock. Reduced
    /// motion snaps instead of animating.
    fn notification_stack_progress(
        &self,
        app: &str,
        expanded: bool,
        reduce: bool,
        now: std::time::Duration,
    ) -> (f32, bool) {
        let mut stacks = self.notification_stacks.borrow_mut();
        let motion = stacks
            .entry(app.to_owned())
            .or_insert_with(|| crate::dock_motion::Elastic::new(0.));
        let target = if expanded { 1. } else { 0. };
        if motion.target() != target {
            if reduce {
                motion.snap(target);
            } else {
                motion.retarget(target, now);
            }
        }
        // Keep the overshoot: the height interpolation turns it into the
        // springy settle at both ends of the fold.
        let sample = motion.sample(now);
        (sample.value, sample.active)
    }
}

/// The horizontal offset of the column at a given appear/disappear progress:
/// fully off the right edge at rest zero, flush with its slot at one.
fn slide_offset(progress: f32, width: f32) -> f32 {
    (1. - progress.clamp(0., 1.)) * (width + 24.)
}

/// The visible name for a notification action. D-Bus apps send the default
/// action with an empty label, which would otherwise paint an empty button;
/// real labels are trimmed and unnamed actions fall back to their key.
fn action_label(key: &str, label: &str) -> String {
    let label = label.trim();
    if !label.is_empty() {
        return label.to_owned();
    }
    match key.trim() {
        "" | "default" => "打开".into(),
        other => other.to_owned(),
    }
}

/// The tallest a card of this kind can become once its title and body reach
/// their line clamps. Cards normally shrink to their content, so this is only
/// the pre-measurement fallback for the folded stack's first frame.
fn notification_card_height(item: &services::Notification, toast: bool) -> f32 {
    let mut text = CARD_HEADER + 3. + TITLE_HEIGHT;
    if !item.body.is_empty() {
        text += 2. + BODY_HEIGHT;
    }
    let mut height = CARD_BORDER + 2. * CARD_PADDING + text.max(APP_ICON);
    if !item.actions.is_empty() {
        height += ACTION_GAP + ACTION_HEIGHT;
    }
    if toast {
        height += ACTION_GAP + TOAST_LINK_HEIGHT;
    }
    height
}

/// The card's own tile on the column's frosted glass. A card standing alone
/// or in an open list keeps the configured surface opacity so the frost reads
/// through it; `stacked` (0..1) lifts the fill towards an opaque tile as the
/// pile folds, otherwise the text of the card underneath shows through. The
/// column behind the cards supplies the blur, so the tile itself only needs
/// enough body to occlude.
fn notification_card_fill(p: Palette, stacked: f32) -> Hsla {
    let stacked = stacked.clamp(0., 1.);
    let base = p.panel.a;
    let dense = (base + 0.24).min(1.);
    // `opacity` multiplies, so set the alpha directly rather than dimming an
    // already translucent panel.
    Hsla {
        a: base + (dense - base) * stacked,
        ..p.panel
    }
    .blend(p.card)
}

/// Group notifications by application without changing their incoming order:
/// pinned first, then newest first. The first item of each group is the card
/// that shows while the stack is folded.
fn notification_groups(
    items: &[services::Notification],
) -> Vec<(String, Vec<&services::Notification>)> {
    let mut groups: Vec<(String, Vec<&services::Notification>)> = Vec::new();
    for item in items {
        if let Some((_, group)) = groups.iter_mut().find(|(app, _)| *app == item.app) {
            group.push(item);
        } else {
            groups.push((item.app.clone(), vec![item]));
        }
    }
    groups
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CalendarMonth {
    year: i32,
    month: u32,
    days: u32,
    /// Weekday of the first day, counting Monday as zero.
    first_weekday: u32,
}

fn calendar_month(year: i32, month: u32) -> CalendarMonth {
    let first =
        NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_else(|| NaiveDate::from_ymd_opt(2000, 1, 1).unwrap());
    let next = shifted_month(year, month, 1);
    let next = NaiveDate::from_ymd_opt(next.0, next.1, 1).unwrap_or(first);
    let days = (next - chrono::Duration::days(1)).day();
    CalendarMonth {
        year,
        month,
        days,
        first_weekday: first.weekday().num_days_from_monday(),
    }
}

/// Step a calendar year/month by whole months, carrying across year bounds.
fn shifted_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let month = month.clamp(1, 12) as i32;
    let index = year.saturating_mul(12) + (month - 1) + delta;
    let year = index.div_euclid(12);
    let month = index.rem_euclid(12) as u32 + 1;
    (year, month)
}

/// A navigation step in the calendar's control row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CalendarStep {
    Month(i32),
    Year(i32),
}

/// The month grid, Monday first, padded with `None` so every row has seven
/// cells.
fn calendar_weeks(month: CalendarMonth) -> Vec<Vec<Option<u32>>> {
    let mut cells = Vec::with_capacity(37);
    cells.extend(std::iter::repeat_n(None, month.first_weekday as usize));
    cells.extend((1..=month.days).map(Some));
    while !cells.len().is_multiple_of(7) {
        cells.push(None);
    }
    cells.chunks(7).map(|week| week.to_vec()).collect()
}

/// Frost the rounded column: the card shape with a bounded number of corner
/// bands, translated to its painted position.
fn container_regions(origin: Point<Pixels>, width: f32, height: f32) -> Vec<Bounds<Pixels>> {
    let mut regions = card_frost_shape(width, height, CONTAINER_RADIUS);
    for region in &mut regions {
        region.origin += origin;
    }
    regions
}

/// The rounded card shape with a bounded number of corner bands. Wayland caps
/// background-effect regions, so a handful of bands approximates the painted
/// radius while staying far below that cap.
fn card_frost_shape(width: f32, height: f32, radius: f32) -> Vec<Bounds<Pixels>> {
    const STEPS: f32 = 5.;
    if width <= 0. || height <= 0. {
        return Vec::new();
    }
    let radius = radius.min(width / 2.).min(height / 2.).max(0.);
    let mut regions = vec![Bounds::new(
        point(px(0.), px(radius)),
        size(px(width), px((height - 2. * radius).max(0.))),
    )];
    let step = radius / STEPS;
    for index in 0..STEPS as u32 {
        let top = index as f32 * step;
        let bottom = (index as f32 + 1.) * step;
        let middle = (top + bottom) / 2.;
        let distance = radius - middle;
        let inset = (radius - (radius * radius - distance * distance).max(0.).sqrt()).ceil();
        let width = (width - 2. * inset).max(0.);
        let band = (bottom - top).max(1.);
        regions.push(Bounds::new(
            point(px(inset), px(top)),
            size(px(width), px(band)),
        ));
        regions.push(Bounds::new(
            point(px(inset), px(height - bottom)),
            size(px(width), px(band)),
        ));
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

    fn fixture(id: u32, app: &str, pinned: bool, created_at: u64) -> services::Notification {
        services::Notification {
            id,
            app: app.into(),
            icon: None,
            title: format!("通知 {id}"),
            body: format!("正文 {id}"),
            actions: Vec::new(),
            created_at,
            unread: true,
            pinned,
        }
    }

    fn private_host(cx: &mut TestAppContext) -> (tempfile::TempDir, gpui::Entity<Host>) {
        use std::collections::HashMap;

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
        let host = cx.update(|cx| crate::app::testing::test_host(cx, catalog, environment));
        host.update(cx, |host, _| {
            // Interaction tests need the column at its resting position rather
            // than mid-slide; the slide itself is covered by its own test.
            host.prefs.reduce_motion = true;
        });
        (directory, host)
    }

    fn open_center(
        cx: &mut TestAppContext,
        host: &gpui::Entity<Host>,
    ) -> (gpui::Entity<ShellView>, VisualTestContext) {
        let window =
            cx.update(|cx| crate::app::testing::open_panel(host, Panel::Notifications, cx));
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.update(|window, _| {
            window.resize(size(px(1200.), px(800.)));
            window.activate_window();
            window.refresh();
        });
        visual.run_until_parked();
        (view, visual)
    }

    #[test]
    fn the_slide_rides_the_whole_column_off_the_right_edge() {
        assert_eq!(slide_offset(1., 368.), 0.);
        assert_eq!(slide_offset(0., 368.), 392.);
        assert_eq!(slide_offset(0.5, 368.), 196.);
    }

    #[test]
    fn calendar_weeks_start_on_monday_and_fill_seven_cells() {
        let month = calendar_month(2026, 10);
        assert_eq!(month.year, 2026);
        assert_eq!(month.month, 10);
        assert_eq!(month.days, 31);
        // 2026-10-01 is a Thursday.
        assert_eq!(month.first_weekday, 3);
        let weeks = calendar_weeks(month);
        assert_eq!(weeks.len(), 5);
        assert_eq!(weeks[0], vec![None, None, None, Some(1), Some(2), Some(3), Some(4)]);
        assert_eq!(
            weeks[4],
            vec![Some(26), Some(27), Some(28), Some(29), Some(30), Some(31), None]
        );
        for week in weeks {
            assert_eq!(week.len(), 7);
        }
    }

    #[test]
    fn calendar_navigation_carries_across_year_bounds() {
        assert_eq!(shifted_month(2026, 10, 1), (2026, 11));
        assert_eq!(shifted_month(2026, 12, 1), (2027, 1));
        assert_eq!(shifted_month(2026, 1, -1), (2025, 12));
        assert_eq!(shifted_month(2026, 1, -13), (2024, 12));
        let december = calendar_month(2026, 12);
        assert_eq!(december.days, 31);
        assert_eq!(calendar_month(2027, 1).days, 31);
        assert_eq!(calendar_month(2024, 2).days, 29);
    }

    #[test]
    fn empty_action_labels_fall_back_to_a_readable_name() {
        assert_eq!(action_label("default", ""), "打开");
        assert_eq!(action_label("default", "   "), "打开");
        assert_eq!(action_label("reply", ""), "reply");
        assert_eq!(action_label("reply", " 回复 "), "回复");
    }

    #[test]
    fn card_heights_cap_the_clamped_sections() {
        let mut item = fixture(1, "Chat", false, 10);
        item.body.clear();
        assert_eq!(notification_card_height(&item, false), 79.);
        item.body = "正文".into();
        assert_eq!(notification_card_height(&item, false), 129.);
        item.actions = vec![("reply".into(), "回复".into())];
        assert_eq!(notification_card_height(&item, false), 161.);
        assert_eq!(notification_card_height(&item, true), 183.);
    }

    #[test]
    fn stacked_cards_thicken_while_lone_cards_stay_airy() {
        let p = Palette::styled(true, rgb(0x0a84ff).into(), 0.8, 0.68, 0.82);
        let lone = notification_card_fill(p, 0.);
        let folded = notification_card_fill(p, 1.);
        assert!(
            folded.a >= 0.9,
            "folded cards must not let the text underneath read through"
        );
        assert!(
            lone.a < folded.a - 0.15,
            "a lone card should keep the frosted glass visible"
        );
    }

    #[gpui::test]
    fn a_short_card_shrinks_to_its_content(cx: &mut TestAppContext) {
        let (_directory, host) = private_host(cx);
        let mut item = fixture(1, "WeChat", false, 10);
        item.title = "喻航".into();
        item.body = "放在一年前早开了".into();
        item.actions = vec![("default".into(), String::new())];
        let limit = notification_card_height(&item, false);
        host.update(cx, |host, _| host.notifications = vec![item]);
        let (_view, mut visual) = open_center(cx, &host);

        let card = visual
            .debug_bounds("notification-card-1")
            .expect("the compact card is painted");
        assert!(
            card.size.height < px(limit),
            "a one-line card must not reserve room for the clamped maximum"
        );
        assert!(
            card.size.height > px(90.),
            "the icon, text and action row still define a real card"
        );
    }

    #[test]
    fn cards_from_the_same_application_share_one_group() {
        let items = vec![
            fixture(1, "Chat", false, 30),
            fixture(2, "Mail", false, 20),
            fixture(3, "Chat", false, 10),
        ];
        let groups = notification_groups(&items);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "Chat");
        assert_eq!(
            groups[0].1.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(groups[1].0, "Mail");
    }

    #[gpui::test]
    fn compact_cards_clamp_text_and_keep_actions_visible(cx: &mut TestAppContext) {
        let (_directory, host) = private_host(cx);
        host.update(cx, |host, _| {
            host.notifications = vec![services::Notification {
                id: 1,
                app: "中文通知测试".into(),
                icon: None,
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
                pinned: false,
            }];
        });
        let (_view, mut visual) = open_center(cx, &host);
        // Opening the center marks it read; capture only what the card sends.
        let received = cx.update(|cx| crate::app::testing::capture_service(&host, cx));

        let title = visual
            .debug_bounds("notification-title-1")
            .expect("the title is painted");
        assert_eq!(title.size.height, px(TITLE_HEIGHT));
        let body = visual
            .debug_bounds("notification-body-1")
            .expect("the body is painted");
        assert_eq!(body.size.height, px(BODY_HEIGHT));
        let card = visual
            .debug_bounds("notification-card-1")
            .expect("the compact card is painted");
        assert_eq!(
            card.size.height,
            px(notification_card_height(
                &host.read_with(cx, |host, _| host.notifications[0].clone()),
                false
            ))
        );
        let app = visual
            .debug_bounds("notification-app-1")
            .expect("the rounded app-icon tile");
        assert_eq!(app.size, size(px(36.), px(36.)));

        // Actions are a real row under the content, not something hidden inside
        // a scrollable body.
        let action = visual
            .debug_bounds("notification-action-1-2")
            .expect("the third action is painted");
        assert!(action.top() > body.bottom() - px(1.));
        assert!(action.bottom() <= card.bottom());

        let pin = visual
            .debug_bounds("notification-pin-1")
            .expect("the pin button is painted");
        visual.simulate_click(pin.center(), Modifiers::default());
        visual.run_until_parked();
        let command = received.try_recv().expect("pinning reaches the service");
        assert!(matches!(
            command,
            services::Command::PinNotification {
                id: 1,
                pinned: true
            }
        ));

        // Drag the card far enough sideways and release: it dismisses.
        let card = visual.debug_bounds("notification-card-1").unwrap();
        let start = card.center();
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            start + point(px(160.), px(0.)),
            None,
            Modifiers::default(),
        );
        visual.simulate_mouse_up(
            start + point(px(160.), px(0.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        let command = received.try_recv().expect("the drag reaches the service");
        assert!(matches!(
            command,
            services::Command::DismissNotification(1)
        ));
    }

    #[gpui::test]
    fn an_empty_default_action_is_named_and_clickable(cx: &mut TestAppContext) {
        let (_directory, host) = private_host(cx);
        let mut item = fixture(1, "WeChat", false, 10);
        item.actions = vec![("default".into(), String::new())];
        host.update(cx, |host, _| host.notifications = vec![item]);
        let (_view, mut visual) = open_center(cx, &host);
        let received = cx.update(|cx| crate::app::testing::capture_service(&host, cx));

        let action = visual
            .debug_bounds("notification-action-1-0")
            .expect("the default action paints");
        assert!(
            action.size.width > px(28.),
            "the fallback label keeps the button from collapsing"
        );
        visual.simulate_click(action.center(), Modifiers::default());
        visual.run_until_parked();
        let command = received.try_recv().expect("the action reaches the service");
        assert!(matches!(
            command,
            services::Command::NotificationAction { id: 1, action } if action == "default"
        ));
    }

    #[gpui::test]
    fn same_app_cards_fold_into_one_stack_that_expands(cx: &mut TestAppContext) {
        let (_directory, host) = private_host(cx);
        host.update(cx, |host, _| {
            host.notifications = vec![
                fixture(1, "Chat", false, 30),
                fixture(2, "Chat", false, 20),
                fixture(3, "Mail", false, 10),
            ];
        });
        let (_view, mut visual) = open_center(cx, &host);

        let first = visual
            .debug_bounds("notification-card-1")
            .expect("the stack's top card");
        let folded = visual
            .debug_bounds("notification-stack-Chat")
            .expect("the chat stack is painted");
        assert_eq!(folded.size.height, first.size.height + px(STACK_STEP));
        // The second notification is a real card layered behind the first:
        // its bottom edge peeks below the top card and it is narrower.
        let second_folded = visual
            .debug_bounds("notification-card-2")
            .expect("the second card is already laid out behind the first");
        assert_eq!(second_folded.bottom(), folded.bottom());
        assert!(second_folded.bottom() > first.bottom());
        assert!(second_folded.size.width < first.size.width);

        visual.simulate_click(first.center(), Modifiers::default());
        visual.run_until_parked();

        let second = visual
            .debug_bounds("notification-card-2")
            .expect("the folded card is laid out");
        let open = visual
            .debug_bounds("notification-stack-Chat")
            .expect("the expanded stack is painted");
        assert_eq!(
            open.size.height,
            first.size.height + second.size.height + px(CARD_GAP)
        );
        assert!(second.top() >= first.bottom(), "the second card follows the first");
        assert_eq!(
            second.size.width, first.size.width,
            "an open stack gives every card the full column width"
        );

        // Clicking the top card again folds the stack.
        visual.simulate_click(first.center(), Modifiers::default());
        visual.run_until_parked();
        let folded = visual.debug_bounds("notification-stack-Chat").unwrap();
        assert_eq!(folded.size.height, first.size.height + px(STACK_STEP));
    }

    #[gpui::test]
    fn a_long_app_stack_caps_its_folded_sheets(cx: &mut TestAppContext) {
        let (_directory, host) = private_host(cx);
        host.update(cx, |host, _| {
            host.notifications = (1..=5)
                .map(|id| fixture(id, "Chat", false, 100 - id as u64))
                .collect();
        });
        let (_view, mut visual) = open_center(cx, &host);

        let first = visual
            .debug_bounds("notification-card-1")
            .expect("the stack's top card");
        let folded = visual
            .debug_bounds("notification-stack-Chat")
            .expect("the chat stack is painted");
        assert_eq!(
            folded.size.height,
            first.size.height + px(STACK_STEP * STACK_SHEETS as f32)
        );
    }

    #[gpui::test]
    fn pinned_and_newest_notifications_paint_from_the_top(cx: &mut TestAppContext) {
        let (_directory, host) = private_host(cx);
        host.update(cx, |host, _| {
            // Exactly the order the service snapshot sends: pinned first, then
            // newest first within each group.
            host.notifications = vec![
                fixture(2, "Pinned", true, 5),
                fixture(3, "Newest", false, 20),
                fixture(1, "Oldest", false, 10),
            ];
        });
        let (_view, mut visual) = open_center(cx, &host);
        let pinned = visual
            .debug_bounds("notification-card-2")
            .expect("the pinned card is painted")
            .top();
        let newest = visual
            .debug_bounds("notification-card-3")
            .expect("the newest card is painted")
            .top();
        let oldest = visual
            .debug_bounds("notification-card-1")
            .expect("the oldest card is painted")
            .top();
        assert!(pinned < newest, "the pinned card leads the column");
        assert!(newest < oldest, "newer unpinned cards come first");
    }

    #[gpui::test]
    fn the_frosted_container_covers_the_whole_column(cx: &mut TestAppContext) {
        let (_directory, host) = private_host(cx);
        let (view, _visual) = open_center(cx, &host);
        let regions = view.read_with(cx, |view, _| view.notification_regions.borrow().clone());
        assert!(!regions.is_empty());
        assert!(regions.len() <= 64, "{}", regions.len());
        // The window is 1200 wide and the column starts at 1200 - 368 - 12.
        assert!(contains(&regions, 1000., 400.));
        assert!(!contains(&regions, 100., 400.));
        assert!(!contains(&regions, 828., 10.));
    }

    #[gpui::test]
    fn the_calendar_browses_months_and_selects_days(cx: &mut TestAppContext) {
        let (_directory, host) = private_host(cx);
        let today = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
        host.update(cx, |host, _| {
            host.system.today = Some(today);
            host.system.date = "10月28日 周三".into();
        });
        let (view, mut visual) = open_center(cx, &host);
        assert!(visual.debug_bounds("notification-calendar-today").is_some());

        // Browsing to the next month removes today's marker from the grid.
        let next = visual
            .debug_bounds("notification-calendar-next-month")
            .expect("the next-month control is painted");
        visual.simulate_click(next.center(), Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(view.calendar_month.get(), Some((2026, 11)));
            assert_eq!(view.calendar_day.get(), None);
        });
        assert!(visual.debug_bounds("notification-calendar-today").is_none());
        assert!(visual.debug_bounds("notification-calendar-today-button").is_some());

        // Picking a day highlights it with the filled circle.
        let day = visual
            .debug_bounds("notification-calendar-day-15")
            .expect("November 15 is painted");
        visual.simulate_click(day.center(), Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.calendar_day.get(),
                NaiveDate::from_ymd_opt(2026, 11, 15)
            );
        });
        assert!(visual
            .debug_bounds("notification-calendar-selected")
            .is_some());

        // The "today" button jumps back and re-selects the current day.
        let today_button = visual
            .debug_bounds("notification-calendar-today-button")
            .expect("the today button is painted while browsing");
        visual.simulate_click(today_button.center(), Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(view.calendar_month.get(), Some((2026, 10)));
            assert_eq!(view.calendar_day.get(), Some(today));
        });
        assert!(visual.debug_bounds("notification-calendar-today").is_some());
        assert!(visual
            .debug_bounds("notification-calendar-selected")
            .is_some());

        // A year step jumps the grid without losing the selected day.
        let next_year = visual
            .debug_bounds("notification-calendar-next-year")
            .expect("the next-year control is painted");
        visual.simulate_click(next_year.center(), Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(view.calendar_month.get(), Some((2027, 10)));
        });
    }

    #[gpui::test]
    fn closing_the_center_rides_out_before_releasing_the_surface(cx: &mut TestAppContext) {
        let (_directory, host) = private_host(cx);
        host.update(cx, |host, _| host.prefs.reduce_motion = false);
        let (_view, mut visual) = open_center(cx, &host);
        visual.simulate_mouse_down(
            point(px(40.), px(300.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        host.read_with(cx, |host, _| {
            assert!(host.panel_exit, "the close starts the slide-out ride");
            assert_eq!(host.panel, Some(Panel::Notifications));
        });
        visual
            .cx
            .background_executor
            .advance_clock(std::time::Duration::from_millis(400));
        visual.run_until_parked();
        host.read_with(cx, |host, _| {
            assert!(host.panel.is_none(), "the surface is released after the ride");
        });
    }

    #[gpui::test]
    fn clicking_outside_the_column_dismisses_the_center(cx: &mut TestAppContext) {
        let (_directory, host) = private_host(cx);
        let (_view, mut visual) = open_center(cx, &host);
        assert!(
            visual.debug_bounds("notifications-backdrop").is_some(),
            "the full-output click-away catcher is present"
        );
        // Reduced motion releases the center immediately, so the press alone is
        // the whole interaction.
        visual.simulate_mouse_down(
            point(px(40.), px(300.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        host.read_with(cx, |host, _| {
            assert!(host.panel.is_none(), "an outside click dismisses the center");
        });
    }
}
