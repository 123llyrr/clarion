use eframe::egui::{self, FontId, Rounding, Stroke, TextStyle, Visuals};

/// Shared colors for panels, controls and custom-painted navigation.
pub mod colors {
    use eframe::egui::Color32;

    pub const BACKGROUND: Color32 = Color32::from_rgb(236, 236, 236);
    pub const SIDEBAR: Color32 = Color32::from_rgb(230, 230, 230);
    pub const SURFACE: Color32 = Color32::WHITE;
    pub const SURFACE_HOVER: Color32 = Color32::from_rgb(246, 246, 246);
    pub const BORDER: Color32 = Color32::from_rgb(189, 189, 189);
    pub const TEXT: Color32 = Color32::from_rgb(36, 36, 36);
    pub const MUTED: Color32 = Color32::from_rgb(96, 96, 96);
    /// Text/icon blue is darker than the decorative system accent so 13 pt
    /// links and white button labels remain readable on the light surfaces.
    pub const ACCENT: Color32 = Color32::from_rgb(0, 98, 210);
    pub const CONTROL_ACCENT: Color32 = Color32::from_rgb(0, 122, 255);
    pub const ACCENT_SOFT: Color32 = Color32::from_rgb(214, 232, 253);
    pub const ON_ACCENT: Color32 = Color32::WHITE;
    pub const SUCCESS: Color32 = Color32::from_rgb(28, 117, 52);
    pub const WARNING: Color32 = Color32::from_rgb(148, 82, 0);
    pub const ERROR: Color32 = Color32::from_rgb(186, 44, 38);
    /// Muted fills behind inline status banners (validation results).
    pub const SUCCESS_SOFT: Color32 = Color32::from_rgb(228, 242, 230);
    pub const ERROR_SOFT: Color32 = Color32::from_rgb(252, 232, 231);
    /// Dimming layer behind modal overlays such as the command palette.
    pub const SCRIM: Color32 = Color32::from_black_alpha(80);
}

/// Logical points, so dimensions continue to scale with the display's DPI.
pub mod metrics {
    pub const SPACE_XS: f32 = 4.;
    pub const SPACE_SM: f32 = 8.;
    pub const SPACE_MD: f32 = 12.;
    pub const SPACE_LG: f32 = 16.;
    pub const SPACE_XL: f32 = 24.;
    pub const SPACE_XXL: f32 = 32.;
    pub const CONTROL_RADIUS: f32 = 4.;
    pub const PANEL_RADIUS: f32 = 6.;
    pub const CONTROL_HEIGHT: f32 = 22.;
    pub const BODY_SIZE: f32 = 13.;
    pub const BODY_LINE_HEIGHT: f32 = 17.;
    pub const ICON_SIZE: f32 = 14.;
}

/// Reuse the fonts extracted for the shell. Static bytes need neither per-start
/// filesystem discovery nor a second copy of the assets in this package.
/// Latin uses SF; Chinese falls through to PingFang. Keep egui's default fonts
/// after them for symbols/emoji and its monospaced face for configuration code.
pub fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "macOS-SF".into(),
        egui::FontData::from_static(include_bytes!("../../clarion-shell/assets/macos/SFNS.ttf")),
    );
    fonts.font_data.insert(
        "macOS-PingFang-SC".into(),
        egui::FontData::from_static(include_bytes!(
            "../../clarion-shell/assets/macos/PingFangSC-Regular.otf"
        )),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .splice(0..0, ["macOS-SF".into(), "macOS-PingFang-SC".into()]);
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("macOS-PingFang-SC".into());
    fonts
}

