//! Compact, keyboard-accessible controls shared by the common preference panes.

use std::hash::Hash;
use std::ops::RangeInclusive;

use eframe::egui::{
    self, Align, Color32, Frame, Layout, Margin, Response, RichText, Rounding, Sense, Stroke, Ui,
    WidgetInfo, WidgetType,
};

use crate::theme::colors;

const CONTROL_WIDTH: f32 = 236.;
const WIDE_ROW: f32 = 620.;

/// Explicit save/close must commit the latest numeric text after this frame's
/// input has been processed. Ordinary editing keeps partial numbers local.
pub fn set_numeric_commit(context: &egui::Context, commit: bool) {
    context.data_mut(|data| data.insert_temp(egui::Id::new("common-numeric-commit"), commit));
}

pub fn page(
    ui: &mut Ui,
    id: impl Hash,
    title: &str,
    description: &str,
    body: impl FnOnce(&mut Ui),
) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.add_space(4.);
            ui.label(
                RichText::new(title)
                    .size(24.)
                    .strong()
                    .color(colors::text()),
            );
            ui.add_space(4.);
            ui.add(
                egui::Label::new(RichText::new(description).size(12.5).color(colors::muted()))
                    .wrap(),
            );
            ui.add_space(20.);
            body(ui);
            ui.add_space(20.);
        });
}

pub fn section(ui: &mut Ui, id: impl Hash, title: &str, body: impl FnOnce(&mut Ui)) {
    ui.push_id(id, |ui| {
        ui.label(
            RichText::new(title)
                .size(13.)
                .strong()
                .color(colors::muted()),
        );
        ui.add_space(6.);
        Frame::none()
            .fill(colors::surface())
            .stroke(Stroke::new(1.0_f32, colors::border().gamma_multiply(0.45)))
            .rounding(Rounding::same(12.))
            .inner_margin(Margin::symmetric(16., 8.))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                body(ui);
            });
        ui.add_space(20.);
    });
}

pub fn separator(ui: &mut Ui) {
    ui.add_space(4.);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0_f32, colors::border().gamma_multiply(0.28)),
    );
    ui.add_space(4.);
}

fn copy(ui: &mut Ui, title: &str, description: &str) {
    ui.add(egui::Label::new(RichText::new(title).size(13.5).color(colors::text())).wrap());
    if !description.is_empty() {
        ui.add_space(2.);
        ui.add(
            egui::Label::new(RichText::new(description).size(11.5).color(colors::muted())).wrap(),
        );
    }
}

/// A switch with native egui focus/activation and an explicit accessibility label.
/// Only its appearance is custom: Space, Enter and accessibility actions follow
/// the same `Sense::click` path used by egui's standard checkbox.
fn switch(ui: &mut Ui, title: &str, description: &str, value: &mut bool) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(42., 24.), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *value, title));
    if ui.is_rect_visible(rect) {
        let fraction = ui.ctx().animate_bool_responsive(response.id, *value);
        let inactive = if colors::is_dark() {
            Color32::from_rgb(92, 92, 97)
        } else {
            Color32::from_rgb(188, 190, 195)
        };
        // Interpolating the track itself avoids an abrupt colour change while
        // keeping the thumb's travel tied to the same visual state.
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * fraction).round() as u8;
        let accent = colors::control_accent();
        let track = Color32::from_rgb(
            mix(inactive.r(), accent.r()),
            mix(inactive.g(), accent.g()),
            mix(inactive.b(), accent.b()),
        );
        let opacity = if ui.is_enabled() { 1. } else { 0.4 };
        ui.painter()
            .rect_filled(rect, 12., track.gamma_multiply(opacity));
        let center = egui::pos2(rect.left() + 12. + fraction * 18., rect.center().y);
        ui.painter().circle_filled(
            center + egui::vec2(0., 0.5),
            9.5,
            Color32::from_black_alpha((28. * opacity) as u8),
        );
        ui.painter()
            .circle_filled(center, 9., Color32::WHITE.gamma_multiply(opacity));
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.expand(3.),
                15.,
                Stroke::new(2.0_f32, colors::control_accent()),
            );
        }
    }
    response.on_hover_text(format!(
        "{title}：{}{}{}",
        if *value { "开启" } else { "关闭" },
        if description.is_empty() { "" } else { "\n" },
        description
    ))
}

