use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, FontId, Rounding, Stroke, TextStyle, Visuals};

/// Shared colors for panels, controls and custom-painted navigation.
pub mod colors {
    use std::cell::Cell;

    use eframe::egui::Color32;

    thread_local! {
        /// The pane draws itself with one palette at a time; the appearance is
        /// resolved once per run, before the first frame.
        static DARK: Cell<bool> = const { Cell::new(false) };
    }

    /// Selects the palette. Light stays the default so a caller that never
    /// resolves an appearance keeps the previous rendering.
    pub fn set_dark(dark: bool) {
        DARK.with(|value| value.set(dark));
    }

    pub fn is_dark() -> bool {
        DARK.with(|value| value.get())
    }

    /// Picks the dark value only while the dark palette is selected.
    fn shade(light: Color32, dark: Color32) -> Color32 {
        if is_dark() {
            dark
        } else {
            light
        }
    }

    pub fn background() -> Color32 {
        shade(
            Color32::from_rgb(236, 236, 236),
            Color32::from_rgb(30, 30, 32),
        )
    }

    pub fn sidebar() -> Color32 {
        shade(
            Color32::from_rgb(230, 230, 230),
            Color32::from_rgb(37, 37, 40),
        )
    }

    pub fn surface() -> Color32 {
        shade(Color32::WHITE, Color32::from_rgb(44, 44, 46))
    }

    pub fn surface_hover() -> Color32 {
        shade(
            Color32::from_rgb(246, 246, 246),
            Color32::from_rgb(58, 58, 60),
        )
    }

    pub fn border() -> Color32 {
        shade(
            Color32::from_rgb(189, 189, 189),
            Color32::from_rgb(84, 84, 88),
        )
    }

    pub fn text() -> Color32 {
        shade(
            Color32::from_rgb(36, 36, 36),
            Color32::from_rgb(242, 242, 247),
        )
    }

    pub fn muted() -> Color32 {
        shade(
            Color32::from_rgb(96, 96, 96),
            Color32::from_rgb(170, 170, 175),
        )
    }

    /// Text/icon blue stays darker than the decorative accent on light
    /// surfaces so 13 pt links and white button labels remain readable.
    pub fn accent() -> Color32 {
        shade(
            Color32::from_rgb(0, 98, 210),
            Color32::from_rgb(10, 132, 255),
        )
    }

    pub fn control_accent() -> Color32 {
        shade(
            Color32::from_rgb(0, 122, 255),
            Color32::from_rgb(10, 132, 255),
        )
    }

    pub fn accent_soft() -> Color32 {
        shade(
            Color32::from_rgb(214, 232, 253),
            Color32::from_rgb(28, 60, 102),
        )
    }

    pub fn on_accent() -> Color32 {
        Color32::WHITE
    }

    pub fn success() -> Color32 {
        shade(
            Color32::from_rgb(28, 117, 52),
            Color32::from_rgb(48, 209, 88),
        )
    }

    pub fn warning() -> Color32 {
        shade(
            Color32::from_rgb(148, 82, 0),
            Color32::from_rgb(255, 214, 10),
        )
    }

    pub fn error() -> Color32 {
        shade(
            Color32::from_rgb(186, 44, 38),
            Color32::from_rgb(255, 69, 58),
        )
    }

    /// Muted fills behind inline status banners (validation results).
    pub fn success_soft() -> Color32 {
        shade(
            Color32::from_rgb(228, 242, 230),
            Color32::from_rgb(28, 58, 34),
        )
    }

    pub fn error_soft() -> Color32 {
        shade(
            Color32::from_rgb(252, 232, 231),
            Color32::from_rgb(58, 31, 30),
        )
    }

