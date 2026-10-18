// SPDX-License-Identifier: GPL-3.0-or-later
//! A compact desktop preferences window, rather than a generic command panel.
use super::*;

#[derive(IntoElement)]
struct NativePrefsText(String);

impl RenderOnce for NativePrefsText {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let style = window.text_style();
        // Keep each row's size/weight/color while explicitly selecting the CJK
        // and Latin faces; Linux GPUI currently ignores font fallback lists.
        menu_text(self.0, style.font_weight, style.color, cx)
    }
}

fn pref_text(text: impl Into<String>) -> NativePrefsText {
    NativePrefsText(text.into())
}

fn form_row(label: &'static str, control: impl IntoElement) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(20.))
        .child(
            div()
                .w(px(132.))
                .flex_none()
                .text_right()
                .text_size(px(13.))
                .child(pref_text(label)),
        )
        .child(div().flex_1().min_w_0().child(control))
}

fn form_separator(line: Hsla) -> Div {
    div().h(px(1.)).my(px(12.)).bg(line)
}

fn miniature_desktop(dark: bool) -> Div {
    let window = if dark { 0x35353b } else { 0xf5f5f7 };
    let sidebar = if dark { 0x29292f } else { 0xe3e4e9 };
    let mut dock = div()
        .absolute()
        .bottom(px(5.))
        .left(px(30.))
        .h(px(12.))
        .px(px(3.))
        .flex()
        .items_center()
        .gap(px(2.))
        .rounded(px(4.))
        .bg(if dark {
            rgba(0x3e3b6177)
        } else {
            rgba(0xffffff77)
        });
    for color in [0x5aabef, 0xf4f4f6, 0xf47f8b, 0x8c9fe8, 0x5fc9a5] {
        dock = dock.child(div().size(px(7.)).rounded(px(2.)).bg(rgb(color)));
    }
    div()
        .w(px(106.))
        .h(px(65.))
        .relative()
        .overflow_hidden()
        .rounded(px(4.))
        .bg(linear_gradient(
            135.,
            linear_color_stop(rgb(if dark { 0x302a77 } else { 0x71b3e8 }), 0.),
            linear_color_stop(rgb(if dark { 0x99578a } else { 0xd8a7d5 }), 1.),
        ))
        .child(
            div()
                .absolute()
                .right(px(-14.))
                .top(px(15.))
                .w(px(82.))
                .h(px(62.))
                .rounded_full()
                .bg(rgb(if dark { 0xb86b79 } else { 0xffbba2 })),
        )
        .child(
            div()
                .absolute()
                .left(px(10.))
                .top(px(12.))
                .w(px(67.))
                .h(px(36.))
                .rounded(px(3.))
                .overflow_hidden()
                .bg(rgb(window))
                .border_1()
                .border_color(rgba(if dark { 0xffffff20 } else { 0x00000020 }))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(20.))
                        .bg(rgb(sidebar)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(25.))
                        .top(px(9.))
                        .w(px(31.))
                        .h(px(3.))
                        .rounded(px(1.))
                        .bg(rgb(if dark { 0x797980 } else { 0xb8b9c1 })),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(25.))
                        .top(px(16.))
                        .w(px(24.))
                        .h(px(2.))
                        .rounded(px(1.))
                        .bg(rgb(if dark { 0x55555d } else { 0xd4d4da })),
                ),
        )
        .child(dock)
}

