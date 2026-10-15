//! Small vector icons, drawn in logical points without an icon or emoji font.

use eframe::egui::{
    self, Color32, Pos2, Rect, Response, Rounding, Sense, Shape, Stroke, Ui, Vec2, WidgetInfo,
    WidgetType,
};

use crate::theme::metrics;

/// Image-backed preference-pane icons are separate from the compact vector
/// toolbar API below. Realand-specific pages keep their own honest symbols.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PreferenceIcon {
    General,
    MissionControl,
    Keyboard,
    Trackpad,
    Displays,
    Advanced,
    Code,
    WindowLayout,
}

impl PreferenceIcon {
    pub const ALL: [Self; 8] = [
        Self::General,
        Self::MissionControl,
        Self::Keyboard,
        Self::Trackpad,
        Self::Displays,
        Self::Advanced,
        Self::Code,
        Self::WindowLayout,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::General => "通用",
            Self::MissionControl => "调度中心",
            Self::Keyboard => "键盘",
            Self::Trackpad => "触控板",
            Self::Displays => "显示器",
            Self::Advanced => "全部设置",
            Self::Code => "配置源码",
            Self::WindowLayout => "窗口布局",
        }
    }

    fn vector_fallback(self) -> Icon {
        match self {
            Self::General => Icon::Appearance,
            Self::MissionControl => Icon::Windows,
            Self::Keyboard => Icon::Input,
            Self::Trackpad => Icon::Input,
            Self::Displays => Icon::Display,
            Self::Advanced => Icon::Settings,
            Self::Code => Icon::Code,
            Self::WindowLayout => Icon::Layout,
        }
    }

    fn asset(self) -> Option<(&'static str, &'static [u8])> {
        match self {
            Self::General => Some((
                "macos-preference-general",
                include_bytes!("../assets/macos/general.png"),
            )),
            Self::MissionControl => Some((
                "macos-preference-mission-control",
                include_bytes!("../assets/macos/mission-control.png"),
            )),
            Self::Keyboard => Some((
                "macos-preference-keyboard",
                include_bytes!("../assets/macos/keyboard.png"),
            )),
            Self::Trackpad => Some((
                "macos-preference-trackpad",
                include_bytes!("../assets/macos/trackpad.png"),
            )),
            Self::Displays => Some((
                "macos-preference-displays",
                include_bytes!("../assets/macos/displays.png"),
            )),
            // These are Realand categories, not additional macOS preference panes.
            Self::Advanced | Self::Code | Self::WindowLayout => None,
        }
    }
}

#[derive(Clone)]
struct PreferenceTexture(Option<egui::TextureHandle>);

fn preference_texture(ctx: &egui::Context, icon: PreferenceIcon) -> Option<egui::TextureHandle> {
    let id = egui::Id::new(("realand-preference-texture", icon));
    if let Some(cached) = ctx.data(|data| data.get_temp::<PreferenceTexture>(id)) {
        return cached.0;
    }
    // Decoding/upload happens at most once per icon and context. Neither the
    // successful nor failed path re-reads or decodes assets on later frames.
    // Do not load a texture inside data_mut: that would re-enter Context's lock.
    let texture = icon.asset().and_then(|(name, bytes)| {
        let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
        let rgba = image.into_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        Some(ctx.load_texture(
            name,
            egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
            egui::TextureOptions::LINEAR,
        ))
    });
    ctx.data_mut(|data| data.insert_temp(id, PreferenceTexture(texture.clone())));
    texture
}

/// Paint a preference icon with its original transparency and aspect ratio.
pub fn paint_preference(ui: &Ui, rect: Rect, icon: PreferenceIcon) {
    if !rect.is_finite() || rect.width() < 2. || rect.height() < 2. || !ui.is_rect_visible(rect) {
        return;
    }
    if let Some(texture) = preference_texture(ui.ctx(), icon) {
        let native_size = texture.size_vec2();
        let scale = (rect.width() / native_size.x).min(rect.height() / native_size.y);
        let bounds = Rect::from_center_size(rect.center(), native_size * scale);
        ui.painter().image(
            texture.id(),
            bounds,
            Rect::from_min_max(Pos2::ZERO, egui::pos2(1., 1.)),
            Color32::WHITE,
        );
    } else {
        paint(
            ui,
            rect,
            icon.vector_fallback(),
            crate::theme::colors::ACCENT,
        );
    }
}