    /// Dimming layer behind modal overlays such as the command palette.
    pub fn scrim() -> Color32 {
        shade(
            Color32::from_black_alpha(80),
            Color32::from_black_alpha(120),
        )
    }
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

/// Resolve a face through fontconfig, returning the family fontconfig chose
/// alongside the face itself.
fn system_font_match(pattern: &str) -> Option<(String, egui::FontData)> {
    let output = std::process::Command::new("fc-match")
        .arg("-f")
        .arg("%{family[0]}\u{1f}%{file}\u{1f}%{index}")
        .arg(pattern)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let listing = String::from_utf8(output.stdout).ok()?;
    let mut fields = listing.trim().split('\u{1f}');
    let family = fields.next()?.trim().to_owned();
    let path = fields.next()?.trim();
    let index = fields
        .next()
        .and_then(|field| field.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let bytes = std::fs::read(path).ok()?;
    // Only accept TrueType/OpenType faces: a stale fontconfig entry pointing at
    // an unrelated file must not panic egui's font parser on the first frame.
    const MAGIC: [[u8; 4]; 4] = [[0, 1, 0, 0], *b"true", *b"ttcf", *b"OTTO"];
    let magic: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    if !MAGIC.contains(&magic) {
        return None;
    }
    Some((
        family,
        egui::FontData {
            index,
            ..egui::FontData::from_owned(bytes)
        },
    ))
}

/// Families whose licences allow use in a GPL-3.0-or-later desktop. SF Pro,
/// PingFang and Microsoft YaHei are deliberately absent: fontconfig often
/// aliases a missing request to one of them, so the resolved family is checked
/// instead of trusting the requested pattern.
const OPEN_FONT_FAMILIES: &[&str] = &[
    "noto",
    "source han",
    "sarasa",
    "wenquanyi",
    "inter",
    "dejavu",
    "liberation",
    "harmonyos",
];

fn is_open_font_family(family: &str) -> bool {
    let family = family.to_lowercase();
    OPEN_FONT_FAMILIES.iter().any(|open| family.contains(open))
}

/// Load the first requested pattern that fontconfig resolves to an
/// open-licensed family. A missing pattern must not silently substitute a
/// proprietary face, so unmatched results are skipped.
fn open_source_font(patterns: &[&str]) -> Option<egui::FontData> {
    patterns.iter().find_map(|pattern| {
        let (family, font) = system_font_match(pattern)?;
        is_open_font_family(&family).then_some(font)
    })
}

/// Clarion ships no font binaries. The UI prefers open-licensed system faces
/// (Noto, Inter, DejaVu, Source Han, Sarasa, WenQuanYi) and keeps egui's own
/// open-source defaults as the final fallback. Proprietary SF Pro, PingFang and
/// Microsoft YaHei are never loaded, even when fontconfig offers them as an
/// alias for a missing family.
pub fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let mut proportional_prefix: Vec<String> = Vec::new();
    if let Some(font) = open_source_font(&["Noto Sans", "Inter", "DejaVu Sans", "Liberation Sans"])
    {
        fonts.font_data.insert("clarion-ui-latin".into(), font);
        proportional_prefix.push("clarion-ui-latin".into());
    }
    if let Some(font) = open_source_font(&[
        "Noto Sans CJK SC",
        "Noto Sans CJK TC",
        "Noto Sans CJK JP",
        "Source Han Sans SC",
        "Source Han Sans CN",
        "Sarasa Gothic SC",
        "WenQuanYi Micro Hei",
        "HarmonyOS Sans SC",
    ]) {
        fonts.font_data.insert("clarion-ui-cjk".into(), font);
        proportional_prefix.push("clarion-ui-cjk".into());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("clarion-ui-cjk".into());
    }
    if !proportional_prefix.is_empty() {
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .splice(0..0, proportional_prefix);
    }
    fonts
}

/// Build the same style for the live application and headless visual checks.
pub fn style() -> egui::Style {
    let mut style = egui::Style {
        visuals: if colors::is_dark() {
            Visuals::dark()
        } else {
            Visuals::light()
        },
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

    let mut visuals = if colors::is_dark() {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    visuals.panel_fill = colors::background();
    visuals.window_fill = colors::background();
    visuals.window_stroke = Stroke::new(1.0_f32, colors::border());
    visuals.window_rounding = Rounding::same(metrics::PANEL_RADIUS);
    visuals.menu_rounding = Rounding::same(metrics::CONTROL_RADIUS);
    visuals.faint_bg_color = colors::surface_hover();
    visuals.extreme_bg_color = colors::surface();
    visuals.code_bg_color = colors::surface();
    visuals.hyperlink_color = colors::accent();
    visuals.warn_fg_color = colors::warning();
    visuals.error_fg_color = colors::error();
    visuals.text_cursor.stroke = Stroke::new(1.0_f32, colors::control_accent());
    visuals.selection.bg_fill = colors::accent_soft();
    visuals.selection.stroke = Stroke::new(1.0_f32, colors::text());
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
    visuals.widgets.noninteractive.bg_fill = colors::surface();
    visuals.widgets.noninteractive.weak_bg_fill = colors::background();
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, colors::border());
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, colors::text());
    visuals.widgets.inactive.bg_fill = colors::surface();
    visuals.widgets.inactive.weak_bg_fill = colors::surface();
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, colors::border());
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, colors::text());
    visuals.widgets.hovered.bg_fill = colors::surface_hover();
    visuals.widgets.hovered.weak_bg_fill = colors::surface_hover();
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, colors::control_accent());
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, colors::text());
    visuals.widgets.active.bg_fill = colors::accent_soft();
    visuals.widgets.active.weak_bg_fill = colors::accent_soft();
    visuals.widgets.active.bg_stroke = Stroke::new(1.5_f32, colors::control_accent());
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, colors::text());
    visuals.widgets.open = visuals.widgets.active;
    style.visuals = visuals;
    style
}

