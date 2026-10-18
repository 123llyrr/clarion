//! Friendly controls for daily use. The model owns configuration preservation;
//! these views only change the specific values the user operates.

use clarion_config::PresetSize;
use eframe::egui::{self, Ui, WidgetInfo, WidgetType};

use crate::common_settings::CommonSettings;
use crate::settings_widgets as controls;

pub fn appearance(ui: &mut Ui, settings: &mut CommonSettings) {
    controls::page(
        ui,
        "common-appearance",
        "通用",
        "调整窗口动效与桌面外观，让日常操作符合你的习惯。",
        |ui| {
            controls::section(ui, "motion", "动效与响应", |ui| {
                controls::toggle(
                    ui,
                    "enabled",
                    "桌面动画",
                    "关闭后立即完成窗口与工作区过渡，原有参数会保留。",
                    &mut settings.animations_enabled,
                );
                controls::separator(ui);
                ui.add_enabled_ui(settings.animations_enabled, |ui| {
                    controls::slider(
                        ui,
                        "speed",
                        "动画时长",
                        "1× 为标准；减小会更快，增大会更舒缓。",
                        &mut settings.animation_speed,
                        0.25..=2.,
                        " ×",
                        2,
                    );
                });
            });
            ui.add_enabled_ui(settings.animations_enabled, |ui| {
                controls::section(ui, "window-motion", "窗口与工作区", |ui| {
                    controls::toggle(
                        ui,
                        "open",
                        "打开窗口",
                        "新窗口出现时平滑过渡。",
                        &mut settings.window_open_enabled,
                    );
                    ui.add_enabled_ui(settings.window_open_enabled, |ui| {
                        let custom = settings.open_custom_shader;
                        if controls::choices_with_state(
                            ui,
                            "open-effect",
                            "打开效果",
                            if custom {
                                "正在使用自定义着色器；选择内置效果后会替换它。"
                            } else {
                                "选择新窗口的呈现方式。"
                            },
                            &mut settings.window_open_effect,
                            &[("fade", "淡入"), ("reveal", "逐渐展开")],
                            custom.then_some("自定义着色器"),
                        ) {
                            settings.open_custom_shader = false;
                        }
                    });
                    controls::separator(ui);
                    controls::toggle(
                        ui,
                        "move",
                        "窗口移动与平铺",
                        "窗口重新排列时，从当前位置连续移动到目标位置。",
                        &mut settings.window_movement_enabled,
                    );
                    controls::separator(ui);
                    controls::toggle(
                        ui,
                        "resize",
                        "窗口尺寸过渡",
                        "调整窗口大小时平滑衔接。",
                        &mut settings.window_resize_enabled,
                    );
                    controls::separator(ui);
                    controls::toggle(
                        ui,
                        "workspace",
                        "切换工作区",
                        "在工作区之间切换时使用过渡动画。",
                        &mut settings.workspace_switch_enabled,
                    );
                    controls::separator(ui);
                    controls::toggle(
                        ui,
                        "close",
                        "关闭窗口",
                        "窗口关闭时使用退出效果。",
                        &mut settings.window_close_enabled,
                    );
                    ui.add_enabled_ui(settings.window_close_enabled, |ui| {
                        let custom = settings.close_custom_shader;
                        if controls::choices_with_state(
                            ui,
                            "close-effect",
                            "关闭效果",
                            if custom {
                                "正在使用自定义着色器；选择内置效果后会替换它。"
                            } else {
                                "不影响窗口内容或应用退出行为。"
                            },
                            &mut settings.window_close_effect,
                            &[
                                ("fade", "淡出"),
                                ("implode", "收拢消失"),
                                ("explosion", "碎片消散"),
                            ],
                            custom.then_some("自定义着色器"),
                        ) {
                            settings.close_custom_shader = false;
                        }
                    });
                });
                egui::CollapsingHeader::new("更多动效")
                    .id_salt("common-extra-motion")
                    .show(ui, |ui| {
                        controls::section(ui, "extra-motion", "增强效果", |ui| {
                            controls::toggle(
                                ui,
                                "wobble",
                                "弹性窗口",
                                "拖动与缩放时加入柔和形变与回弹。",
                                &mut settings.wobble_enabled,
                            );
                            controls::separator(ui);
                            controls::toggle(
                                ui,
                                "cube",
                                "立方体切换",
                                "为支持的工作区切换使用立体效果。",
                                &mut settings.cube_enabled,
                            );
                            controls::separator(ui);
                            controls::toggle(
                                ui,
                                "glide",
                                "窗口立体展开",
                                "打开窗口时加入透视与倾斜变化。",
                                &mut settings.glide_enabled,
                            );
                        });
                    });
                ui.add_space(14.);
            });
            controls::section(ui, "decoration", "窗口细节", |ui| {
                controls::toggle(
                    ui,
                    "border",
                    "窗口边框",
                    "为窗口边缘绘制轮廓。",
                    &mut settings.border_enabled,
                );
                ui.add_enabled_ui(settings.border_enabled, |ui| {
                    controls::slider(
                        ui,
                        "border-width",
                        "边框宽度",
                        "",
                        &mut settings.border_width,
                        0.5..=12.,
                        " px",
                        1,
                    )
                });
                ui.add_enabled_ui(settings.border_enabled, |ui| {
                    controls::toggle(
                        ui,
                        "gradient-enabled",
                        "动态渐变边框",
                        "让活动窗口的渐变边框持续旋转；首次开启使用当前边框配色。",
                        &mut settings.border_gradient_enabled,
                    );
                    ui.add_enabled_ui(settings.border_gradient_enabled, |ui| {
                        let original_speed = settings.border_gradient_speed;
                        let mut magnitude = original_speed.abs();
                        controls::slider(
                            ui,
                            "gradient-speed",
                            "渐变旋转速度",
                            "数值越大旋转越快。关闭开关会记住原有速度。",
                            &mut magnitude,
                            1.0..=180.0,
                            " °/秒",
                            1,
                        );
                        if magnitude != original_speed.abs() {
                            settings.border_gradient_speed = magnitude.copysign(original_speed);
                        }
                        let mut direction = if settings.border_gradient_speed.is_sign_negative() {
                            "reverse".to_owned()
                        } else {
                            "forward".to_owned()
                        };
                        controls::choices(
                            ui,
                            "gradient-direction",
                            "旋转方向",
                            "",
                            &mut direction,
                            &[("forward", "正向"), ("reverse", "反向")],
                        );
                        let negative = direction == "reverse";
                        if negative != settings.border_gradient_speed.is_sign_negative() {
                            settings.border_gradient_speed = -settings.border_gradient_speed;
                        }
                    });
                });
                controls::separator(ui);
                controls::toggle(
                    ui,
                    "focus",
                    "焦点光环",
                    "清晰标识当前操作的窗口。",
                    &mut settings.focus_ring_enabled,
                );
                ui.add_enabled_ui(settings.focus_ring_enabled, |ui| {
                    controls::slider(
                        ui,
                        "focus-width",
                        "光环宽度",
                        "",
                        &mut settings.focus_ring_width,
                        0.5..=12.,
                        " px",
                        1,
                    )
                });
                controls::separator(ui);
                controls::toggle(
                    ui,
                    "shadow",
                    "窗口阴影",
                    "用柔和阴影增加窗口层次。",
                    &mut settings.shadow_enabled,
                );
                controls::separator(ui);
                controls::slider(
                    ui,
                    "rounding",
                    "窗口圆角",
                    "0 表示直角，数值越大边角越圆润。",
                    &mut settings.rounding,
                    0.0..=32.0,
                    " px",
                    1,
                );
            });
            controls::section(ui, "blur", "背景模糊", |ui| {
                controls::toggle(
                    ui,
                    "enabled",
                    "背景模糊",
                    "模糊半透明窗口后方的内容。",
                    &mut settings.blur_enabled,
                );
                ui.add_enabled_ui(settings.blur_enabled, |ui| {
                    controls::slider(
                        ui,
                        "passes",
                        "模糊层次",
                        "层次越高效果越柔和，也会增加显卡工作量。",
                        &mut settings.blur_passes,
                        1..=8,
                        " 层",
                        0,
                    );
                    controls::slider(
                        ui,
                        "offset",
                        "模糊范围",
                        "调整半透明背景的扩散程度。",
                        &mut settings.blur_offset,
                        0.5..=12.,
                        " px",
                        1,
                    );
                });
            });
        },
    );
}

