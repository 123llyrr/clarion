// SPDX-License-Identifier: GPL-3.0-or-later
//! The standalone battery popup: one summary card followed by the circular
//! platform-profile switcher. The popup owns its outer glass and shadow.
use super::macos_controls::{control_card, ControlSlider};
use super::*;
use crate::power::PowerProfile;

/// The range the charge-limit slider offers. Drivers commonly accept 40, but
/// 50 is the floor every implementation understands; 100 means no ceiling.
const CHARGE_LIMIT_MIN: u8 = 50;
const CHARGE_LIMIT_MAX: u8 = 100;

/// The charge-limit track fills white, so its prefix glyph stays dark in both
/// appearances instead of following the theme's light/dark battery artwork.
fn charge_limit_icon_color() -> Hsla {
    rgb(0x363636).into()
}

/// One turn of the confirmation ring.
const POWER_SPIN_PERIOD: f32 = 0.9;
/// How long the green completion burst and its check badge stay on screen.
const POWER_SUCCESS_BURST: f32 = 0.62;
/// The red counterpart shown when the platform keeps the old profile.
const POWER_FAILURE_BURST: f32 = 0.5;

#[derive(Clone, Copy, Default)]
struct PowerButtonFx {
    /// This is the button the worker is currently confirming.
    pending: bool,
    /// Rotation of the ring outside the circle, in radians.
    spin: f32,
    /// `(progress, success)` for the short burst after the read-back.
    burst: Option<(f32, bool)>,
}

#[derive(IntoElement)]
struct PowerText {
    text: String,
    size: f32,
    weight: FontWeight,
    color: Hsla,
}

impl RenderOnce for PowerText {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .text_size(px(self.size))
            .line_height(px(self.size + 4.))
            .whitespace_nowrap()
            .child(menu_text(self.text, self.weight, self.color, cx))
    }
}

fn text(text: impl Into<String>, size: f32, weight: FontWeight, color: Hsla) -> PowerText {
    PowerText {
        text: text.into(),
        size,
        weight,
        color,
    }
}

/// One aligned metric row. The fixed label column keeps the two values on the
/// same vertical line, so health and power source read as one little table.
fn metric(id: &'static str, label: &'static str, value: String, p: Palette) -> AnyElement {
    div()
        .debug_selector(move || id.into())
        .flex()
        .items_baseline()
        .gap(px(8.))
        .child(
            div()
                .w(px(34.))
                .flex_shrink_0()
                .child(text(label, 10., FontWeight::NORMAL, p.muted)),
        )
        .child(text(value, 12., FontWeight::MEDIUM, p.fg))
        .into_any_element()
}