/// Selects the palette and the egui visuals for `dark`, then installs the
/// style built from it. Callers resolve the appearance once per run.
pub fn apply_style(ctx: &egui::Context, dark: bool) {
    colors::set_dark(dark);
    ctx.set_theme(if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    ctx.set_style(style());
}

/// For descriptions and help copy that benefit from a relaxed, explicit line height.
pub fn paragraph(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text)
        .size(metrics::BODY_SIZE)
        .line_height(Some(metrics::BODY_LINE_HEIGHT))
        .color(colors::muted())
}

pub fn setup_custom_fonts_and_styles(ctx: &egui::Context, dark: bool) {
    ctx.set_fonts(font_definitions());
    apply_style(ctx, dark);
}

/// The desktop shell keeps its appearance in `preferences.json`; the settings
/// window follows the same file so the two never disagree.
fn shell_config_root() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            return Some(path);
        }
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .map(|home| home.join(".config"))
}

/// Canonical location, with the pre-rename directory kept as a read-only
/// fallback for existing sessions.
fn preferences_path_in(root: &Path) -> PathBuf {
    let canonical = root.join("clarion-shell").join("preferences.json");
    let legacy = root.join("realand-shell").join("preferences.json");
    if !canonical.exists() && legacy.exists() {
        legacy
    } else {
        canonical
    }
}

fn shell_preferences_path() -> Option<PathBuf> {
    shell_config_root().map(|root| preferences_path_in(&root))
}