/// Reserve a preference icon region without creating a second click target.
pub fn preference_icon(ui: &mut Ui, icon: PreferenceIcon, size: f32) -> Response {
    let size = if size.is_finite() { size.max(0.) } else { 0. };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, ui.is_enabled(), icon.label()));
    paint_preference(ui, rect, icon);
    response
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Icon {
    Brand,
    Home,
    Windows,
    Layout,
    Appearance,
    Input,
    Display,
    Settings,
    Code,
    Search,
    Save,
    Check,
    Refresh,
    Undo,
    Warning,
    ChevronRight,
}

impl Icon {
    pub const ALL: [Self; 16] = [
        Self::Brand,
        Self::Home,
        Self::Windows,
        Self::Layout,
        Self::Appearance,
        Self::Input,
        Self::Display,
        Self::Settings,
        Self::Code,
        Self::Search,
        Self::Save,
        Self::Check,
        Self::Refresh,
        Self::Undo,
        Self::Warning,
        Self::ChevronRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Brand => "Realand",
            Self::Home => "总览",
            Self::Windows => "窗口",
            Self::Layout => "布局",
            Self::Appearance => "外观",
            Self::Input => "输入",
            Self::Display => "显示器",
            Self::Settings => "设置",
            Self::Code => "配置源码",
            Self::Search => "搜索",
            Self::Save => "保存",
            Self::Check => "已完成",
            Self::Refresh => "重新加载",
            Self::Undo => "撤销",
            Self::Warning => "提示",
            Self::ChevronRight => "打开",
        }
    }
}