pub fn toggle(ui: &mut Ui, id: impl Hash, title: &str, description: &str, value: &mut bool) {
    ui.push_id(id, |ui| {
        ui.add_space(6.);
        let width = ui.available_width();
        let label_width = (width - 58.).max(1.);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(label_width, 28.),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_min_width(label_width);
                    copy(ui, title, description);
                },
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                switch(ui, title, description, value);
            });
        });
        ui.add_space(6.);
    });
}

/// At narrow widths the control moves below its explanation. This keeps long
/// translated labels and exact numeric entry usable without horizontal scroll.
pub fn control_row(
    ui: &mut Ui,
    id: impl Hash,
    title: &str,
    description: &str,
    control: impl FnOnce(&mut Ui),
) {
    ui.push_id(id, |ui| {
        ui.add_space(7.);
        let width = ui.available_width();
        if width >= WIDE_ROW {
            let label_width = width - CONTROL_WIDTH - 20.;
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(label_width, 34.),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_min_width(label_width);
                        copy(ui, title, description);
                    },
                );
                ui.add_space(12.);
                ui.allocate_ui_with_layout(
                    egui::vec2(CONTROL_WIDTH, 34.),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_min_width(CONTROL_WIDTH);
                        control(ui);
                    },
                );
            });
        } else {
            copy(ui, title, description);
            ui.add_space(8.);
            control(ui);
        }
        ui.add_space(7.);
    });
}

#[allow(clippy::too_many_arguments)]
pub fn slider<Num: egui::emath::Numeric>(
    ui: &mut Ui,
    id: impl Hash,
    title: &str,
    description: &str,
    value: &mut Num,
    range: RangeInclusive<Num>,
    suffix: &str,
    decimals: usize,
) {
    control_row(ui, id, title, description, |ui| {
        // Reserve the right-hand value editor even for short sliders. An
        // existing custom value outside the convenient drag range is preserved
        // until the user actually edits it.
        ui.spacing_mut().slider_width = (ui.available_width() - 88.).clamp(40., 190.);
        let explicit_commit = ui.ctx().data(|data| {
            data.get_temp::<bool>(egui::Id::new("common-numeric-commit"))
                .unwrap_or(false)
        });
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::Slider::new(value, range.clone())
                    .clamping(egui::SliderClamping::Edits)
                    .fixed_decimals(decimals)
                    .show_value(false),
            );
            response.widget_info(|| WidgetInfo::slider(ui.is_enabled(), value.to_f64(), title));
            // Slider's built-in value editor commits each parsed keystroke.
            // A separate DragValue keeps a partial number local until Enter or
            // blur, so an autosave cannot interrupt a slow numeric edit.
            // Ctrl+S alone does not emit a TextEdit changed event. Commit with
            // Enter after this frame's queued text, letting egui parse both
            // earlier edits and text received in the same frame as Save/Close.
            let editor_id = ui.next_auto_id();
            if explicit_commit && ui.memory(|memory| memory.has_focus(editor_id)) {
                ui.input_mut(|input| {
                    input.events.push(egui::Event::Key {
                        key: egui::Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    })
                });
            }
            let response = ui.add(
                egui::DragValue::new(value)
                    .range(range)
                    .clamp_existing_to_range(false)
                    .speed(10.0_f64.powi(-(decimals as i32)))
                    .fixed_decimals(decimals)
                    .suffix(suffix)
                    .update_while_editing(false),
            );
            response.widget_info(|| {
                let mut info = WidgetInfo::drag_value(ui.is_enabled(), value.to_f64());
                info.label = Some(title.to_owned());
                info
            });
        });
    });
}

