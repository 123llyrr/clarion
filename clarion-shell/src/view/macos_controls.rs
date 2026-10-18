// SPDX-License-Identifier: GPL-3.0-or-later
//! Compact Control Center content. The popup owns its outer glass and shadow.
use super::*;

#[derive(IntoElement)]
struct ControlText {
    text: String,
    size: f32,
    weight: FontWeight,
    color: Hsla,
}

impl RenderOnce for ControlText {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .text_size(px(self.size))
            .line_height(px(self.size + 4.))
            .whitespace_nowrap()
            .child(menu_text(self.text, self.weight, self.color, cx))
    }
}

fn text(text: impl Into<String>, size: f32, weight: FontWeight, color: Hsla) -> ControlText {
    ControlText {
        text: text.into(),
        size,
        weight,
        color,
    }
}

fn card(dark: bool) -> Div {
    div()
        .rounded(px(11.))
        .border_1()
        .border_color(if dark {
            rgba(0xffffff16)
        } else {
            rgba(0xffffff80)
        })
        .bg(linear_gradient(
            180.,
            linear_color_stop(
                if dark {
                    rgba(0xffffff13)
                } else {
                    rgba(0xffffff9c)
                },
                0.,
            ),
            linear_color_stop(
                if dark {
                    rgba(0xffffff09)
                } else {
                    rgba(0xffffff59)
                },
                1.,
            ),
        ))
        .shadow(vec![BoxShadow {
            color: rgba(if dark { 0x0000001c } else { 0x0000000b }).into(),
            offset: point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
        }])
}

fn percentage(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map(|value| format!("{}%", (value.clamp(0., 1.) * 100.).round()))
        .unwrap_or_else(|| "不可用".into())
}

// The shared gesture uses an eight-point end inset. Another five points keep
// the capsule's 26-point handle inside its bounds at both limits.
fn gesture_bounds(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::new(
        point(bounds.left() + px(5.), bounds.top()),
        size(
            (bounds.size.width - px(10.)).max(px(1.)),
            bounds.size.height,
        ),
    )
}