pub fn layout(ui: &mut Ui, settings: &mut CommonSettings) {
    controls::page(
        ui,
        "common-layout",
        "窗口布局",
        "设置新窗口的位置、排列密度与聚焦方式。",
        |ui| {
            controls::section(ui, "tiling", "自动排列", |ui| {
                controls::toggle(
                    ui,
                    "auto-stack",
                    "自动堆叠窗口",
                    "将新窗口放入当前列，达到数量上限后创建新列。",
                    &mut settings.auto_stack,
                );
                ui.add_enabled_ui(settings.auto_stack, |ui| {
                    controls::slider(
                        ui,
                        "max-windows",
                        "每列窗口上限",
                        "达到上限后，新窗口会放入下一列。",
                        &mut settings.max_windows_per_column,
                        1..=8,
                        " 个",
                        0,
                    )
                });
                controls::separator(ui);
                default_width(ui, &mut settings.default_width);
                controls::separator(ui);
                controls::slider(
                    ui,
                    "gaps",
                    "窗口间距",
                    "调整窗口之间及屏幕边缘的留白。",
                    &mut settings.gaps,
                    0.0..=40.0,
                    " px",
                    1,
                );
            });
            controls::section(ui, "focus", "聚焦与居中", |ui| {
                controls::choices(
                    ui,
                    "center",
                    "聚焦列居中",
                    "决定选中的窗口列何时移到屏幕中央。",
                    &mut settings.center_column,
                    &[
                        ("never", "从不"),
                        ("on-overflow", "超出屏幕时"),
                        ("always", "始终"),
                    ],
                );
                controls::separator(ui);
                controls::toggle(
                    ui,
                    "center-single",
                    "单列始终居中",
                    "工作区只有一列窗口时，让它保持在中央。",
                    &mut settings.center_single,
                );
            });
        },
    );
}