pub fn choices(
    ui: &mut Ui,
    id: impl Hash,
    title: &str,
    description: &str,
    value: &mut String,
    choices: &[(&str, &str)],
) {
    choices_with_state(ui, id, title, description, value, choices, None);
}

/// Returns an explicit selection intent even when the chosen string already
/// matches the stored fallback. This matters when a custom shader currently
/// overrides that fallback: selecting the built-in "fade" is still an edit.
#[allow(clippy::too_many_arguments)]
pub fn choices_with_state(
    ui: &mut Ui,
    id: impl Hash,
    title: &str,
    description: &str,
    value: &mut String,
    choices: &[(&str, &str)],
    selected_override: Option<&str>,
) -> bool {
    let mut chosen = false;
    control_row(ui, id, title, description, |ui| {
        let selected = selected_override.unwrap_or_else(|| {
            choices
                .iter()
                .find(|(key, _)| *key == value.as_str())
                .map_or("自定义", |(_, label)| *label)
        });
        let response = egui::ComboBox::from_id_salt("choice")
            .width(ui.available_width().min(CONTROL_WIDTH))
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for (key, label) in choices {
                    if ui
                        .selectable_label(
                            selected_override.is_none() && *key == value.as_str(),
                            *label,
                        )
                        .clicked()
                    {
                        *value = (*key).to_owned();
                        chosen = true;
                    }
                }
            });
        response
            .response
            .widget_info(|| WidgetInfo::labeled(WidgetType::ComboBox, ui.is_enabled(), title));
    });
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_text_waits_for_enter_before_changing_the_model() {
        fn frame(
            context: &egui::Context,
            value: &mut f64,
            time: f64,
            events: Vec<egui::Event>,
        ) -> egui::FullOutput {
            context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(480., 500.),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        slider(ui, "duration", "动画时长", "", value, 0.25..=2., " ×", 2);
                    });
                },
            )
        }
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let context = egui::Context::default();
        let mut value = 1.;
        let output = frame(&context, &mut value, 0., vec![]);
        let center = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "1.00 ×" => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
                }
                _ => None,
            })
            .expect("visible precise value editor");
        for (time, pressed) in [(0.01, true), (0.02, false)] {
            frame(
                &context,
                &mut value,
                time,
                vec![
                    egui::Event::PointerMoved(center),
                    egui::Event::PointerButton {
                        pos: center,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        frame(
            &context,
            &mut value,
            0.03,
            vec![
                key(egui::Key::A, egui::Modifiers::CTRL),
                egui::Event::Text("0.".to_owned()),
            ],
        );
        assert_eq!(value, 1., "partial text must not start an autosave");
        frame(&context, &mut value, 2., vec![]);
        assert_eq!(value, 1., "waiting must not commit partial text");
        frame(
            &context,
            &mut value,
            2.01,
            vec![egui::Event::Text("75".to_owned())],
        );
        assert_eq!(value, 1., "complete text stays local until committed");
        frame(
            &context,
            &mut value,
            2.02,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert_eq!(value, 0.75, "Enter commits the intended final value");
    }

    #[test]
    fn control_columns_align_independently_of_the_copy_length() {
        for width in [480., 700., 900.] {
            let context = egui::Context::default();
            let mut positions = Vec::new();
            let mut value = 1.;
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 500.),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        for (index, (title, description)) in [
                            ("指针速度", "负值更慢，正值更快。"),
                            (
                                "滚动速度",
                                "1× 为标准速度；高级设置中的横向、纵向独立倍率仍会保留。",
                            ),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            control_row(ui, index, title, description, |ui| {
                                positions.push(
                                    ui.add(egui::Slider::new(&mut value, 0.0..=2.0)).rect.left(),
                                );
                            });
                        }
                    });
                },
            );
            assert_eq!(positions.len(), 2);
            assert!(
                (positions[0] - positions[1]).abs() < 0.5,
                "control columns drift at width {width}: {positions:?}"
            );
        }
    }

    #[test]
    fn selecting_a_builtin_replaces_a_shader_even_for_the_same_fallback() {
        fn frame(
            context: &egui::Context,
            value: &mut String,
            events: Vec<egui::Event>,
        ) -> (egui::FullOutput, bool) {
            let mut chosen = false;
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(480., 500.),
                    )),
                    events,
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        chosen = choices_with_state(
                            ui,
                            "effect",
                            "打开效果",
                            "",
                            value,
                            &[("fade", "淡入"), ("reveal", "展开")],
                            Some("自定义着色器"),
                        );
                    });
                },
            );
            (output, chosen)
        }

        fn text_center(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing visible control {label}"))
        }

        let pointer = |position, pressed| {
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let context = egui::Context::default();
        let mut value = "fade".to_owned();
        let (output, chosen) = frame(&context, &mut value, vec![]);
        assert!(!chosen);
        let combo = text_center(&output, "自定义着色器");
        frame(&context, &mut value, pointer(combo, true));
        frame(&context, &mut value, pointer(combo, false));
        let (output, _) = frame(&context, &mut value, vec![]);
        let fade = text_center(&output, "淡入");
        frame(&context, &mut value, pointer(fade, true));
        assert!(frame(&context, &mut value, pointer(fade, false)).1);
        assert_eq!(value, "fade");
    }

    #[test]
    fn switches_support_keyboard_activation_and_disabled_state() {
        fn frame(
            context: &egui::Context,
            value: &mut bool,
            events: Vec<egui::Event>,
            enabled: bool,
        ) -> (egui::Id, bool) {
            let mut result = None;
            let _ = context.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        ui.add_enabled_ui(enabled, |ui| {
                            let response = switch(ui, "桌面动画", "平滑过渡", value);
                            result = Some((response.id, response.changed()));
                        });
                    });
                },
            );
            result.unwrap()
        }

        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let context = egui::Context::default();
        let mut value = false;
        let (id, changed) = frame(&context, &mut value, vec![], true);
        assert!(!changed);
        context.memory_mut(|memory| memory.request_focus(id));
        assert!(frame(&context, &mut value, vec![key(egui::Key::Enter)], true).1);
        assert!(value);
        assert!(frame(&context, &mut value, vec![key(egui::Key::Space)], true).1);
        assert!(!value);
        assert!(!frame(&context, &mut value, vec![key(egui::Key::Enter)], false).1);
        assert!(!value);
    }

    #[test]
    fn translated_controls_fit_narrow_and_wide_cards() {
        for width in [320., 480., 900.] {
            let context = egui::Context::default();
            let mut enabled = true;
            let mut amount = 6.5;
            let mut choice = "overflow".to_owned();
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 700.),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        page(ui, "responsive", "设置", "常用设置", |ui| {
                            section(ui, "card", "窗口", |ui| {
                                let right = ui.max_rect().right();
                                toggle(
                                    ui,
                                    "toggle",
                                    "窗口移动与平铺",
                                    "窗口重新排列时，从当前位置连续移动到目标位置。",
                                    &mut enabled,
                                );
                                slider(
                                    ui,
                                    "slider",
                                    "窗口之间的留白距离",
                                    "在窄窗口中仍应保留完整说明与可编辑的精确数值。",
                                    &mut amount,
                                    0.0..=40.0,
                                    " px",
                                    1,
                                );
                                choices(
                                    ui,
                                    "choice",
                                    "聚焦窗口的位置",
                                    "选择适合自己的工作方式。",
                                    &mut choice,
                                    &[("overflow", "超出屏幕时居中"), ("always", "始终居中")],
                                );
                                assert!(
                                    ui.min_rect().right() <= right + 1.,
                                    "controls overflow at width {width}: {} > {right}",
                                    ui.min_rect().right()
                                );
                            });
                        });
                    });
                },
            );
        }
    }
}