impl ShellView {
    pub(super) fn macos_controls(&self, host: &ViewState, p: Palette) -> AnyElement {
        let dark = host.dark();
        let output = self.output.clone();
        let brightness = host
            .desktop
            .outputs
            .get(&output)
            .filter(|output| output.hdr_active)
            .and_then(|output| output.hdr_brightness)
            .map(|value| f64::from(value) / 100.)
            .or(host.system.brightness);
        let mut footer = card(dark).w_full().p(px(4.)).flex().flex_col();
        for (index, (id, label, icon, action)) in [
            (
                "controls-desktop",
                "桌面与 Dock",
                "display",
                Action::Panel(Panel::Preferences),
            ),
            (
                "controls-settings",
                "系统设置",
                "settings",
                Action::Settings,
            ),
            (
                "controls-wallpaper",
                "更换壁纸",
                "wallpaper",
                Action::PickWallpaper,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if index != 0 {
                footer = footer.child(div().ml(px(32.)).mr(px(8.)).h(px(1.)).bg(if dark {
                    rgba(0xffffff0f)
                } else {
                    rgba(0x0000000b)
                }));
            }
            footer = footer.child(self.control_entry(id, label, icon, action, p));
        }

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .w_full()
                    .flex()
                    .gap(px(8.))
                    .child(self.control_tile(
                        "controls-dnd",
                        "勿扰模式",
                        if host.dnd { "已开启" } else { "已关闭" },
                        "moon",
                        host.dnd,
                        Action::DoNotDisturb(!host.dnd),
                        dark,
                        p,
                    ))
                    .child(self.control_tile(
                        "controls-screenshot",
                        "屏幕截图",
                        "选择截取区域",
                        "scan",
                        false,
                        Action::Panel(Panel::Screenshot),
                        dark,
                        p,
                    )),
            )
            .child(self.control_slider(
                "brightness",
                "显示器",
                "sun",
                brightness,
                None,
                move |value| {
                    Action::Backend(backend::Command::SetOutputBrightness {
                        output: output.clone(),
                        value,
                    })
                },
                dark,
                p,
            ))
            .child(self.control_slider(
                "volume",
                "声音",
                if host.system.muted {
                    "speaker-muted"
                } else {
                    "speaker"
                },
                host.system.volume,
                Some((
                    host.system.muted,
                    Action::Backend(backend::Command::SetMuted(!host.system.muted)),
                )),
                |value| Action::Backend(backend::Command::SetVolume(value)),
                dark,
                p,
            ))
            .child(footer)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn control_tile(
        &self,
        id: &'static str,
        label: &'static str,
        status: &'static str,
        icon: &'static str,
        selected: bool,
        action: Action,
        dark: bool,
        p: Palette,
    ) -> AnyElement {
        let host = self.host.clone();
        let keyboard_host = self.host.clone();
        let keyboard_action = action.clone();
        let icon_color = if selected { rgb(0xffffff).into() } else { p.fg };
        card(dark)
            .id(id)
            .flex_1()
            .min_w_0()
            .h(px(80.))
            .px(px(11.))
            .flex()
            .items_center()
            .gap(px(8.))
            .focusable()
            .key_context("ClarionControlButton")
            .debug_selector(move || id.into())
            .cursor_pointer()
            .hover(move |style| {
                style.bg(if dark {
                    rgba(0xffffff20)
                } else {
                    rgba(0xffffffba)
                })
            })
            .focus(move |style| style.border_color(p.accent))
            .child(
                div()
                    .size(px(30.))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(if selected {
                        rgb(0x7761c5)
                    } else if dark {
                        rgba(0xffffff20)
                    } else {
                        rgba(0xffffffb3)
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(self.symbol(
                        icon,
                        17.,
                        Palette {
                            fg: icon_color,
                            ..p
                        },
                    )),
            )
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(text(label, 13., FontWeight::MEDIUM, p.fg))
                    .child(text(status, 11., FontWeight::NORMAL, p.muted)),
            )
            .on_click(move |_, _, cx| host.update(cx, |host, cx| host.handle(action.clone(), cx)))
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
                    keyboard_host.update(cx, |host, cx| host.handle(keyboard_action.clone(), cx));
                    cx.stop_propagation();
                }
            })
            .into_any_element()
    }

    fn control_entry(
        &self,
        id: &'static str,
        label: &'static str,
        icon: &'static str,
        action: Action,
        p: Palette,
    ) -> AnyElement {
        let host = self.host.clone();
        let keyboard_host = self.host.clone();
        let keyboard_action = action.clone();
        div()
            .id(id)
            .h(px(31.))
            .px(px(8.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .gap(px(9.))
            .focusable()
            .key_context("ClarionControlButton")
            .debug_selector(move || id.into())
            .cursor_pointer()
            .hover(move |style| style.bg(p.line))
            .focus(move |style| style.bg(p.line))
            .child(self.symbol(icon, 16., p))
            .child(text(label, 12., FontWeight::NORMAL, p.fg))
            .on_click(move |_, _, cx| host.update(cx, |host, cx| host.handle(action.clone(), cx)))
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
                    keyboard_host.update(cx, |host, cx| host.handle(keyboard_action.clone(), cx));
                    cx.stop_propagation();
                }
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn control_slider(
        &self,
        id: &'static str,
        label: &'static str,
        icon: &'static str,
        value: Option<f64>,
        mute: Option<(bool, Action)>,
        action: impl Fn(f64) -> Action + Clone + 'static,
        dark: bool,
        p: Palette,
    ) -> AnyElement {
        let value = value
            .filter(|value| value.is_finite())
            .map(|value| value.clamp(0., 1.));
        let mut trailing = div().flex().items_center().gap(px(8.)).child(text(
            percentage(value),
            11.,
            FontWeight::NORMAL,
            p.muted,
        ));
        if let Some((muted, mute_action)) = mute.filter(|_| value.is_some()) {
            let host = self.host.clone();
            let keyboard_host = self.host.clone();
            let keyboard_action = mute_action.clone();
            trailing = trailing.child(
                div()
                    .id("controls-mute")
                    .debug_selector(|| "controls-mute".into())
                    .h(px(20.))
                    .px(px(5.))
                    .rounded(px(5.))
                    .focusable()
                    .key_context("ClarionControlButton")
                    .cursor_pointer()
                    .hover(move |style| style.bg(p.line))
                    .focus(move |style| style.bg(p.line))
                    .child(text(
                        if muted { "取消静音" } else { "静音" },
                        11.,
                        FontWeight::NORMAL,
                        p.fg,
                    ))
                    .on_click(move |_, _, cx| {
                        host.update(cx, |host, cx| host.handle(mute_action.clone(), cx))
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
            );
        }
        let body = card(dark)
            .h(px(86.))
            .w_full()
            .px(px(12.))
            .py(px(11.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .h(px(19.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(text(label, 13., FontWeight::SEMIBOLD, p.fg))
                    .child(trailing),
            );
        let Some(value) = value else {
            return body
                .child(
                    div()
                        .h(px(26.))
                        .w_full()
                        .rounded_full()
                        .bg(if dark {
                            rgba(0x00000022)
                        } else {
                            rgba(0x00000010)
                        })
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .child(self.symbol(icon, 15., Palette { fg: p.muted, ..p })),
                )
                .into_any_element();
        };
        body.child(ControlSlider {
            key: SharedString::from(format!("control-slider-{id}")),
            selector: format!("slider-{id}"),
            host: self.host.clone(),
            action: std::rc::Rc::new(action),
            value,
            dark,
            p,
            icon: self.symbol(
                icon,
                15.,
                Palette {
                    fg: rgb(0x707077).into(),
                    ..p
                },
            ),
        })
        .into_any_element()
    }
}

#[derive(IntoElement)]
struct ControlSlider {
    key: SharedString,
    selector: String,
    host: Entity<Host>,
    action: std::rc::Rc<dyn Fn(f64) -> Action>,
    value: f64,
    dark: bool,
    p: Palette,
    icon: AnyElement,
}

#[allow(clippy::too_many_arguments)]
fn step_slider(
    gesture: &Entity<SliderGesture>,
    value: f64,
    step: f64,
    host: &Entity<Host>,
    action: &std::rc::Rc<dyn Fn(f64) -> Action>,
    window: &mut Window,
    cx: &mut App,
) {
    let next = gesture.update(cx, |state, cx| {
        let current = state.preview.map(|(value, _)| value).unwrap_or(value);
        let next = (current + step).clamp(0., 1.);
        let now = std::time::Instant::now();
        state.preview = Some((next, now));
        state.last_sent = Some(now);
        state.last_sent_value = Some(next);
        cx.notify();
        next
    });
    host.update(cx, |host, cx| host.handle(action(next), cx));
    window.refresh();
}

impl RenderOnce for ControlSlider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            key,
            selector,
            host,
            action,
            value,
            dark,
            p,
            icon,
        } = self;
        // RenderOnce runs in GPUI's prepaint/layout phase; state cannot be
        // created later from a keyboard callback or on a different canvas node.
        let gesture = window.use_keyed_state(key.clone(), cx, |_, _| SliderGesture::default());
        let focus_state =
            window.use_keyed_state(SharedString::from(format!("{key}-focus")), cx, |_, cx| {
                cx.focus_handle()
            });
        let focus = focus_state.read(cx).clone();
        div()
            .id(key.clone())
            .relative()
            .h(px(26.))
            .w_full()
            .track_focus(&focus)
            .rounded_full()
            .focus(move |style| {
                style.shadow(vec![BoxShadow {
                    color: p.accent.opacity(0.55),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(2.),
                    spread_radius: px(2.),
                }])
            })
            .key_context("ClarionControlSlider")
            .on_action({
                let gesture = gesture.clone();
                let host = host.clone();
                let action = action.clone();
                move |_: &Previous, window, cx| {
                    step_slider(&gesture, value, 0.05, &host, &action, window, cx);
                    cx.stop_propagation();
                }
            })
            .on_action({
                let gesture = gesture.clone();
                let host = host.clone();
                let action = action.clone();
                move |_: &Next, window, cx| {
                    step_slider(&gesture, value, -0.05, &host, &action, window, cx);
                    cx.stop_propagation();
                }
            })
            .on_key_down({
                let gesture = gesture.clone();
                let host = host.clone();
                let action = action.clone();
                move |event, window, cx| {
                    if event.keystroke.modifiers != Modifiers::default() {
                        return;
                    }
                    let step = match event.keystroke.key.as_str() {
                        "left" => -0.05,
                        "right" => 0.05,
                        _ => return,
                    };
                    step_slider(&gesture, value, step, &host, &action, window, cx);
                    cx.stop_propagation();
                }
            })
            .cursor_pointer()
            .debug_selector(move || selector)
            .child(
                canvas(
                    move |bounds, window, _| {
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
                        let interaction = gesture_bounds(bounds);
                        let start = interaction.left() + px(8.);
                        let width = (interaction.size.width - px(16.)).max(px(1.));
                        let thumb_x = start + width * displayed as f32;
                        window.paint_quad(
                            fill(
                                bounds,
                                if dark {
                                    rgba(0x00000040)
                                } else {
                                    rgba(0x58586229)
                                },
                            )
                            .corner_radii(px(13.)),
                        );
                        window.paint_quad(
                            fill(
                                Bounds::new(
                                    bounds.origin,
                                    size(
                                        (thumb_x - bounds.left() + px(13.)).min(bounds.size.width),
                                        bounds.size.height,
                                    ),
                                ),
                                linear_gradient(
                                    180.,
                                    linear_color_stop(rgba(0xfffffff5), 0.),
                                    linear_color_stop(rgba(0xffffffe0), 1.),
                                ),
                            )
                            .corner_radii(px(13.)),
                        );
                        let thumb = Bounds::new(
                            point(thumb_x - px(12.), bounds.top() + px(1.)),
                            size(px(24.), px(24.)),
                        );
                        window.paint_shadows(
                            thumb,
                            Corners::all(px(12.)),
                            &[BoxShadow {
                                color: rgba(0x00000024).into(),
                                offset: point(px(0.), px(1.)),
                                blur_radius: px(3.),
                                spread_radius: px(0.),
                            }],
                        );
                        window.paint_quad(
                            fill(thumb, rgb(0xffffff))
                                .corner_radii(px(12.))
                                .border_widths(px(0.5))
                                .border_color(rgba(0x00000012)),
                        );
                        // The focused outer Div owns keyboard dispatch. The canvas
                        // only draws the track and manages pointer capture.
                        let down_state = gesture.clone();
                        let down_host = host.clone();
                        let down_action = action.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble
                                || event.button != MouseButton::Left
                                || !hitbox.is_hovered(window)
                            {
                                return;
                            }
                            window.focus(&focus);
                            window.prevent_default();
                            down_state.update(cx, |state, _| state.dragging = true);
                            if let Some(value) = update_slider(
                                &down_state,
                                interaction,
                                event.position.x,
                                true,
                                window,
                                cx,
                            ) {
                                down_host
                                    .update(cx, |host, cx| host.handle(down_action(value), cx));
                            }
                            cx.stop_propagation();
                        });
                        let move_state = gesture.clone();
                        let move_host = host.clone();
                        let move_action = action.clone();
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
                                interaction,
                                event.position.x,
                                false,
                                window,
                                cx,
                            ) {
                                move_host
                                    .update(cx, |host, cx| host.handle(move_action(value), cx));
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble
                                || event.button != MouseButton::Left
                                || !gesture.read(cx).dragging
                            {
                                return;
                            }
                            if let Some(value) = update_slider(
                                &gesture,
                                interaction,
                                event.position.x,
                                true,
                                window,
                                cx,
                            ) {
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
            .child(div().absolute().left(px(8.)).top(px(5.5)).child(icon))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing;
    use ::core::prelude::v1::test;
    use clarion_launcher::catalog::AppCatalog;
    use std::collections::HashMap;

    fn private_host(cx: &mut TestAppContext) -> (tempfile::TempDir, Entity<Host>) {
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
        let catalog = AppCatalog::load(&environment).unwrap();
        cx.update(bind_keys);
        let host = cx.update(|cx| testing::test_host(cx, catalog, environment));
        (root, host)
    }

    fn activate(visual: &mut VisualTestContext, width: f32, height: f32) {
        visual.update(|window, _| {
            window.resize(size(px(width), px(height)));
            window.activate_window();
            window.refresh();
        });
        visual.run_until_parked();
    }

    // Focus through the real pointer path, cancelling only the subsequent click.
    fn focus_without_click(visual: &mut VisualTestContext, selector: &'static str) {
        let bounds = visual
            .debug_bounds(selector)
            .expect("actual control bounds");
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(
            point(px(-20.), px(-20.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
    }

    #[gpui::test]
    fn controls_tile_enter_and_space_update_real_dnd_after_pointer_focus(cx: &mut TestAppContext) {
        let (_root, host) = private_host(cx);
        let window = cx.update(|cx| testing::open_panel(&host, Panel::Controls, cx));
        let mut visual = VisualTestContext::from_window(*window, cx);
        activate(&mut visual, 320., 424.);
        focus_without_click(&mut visual, "controls-dnd");
        host.read_with(cx, |host, _| assert!(!host.dnd));
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        host.read_with(cx, |host, _| {
            assert!(host.dnd, "Enter must reach the tile's Activate handler");
            assert_eq!(host.panel, Some(Panel::Controls));
        });
        visual.simulate_keystrokes("space");
        visual.run_until_parked();
        host.read_with(cx, |host, _| {
            assert!(!host.dnd, "Space must toggle the same focused tile")
        });
    }

    #[gpui::test]
    fn controls_tile_and_entry_enter_open_the_actual_target_windows(cx: &mut TestAppContext) {
        let (_root, host) = private_host(cx);
        for (selector, target) in [
            ("controls-screenshot", Panel::Screenshot),
            ("controls-desktop", Panel::Preferences),
        ] {
            let window = cx.update(|cx| testing::open_panel(&host, Panel::Controls, cx));
            let mut visual = VisualTestContext::from_window(*window, cx);
            activate(&mut visual, 320., 424.);
            focus_without_click(&mut visual, selector);
            host.read_with(cx, |host, _| assert_eq!(host.panel, Some(Panel::Controls)));
            visual.simulate_keystrokes("enter");
            cx.run_until_parked();
            host.read_with(cx, |host, _| assert_eq!(host.panel.as_ref(), Some(&target)));
            assert!(
                !cx.read(|cx| cx
                    .windows()
                    .iter()
                    .any(|handle| handle.window_id() == window.window_id())),
                "Enter must replace the real popup, not merely change a selection"
            );
        }
    }

    struct SliderHarness {
        view: Entity<ShellView>,
        host: Entity<Host>,
        focus: FocusHandle,
    }

    impl Render for SliderHarness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let shell = self.view.read(cx);
            let value = (self.host.read(cx).prefs.bar_height - 22.) / 26.;
            div()
                .w(px(296.))
                .h(px(110.))
                .key_context("ClarionPanel")
                .track_focus(&self.focus)
                .on_action(|_: &Previous, _, _| panic!("Up escaped focused slider"))
                .on_action(|_: &Next, _, _| panic!("Down escaped focused slider"))
                .child(shell.control_slider(
                    "keyboard-regression",
                    "测试亮度",
                    "sun",
                    Some(value),
                    None,
                    |value| Action::BarHeight(22. + 26. * value),
                    true,
                    Palette::new(&shell.state),
                ))
        }
    }

    #[gpui::test]
    fn controls_slider_pointer_focus_and_arrows_change_host_values_across_redraws(
        cx: &mut TestAppContext,
    ) {
        let (_root, host) = private_host(cx);
        let controls = cx.update(|cx| testing::open_panel(&host, Panel::Controls, cx));
        let view = controls.root(cx).unwrap();
        // The production slider receives a real Host action with a numeric
        // result, without enabling a host audio/backlight tool in this test.
        let (harness, visual) = cx.add_window_view(|window, cx| {
            let focus = cx.focus_handle();
            window.focus(&focus);
            cx.observe(&host, |_, _, cx| cx.notify()).detach();
            SliderHarness {
                view,
                host: host.clone(),
                focus,
            }
        });
        activate(visual, 320., 160.);
        let bounds = visual
            .debug_bounds("slider-keyboard-regression")
            .expect("actual capsule track");
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.run_until_parked();
        host.read_with(visual, |host, _| {
            assert!((host.prefs.bar_height - 35.).abs() < 0.001)
        });
        let focused = visual.update(|window, cx| {
            window
                .focused(cx)
                .expect("track click must focus the slider")
        });
        let root_focus = harness.read_with(visual, |harness, _| harness.focus.clone());
        assert_ne!(
            focused, root_focus,
            "canvas stop_propagation must not leave focus on the ancestor"
        );
        for (key, expected) in [("right", 36.3), ("up", 37.6), ("left", 36.3), ("down", 35.)] {
            visual.simulate_keystrokes(key);
            visual.run_until_parked();
            host.read_with(visual, |host, _| {
                assert!(
                    (host.prefs.bar_height - expected).abs() < 0.001,
                    "{key} did not reach the focused capsule"
                )
            });
            assert_eq!(
                visual.update(|window, cx| window.focused(cx)),
                Some(focused.clone()),
                "slider focus must survive the action's Host redraw"
            );
        }
        visual.simulate_mouse_move(
            point(bounds.right() + px(20.), bounds.center().y),
            None,
            Modifiers::default(),
        );
        host.read_with(visual, |host, _| {
            assert!(
                (host.prefs.bar_height - 35.).abs() < 0.001,
                "keyboard steps must not leave a pointer drag active"
            )
        });
    }

    #[::core::prelude::v1::test]
    fn capsule_gesture_matches_handle_endpoints() {
        let bounds = Bounds::new(point(px(10.), px(20.)), size(px(260.), px(26.)));
        let interaction = gesture_bounds(bounds);
        assert_eq!(slider_value(interaction, px(23.)), 0.);
        assert_eq!(slider_value(interaction, px(257.)), 1.);
        assert_eq!(slider_value(interaction, px(140.)), 0.5);
        assert_eq!(slider_value(interaction, px(-100.)), 0.);
        assert_eq!(slider_value(interaction, px(500.)), 1.);
    }

    #[::core::prelude::v1::test]
    fn unavailable_and_invalid_values_are_not_fake_live_percentages() {
        assert_eq!(percentage(None), "不可用");
        assert_eq!(percentage(Some(f64::NAN)), "不可用");
        assert_eq!(percentage(Some(0.537)), "54%");
        assert_eq!(percentage(Some(-1.)), "0%");
        assert_eq!(percentage(Some(2.)), "100%");
    }
}