fn default_width(ui: &mut Ui, value: &mut Option<PresetSize>) {
    controls::control_row(
        ui,
        "default-width",
        "新窗口默认宽度",
        "以可用屏幕宽度为基准；已有自定义数值会保留。",
        |ui| {
            let options = [
                (None, "应用建议"),
                (Some(PresetSize::Proportion(0.25)), "四分之一"),
                (Some(PresetSize::Proportion(1. / 3.)), "三分之一"),
                (Some(PresetSize::Proportion(0.5)), "二分之一"),
                (Some(PresetSize::Proportion(2. / 3.)), "三分之二"),
                (Some(PresetSize::Proportion(0.75)), "四分之三"),
                (Some(PresetSize::Proportion(1.)), "整个可用宽度"),
            ];
            let selected = options
                .iter()
                .find(|(option, _)| option == value)
                .map(|(_, label)| (*label).to_owned())
                .unwrap_or_else(|| match value {
                    Some(PresetSize::Proportion(proportion)) => {
                        format!("自定义 · {:.1}%", *proportion * 100.)
                    }
                    Some(PresetSize::Fixed(width)) => format!("自定义 · {width} px"),
                    None => "应用建议".to_owned(),
                });
            let response = egui::ComboBox::from_id_salt("width-choice")
                .width(ui.available_width().min(236.))
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for (option, label) in options {
                        ui.selectable_value(value, option, label);
                    }
                });
            response.response.widget_info(|| {
                WidgetInfo::labeled(WidgetType::ComboBox, ui.is_enabled(), "新窗口默认宽度")
            });
        },
    );
}

