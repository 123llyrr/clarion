// SPDX-License-Identifier: GPL-3.0-or-later
//! The polkit authentication card. It is a semi-transparent, frosted modal:
//! the compositor blurs only the card's rounded region, so the wallpaper (and
//! whatever panel was open behind it) stays visible around the prompt.
use super::*;

pub(super) const AUTH_WIDTH: f32 = 340.;
pub(super) const AUTH_RADIUS: f32 = 32.;
const AUTH_PAD_X: f32 = 25.;
const AUTH_PAD_TOP: f32 = 30.;
const AUTH_PAD_BOTTOM: f32 = 23.;
const AUTH_GAP: f32 = 21.;
const AUTH_BORDER: f32 = 1.;
pub(super) const AUTH_ICON_BOX: f32 = 88.;
/// 15px title, wrapped up to three lines.
const AUTH_TITLE_LINE: f32 = 23.;
const AUTH_TITLE_MAX_LINES: usize = 3;
const AUTH_SUB_H: f32 = 18.;
const AUTH_DETAIL_LINE: f32 = 16.;
const AUTH_DETAIL_PAD: f32 = 7.;
const AUTH_FIELD_H: f32 = 48.;
const AUTH_MESSAGE_H: f32 = 18.;
const AUTH_QUEUE_H: f32 = 17.;
pub(super) const AUTH_BUTTON_H: f32 = 44.;

/// The card's exact outer size. Rendering and the compositor's glass region
/// both read it, so the blur follows the painted silhouette instead of an
/// estimate.
pub(super) fn auth_card_size(host: &ViewState, width: f32) -> (f32, f32) {
    let width = AUTH_WIDTH.min((width - 48.).max(280.));
    let Some(auth) = host.auth.as_ref() else {
        return (width, 0.);
    };
    let detail_lines = visible_details(auth).len();
    let mut height = AUTH_PAD_TOP
        + AUTH_ICON_BOX
        + title_height(auth, width)
        + AUTH_SUB_H
        + AUTH_FIELD_H
        + AUTH_BUTTON_H
        + AUTH_PAD_BOTTOM
        + AUTH_BORDER * 2.;
    let mut children = 5;
    if detail_lines > 0 {
        height += AUTH_DETAIL_PAD * 2. + detail_lines as f32 * AUTH_DETAIL_LINE;
        children += 1;
    }
    if auth.info.is_some() {
        height += AUTH_MESSAGE_H;
        children += 1;
    }
    if auth.error.is_some() {
        height += AUTH_MESSAGE_H;
        children += 1;
    }
    if auth.request.queued > 0 {
        height += AUTH_QUEUE_H;
        children += 1;
    }
    height += AUTH_GAP * (children - 1) as f32;
    (width, height)
}

/// The details a person can use, already stripped of polkit bookkeeping.
fn visible_details(auth: &services::AuthView) -> Vec<(&str, &str)> {
    crate::polkit::visible_details(&auth.request.details)
}

/// One conservative line estimate, capped so a pathological message cannot
/// grow the card without bound. The rendered text wraps inside this height.
fn title_height(auth: &services::AuthView, card_width: f32) -> f32 {
    let available = (card_width - AUTH_PAD_X * 2. - AUTH_BORDER * 2.).max(1.);
    let text_width: f32 = auth
        .request
        .message
        .chars()
        .map(|character| if character.is_ascii() { 8. } else { 15. })
        .sum();
    let lines = (text_width / available)
        .ceil()
        .clamp(1., AUTH_TITLE_MAX_LINES as f32);
    lines * AUTH_TITLE_LINE
}