/// Build the same style for the live application and headless visual checks.
pub fn style() -> egui::Style {
    let mut style = egui::Style {
        text_styles: [
            (TextStyle::Heading, FontId::proportional(20.)),
            (TextStyle::Body, FontId::proportional(metrics::BODY_SIZE)),
            (TextStyle::Button, FontId::proportional(metrics::BODY_SIZE)),
            (TextStyle::Small, FontId::proportional(11.)),
            (TextStyle::Monospace, FontId::monospace(12.)),
        ]
        .into(),
        ..Default::default()
    };
    style.spacing.item_spacing = egui::vec2(metrics::SPACE_SM, 6.);
    style.spacing.button_padding = egui::vec2(8., 3.);
    style.spacing.interact_size = egui::vec2(40., metrics::CONTROL_HEIGHT);
    style.spacing.window_margin = egui::Margin::same(metrics::SPACE_MD);
    style.spacing.menu_margin = egui::Margin::same(6.);
    style.spacing.indent = 16.;
    style.spacing.slider_width = 160.;
    style.spacing.combo_width = 180.;
    style.spacing.text_edit_width = 260.;
    style.spacing.icon_width = metrics::ICON_SIZE;
    style.spacing.icon_width_inner = 8.;
    style.spacing.icon_spacing = 5.;
    style.spacing.tooltip_width = 360.;
    style.spacing.scroll = egui::style::ScrollStyle::thin();
    style.spacing.scroll.bar_width = 7.;
    style.spacing.scroll.handle_min_length = 28.;
    style.interaction.tooltip_delay = 0.35;

    let mut visuals = Visuals::light();
    visuals.panel_fill = colors::BACKGROUND;
    visuals.window_fill = colors::BACKGROUND;
    visuals.window_stroke = Stroke::new(1.0_f32, colors::BORDER);
    visuals.window_rounding = Rounding::same(metrics::PANEL_RADIUS);
    visuals.menu_rounding = Rounding::same(metrics::CONTROL_RADIUS);
    visuals.faint_bg_color = colors::SURFACE_HOVER;
    visuals.extreme_bg_color = colors::SURFACE;
    visuals.code_bg_color = colors::SURFACE;
    visuals.hyperlink_color = colors::ACCENT;
    visuals.warn_fg_color = colors::WARNING;
    visuals.error_fg_color = colors::ERROR;
    visuals.text_cursor.stroke = Stroke::new(1.0_f32, colors::CONTROL_ACCENT);
    visuals.selection.bg_fill = colors::ACCENT_SOFT;
    visuals.selection.stroke = Stroke::new(1.0_f32, colors::TEXT);
    visuals.slider_trailing_fill = true;
    visuals.interact_cursor = None;

    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.rounding = Rounding::same(metrics::CONTROL_RADIUS);
        widget.expansion = 0.;
    }
    visuals.widgets.noninteractive.bg_fill = colors::SURFACE;
    visuals.widgets.noninteractive.weak_bg_fill = colors::BACKGROUND;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, colors::BORDER);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, colors::TEXT);
    visuals.widgets.inactive.bg_fill = colors::SURFACE;
    visuals.widgets.inactive.weak_bg_fill = colors::SURFACE;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, colors::BORDER);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, colors::TEXT);
    visuals.widgets.hovered.bg_fill = colors::SURFACE_HOVER;
    visuals.widgets.hovered.weak_bg_fill = colors::SURFACE_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, colors::CONTROL_ACCENT);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, colors::TEXT);
    visuals.widgets.active.bg_fill = colors::ACCENT_SOFT;
    visuals.widgets.active.weak_bg_fill = colors::ACCENT_SOFT;
    visuals.widgets.active.bg_stroke = Stroke::new(1.5_f32, colors::CONTROL_ACCENT);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, colors::TEXT);
    visuals.widgets.open = visuals.widgets.active;
    style.visuals = visuals;
    style
}

pub fn apply_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.set_style(style());
}

/// For descriptions and help copy that benefit from a relaxed, explicit line height.
pub fn paragraph(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text)
        .size(metrics::BODY_SIZE)
        .line_height(Some(metrics::BODY_LINE_HEIGHT))
        .color(colors::MUTED)
}