pub fn keyboard(ui: &mut Ui, settings: &mut CommonSettings) {
    controls::page(
        ui,
        "common-keyboard",
        "键盘",
        "调整按键重复速度与数字键盘的启动状态。",
        |ui| {
            controls::section(ui, "typing", "输入体验", |ui| {
                controls::slider(
                    ui,
                    "repeat-rate",
                    "按键重复速度",
                    "长按按键后每秒重复的次数；0 表示关闭重复。",
                    &mut settings.keyboard_repeat_rate,
                    0..=100,
                    " 次/秒",
                    0,
                );
                controls::separator(ui);
                controls::slider(
                    ui,
                    "repeat-delay",
                    "重复前延迟",
                    "从按住按键到开始连续输入的等待时间。",
                    &mut settings.keyboard_repeat_delay,
                    100..=1000,
                    " ms",
                    0,
                );
            });
            controls::section(ui, "startup", "数字键盘", |ui| {
                controls::toggle(
                    ui,
                    "numlock",
                    "启动时启用数字锁定",
                    "下次登录时使用此状态；当前键盘仍可用 Num Lock 切换。",
                    &mut settings.keyboard_numlock,
                );
            });
        },
    );
}

pub fn trackpad(ui: &mut Ui, settings: &mut CommonSettings) {
    controls::page(
        ui,
        "common-trackpad",
        "触控板与鼠标",
        "调整指针、滚动方向与触控板操作。",
        |ui| {
            controls::section(ui, "touchpad", "触控板", |ui| {
                controls::toggle(
                    ui,
                    "tap",
                    "轻点来点按",
                    "轻触触控板即可完成一次点击。",
                    &mut settings.touchpad_tap,
                );
                controls::separator(ui);
                controls::toggle(
                    ui,
                    "natural",
                    "自然滚动",
                    "内容跟随手指移动的方向滚动。",
                    &mut settings.touchpad_natural_scroll,
                );
                controls::separator(ui);
                controls::toggle(
                    ui,
                    "dwt",
                    "打字时防止误触",
                    "输入文字时暂时忽略触控板操作。",
                    &mut settings.touchpad_dwt,
                );
                controls::separator(ui);
                controls::slider(
                    ui,
                    "speed",
                    "指针速度",
                    "负值更慢，正值更快。",
                    &mut settings.touchpad_speed,
                    -1.0..=1.0,
                    "",
                    2,
                );
                controls::separator(ui);
                controls::slider(
                    ui,
                    "scroll",
                    "滚动速度",
                    "1× 为标准速度；高级设置中的横向、纵向独立倍率仍会保留。",
                    &mut settings.touchpad_scroll_factor,
                    0.1..=3.,
                    " ×",
                    2,
                );
            });
            controls::section(ui, "mouse", "鼠标", |ui| {
                controls::toggle(
                    ui,
                    "natural",
                    "自然滚动",
                    "反转鼠标滚轮的滚动方向。",
                    &mut settings.mouse_natural_scroll,
                );
                controls::separator(ui);
                controls::slider(
                    ui,
                    "speed",
                    "指针速度",
                    "负值更慢，正值更快。",
                    &mut settings.mouse_speed,
                    -1.0..=1.0,
                    "",
                    2,
                );
            });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_preferences_preserves_custom_values_at_narrow_and_wide_widths() {
        // Rendering must never clamp unusual but valid user values merely
        // because the friendly slider offers a smaller everyday drag range.
        let mut settings = CommonSettings {
            gaps: 53.5,
            default_width: Some(PresetSize::Fixed(917)),
            animation_speed: 3.25,
            open_custom_shader: true,
            close_custom_shader: true,
            border_enabled: true,
            border_gradient_enabled: true,
            border_gradient_speed: -217.,
            ..Default::default()
        };
        let original = settings.clone();
        for width in [320., 480., 900.] {
            let context = egui::Context::default();
            for page in [appearance, layout, keyboard, trackpad] {
                let _ = context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 900.),
                        )),
                        ..Default::default()
                    },
                    |context| {
                        egui::CentralPanel::default().show(context, |ui| page(ui, &mut settings));
                    },
                );
                assert_eq!(
                    settings, original,
                    "reading a page changed configuration at width {width}"
                );
            }
        }
    }
}