pub(super) fn auth_surface(
    view: &ShellView,
    host: &ViewState,
    p: Palette,
    width: f32,
    height: f32,
    window: &mut Window,
    cx: &Context<ShellView>,
) -> AnyElement {
    let Some(auth) = host.auth.as_ref() else {
        return div().size_full().into_any_element();
    };
    let dark = host.dark();
    let danger: Hsla = rgb(0xff453a).into();
    let field_bg = if dark {
        rgba(0x00000047)
    } else {
        rgba(0xffffff8c)
    };
    let cancel_bg = if dark {
        rgba(0xffffff1f)
    } else {
        rgba(0x00000014)
    };
    let cancel_hover = if dark {
        rgba(0xffffff2e)
    } else {
        rgba(0x00000022)
    };
    let (card_width, card_height) = auth_card_size(host, width);

    // An app icon keeps its own artwork (many are full-colour SVGs); the
    // action icon and the generic fallback are symbolic, so they are tinted.
    let app_icon = auth
        .request
        .app_icon
        .as_deref()
        .filter(|name| !name.is_empty());
    let icon = match app_icon.map(|name| {
        if name.starts_with('/') {
            let path = std::path::PathBuf::from(name);
            path.is_file().then_some(path)
        } else {
            let mut cache = view.theme_icons.borrow_mut();
            theme_icon(name, &mut cache)
        }
    }) {
        Some(Some(path)) => img(path)
            .size(px(60.))
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        Some(None) => {
            let path = view.themed_glyph_path(
                "dialog-password-symbolic",
                "dialog-password-symbolic",
            );
            view.glyph(&path, 60., p)
        }
        None => {
            let name = auth
                .request
                .icon
                .as_deref()
                .filter(|name| !name.is_empty())
                .unwrap_or("dialog-password-symbolic");
            let path = view.themed_glyph_path(name, "dialog-password-symbolic");
            view.glyph(&path, 60., p)
        }
    };

    let subtitle = match auth
        .request
        .app_name
        .as_deref()
        .filter(|name| !name.is_empty())
    {
        Some(app) => format!("{app} 请求以 {} 的身份认证", auth.request.identity.username),
        None => format!("以 {} 的身份认证", auth.request.identity.username),
    };
    let details = visible_details(auth);
    let title_h = title_height(auth, card_width);
    let focused = view.input.read(cx).focused(window);
    let submitting = auth.submitting;

    let view_handle = cx.entity().downgrade();
    let cancel_handle = view_handle.clone();
    let submit_handle = view_handle.clone();

    // The prompt label is gone: a lock/person prefix inside the capsule says
    // what the field is, and the placeholder clears as soon as the user types.
    let field_icon_name = if auth.echo {
        "avatar-default-symbolic"
    } else {
        "system-lock-screen-symbolic"
    };
    let field_icon = view.themed_glyph_path(field_icon_name, "dialog-password-symbolic");
    let field = div()
        .id("auth-field")
        .debug_selector(|| "auth-field".into())
        .h(px(AUTH_FIELD_H))
        .w_full()
        .rounded(px(AUTH_FIELD_H / 2.))
        .border_1()
        .border_color(if focused { p.accent } else { p.line })
        .bg(field_bg)
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(8.))
        .child(view.glyph(&field_icon, 18., p))
        .child(div().flex_1().min_w_0().child(view.input.clone()));

    let cancel = div()
        .id("auth-cancel")
        .debug_selector(|| "auth-cancel".into())
        .flex_1()
        .h(px(AUTH_BUTTON_H))
        .rounded(px(AUTH_BUTTON_H / 2.))
        .bg(cancel_bg)
        .hover(move |style| style.bg(cancel_hover))
        .text_color(p.fg)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(15.))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            let _ = cancel_handle.update(cx, |view, cx| view.invoke(Action::AuthCancel, cx));
        })
        .child(menu_text(
            "取消".to_owned(),
            FontWeight::MEDIUM,
            p.fg,
            cx,
        ));

    let mut primary = div()
        .id("auth-submit")
        .debug_selector(|| "auth-submit".into())
        .flex_1()
        .h(px(AUTH_BUTTON_H))
        .rounded(px(AUTH_BUTTON_H / 2.))
        .bg(if submitting {
            p.accent.opacity(0.55)
        } else {
            p.accent
        })
        .text_color(rgb(0xffffffff))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(15.))
        .child(menu_text(
            if submitting { "认证中…" } else { "认证" }.to_owned(),
            FontWeight::SEMIBOLD,
            rgb(0xffffffff).into(),
            cx,
        ));
    if !submitting {
        primary = primary
            .hover(move |style| style.bg(p.accent.blend(rgba(0xffffff1f).into())))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                let _ = submit_handle.update(cx, |view, cx| view.auth_submit(cx));
            });
    }

    let card = div()
        .id("auth-card")
        .debug_selector(|| "auth-card".into())
        .w(px(card_width))
        .h(px(card_height))
        .rounded(px(AUTH_RADIUS))
        .border_1()
        .border_color(p.line)
        // The palette is already semi-transparent and carries the frost wash;
        // the compositor blurs exactly this rounded rectangle.
        .bg(p.panel)
        .text_color(p.fg)
        .px(px(AUTH_PAD_X))
        .pt(px(AUTH_PAD_TOP))
        .pb(px(AUTH_PAD_BOTTOM))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(AUTH_GAP))
        .overflow_hidden()
        // A dialog, not a popup: a click inside must never reach the scrim.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .id("auth-icon")
                .debug_selector(|| "auth-icon".into())
                .size(px(AUTH_ICON_BOX))
                .rounded(px(23.))
                .bg(p.card)
                .border_1()
                .border_color(p.line)
                .flex()
                .items_center()
                .justify_center()
                .child(icon),
        )
        .child(
            div()
                .w_full()
                .h(px(title_h))
                .text_center()
                .text_size(px(17.))
                .line_height(px(AUTH_TITLE_LINE))
                .whitespace_normal()
                .overflow_hidden()
                .child(menu_text(
                    auth.request.message.clone(),
                    FontWeight::SEMIBOLD,
                    p.fg,
                    cx,
                )),
        )
        .child(
            div()
                .w_full()
                .h(px(AUTH_SUB_H))
                .flex()
                .items_center()
                .justify_center()
                .text_center()
                .text_size(px(13.5))
                .text_color(p.muted)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(menu_text(
                    subtitle,
                    FontWeight::NORMAL,
                    p.muted,
                    cx,
                )),
        )
        .when(!details.is_empty(), |card| {
            card.child(
                div()
                    .id("auth-details")
                    .debug_selector(|| "auth-details".into())
                    .w_full()
                    .rounded(px(9.))
                    .bg(p.card)
                    .px(px(10.))
                    .py(px(AUTH_DETAIL_PAD))
                    .flex()
                    .flex_col()
                    .children(details.iter().map(|(key, value)| {
                        div()
                            .w_full()
                            .h(px(AUTH_DETAIL_LINE))
                            .flex()
                            .items_center()
                            .text_size(px(12.))
                            .text_color(p.muted)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(menu_text(
                                format!("{key}  {value}"),
                                FontWeight::NORMAL,
                                p.muted,
                                cx,
                            ))
                    })),
            )
        })
        .child(field)
        .when_some(auth.info.clone(), |card, info| {
            card.child(
                div()
                    .w_full()
                    .h(px(AUTH_MESSAGE_H))
                    .flex()
                    .items_center()
                    .text_size(px(12.5))
                    .text_color(p.muted)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(menu_text(info, FontWeight::NORMAL, p.muted, cx)),
            )
        })
        .when_some(auth.error.clone(), |card, error| {
            card.child(
                div()
                    .w_full()
                    .h(px(AUTH_MESSAGE_H))
                    .flex()
                    .items_center()
                    .text_size(px(12.5))
                    .text_color(danger)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(menu_text(error, FontWeight::NORMAL, danger, cx)),
            )
        })
        .when(auth.request.queued > 0, |card| {
            card.child(
                div()
                    .w_full()
                    .h(px(AUTH_QUEUE_H))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(p.muted)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(menu_text(
                        format!("还有 {} 个认证请求", auth.request.queued),
                        FontWeight::NORMAL,
                        p.muted,
                        cx,
                    )),
            )
        })
        .child(
            div()
                .w_full()
                .flex()
                .gap(px(8.))
                .child(cancel)
                .child(primary),
        );

    let dismiss = |_: &MouseDownEvent, _: &mut Window, cx: &mut App| {
        cx.stop_propagation();
    };
    div()
        .id("auth-layer")
        .absolute()
        .left(px(0.))
        .top(px(0.))
        .w(px(width))
        .h(px(height))
        .bg(p.scrim)
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.))
        .on_mouse_down(MouseButton::Left, dismiss)
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(card)
        .into_any_element()
}
