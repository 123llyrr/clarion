//! Display controls only. The application owns configuration transactions and
//! the compositor's reversible output preview; this view never performs I/O.

use std::collections::BTreeSet;

use clarion_ipc::{ConfiguredMode, Output, Transform};
use eframe::egui::{self, RichText, Ui};

use crate::output_settings::{OutputSettings, VrrMode};
use crate::settings_widgets as controls;
use crate::theme::colors;

/// Selecting a display is navigation, independent of changing its settings.
pub fn selector(ui: &mut Ui, outputs: &[Output], selected: &mut Option<String>) -> bool {
    let before = selected.clone();
    if ui.is_enabled()
        && !outputs
            .iter()
            .any(|output| Some(&output.name) == selected.as_ref())
    {
        *selected = outputs
            .iter()
            .find(|output| output.logical.is_some())
            .or_else(|| outputs.first())
            .map(|output| output.name.clone());
    }
    if outputs.len() > 1 {
        controls::control_row(
            ui,
            "output-selector",
            "显示器",
            "选择要调整的屏幕。",
            |ui| {
                let label = outputs
                    .iter()
                    .find(|output| Some(&output.name) == selected.as_ref())
                    .map_or("选择显示器".to_owned(), display_label);
                egui::ComboBox::from_id_salt("monitor")
                    .width(ui.available_width().min(360.))
                    .truncate()
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for output in outputs {
                            ui.selectable_value(
                                selected,
                                Some(output.name.clone()),
                                display_label(output),
                            );
                        }
                    });
            },
        );
    }
    before != *selected
}