pub fn setup_custom_fonts_and_styles(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
    apply_style(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(foreground: egui::Color32, background: egui::Color32) -> f32 {
        let luminance = |color: egui::Color32| {
            let linear = |channel: u8| {
                let value = f32::from(channel) / 255.;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
        };
        let a = luminance(foreground);
        let b = luminance(background);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn theme_text_and_status_colors_remain_readable_on_all_surfaces() {
        for background in [
            colors::BACKGROUND,
            colors::SIDEBAR,
            colors::SURFACE,
            colors::SURFACE_HOVER,
        ] {
            for foreground in [
                colors::TEXT,
                colors::MUTED,
                colors::ACCENT,
                colors::SUCCESS,
                colors::WARNING,
                colors::ERROR,
            ] {
                assert!(
                    contrast(foreground, background) >= 4.5,
                    "{foreground:?} on {background:?} must remain readable"
                );
            }
        }
        assert!(contrast(colors::ON_ACCENT, colors::ACCENT) >= 4.5);
    }

    #[test]
    fn theme_can_style_a_headless_context_without_font_discovery() {
        let context = egui::Context::default();
        apply_style(&context);
        let output = context.run(
            egui::RawInput {
                system_theme: Some(egui::Theme::Dark),
                ..Default::default()
            },
            |ctx| {
                assert!(!ctx.style().visuals.dark_mode);
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.button("Save settings");
                    assert!(response.rect.height() >= metrics::CONTROL_HEIGHT);
                    ui.label(paragraph("Settings\nChanges apply after saving."));
                });
            },
        );
        assert!(output
            .shapes
            .iter()
            .any(|shape| matches!(shape.shape, egui::Shape::Text(_))));
    }

    #[test]
    fn bundled_sf_and_pingfang_parse_and_rasterize_latin_and_chinese() {
        let context = egui::Context::default();
        let mut fonts = font_definitions();
        // Isolate each bundled face: fallback tofu or an installed Noto font
        // must not accidentally make this compatibility check pass.
        let latin = egui::FontFamily::Name("test-sf-only".into());
        let chinese = egui::FontFamily::Name("test-pingfang-only".into());
        fonts
            .families
            .insert(latin.clone(), vec!["macOS-SF".into()]);
        fonts
            .families
            .insert(chinese.clone(), vec!["macOS-PingFang-SC".into()]);
        context.set_fonts(fonts);
        apply_style(&context);
        for scale in [1., 1.25, 2.] {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 200.),
                )),
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(scale);
            let output = context.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    for (family, sample) in [
                        (latin.clone(), "Clarion Settings 0123456789 Save & Apply"),
                        (
                            chinese.clone(),
                            "系统偏好设置显示器键盘鼠标窗口布局通用搜索，。",
                        ),
                    ] {
                        let font_id = FontId::new(metrics::BODY_SIZE, family);
                        ctx.fonts(|fonts| {
                            assert!(fonts.has_glyphs(&font_id, sample));
                            for ch in sample.chars() {
                                let width = fonts.glyph_width(&font_id, ch);
                                assert!(width.is_finite() && width > 0., "{ch}");
                            }
                        });
                        let response = ui.label(egui::RichText::new(sample).font(font_id));
                        assert!(response.rect.is_finite());
                        assert!(response.rect.width() > 100.);
                    }
                });
            });
            assert!(
                output.textures_delta.set.iter().any(|(_, delta)| {
                    matches!(&delta.image, egui::ImageData::Font(image)
                    if image.pixels.iter().any(|coverage| *coverage > 0.))
                }),
                "the bundled glyphs must produce real raster coverage at {scale}"
            );
            assert!(!context
                .tessellate(output.shapes, output.pixels_per_point)
                .is_empty());
        }
    }

    #[test]
    fn light_controls_keep_compact_metrics_and_visible_focus() {
        let style = style();
        assert!(!style.visuals.dark_mode);
        assert_eq!(style.visuals.panel_fill, colors::BACKGROUND);
        assert_eq!(style.text_styles[&TextStyle::Body].size, 13.);
        assert_eq!(style.spacing.interact_size.y, 22.);
        assert_eq!(style.visuals.widgets.inactive.rounding, Rounding::same(4.));
        assert_ne!(
            style.visuals.widgets.hovered.bg_stroke,
            style.visuals.widgets.inactive.bg_stroke
        );
        assert!(contrast(colors::SUCCESS, colors::SUCCESS_SOFT) >= 4.5);
        assert!(contrast(colors::ERROR, colors::ERROR_SOFT) >= 4.5);
    }
}