/// Paint into an existing rectangle; useful for navigation rows and custom controls.
pub fn paint(ui: &Ui, rect: Rect, icon: Icon, color: Color32) {
    if !rect.is_finite() || rect.width() < 2. || rect.height() < 2. {
        return;
    }
    let side = rect.width().min(rect.height());
    let bounds = Rect::from_center_size(rect.center(), Vec2::splat(side));
    let unit = side / 24.;
    let point = |x: f32, y: f32| bounds.min + egui::vec2(x, y) * unit;
    let stroke = Stroke::new((1.75 * unit).max(0.8), color);
    let painter = ui.painter();
    let line = |points: &[(f32, f32)]| {
        painter.add(Shape::line(
            points.iter().map(|&(x, y)| point(x, y)).collect(),
            stroke,
        ));
    };
    let rounded_rect = |x: f32, y: f32, w: f32, h: f32, radius: f32| {
        painter.rect_stroke(
            Rect::from_min_size(point(x, y), egui::vec2(w, h) * unit),
            Rounding::same(radius * unit),
            stroke,
        );
    };
    let circle = |x: f32, y: f32, radius: f32| {
        painter.circle_stroke(point(x, y), radius * unit, stroke);
    };
    let arc = |x: f32, y: f32, radius: f32, start: f32, end: f32| {
        painter.add(Shape::line(
            (0..=24)
                .map(|i| {
                    let angle = start + (end - start) * i as f32 / 24.;
                    point(x + radius * angle.cos(), y + radius * angle.sin())
                })
                .collect(),
            stroke,
        ));
    };
    use std::f32::consts::PI;
    match icon {
        Icon::Brand => {
            rounded_rect(3., 3., 18., 18., 4.);
            line(&[
                (8., 17.),
                (8., 7.),
                (12.5, 7.),
                (15.5, 9.5),
                (12.5, 12.),
                (8., 12.),
            ]);
            line(&[(12., 12.), (16.5, 17.)]);
        }
        Icon::Home => {
            line(&[(3., 10.5), (12., 3.), (21., 10.5)]);
            line(&[(5.5, 9.), (5.5, 20.), (18.5, 20.), (18.5, 9.)]);
            line(&[(9.5, 20.), (9.5, 14.), (14.5, 14.), (14.5, 20.)]);
        }
        Icon::Windows => {
            line(&[(3., 14.), (3., 4.), (17., 4.)]);
            rounded_rect(7., 8., 14., 12., 2.);
            line(&[(7., 12.), (21., 12.)]);
        }
        Icon::Layout => {
            rounded_rect(3., 4., 18., 16., 2.);
            line(&[(10., 4.), (10., 20.)]);
            line(&[(10., 12.), (21., 12.)]);
        }
        Icon::Appearance => {
            circle(12., 12., 4.5);
            for i in 0..8 {
                let angle = i as f32 * PI / 4.;
                line(&[
                    (12. + 7.5 * angle.cos(), 12. + 7.5 * angle.sin()),
                    (12. + 10. * angle.cos(), 12. + 10. * angle.sin()),
                ]);
            }
        }
        Icon::Input => {
            rounded_rect(3., 5., 18., 14., 2.);
            for y in [8.5, 12.] {
                for x in [6., 10., 14., 18.] {
                    line(&[(x - 0.5, y), (x + 0.5, y)]);
                }
            }
            line(&[(7., 16.), (17., 16.)]);
        }
        Icon::Display => {
            rounded_rect(3., 3.5, 18., 13., 2.);
            line(&[(12., 16.5), (12., 21.)]);
            line(&[(7.5, 21.), (16.5, 21.)]);
        }
        Icon::Settings => {
            for (y, x) in [(5., 9.), (12., 16.), (19., 7.)] {
                line(&[(3., y), (x - 2.5, y)]);
                line(&[(x + 2.5, y), (21., y)]);
                circle(x, y, 2.5);
            }
        }
        Icon::Code => {
            line(&[(7., 7.), (3., 12.), (7., 17.)]);
            line(&[(17., 7.), (21., 12.), (17., 17.)]);
            line(&[(14., 4.), (10., 20.)]);
        }
        Icon::Search => {
            circle(10.5, 10.5, 6.5);
            line(&[(15.5, 15.5), (21., 21.)]);
        }
        Icon::Save => {
            line(&[
                (4., 3.5),
                (17., 3.5),
                (20.5, 7.),
                (20.5, 20.5),
                (4., 20.5),
                (4., 3.5),
            ]);
            line(&[(8., 3.5), (8., 9.), (16., 9.), (16., 3.5)]);
            rounded_rect(8., 13., 8., 7.5, 1.);
        }
        Icon::Check => line(&[(4., 12.5), (9., 17.5), (20., 6.5)]),
        Icon::Refresh => {
            arc(12., 12., 8., -0.2 * PI, 1.55 * PI);
            line(&[(20., 3.), (20., 8.), (15., 8.)]);
        }
        Icon::Undo => {
            line(&[(8., 3.5), (3.5, 8.), (8., 12.5)]);
            line(&[(3.5, 8.), (13.5, 8.)]);
            arc(13.5, 14., 6., -0.5 * PI, 0.5 * PI);
            line(&[(13.5, 20.), (10., 20.)]);
        }
        Icon::Warning => {
            line(&[(12., 3.), (22., 20.), (2., 20.), (12., 3.)]);
            line(&[(12., 9.), (12., 14.)]);
            painter.circle_filled(point(12., 17.), unit, color);
        }
        Icon::ChevronRight => line(&[(9., 5.), (16., 12.), (9., 19.)]),
    }
}

/// Reserve an icon-sized region without adding a background or click behavior.
pub fn icon(ui: &mut Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let size = if size.is_finite() { size.max(0.) } else { 0. };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, ui.is_enabled(), icon.label()));
    if ui.is_rect_visible(rect) {
        paint(ui, rect, icon, color);
    }
    response
}

/// A regular, keyboard-focusable button with a vector icon and an accessible text label.
pub fn button(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let padding = ui.spacing().button_padding;
    let desired = egui::vec2(
        padding.x * 2. + metrics::ICON_SIZE + metrics::SPACE_SM + galley.size().x,
        (galley.size().y.max(metrics::ICON_SIZE) + padding.y * 2.)
            .max(ui.spacing().interact_size.y),
    );
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        ui.painter().rect(
            rect,
            visuals.rounding,
            visuals.weak_bg_fill,
            visuals.bg_stroke,
        );
        let icon_rect = Rect::from_center_size(
            Pos2::new(
                rect.left() + padding.x + metrics::ICON_SIZE / 2.,
                rect.center().y,
            ),
            Vec2::splat(metrics::ICON_SIZE),
        );
        paint(ui, icon_rect, icon, visuals.text_color());
        ui.painter().galley(
            Pos2::new(
                icon_rect.right() + metrics::SPACE_SM,
                rect.center().y - galley.size().y / 2.,
            ),
            galley,
            visuals.text_color(),
        );
    }
    response
}