/// Returns true only after an actual model edit. Rendering a custom mode or
/// automatic luminance value does not replace it with a convenient preset.
pub fn show(
    ui: &mut Ui,
    output: &Output,
    settings: &mut OutputSettings,
    active_output_count: usize,
) -> bool {
    let before = settings.clone();
    ui.push_id((&output.name, "display-preferences"), |ui| {
        ui.label(
            RichText::new("显示器")
                .size(24.)
                .strong()
                .color(colors::text()),
        );
        ui.add_space(4.);
        ui.add(
            egui::Label::new(
                RichText::new(display_label(output))
                    .size(12.5)
                    .color(colors::muted()),
            )
            .wrap(),
        );
        ui.add_space(20.);
        current_status(ui, output);
        controls::section(ui, "enabled", "连接与显示", |ui| {
            let last_active = output.logical.is_some() && active_output_count <= 1;
            ui.add_enabled_ui(!settings.enabled || !last_active, |ui| {
                controls::toggle(
                    ui,
                    "enabled",
                    "启用这台显示器",
                    if last_active {
                        "这是当前唯一的活动显示器，保留开启以免失去画面。"
                    } else {
                        "关闭后停止向这台屏幕输出画面。"
                    },
                    &mut settings.enabled,
                );
            });
        });
        controls::section(ui, "geometry", "分辨率与缩放", |ui| {
            resolution(ui, output, settings);
            controls::separator(ui);
            refresh_rate(ui, output, settings);
            controls::separator(ui);
            scale(ui, settings);
            controls::separator(ui);
            transform(ui, &mut settings.transform);
        });
        controls::section(ui, "colour", "色彩与动态范围", |ui| {
            ui.add_enabled_ui(output.hdr_supported || settings.hdr, |ui| {
                controls::toggle(
                    ui,
                    "hdr",
                    "高动态范围（HDR）",
                    if output.hdr_supported {
                        "为支持的内容呈现更丰富的亮部与色彩。"
                    } else if settings.hdr {
                        "这条输出链路不支持 HDR；可以关闭已配置的请求。"
                    } else {
                        "当前显示器或驱动不支持 HDR。"
                    },
                    &mut settings.hdr,
                );
            });
            if let Some(status) = output
                .hdr_status
                .as_deref()
                .filter(|status| !status.is_empty())
            {
                ui.add(
                    egui::Label::new(RichText::new(status).size(11.5).color(colors::muted()))
                        .wrap(),
                );
                ui.add_space(6.);
            }
            ui.add_enabled_ui(settings.hdr && output.hdr_supported, |ui| {
                let mut white = settings.sdr_white_level.unwrap_or(203.);
                let original = white;
                controls::slider(
                    ui,
                    "white-level",
                    "SDR 内容白亮度",
                    "仅在 HDR 下生效；标准参考白为 203 尼特。",
                    &mut white,
                    80.0..=500.0,
                    " 尼特",
                    0,
                );
                if white != original {
                    settings.sdr_white_level = Some(white);
                }
                if settings.sdr_white_level.is_some() && ui.small_button("恢复标准白亮度").clicked()
                {
                    settings.sdr_white_level = None;
                }
                egui::CollapsingHeader::new("更多 HDR 选项")
                    .id_salt("advanced-hdr")
                    .show(ui, |ui| {
                        let mut brightness = settings.hdr_brightness.unwrap_or(100.);
                        let original = brightness;
                        controls::slider(
                            ui,
                            "hdr-brightness",
                            "HDR 输出亮度",
                            "调整 HDR 输出的整体亮度，不改变显示器硬件背光。",
                            &mut brightness,
                            1.0..=100.0,
                            "%",
                            0,
                        );
                        if brightness != original {
                            settings.hdr_brightness = Some(brightness);
                        }
                        if settings.hdr_brightness.is_some()
                            && ui.small_button("恢复标准输出亮度").clicked()
                        {
                            settings.hdr_brightness = None;
                        }
                        controls::separator(ui);
                        let was_custom = settings.hdr_peak_luminance.is_some();
                        let memory_id = ui.make_persistent_id("last-hdr-peak");
                        if let Some(peak) = settings.hdr_peak_luminance {
                            ui.data_mut(|data| data.insert_temp(memory_id, peak));
                        }
                        let mut custom = was_custom;
                        controls::toggle(
                            ui,
                            "hdr-peak-override",
                            "覆盖 HDR 峰值亮度",
                            "校准 HDR 内容映射使用的峰值。关闭时自动读取显示器信息。",
                            &mut custom,
                        );
                        if custom != was_custom {
                            settings.hdr_peak_luminance = custom.then(|| {
                                ui.data(|data| data.get_temp::<f64>(memory_id))
                                    .or_else(|| output.hdr_peak_luminance.map(f64::from))
                                    .filter(|value| value.is_finite())
                                    .unwrap_or(1000.)
                                    .clamp(80., 10_000.)
                            });
                        }
                        if let Some(peak) = settings.hdr_peak_luminance.as_mut() {
                            controls::slider(
                                ui,
                                "hdr-peak",
                                "HDR 峰值覆盖",
                                "设置内容映射参考的最高亮度；通常保持显示器报告的值即可。",
                                peak,
                                80.0..=10_000.0,
                                " 尼特",
                                0,
                            );
                        }
                    });
            });
        });
        controls::section(ui, "refresh", "自适应刷新", |ui| {
            let was_enabled = settings.vrr != VrrMode::Off;
            let mut enabled = was_enabled;
            let memory_id = ui.make_persistent_id("last-vrr-mode");
            if was_enabled {
                ui.data_mut(|data| data.insert_temp(memory_id, settings.vrr));
            }
            ui.add_enabled_ui(output.vrr_supported || was_enabled, |ui| {
                controls::toggle(
                    ui,
                    "vrr",
                    "可变刷新率（VRR）",
                    if output.vrr_supported {
                        "让屏幕刷新跟随内容节奏；首次开启默认按应用需要启用。"
                    } else if was_enabled {
                        "当前屏幕不支持 VRR；可以关闭已配置的请求。"
                    } else {
                        "当前显示器不支持可变刷新率。"
                    },
                    &mut enabled,
                );
            });
            if enabled != was_enabled {
                settings.vrr = if enabled {
                    ui.data(|data| data.get_temp::<VrrMode>(memory_id))
                        .unwrap_or(VrrMode::OnDemand)
                } else {
                    VrrMode::Off
                };
            }
            ui.add_enabled_ui(enabled && output.vrr_supported, |ui| {
                let mut policy = if settings.vrr == VrrMode::OnDemand {
                    "on-demand"
                } else {
                    "always"
                }
                .to_owned();
                controls::choices(
                    ui,
                    "vrr-policy",
                    "启用方式",
                    "可持续启用，或仅在符合应用规则时启用。",
                    &mut policy,
                    &[("always", "始终启用"), ("on-demand", "按应用需要")],
                );
                if enabled {
                    settings.vrr = if policy == "on-demand" {
                        VrrMode::OnDemand
                    } else {
                        VrrMode::Always
                    };
                }
            });
        });
    });
    before != *settings
}

fn display_label(output: &Output) -> String {
    if output.model.is_empty() || output.model == "Unknown" {
        output.name.clone()
    } else {
        format!("{} · {}", output.model, output.name)
    }
}