/// Reads the raw `theme` value. `None` means the file is unavailable; the inner
/// `None` means it asks to follow the platform (or predates the key).
fn read_theme(path: &Path) -> Option<Option<String>> {
    const LIMIT: u64 = 64 * 1024;
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > LIMIT {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(
        json.get("theme")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    )
}

fn explicit_theme(theme: Option<&str>) -> Option<bool> {
    match theme? {
        "dark" => Some(true),
        "light" => Some(false),
        _ => None,
    }
}

/// The appearance declared by the desktop, when it is not "follow system".
pub fn desktop_dark() -> Option<bool> {
    explicit_theme(read_theme(&shell_preferences_path()?)?.as_deref())
}

/// Follows the desktop's saved appearance for the lifetime of one window.
///
/// The launcher already applied the appearance at startup, so the value read
/// when the window opens is only recorded: a preferences write that races with
/// launch cannot override the value the window opened with. Values written
/// afterwards are applied in place, which is what keeps a running settings
/// window in sync with the desktop's light/dark switch — including the first
/// write that creates the preferences file.
pub struct DesktopThemeWatcher {
    path: Option<PathBuf>,
    seen: Option<Option<String>>,
    last_poll: Option<Instant>,
}

impl Default for DesktopThemeWatcher {
    fn default() -> Self {
        let path = shell_preferences_path();
        let seen = path.as_deref().and_then(read_theme);
        Self {
            path,
            seen,
            last_poll: None,
        }
    }
}

impl DesktopThemeWatcher {
    #[cfg(test)]
    fn at(path: PathBuf) -> Self {
        let seen = read_theme(&path);
        Self {
            path: Some(path),
            seen,
            last_poll: None,
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if self
            .last_poll
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            return;
        }
        self.last_poll = Some(now);
        self.poll_now(ctx);
    }

    fn poll_now(&mut self, ctx: &egui::Context) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let Some(theme) = read_theme(path) else {
            return;
        };
        if let Some(seen) = &self.seen {
            if seen == &theme {
                // The file did not change. A "system" preference still follows
                // the platform, where the egui backend reports one.
                if theme.is_none() {
                    if let Some(dark) = platform_dark(ctx) {
                        if colors::is_dark() != dark {
                            apply_style(ctx, dark);
                            ctx.request_repaint();
                        }
                    }
                }
                return;
            }
        }
        let dark = explicit_theme(theme.as_deref()).or_else(|| platform_dark(ctx));
        self.seen = Some(theme);
        if let Some(dark) = dark {
            if colors::is_dark() != dark {
                apply_style(ctx, dark);
                ctx.request_repaint();
            }
        }
    }
}