/// Emphasize the page's main action while preserving the regular button's behavior.
pub fn primary_button(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        for widget in [
            &mut widgets.noninteractive,
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.open,
        ] {
            widget.bg_fill = crate::theme::colors::ACCENT;
            widget.weak_bg_fill = crate::theme::colors::ACCENT;
            widget.fg_stroke.color = crate::theme::colors::ON_ACCENT;
        }
        // egui uses the hovered visuals for keyboard focus too.
        widgets.hovered.bg_stroke = Stroke::new(1.5_f32, crate::theme::colors::ON_ACCENT);
        widgets.active.bg_stroke = widgets.hovered.bg_stroke;
        button(ui, icon, label)
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_assets_decode_with_visible_pixels_and_transparency() {
        let mut original_assets = 0;
        for icon in PreferenceIcon::ALL {
            if let Some((name, bytes)) = icon.asset() {
                original_assets += 1;
                let rgba = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
                    .unwrap_or_else(|error| panic!("{name}: {error}"))
                    .into_rgba8();
                assert!(rgba.width() > 0 && rgba.height() > 0);
                assert!(rgba.width() <= 1024 && rgba.height() <= 1024);
                assert!(rgba.pixels().any(|pixel| pixel[3] > 0), "{name}");
                assert!(rgba.pixels().any(|pixel| pixel[3] == 0), "{name}");
            }
        }
        assert_eq!(original_assets, 5);
        assert!(PreferenceIcon::Code.asset().is_none());
        assert!(PreferenceIcon::WindowLayout.asset().is_none());
    }

    #[test]
    fn preference_textures_upload_once_per_context_and_keep_fractional_dpi_bounds() {
        let expected_uploads = PreferenceIcon::ALL
            .iter()
            .filter(|icon| icon.asset().is_some())
            .count();
        // A second egui context needs its own GPU handles, while repeated
        // frames and monitor DPI changes within one context reuse the textures.
        for _ in 0..2 {
            let context = egui::Context::default();
            let mut original_ids = Vec::new();
            for (frame, scale) in [1., 1.25, 1.5, 2., 1.].into_iter().enumerate() {
                let mut input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(560., 100.))),
                    ..Default::default()
                };
                input
                    .viewports
                    .get_mut(&egui::ViewportId::ROOT)
                    .unwrap()
                    .native_pixels_per_point = Some(scale);
                let mut image_bounds = Vec::new();
                let mut texture_ids = Vec::new();
                let output = context.run(input, |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(ctx, |ui| {
                            for (index, icon) in PreferenceIcon::ALL.into_iter().enumerate() {
                                let rect = Rect::from_min_size(
                                    egui::pos2(10. + index as f32 * 70., 20.),
                                    egui::vec2(48., 40.),
                                );
                                paint_preference(ui, rect, icon);
                                if let Some(texture) = preference_texture(ctx, icon) {
                                    texture_ids.push(texture.id());
                                    image_bounds.push((texture.id(), rect));
                                }
                            }
                        });
                });
                let uploads = output
                    .textures_delta
                    .set
                    .iter()
                    .filter(|(_, delta)| matches!(delta.image, egui::ImageData::Color(_)))
                    .count();
                assert_eq!(uploads, if frame == 0 { expected_uploads } else { 0 });
                if frame == 0 {
                    original_ids = texture_ids.clone();
                }
                assert_eq!(texture_ids, original_ids);
                for primitive in context.tessellate(output.shapes, output.pixels_per_point) {
                    if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
                        if let Some((_, bounds)) =
                            image_bounds.iter().find(|(id, _)| *id == mesh.texture_id)
                        {
                            assert!(!mesh.vertices.is_empty());
                            assert!(mesh
                                .vertices
                                .iter()
                                .all(|vertex| bounds.contains(vertex.pos)));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn invisible_and_invalid_preference_icons_do_not_decode_or_upload() {
        let context = egui::Context::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(100.))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(ctx, |ui| {
                        for rect in [
                            Rect::from_min_size(Pos2::ZERO, Vec2::ZERO),
                            Rect::from_min_size(Pos2::ZERO, egui::vec2(-20., 20.)),
                            Rect::from_min_size(Pos2::ZERO, egui::vec2(f32::NAN, 20.)),
                            Rect::from_min_size(egui::pos2(300., 300.), Vec2::splat(40.)),
                        ] {
                            paint_preference(ui, rect, PreferenceIcon::General);
                        }
                    });
            },
        );
        assert!(output
            .textures_delta
            .set
            .iter()
            .all(|(_, delta)| !matches!(delta.image, egui::ImageData::Color(_))));
        assert!(context
            .tessellate(output.shapes, output.pixels_per_point)
            .is_empty());
        assert!(context
            .data(|data| data.get_temp::<PreferenceTexture>(egui::Id::new((
                "realand-preference-texture",
                PreferenceIcon::General
            )),))
            .is_none());
    }

    #[test]
    fn vector_icons_stay_inside_their_allocated_bounds_at_fractional_dpi() {
        let context = egui::Context::default();
        crate::theme::apply_style(&context);
        for scale in [1., 1.25, 1.5, 2.] {
            for size in [14., 18., 24., 32.] {
                for icon in Icon::ALL {
                    let rect =
                        Rect::from_min_size(egui::pos2(20., 20.), egui::vec2(size * 1.4, size));
                    let mut input = egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(100., 100.))),
                        ..Default::default()
                    };
                    input
                        .viewports
                        .get_mut(&egui::ViewportId::ROOT)
                        .unwrap()
                        .native_pixels_per_point = Some(scale);
                    let output = context.run(input, |ctx| {
                        egui::CentralPanel::default()
                            .frame(egui::Frame::none())
                            .show(ctx, |ui| {
                                paint(ui, rect, icon, crate::theme::colors::ACCENT);
                            });
                    });
                    assert!(!output
                        .shapes
                        .iter()
                        .any(|shape| matches!(shape.shape, Shape::Text(_))));
                    let primitives = context.tessellate(output.shapes, output.pixels_per_point);
                    let mut vertices = 0;
                    for primitive in primitives {
                        if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
                            for vertex in mesh.vertices {
                                vertices += 1;
                                assert!(vertex.pos.is_finite(), "{icon:?} at {size}/{scale}");
                                // Tessellation adds a one-physical-pixel antialias fringe.
                                assert!(
                                    rect.expand(1. / scale).contains(vertex.pos),
                                    "{icon:?} at {size}/{scale}: {:?} outside {rect:?}",
                                    vertex.pos
                                );
                            }
                        }
                    }
                    assert!(
                        vertices > 0,
                        "{icon:?} must remain visible at {size}/{scale}"
                    );
                }
            }
        }
    }

    #[test]
    fn invalid_icon_rectangles_do_not_emit_shapes() {
        let context = egui::Context::default();
        let output = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    for rect in [
                        Rect::from_min_size(Pos2::ZERO, Vec2::ZERO),
                        Rect::from_min_size(Pos2::ZERO, egui::vec2(-20., 20.)),
                        Rect::from_min_size(Pos2::ZERO, egui::vec2(f32::NAN, 20.)),
                    ] {
                        paint(ui, rect, Icon::Save, Color32::WHITE);
                    }
                });
        });
        // The empty panel still emits a transparent frame shape; invalid icons
        // must contribute no tessellated drawing geometry.
        assert!(context
            .tessellate(output.shapes, output.pixels_per_point)
            .is_empty());
    }

    #[test]
    fn icon_buttons_keep_pointer_keyboard_and_disabled_behavior() {
        for paint_button in [button, primary_button] {
            let context = egui::Context::default();
            crate::theme::apply_style(&context);
            let mut rect = Rect::NOTHING;
            let mut id = egui::Id::NULL;
            let mut clicked = false;
            let mut enabled = true;
            let mut frame = |input| {
                let _ = context.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.add_enabled_ui(enabled, |ui| {
                            let response = paint_button(ui, Icon::Save, "Save settings");
                            rect = response.rect;
                            id = response.id;
                            clicked = response.clicked();
                        });
                    });
                });
                (rect, id, clicked)
            };
            let (rect, id, _) = frame(Default::default());
            let pointer = |pressed| egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            };
            assert!(!frame(pointer(true)).2);
            assert!(frame(pointer(false)).2);
            context.memory_mut(|memory| memory.request_focus(id));
            assert!(
                frame(egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                })
                .2
            );
            enabled = false;
            for pressed in [true, false] {
                let _ = context.run(pointer(pressed), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.add_enabled_ui(enabled, |ui| {
                            assert!(!paint_button(ui, Icon::Save, "Save settings").clicked());
                        });
                    });
                });
            }
        }
    }
}