impl ShellView {
    fn appearance_choice(&self, dark: bool, selected: bool, p: Palette) -> AnyElement {
        let host = self.host.clone();
        let theme = if dark { "dark" } else { "light" };
        let id = format!("prefs-appearance-{theme}");
        let selector = id.clone();
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector)
            .flex()
            .flex_col()
            .items_center()
            .gap(px(5.))
            .cursor_pointer()
            .child(
                div()
                    .p(px(3.))
                    .rounded(px(7.))
                    .border_2()
                    .border_color(if selected {
                        p.accent
                    } else {
                        Hsla::transparent_black()
                    })
                    .hover(move |s| s.bg(p.line))
                    .child(miniature_desktop(dark)),
            )
            .child(div().text_size(px(11.)).child(pref_text(if dark {
                "深色"
            } else {
                "浅色"
            })))
            .on_click(move |_, _, cx| {
                host.update(cx, |host, cx| host.handle(Action::Theme(theme.into()), cx))
            })
            .into_any_element()
    }

    fn preference_check(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        action: Action,
        p: Palette,
    ) -> AnyElement {
        let host = self.host.clone();
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .flex()
            .items_center()
            .gap(px(7.))
            .py(px(5.))
            .cursor_pointer()
            .child(
                div()
                    .size(px(14.))
                    .flex_none()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if enabled { p.accent } else { p.line })
                    .bg(if enabled { p.accent } else { p.card })
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(enabled, |s| {
                        s.child(
                            canvas(
                                |_, _, _| (),
                                |bounds, _, window, _| {
                                    let origin = bounds.origin;
                                    let mut check = PathBuilder::stroke(px(1.5));
                                    check.move_to(origin + point(px(1.5), px(5.5)));
                                    check.line_to(origin + point(px(4.), px(8.)));
                                    check.line_to(origin + point(px(9.5), px(2.5)));
                                    if let Ok(path) = check.build() {
                                        window.paint_path(path, rgb(0xffffff));
                                    }
                                },
                            )
                            .size(px(11.)),
                        )
                    }),
            )
            .child(div().text_size(px(13.)).child(pref_text(label)))
            .on_click(move |_, _, cx| host.update(cx, |host, cx| host.handle(action.clone(), cx)))
            .into_any_element()
    }

    /// Reuses the shared gesture state, endpoint math and 80 ms command throttle;
    /// this form supplies a compact track without the generic panel's heading.
    fn preference_slider(
        &self,
        id: &'static str,
        value: f64,
        action: fn(f64) -> Action,
        p: Palette,
    ) -> AnyElement {
        let key = SharedString::from(format!("slider-{id}"));
        let selector = format!("slider-{id}");
        let host = self.host.clone();
        div()
            .id(key.clone())
            .debug_selector(move || selector)
            .w_full()
            .h(px(22.))
            .cursor_pointer()
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let gesture =
                            window.use_keyed_state(key, cx, |_, _| SliderGesture::default());
                        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                        (gesture, hitbox)
                    },
                    move |bounds, (gesture, hitbox), window, cx| {
                        let displayed = gesture.update(cx, |state, _| {
                            if !state.dragging
                                && state.preview.is_some_and(|(preview, changed)| {
                                    (preview - value).abs() < 0.001
                                        || changed.elapsed() > std::time::Duration::from_secs(2)
                                })
                            {
                                state.preview = None;
                            }
                            state
                                .preview
                                .map(|(value, _)| value)
                                .unwrap_or(value)
                                .clamp(0., 1.)
                        });
                        let y = bounds.center().y;
                        let start = bounds.left() + px(8.);
                        let width = (bounds.size.width - px(16.)).max(px(1.));
                        let x = start + width * displayed as f32;
                        window.paint_quad(
                            fill(
                                Bounds::new(point(start, y - px(1.5)), size(width, px(3.))),
                                p.line,
                            )
                            .corner_radii(px(1.5)),
                        );
                        window.paint_quad(
                            fill(
                                Bounds::new(
                                    point(start, y - px(1.5)),
                                    size((x - start).max(px(0.)), px(3.)),
                                ),
                                p.accent,
                            )
                            .corner_radii(px(1.5)),
                        );
                        window.paint_quad(
                            fill(
                                Bounds::new(
                                    point(x - px(6.5), y - px(5.5)),
                                    size(px(13.), px(13.)),
                                ),
                                rgba(0x00000019),
                            )
                            .corner_radii(px(6.5)),
                        );
                        window.paint_quad(
                            fill(
                                Bounds::new(
                                    point(x - px(6.5), y - px(6.5)),
                                    size(px(13.), px(13.)),
                                ),
                                rgb(0xffffff),
                            )
                            .corner_radii(px(6.5))
                            .border_widths(px(0.5))
                            .border_color(rgba(0x00000026)),
                        );
                        let down_state = gesture.clone();
                        let down_host = host.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble
                                || event.button != MouseButton::Left
                                || !hitbox.is_hovered(window)
                            {
                                return;
                            }
                            down_state.update(cx, |state, _| state.dragging = true);
                            if let Some(value) = update_slider(
                                &down_state,
                                bounds,
                                event.position.x,
                                true,
                                window,
                                cx,
                            ) {
                                down_host.update(cx, |host, cx| host.handle(action(value), cx));
                            }
                            cx.stop_propagation();
                        });
                        let move_state = gesture.clone();
                        let move_host = host.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble || !move_state.read(cx).dragging {
                                return;
                            }
                            if event.pressed_button != Some(MouseButton::Left) {
                                move_state.update(cx, |state, cx| {
                                    state.dragging = false;
                                    cx.notify();
                                });
                                return;
                            }
                            if let Some(value) = update_slider(
                                &move_state,
                                bounds,
                                event.position.x,
                                false,
                                window,
                                cx,
                            ) {
                                move_host.update(cx, |host, cx| host.handle(action(value), cx));
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble
                                || event.button != MouseButton::Left
                                || !gesture.read(cx).dragging
                            {
                                return;
                            }
                            if let Some(value) =
                                update_slider(&gesture, bounds, event.position.x, true, window, cx)
                            {
                                host.update(cx, |host, cx| host.handle(action(value), cx));
                            }
                            gesture.update(cx, |state, cx| {
                                state.dragging = false;
                                cx.notify();
                            });
                            cx.stop_propagation();
                        });
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }

    pub(super) fn macos_preferences(&self, host: &ViewState, p: Palette) -> AnyElement {
        let dark = host.dark();
        let background = rgb(if dark { 0x28282b } else { 0xf5f5f5 });
        let titlebar = rgb(if dark { 0x333336 } else { 0xebe9e8 });
        let separator: Hsla = rgba(if dark { 0xffffff18 } else { 0x00000019 }).into();
        let close_host = self.host.clone();
        let mut lights = div()
            .absolute()
            .left(px(15.))
            .top(px(18.))
            .flex()
            .gap(px(8.));
        lights = lights.child(
            div()
                .id("preferences-close")
                .debug_selector(|| "preferences-close".into())
                .size(px(12.))
                .rounded_full()
                .bg(rgb(0xff5f57))
                .border_1()
                .border_color(rgba(0x9f292c66))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    close_host.update(cx, |host, cx| host.handle(Action::Close, cx))
                }),
        );
        lights = lights.child(
            div()
                .id("preferences-minimize")
                .size(px(12.))
                .rounded_full()
                .bg(rgb(0xfebc2e))
                .border_1()
                .border_color(rgba(0xa66b1466))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, window, _| window.minimize_window()),
        );
        // This fixed-size preferences window has no zoom action.
        lights = lights.child(div().size(px(12.)).rounded_full().bg(rgb(if dark {
            0x626267
        } else {
            0xc8c8cb
        })));
        let header = div()
            .id("preferences-titlebar")
            .h(px(48.))
            .flex_none()
            .relative()
            .bg(titlebar)
            .border_b_1()
            .border_color(separator)
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 1 {
                    window.start_window_move();
                }
            })
            .child(lights)
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(pref_text("桌面与程序坞")),
            );
        let appearance = div()
            .flex()
            .gap(px(18.))
            .child(self.appearance_choice(false, host.prefs.theme == "light", p))
            .child(self.appearance_choice(true, host.prefs.theme != "light", p));
        let mut accents = div().flex().items_center().gap(px(8.));
        for color in [
            "#0a84ff", "#bf5af2", "#ff375f", "#ff453a", "#ff9f0a", "#ffd60a", "#30d158", "#8e8e93",
        ] {
            let selected = host.prefs.accent.eq_ignore_ascii_case(color);
            let fill = u32::from_str_radix(&color[1..], 16).unwrap();
            let h = self.host.clone();
            let id = format!("prefs-accent-{}", &color[1..]);
            let selector = id.clone();
            accents = accents.child(
                div()
                    .id(SharedString::from(id))
                    .debug_selector(move || selector)
                    .size(px(22.))
                    .rounded_full()
                    .p(px(2.))
                    .border_1()
                    .border_color(if selected {
                        p.muted
                    } else {
                        Hsla::transparent_black()
                    })
                    .cursor_pointer()
                    .child(
                        div()
                            .size_full()
                            .rounded_full()
                            .bg(rgb(fill))
                            .border_1()
                            .border_color(rgba(0x00000015)),
                    )
                    .on_click(move |_, _, cx| {
                        h.update(cx, |host, cx| host.handle(Action::Accent(color.into()), cx))
                    }),
            );
        }
        let mut positions = div()
            .w(px(282.))
            .h(px(24.))
            .flex()
            .rounded(px(5.))
            .bg(p.card)
            .border_1()
            .border_color(separator);
        for (key, label, id) in [
            (DockPosition::Left, "左侧", "prefs-position-left"),
            (DockPosition::Bottom, "底部", "prefs-position-bottom"),
            (DockPosition::Right, "右侧", "prefs-position-right"),
        ] {
            let h = self.host.clone();
            let selected = host.prefs.dock_position == key;
            positions = positions.child(
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .text_size(px(12.))
                    .when(selected, |s| s.bg(p.accent).text_color(rgb(0xffffff)))
                    .when(!selected, |s| s.hover(move |s| s.bg(p.line)))
                    .cursor_pointer()
                    .child(pref_text(label))
                    .on_click(move |_, _, cx| {
                        h.update(cx, |host, cx| host.handle(Action::DockPosition(key), cx))
                    }),
            );
        }
        let size_slider = div()
            .w(px(304.))
            .flex()
            .items_center()
            .gap(px(10.))
            .child(div().flex_1().child(self.preference_slider(
                "dock-size",
                (host.prefs.dock_size - 32.) / 32.,
                |v| Action::DockSize(32. + 32. * v),
                p,
            )))
            .child(
                div()
                    .w(px(35.))
                    .text_right()
                    .text_size(px(11.))
                    .text_color(p.muted)
                    .child(pref_text(format!("{:.0} pt", host.prefs.dock_size))),
            );
        let magnification_slider = div()
            .w(px(304.))
            .flex()
            .items_center()
            .gap(px(10.))
            .child(div().flex_1().child(self.preference_slider(
                "magnification-scale",
                host.prefs.magnification_scale - 1.,
                |v| Action::MagnificationScale(1. + v),
                p,
            )))
            .child(
                div()
                    .w(px(35.))
                    .text_right()
                    .text_size(px(11.))
                    .text_color(p.muted)
                    .child(pref_text(format!(
                        "{:.0}%",
                        host.prefs.magnification_scale * 100.
                    ))),
            );
        let bar_slider = div()
            .w(px(304.))
            .flex()
            .items_center()
            .gap(px(10.))
            .child(div().flex_1().child(self.preference_slider(
                "bar-height",
                (host.prefs.bar_height - 22.) / 26.,
                |v| Action::BarHeight(22. + 26. * v),
                p,
            )))
            .child(
                div()
                    .w(px(35.))
                    .text_right()
                    .text_size(px(11.))
                    .text_color(p.muted)
                    .child(pref_text(format!("{:.0} pt", host.prefs.bar_height))),
            );
        let body = div()
            .id("macos-preferences-form")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(27.))
            .pt(px(14.))
            .pb(px(12.))
            .flex()
            .flex_col()
            .child(form_row("外观：", appearance).min_h(px(97.)))
            .child(form_row("强调色：", accents).min_h(px(32.)))
            .child(form_separator(separator))
            .child(form_row("程序坞大小：", size_slider).min_h(px(32.)))
            .child(
                form_row(
                    "放大效果：",
                    self.preference_check(
                        "prefs-magnification",
                        "指针经过时放大图标",
                        host.prefs.magnification,
                        Action::Magnification(!host.prefs.magnification),
                        p,
                    ),
                )
                .min_h(px(30.)),
            )
            .child(form_row("放大比例：", magnification_slider).min_h(px(32.)))
            .child(form_row("屏幕上的位置：", positions).min_h(px(36.)))
            .child(
                form_row(
                    "程序坞：",
                    self.preference_check(
                        "prefs-auto-hide",
                        "自动隐藏和显示程序坞",
                        host.prefs.dock_auto_hide,
                        Action::AutoHide(!host.prefs.dock_auto_hide),
                        p,
                    ),
                )
                .min_h(px(30.)),
            )
            .child(
                form_row(
                    "启动动画：",
                    self.preference_check(
                        "prefs-launch-animation",
                        "启动应用时弹跳图标",
                        host.prefs.animate_opening_applications,
                        Action::AnimateOpeningApplications(
                            !host.prefs.animate_opening_applications,
                        ),
                        p,
                    ),
                )
                .min_h(px(30.)),
            )
            .child(form_separator(separator))
            .child(form_row("菜单栏高度：", bar_slider).min_h(px(32.)))
            .child(
                form_row(
                    "辅助功能：",
                    div()
                        .flex()
                        .flex_col()
                        .child(self.preference_check(
                            "prefs-reduce-motion",
                            "减少动态效果",
                            host.prefs.reduce_motion,
                            Action::ReduceMotion(!host.prefs.reduce_motion),
                            p,
                        ))
                        .child(
                            div()
                                .pl(px(21.))
                                .text_size(px(11.))
                                .text_color(p.muted)
                                .child(pref_text("减少窗口与工作区的动态效果")),
                        ),
                )
                .min_h(px(47.)),
            );
        let footer = div()
            .h(px(49.))
            .flex_none()
            .border_t_1()
            .border_color(separator)
            .px(px(22.))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(p.muted)
                    .child(pref_text("更改会自动保存并应用")),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(self.preference_link(
                        "prefs-wallpaper",
                        "更换壁纸…",
                        Action::PickWallpaper,
                        dark,
                        p,
                    ))
                    .child(self.preference_link(
                        "prefs-advanced",
                        "高级设置…",
                        Action::Settings,
                        dark,
                        p,
                    )),
            );
        div()
            .id("macos-preferences")
            .size_full()
            .rounded(px(10.))
            .overflow_hidden()
            .bg(background)
            .text_color(p.fg)
            .text_size(px(13.))
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .child(footer)
            .into_any_element()
    }

    fn preference_link(
        &self,
        id: &'static str,
        label: &'static str,
        action: Action,
        dark: bool,
        p: Palette,
    ) -> AnyElement {
        let h = self.host.clone();
        div()
            .id(id)
            .px(px(11.))
            .h(px(23.))
            .rounded(px(5.))
            .border_1()
            .border_color(p.line)
            .bg(rgb(if dark { 0x454549 } else { 0xffffff }))
            .text_size(px(12.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(move |s| s.bg(p.card))
            .child(pref_text(label))
            .on_click(move |_, _, cx| h.update(cx, |host, cx| host.handle(action.clone(), cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use crate::app::{testing, Panel};
    use gpui::{
        point, px, size, Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext,
        VisualTestContext,
    };
    use std::collections::HashMap;

    #[gpui::test]
    fn preferences_controls_apply_real_values_and_close_the_actual_window(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private").to_string_lossy().into_owned();
        let environment: HashMap<String, String> = [
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            ("XDG_CONFIG_HOME".into(), private.clone()),
            ("XDG_CONFIG_DIRS".into(), private.clone()),
            ("XDG_DATA_HOME".into(), private.clone()),
            ("XDG_DATA_DIRS".into(), private),
        ]
        .into();
        let catalog = clarion_launcher::catalog::AppCatalog::load(&environment).unwrap();
        cx.update(crate::view::bind_keys);
        let host = cx.update(|cx| testing::test_host(cx, catalog, environment));
        let window = cx.update(|cx| testing::open_panel(&host, Panel::Preferences, cx));
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.update(|window, _| {
            window.resize(size(px(640.), px(520.)));
            window.activate_window();
            window.refresh();
        });
        visual.run_until_parked();
        let scale = visual
            .debug_bounds("slider-magnification-scale")
            .expect("actual magnification slider");
        visual.simulate_click(
            point(
                scale.left() + px(8.) + (scale.size.width - px(16.)) * 0.75,
                scale.center().y,
            ),
            Modifiers::default(),
        );
        visual.run_until_parked();
        host.read_with(&visual, |host, _| {
            assert!((host.prefs.magnification_scale - 1.75).abs() < 0.001)
        });
        for id in [
            "prefs-appearance-light",
            "prefs-accent-bf5af2",
            "prefs-magnification",
            "prefs-launch-animation",
            "prefs-reduce-motion",
            "prefs-position-right",
            "prefs-auto-hide",
        ] {
            let bounds = visual
                .debug_bounds(id)
                .unwrap_or_else(|| panic!("missing actual control: {id}"));
            if bounds.center().y > px(460.) {
                visual.simulate_event(ScrollWheelEvent {
                    position: point(px(320.), px(240.)),
                    delta: ScrollDelta::Pixels(point(px(0.), px(-500.))),
                    ..Default::default()
                });
                visual.run_until_parked();
            }
            let bounds = visual.debug_bounds(id).expect("control after scrolling");
            assert!(bounds.center().y > px(48.) && bounds.center().y < px(471.));
            visual.simulate_click(bounds.center(), Modifiers::default());
            visual.run_until_parked();
            host.read_with(&visual, |host, _| match id {
                "prefs-appearance-light" => assert_eq!(host.prefs.theme, "light"),
                "prefs-accent-bf5af2" => assert_eq!(host.prefs.accent, "#bf5af2"),
                "prefs-magnification" => assert!(!host.prefs.magnification),
                "prefs-launch-animation" => assert!(!host.prefs.animate_opening_applications),
                "prefs-reduce-motion" => assert!(host.prefs.reduce_motion),
                "prefs-position-right" => assert_eq!(
                    host.prefs.dock_position,
                    crate::preferences::DockPosition::Right
                ),
                "prefs-auto-hide" => assert!(host.prefs.dock_auto_hide),
                _ => unreachable!(),
            });
        }
        for (value, expected) in [(0., 1.), (3., 2.), (f64::NAN, 2.), (1.45, 1.45)] {
            host.update(&mut visual, |host, cx| {
                host.handle(crate::app::Action::MagnificationScale(value), cx);
            });
            visual.run_until_parked();
            host.read_with(&visual, |host, _| {
                assert_eq!(host.prefs.magnification_scale, expected)
            });
        }
        let close = visual
            .debug_bounds("preferences-close")
            .expect("real close control");
        visual.simulate_click(close.center(), Modifiers::default());
        visual.run_until_parked();
        host.read_with(cx, |host, _| assert!(host.panel.is_none()));
        assert!(!cx.read(|cx| cx
            .windows()
            .iter()
            .any(|handle| handle.window_id() == window.window_id())));
    }
}