fn platform_dark(ctx: &egui::Context) -> Option<bool> {
    ctx.system_theme().map(|theme| theme == egui::Theme::Dark)
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
    fn dark_palette_changes_surfaces_and_follows_the_egui_theme() {
        let light_background = {
            colors::set_dark(false);
            colors::background()
        };
        let light_text = colors::text();
        colors::set_dark(true);
        assert_ne!(colors::background(), light_background);
        assert_ne!(colors::text(), light_text);
        assert!(colors::background().r() < 64, "dark surfaces stay dark");

        // The style builder follows the palette without a context...
        assert!(style().visuals.dark_mode);
        colors::set_dark(false);
        assert!(!style().visuals.dark_mode);

        // ...and the installed context reports it once a frame has run.
        let context = egui::Context::default();
        apply_style(&context, true);
        let _ = context.run(egui::RawInput::default(), |ctx| {
            assert!(ctx.style().visuals.dark_mode);
        });
        apply_style(&context, false);
        let _ = context.run(egui::RawInput::default(), |ctx| {
            assert!(!ctx.style().visuals.dark_mode);
        });
        assert_eq!(colors::background(), light_background);
    }

    #[test]
    fn theme_can_style_a_headless_context_without_font_discovery() {
        let context = egui::Context::default();
        apply_style(&context, false);
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
    fn proprietary_faces_are_never_selected_even_when_fontconfig_aliases_them() {
        // fontconfig aliases unknown requests to whatever it has configured,
        // so the family returned for a request decides whether it is used.
        for open in [
            "Noto Sans",
            "Noto Sans CJK SC",
            "Source Han Sans SC",
            "Sarasa Gothic SC",
            "Noto Sans CJK JP",
            "DejaVu Sans",
            "Liberation Sans",
            "Inter",
            "WenQuanYi Micro Hei",
        ] {
            assert!(is_open_font_family(open), "{open} must stay available");
        }
        for proprietary in [
            "SF Pro Text",
            "PingFang SC",
            "Microsoft YaHei",
            "Helvetica Neue",
            "Apple SD Gothic Neo",
        ] {
            assert!(
                !is_open_font_family(proprietary),
                "{proprietary} must never be loaded"
            );
        }
    }

    #[test]
    fn system_font_resolution_rasterizes_without_bundled_apple_assets() {
        // Clarion ships no font binaries. Whatever fontconfig returns must still
        // produce real Latin coverage at every supported DPI, and Chinese text
        // must be covered whenever an open-licensed CJK face was resolved.
        let context = egui::Context::default();
        let fonts = font_definitions();
        let has_cjk = fonts.font_data.contains_key("clarion-ui-cjk");
        assert!(
            fonts
                .families
                .get(&egui::FontFamily::Proportional)
                .is_some_and(|list| !list.is_empty()),
            "proportional family must always resolve"
        );
        context.set_fonts(fonts);
        apply_style(&context, false);
        let latin = FontId::new(metrics::BODY_SIZE, egui::FontFamily::Proportional);
        let mut samples = vec!["Clarion Settings 0123456789 Save & Apply"];
        if has_cjk {
            samples.push("系统偏好设置显示器键盘鼠标窗口布局通用搜索，。");
        }
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
                    for sample in &samples {
                        let font_id = latin.clone();
                        ctx.fonts(|fonts| {
                            assert!(fonts.has_glyphs(&font_id, sample));
                            for ch in sample.chars() {
                                let width = fonts.glyph_width(&font_id, ch);
                                assert!(width.is_finite() && width > 0., "{ch}");
                            }
                        });
                        let response = ui.label(egui::RichText::new(*sample).font(font_id));
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
                "resolved glyphs must produce real raster coverage at {scale}"
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
        assert_eq!(style.visuals.panel_fill, colors::background());
        assert_eq!(style.text_styles[&TextStyle::Body].size, 13.);
        assert_eq!(style.spacing.interact_size.y, 22.);
        assert_eq!(style.visuals.widgets.inactive.rounding, Rounding::same(4.));
        assert_ne!(
            style.visuals.widgets.hovered.bg_stroke,
            style.visuals.widgets.inactive.bg_stroke
        );
        assert!(contrast(colors::success(), colors::success_soft()) >= 4.5);
        assert!(contrast(colors::error(), colors::error_soft()) >= 4.5);
    }

    #[test]
    fn desktop_theme_reads_the_canonical_file_and_the_legacy_fallback() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            preferences_path_in(directory.path()),
            directory
                .path()
                .join("clarion-shell")
                .join("preferences.json")
        );

        let legacy = directory
            .path()
            .join("realand-shell")
            .join("preferences.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, br#"{"theme":"dark"}"#).unwrap();
        assert_eq!(preferences_path_in(directory.path()), legacy);

        assert_eq!(read_theme(&legacy).flatten().as_deref(), Some("dark"));
        assert_eq!(explicit_theme(Some("dark")), Some(true));
        assert_eq!(explicit_theme(Some("light")), Some(false));
        assert_eq!(explicit_theme(Some("system")), None);
        assert_eq!(explicit_theme(None), None);

        std::fs::write(&legacy, br#"{"theme":"system"}"#).unwrap();
        assert_eq!(read_theme(&legacy).flatten().as_deref(), Some("system"));
        assert_eq!(explicit_theme(Some("system")), None);
        std::fs::write(&legacy, b"not json").unwrap();
        assert!(read_theme(&legacy).is_none());
    }

    #[test]
    fn the_watcher_applies_a_desktop_theme_change_after_the_initial_read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");
        std::fs::write(&path, br#"{"theme":"light"}"#).unwrap();
        let context = egui::Context::default();
        apply_style(&context, false);

        // The launcher already chose the appearance; the value present when
        // the window opens is only recorded, so a preferences write racing
        // with launch cannot override it.
        let mut watcher = DesktopThemeWatcher::at(path.clone());
        watcher.poll_now(&context);
        assert!(!colors::is_dark());

        std::fs::write(&path, br#"{"theme":"dark"}"#).unwrap();
        watcher.poll_now(&context);
        assert!(colors::is_dark());
        let _ = context.run(egui::RawInput::default(), |ctx| {
            assert!(ctx.style().visuals.dark_mode);
        });

        // "system" without a platform report keeps the appearance in place.
        std::fs::write(&path, br#"{"theme":"system"}"#).unwrap();
        watcher.poll_now(&context);
        assert!(colors::is_dark());

        // The first write that creates the file is a real change: the window
        // was opened before any preference existed, so it must not be ignored.
        let created = directory.path().join("created.json");
        let mut fresh = DesktopThemeWatcher::at(created.clone());
        std::fs::write(&created, br#"{"theme":"light"}"#).unwrap();
        fresh.poll_now(&context);
        assert!(!colors::is_dark());

        apply_style(&context, false);
        assert!(!colors::is_dark());
    }
}