fn current_status(ui: &mut Ui, output: &Output) {
    controls::section(ui, "current", "当前状态", |ui| {
        let active = output.logical.is_some();
        ui.label(
            RichText::new(if active {
                "正在显示画面"
            } else {
                "已连接，当前未启用"
            })
            .color(if active {
                colors::success()
            } else {
                colors::muted()
            })
            .strong(),
        );
        if let Some(mode) = output
            .current_mode
            .and_then(|index| output.modes.get(index))
        {
            ui.add(
                egui::Label::new(format!(
                    "{} × {} · {:.2} Hz",
                    mode.width,
                    mode.height,
                    mode.refresh_rate as f64 / 1000.
                ))
                .wrap(),
            );
        }
        if let Some(logical) = output.logical {
            ui.add(
                egui::Label::new(format!(
                    "缩放 {:.2}× · 桌面空间 {} × {}",
                    logical.scale, logical.width, logical.height
                ))
                .wrap(),
            );
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(if output.hdr_active {
                "HDR 已启用"
            } else {
                "HDR 未启用"
            });
            ui.label("·");
            ui.label(if output.vrr_enabled {
                "VRR 已启用"
            } else {
                "VRR 未启用"
            });
        });
    });
}

fn resolution(ui: &mut Ui, output: &Output, settings: &mut OutputSettings) {
    controls::control_row(
        ui,
        "resolution",
        "分辨率",
        "以屏幕物理像素为单位。自动选项使用显示器建议的模式。",
        |ui| {
            let selected = if settings.custom_modeline {
                "自定义时序".to_owned()
            } else {
                settings.mode.map_or("自动（推荐）".to_owned(), |mode| {
                    format!("{} × {}", mode.width, mode.height)
                })
            };
            let mut sizes: Vec<_> = output
                .modes
                .iter()
                .map(|mode| (mode.width, mode.height))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            sizes.sort_by(|a, b| b.cmp(a));
            egui::ComboBox::from_id_salt("resolution-choice")
                .width(ui.available_width().min(236.))
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(
                            settings.mode.is_none() && !settings.custom_modeline,
                            "自动（推荐）",
                        )
                        .clicked()
                    {
                        settings.mode = None;
                        settings.custom_modeline = false;
                    }
                    for (width, height) in sizes {
                        let selected = !settings.custom_modeline
                            && settings
                                .mode
                                .is_some_and(|mode| mode.width == width && mode.height == height);
                        let preferred = output.modes.iter().any(|mode| {
                            mode.width == width && mode.height == height && mode.is_preferred
                        });
                        let label = format!(
                            "{width} × {height}{}",
                            if preferred { "（推荐）" } else { "" }
                        );
                        if ui.selectable_label(selected, label).clicked() {
                            let refresh =
                                settings
                                    .mode
                                    .and_then(|mode| mode.refresh)
                                    .filter(|refresh| {
                                        output.modes.iter().any(|mode| {
                                            mode.width == width
                                                && mode.height == height
                                                && (mode.refresh_rate as f64 / 1000. - refresh)
                                                    .abs()
                                                    < 0.000_1
                                        })
                                    });
                            settings.mode = Some(ConfiguredMode {
                                width,
                                height,
                                refresh,
                            });
                            settings.custom_modeline = false;
                        }
                    }
                });
        },
    );
}

fn refresh_rate(ui: &mut Ui, output: &Output, settings: &mut OutputSettings) {
    controls::control_row(
        ui,
        "refresh-rate",
        "刷新率",
        "只列出当前分辨率支持的刷新率。",
        |ui| {
            let dimensions = settings
                .mode
                .map(|mode| (mode.width, mode.height))
                .or_else(|| {
                    output
                        .current_mode
                        .and_then(|index| output.modes.get(index))
                        .map(|mode| (mode.width, mode.height))
                });
            let rates: BTreeSet<_> = output
                .modes
                .iter()
                .filter(|mode| Some((mode.width, mode.height)) == dimensions)
                .map(|mode| mode.refresh_rate)
                .collect();
            let selected = if settings.custom_modeline {
                "自定义时序".to_owned()
            } else {
                settings
                    .mode
                    .and_then(|mode| mode.refresh)
                    .map_or("自动（推荐）".to_owned(), |rate| {
                        format!("{rate:.2} Hz")
                    })
            };
            egui::ComboBox::from_id_salt("refresh-choice")
                .width(ui.available_width().min(236.))
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(
                            !settings.custom_modeline
                                && settings.mode.and_then(|mode| mode.refresh).is_none(),
                            "自动（推荐）",
                        )
                        .clicked()
                    {
                        if let Some(mode) = settings.mode.as_mut() {
                            mode.refresh = None;
                        }
                        settings.custom_modeline = false;
                    }
                    for millihertz in rates.iter().rev() {
                        let rate = *millihertz as f64 / 1000.;
                        let selected = !settings.custom_modeline
                            && settings
                                .mode
                                .and_then(|mode| mode.refresh)
                                .is_some_and(|refresh| (refresh - rate).abs() < 0.000_1);
                        if ui
                            .selectable_label(selected, format!("{rate:.2} Hz"))
                            .clicked()
                        {
                            if let Some((width, height)) = dimensions {
                                settings.mode = Some(ConfiguredMode {
                                    width,
                                    height,
                                    refresh: Some(rate),
                                });
                                settings.custom_modeline = false;
                            }
                        }
                    }
                });
        },
    );
}