impl ShellView {
    pub(super) fn macos_battery(
        &self,
        host: &ViewState,
        p: Palette,
        window: &mut Window,
    ) -> AnyElement {
        let percent = host.system.battery;
        let state = crate::battery::state(
            percent,
            host.system.battery_status,
            host.system.ac_online,
        );
        let tone = crate::battery::tone(state);
        let icon = crate::battery::icon(state, percent);
        let percent_text = percent
            .map(crate::battery::percent_text)
            .unwrap_or_else(|| "不可用".into());
        let health = host
            .system
            .battery_health
            .filter(|health| health.is_finite())
            .map(crate::battery::percent_text)
            .unwrap_or_else(|| "—".into());
        let source = if host.system.ac_online {
            "电源适配器"
        } else if percent.is_some() {
            "电池供电"
        } else {
            "—"
        };
        // Two balanced groups: the battery mark with the hero percentage and
        // its state subtitle on the left, then an aligned two-row table for
        // health and power source on the right. Nothing shares a line that can
        // outgrow the card.
        let summary = control_card(p)
            .debug_selector(|| "battery-summary".into())
            .w_full()
            .px(px(14.))
            .py(px(12.))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .size(px(48.))
                            .flex_shrink_0()
                            .rounded_full()
                            .border_1()
                            .border_color(p.line)
                            .bg(p.card)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(self.battery_icon(&icon, percent.unwrap_or(0.), tone, 32., p)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(text(percent_text, 26., FontWeight::SEMIBOLD, p.fg))
                            .child(text(
                                crate::battery::label(state),
                                11.,
                                FontWeight::NORMAL,
                                p.muted,
                            )),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(div().w(px(1.)).h(px(40.)).bg(p.line.opacity(0.55)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(metric("battery-health", "健康度", health, p))
                            .child(metric("battery-source", "供电", source.into(), p)),
                    ),
            );

        let now = std::time::Instant::now();
        let reduce = host.prefs.reduce_motion;
        let pending = host.power_profile_pending;
        // The spinner runs while the worker confirms. Once the pending target
        // clears, the corresponding button gets a burst; the accent fill only
        // appears at that point because the host waits for the read-back too.
        let (spin, burst) = {
            let mut anim = self.power_switch.borrow_mut();
            if anim.seen_pending != pending {
                match pending {
                    Some(_) => {
                        anim.spinner_start = Some(now);
                        anim.burst = None;
                    }
                    None => {
                        anim.spinner_start = None;
                        if let Some(target) = anim.seen_pending {
                            let success = host.system.power_profile == Some(target);
                            anim.burst = Some((target, now, success));
                        }
                    }
                }
                anim.seen_pending = pending;
            }
            let spin = anim
                .spinner_start
                .map(|start| {
                    now.duration_since(start).as_secs_f32() * std::f32::consts::TAU
                        / POWER_SPIN_PERIOD
                })
                .unwrap_or(0.);
            let burst = anim.burst.and_then(|(profile, start, success)| {
                let duration = if success {
                    POWER_SUCCESS_BURST
                } else {
                    POWER_FAILURE_BURST
                };
                let progress = now.duration_since(start).as_secs_f32() / duration;
                (progress < 1.).then_some((profile, progress, success))
            });
            if burst.is_none() {
                anim.burst = None;
            }
            (
                if reduce { 0. } else { spin },
                if reduce { None } else { burst },
            )
        };
        let animating = !reduce && (pending.is_some() || burst.is_some());

        let profiles: Vec<PowerProfile> = if host.system.power_profiles.is_empty() {
            PowerProfile::ALL.to_vec()
        } else {
            host.system.power_profiles.clone()
        };
        let mut buttons = div().w_full().flex().gap(px(8.));
        for profile in profiles {
            let fx = PowerButtonFx {
                pending: pending == Some(profile),
                spin,
                burst: burst.and_then(|(target, progress, success)| {
                    (target == profile).then_some((progress, success))
                }),
            };
            buttons = buttons.child(self.power_profile_button(
                profile,
                host.system.power_profile == Some(profile),
                fx,
                p,
            ));
        }
        let modes = div()
            .debug_selector(|| "battery-modes".into())
            .w_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(text("电源模式", 13., FontWeight::SEMIBOLD, p.fg))
                    .child(text(
                        if host.system.power_profiles.is_empty() {
                            "未检测到"
                        } else {
                            "点击切换"
                        },
                        11.,
                        FontWeight::NORMAL,
                        p.muted,
                    )),
            )
            .child(buttons);

        if animating {
            window.request_animation_frame();
        }

        // One card carries both the profile switch and the charge ceiling; the
        // top padding keeps the "电源模式" line clear of the large corner arc.
        let settings = control_card(p)
            .debug_selector(|| "battery-settings".into())
            .w_full()
            .px(px(16.))
            .pt(px(18.))
            .pb(px(14.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(modes)
            .child(div().h(px(1.)).bg(p.line.opacity(0.55)))
            .child(self.battery_charge_limit_content(host, p));

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(summary)
            .child(settings)
            .into_any_element()
    }

    /// The charge ceiling. Supported machines get the same slim slider as the
    /// Control Center's brightness and volume; unsupported ones keep the card
    /// at the default 100% and say so instead of hiding the row.
    fn battery_charge_limit_content(&self, host: &ViewState, p: Palette) -> AnyElement {
        let supported = host.battery_charge_limit.is_some();
        let limit = host.battery_charge_limit.unwrap_or(1.).clamp(0., 1.);
        let percent = format!("{}%", (limit * 100.).round());
        let value_debug = format!("battery-charge-limit-value-{percent}");
        let percent_color = if supported { p.fg } else { p.muted };
        let section = div()
            .debug_selector(|| "battery-charge-limit".into())
            .w_full()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .h(px(19.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(text("充电上限", 13., FontWeight::SEMIBOLD, p.fg))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .debug_selector(move || value_debug.clone())
                                    .child(text(percent, 11., FontWeight::NORMAL, percent_color)),
                            )
                            .when(!supported, |row| {
                                row.child(
                                    div()
                                        .debug_selector(|| {
                                            "battery-charge-limit-unavailable".into()
                                        })
                                        .child(text("不可用", 10., FontWeight::NORMAL, p.muted)),
                                )
                            }),
                    ),
            );
        if !supported {
            // The unsupported pill does not sit on the white track, so it keeps
            // the theme artwork's own light/dark colours.
            let state = crate::battery::state(
                host.system.battery,
                host.system.battery_status,
                host.system.ac_online,
            );
            let tone = crate::battery::tone(state);
            let icon = self.battery_icon("battery-symbolic", limit, tone, 15., p);
            return section
                .child(
                    div()
                        .h(px(26.))
                        .w_full()
                        .rounded_full()
                        .bg(if host.dark() {
                            rgba(0x00000022)
                        } else {
                            rgba(0x00000010)
                        })
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .child(icon),
                )
                .into_any_element();
        }
        // The slider is a ceiling, not the current charge: a generic battery
        // mark reads better than the state-tinted level icon beside a
        // percentage that describes the limit. The white track is why the
        // glyph below is fixed dark instead of following the theme.
        // The gesture is 0–1 across the track; map it onto the percentage
        // range the firmware accepts.
        let normalized = ((limit * 100. - f64::from(CHARGE_LIMIT_MIN))
            / f64::from(CHARGE_LIMIT_MAX - CHARGE_LIMIT_MIN))
            .clamp(0., 1.);
        section.child(ControlSlider {
            key: SharedString::from("battery-charge-limit-slider"),
            selector: "slider-battery-charge-limit".into(),
            host: self.host.clone(),
            action: std::rc::Rc::new(|value: f64| {
                let span = f64::from(CHARGE_LIMIT_MAX - CHARGE_LIMIT_MIN);
                let percent = f64::from(CHARGE_LIMIT_MIN) + value.clamp(0., 1.) * span;
                Action::BatteryChargeLimit(percent.round() as u8)
            }),
            value: normalized,
            dark: host.dark(),
            p,
            icon: self.battery_glyph(limit, charge_limit_icon_color(), 15.),
        })
        .into_any_element()
    }

    /// Draws the confirmation ring outside the 58 px button and the short
    /// completion burst once the worker answers. Everything is absolutely
    /// positioned, so the button's layout and hitbox stay identical.
    fn power_button_fx(&self, fx: PowerButtonFx, p: Palette) -> AnyElement {
        let mut layer = div().absolute().left(px(0.)).top(px(0.)).size(px(58.));
        if fx.pending {
            layer = layer
                .child(
                    div()
                        .absolute()
                        .left(px(-4.))
                        .top(px(-4.))
                        .size(px(66.))
                        .rounded_full()
                        .border_1()
                        .border_color(p.accent.opacity(0.22)),
                )
                .child(
                    div()
                        .debug_selector(|| "battery-profile-spinner".into())
                        .absolute()
                        .left(px(-4.))
                        .top(px(-4.))
                        .size(px(66.))
                        .child(self.glyph_rotated(
                            &icon_path("power-spinner"),
                            66.,
                            fx.spin,
                            Palette { fg: p.accent, ..p },
                        )),
                );
        }
        if let Some((progress, success)) = fx.burst {
            let color: Hsla = if success {
                rgb(0x30d158).into()
            } else {
                rgb(0xff453a).into()
            };
            let radius = 58. + 30. * progress;
            let offset = (58. - radius) / 2.;
            layer = layer.child(
                div()
                    .debug_selector(move || {
                        if success {
                            "battery-profile-success".into()
                        } else {
                            "battery-profile-failure".into()
                        }
                    })
                    .absolute()
                    .left(px(offset))
                    .top(px(offset))
                    .size(px(radius))
                    .rounded_full()
                    .border_2()
                    .border_color(color.opacity((1. - progress).powi(2))),
            );
            if success {
                let badge_alpha = if progress < 0.7 {
                    1.
                } else {
                    ((1. - progress) / 0.3).clamp(0., 1.)
                };
                layer = layer.child(
                    div()
                        .debug_selector(|| "battery-profile-check".into())
                        .absolute()
                        .right(px(-3.))
                        .top(px(-3.))
                        .size(px(18.))
                        .rounded_full()
                        .bg(color)
                        .opacity(badge_alpha)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(self.glyph(
                            &icon_path("power-check"),
                            11.,
                            Palette {
                                fg: rgb(0xffffff).into(),
                                ..p
                            },
                        )),
                );
            }
        }
        layer.into_any_element()
    }

    fn power_profile_button(
        &self,
        profile: PowerProfile,
        selected: bool,
        fx: PowerButtonFx,
        p: Palette,
    ) -> AnyElement {
        let host = self.host.clone();
        let keyboard_host = self.host.clone();
        let action = Action::PowerProfile(profile);
        let keyboard_action = action.clone();
        let id = match profile {
            PowerProfile::LowPower => "battery-profile-low-power",
            PowerProfile::Balanced => "battery-profile-balanced",
            PowerProfile::Performance => "battery-profile-performance",
        };
        let icon_color = if selected {
            rgb(0xffffff).into()
        } else {
            p.fg
        };
        let fx_layer = self.power_button_fx(fx, p);
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(7.))
            .child(
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .size(px(58.))
                    .rounded_full()
                    .border_1()
                    .border_color(if selected { p.accent } else { p.line })
                    .bg(if selected { p.accent } else { p.card })
                    .flex()
                    .items_center()
                    .justify_center()
                    .focusable()
                    .key_context("ClarionControlButton")
                    .cursor_pointer()
                    .hover(move |style| {
                        if selected {
                            style
                        } else {
                            style.bg(p.line)
                        }
                    })
                    .focus(move |style| style.border_color(p.accent))
                    .child(fx_layer)
                    .child(self.symbol(
                        profile.icon(),
                        26.,
                        Palette {
                            fg: icon_color,
                            ..p
                        },
                    ))
                    .on_click(move |_, _, cx| {
                        host.update(cx, |host, cx| host.handle(action.clone(), cx))
                    })
                    .on_action({
                        let host = keyboard_host.clone();
                        let action = keyboard_action.clone();
                        move |_: &Activate, _, cx| {
                            host.update(cx, |host, cx| host.handle(action.clone(), cx));
                            cx.stop_propagation();
                        }
                    })
                    .on_key_down(move |event, _, cx| {
                        if event.keystroke.key == "space"
                            && event.keystroke.modifiers == Modifiers::default()
                        {
                            keyboard_host
                                .update(cx, |host, cx| host.handle(keyboard_action.clone(), cx));
                            cx.stop_propagation();
                        }
                    }),
            )
            .child(text(
                profile.label(),
                12.,
                if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                },
                if selected { p.fg } else { p.muted },
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn the_charge_limit_prefix_stays_dark_on_the_white_track() {
        let color = charge_limit_icon_color();
        assert!(
            color.l < 0.35,
            "the track is always white, so the prefix must stay dark: {color:?}"
        );
        assert_eq!(color.a, 1.0);
    }
}