fn scale(ui: &mut Ui, settings: &mut OutputSettings) {
    controls::control_row(
        ui,
        "scale",
        "界面缩放",
        "增大缩放可让文字和界面更容易阅读。",
        |ui| {
            let selected = settings
                .scale
                .map_or("自动".to_owned(), |scale| format!("{:.0}%", scale * 100.));
            egui::ComboBox::from_id_salt("scale-choice")
                .width(ui.available_width().min(236.))
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut settings.scale, None, "自动");
                    for value in [1., 1.25, 1.5, 1.75, 2., 2.25, 2.5, 3., 4.] {
                        ui.selectable_value(
                            &mut settings.scale,
                            Some(value),
                            format!("{:.0}%", value * 100.),
                        );
                    }
                });
        },
    );
}

fn transform(ui: &mut Ui, transform: &mut Transform) {
    let variants = [
        ("normal", "标准", Transform::Normal),
        ("90", "向左旋转 90°", Transform::_90),
        ("180", "倒转 180°", Transform::_180),
        ("270", "向右旋转 90°", Transform::_270),
        ("flipped", "水平镜像", Transform::Flipped),
        ("flipped-90", "左转 90° / 镜像", Transform::Flipped90),
        ("flipped-180", "垂直镜像", Transform::Flipped180),
        ("flipped-270", "右转 90° / 镜像", Transform::Flipped270),
    ];
    let mut selected = variants
        .iter()
        .find(|(_, _, value)| value == transform)
        .map_or("normal", |(key, _, _)| *key)
        .to_owned();
    let options: Vec<_> = variants
        .iter()
        .map(|(key, label, _)| (*key, *label))
        .collect();
    controls::choices(
        ui,
        "rotation",
        "旋转与镜像",
        "调整这台屏幕的画面方向。",
        &mut selected,
        &options,
    );
    if let Some((_, _, value)) = variants.iter().find(|(key, _, _)| *key == selected) {
        *transform = *value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> Output {
        serde_json::from_value(serde_json::json!({
            "name":"DP-QA", "make":"Fixture", "model":"Isolated display", "serial":null,
            "physical_size":null, "modes":[{"width":3840,"height":2160,"refresh_rate":59940,"is_preferred":true}],
            "current_mode":0, "is_custom_mode":false, "vrr_supported":true, "vrr_enabled":false,
            "logical":{"x":0,"y":0,"width":1920,"height":1080,"scale":2.0,"transform":"Normal"},
            "max_bpc":10, "hdr_active":false, "hdr_supported":true,
            "hdr_status":null, "hdr_peak_luminance":600.0, "sdr_white_level":203.0, "hdr_brightness":100.0
        })).unwrap()
    }

    #[test]
    fn viewing_displays_preserves_automatic_and_custom_configuration() {
        let output = output();
        let mut settings = OutputSettings {
            mode: Some(ConfiguredMode {
                width: 2880,
                height: 1800,
                refresh: Some(71.125),
            }),
            scale: Some(1.37),
            transform: Transform::Flipped90,
            custom_modeline: true,
            hdr: true,
            sdr_white_level: None,
            vrr: VrrMode::OnDemand,
            ..Default::default()
        };
        let original = settings.clone();
        for width in [480., 700., 900.] {
            let context = egui::Context::default();
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 900.),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        let right = ui.max_rect().right();
                        assert!(!show(ui, &output, &mut settings, 1));
                        assert!(
                            ui.min_rect().right() <= right + 1.,
                            "display controls overflow at width {width}"
                        );
                    });
                },
            );
            assert_eq!(settings, original);
        }
    }

    #[test]
    fn selecting_a_monitor_does_not_modify_its_configuration() {
        let mut selected = None;
        let outputs = vec![output()];
        let context = egui::Context::default();
        let _ = context.run(Default::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                assert!(selector(ui, &outputs, &mut selected));
            });
        });
        assert_eq!(selected.as_deref(), Some("DP-QA"));
        let _ = context.run(Default::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                assert!(!selector(ui, &outputs, &mut selected));
            });
        });
    }

    #[test]
    fn disabled_selector_keeps_a_disconnected_monitor_selected() {
        let mut selected = Some("DISCONNECTED-QA".to_owned());
        let original = selected.clone();
        let outputs = vec![output()];
        let context = egui::Context::default();
        let _ = context.run(Default::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                ui.add_enabled_ui(false, |ui| {
                    assert!(!selector(ui, &outputs, &mut selected));
                });
            });
        });
        assert_eq!(
            selected, original,
            "pending display ownership must survive disconnection"
        );
    }
}
