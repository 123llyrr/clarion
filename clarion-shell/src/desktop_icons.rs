// SPDX-License-Identifier: GPL-3.0-or-later
//! Embedded desktop artwork. Each immutable image gets a stable GPUI asset key,
//! so drawing another file never reopens an icon or starts another decode.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex, OnceLock};

use gpui::prelude::*;
use gpui::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DesktopIconKind {
    Folder,
    Document,
    Image,
    Application,
}

fn embedded(kind: DesktopIconKind) -> &'static [u8] {
    match kind {
        DesktopIconKind::Folder => include_bytes!("../assets/icons/desktop-folder.png"),
        DesktopIconKind::Document => include_bytes!("../assets/icons/desktop-document.png"),
        DesktopIconKind::Image => include_bytes!("../assets/icons/desktop-image.png"),
        DesktopIconKind::Application => include_bytes!("../assets/icons/desktop-application.png"),
    }
}

fn image(kind: DesktopIconKind) -> Arc<Image> {
    static FOLDER: OnceLock<Arc<Image>> = OnceLock::new();
    static DOCUMENT: OnceLock<Arc<Image>> = OnceLock::new();
    static PICTURE: OnceLock<Arc<Image>> = OnceLock::new();
    static APPLICATION: OnceLock<Arc<Image>> = OnceLock::new();

    let cache = match kind {
        DesktopIconKind::Folder => &FOLDER,
        DesktopIconKind::Document => &DOCUMENT,
        DesktopIconKind::Image => &PICTURE,
        DesktopIconKind::Application => &APPLICATION,
    };
    cache
        .get_or_init(|| {
            Arc::new(Image::from_bytes(
                ImageFormat::Png,
                embedded(kind).to_vec(),
            ))
        })
        .clone()
}

fn rotated_image(kind: DesktopIconKind, degrees: f32) -> Arc<Image> {
    static CACHE: OnceLock<Mutex<HashMap<(DesktopIconKind, i16), Arc<Image>>>> =
        OnceLock::new();
    let key = (kind, (degrees * 10.).round() as i16);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(image) = cache.lock().unwrap().get(&key).cloned() {
        return image;
    }
    let rotated = rotate_png(embedded(kind), degrees);
    let image = Arc::new(Image::from_bytes(ImageFormat::Png, rotated));
    cache.lock().unwrap().insert(key, image.clone());
    image
}

fn rotate_png(bytes: &[u8], degrees: f32) -> Vec<u8> {
    let Ok(source) = ::image::load_from_memory_with_format(bytes, ::image::ImageFormat::Png) else {
        return bytes.to_vec();
    };
    let source = source.to_rgba8();
    let (width, height) = source.dimensions();
    let angle = degrees.to_radians();
    let (sin, cos) = angle.sin_cos();
    let center_x = width as f32 / 2.;
    let center_y = height as f32 / 2.;
    let mut output = ::image::RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 + 0.5 - center_x;
            let dy = y as f32 + 0.5 - center_y;
            let sx = cos * dx + sin * dy + center_x;
            let sy = -sin * dx + cos * dy + center_y;
            output.put_pixel(x, y, sample(&source, sx, sy));
        }
    }
    let mut encoded = Cursor::new(Vec::new());
    let _ = ::image::DynamicImage::ImageRgba8(output)
        .write_to(&mut encoded, ::image::ImageFormat::Png);
    encoded.into_inner()
}

fn sample(source: &::image::RgbaImage, x: f32, y: f32) -> ::image::Rgba<u8> {
    let (width, height) = source.dimensions();
    if x < 0. || y < 0. || x >= width as f32 || y >= height as f32 {
        return ::image::Rgba([0, 0, 0, 0]);
    }
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(width.saturating_sub(1));
    let y1 = (y0 + 1).min(height.saturating_sub(1));
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let p00 = source.get_pixel(x0, y0).0;
    let p10 = source.get_pixel(x1, y0).0;
    let p01 = source.get_pixel(x0, y1).0;
    let p11 = source.get_pixel(x1, y1).0;
    let mut out = [0u8; 4];
    for channel in 0..4 {
        let top = p00[channel] as f32 * (1. - fx) + p10[channel] as f32 * fx;
        let bottom = p01[channel] as f32 * (1. - fx) + p11[channel] as f32 * fx;
        out[channel] = (top * (1. - fy) + bottom * fy).round() as u8;
    }
    ::image::Rgba(out)
}

pub(crate) fn icon(kind: DesktopIconKind, size: f32) -> AnyElement {
    render_image(image(kind), size)
}

pub(crate) fn rotated_icon(kind: DesktopIconKind, degrees: f32, size: f32) -> AnyElement {
    render_image(rotated_image(kind, degrees), size)
}

#[cfg(test)]
mod tests {
    use ::core::prelude::v1::test;
    use super::*;

    #[test]
    fn rotated_icons_are_cached_by_kind_and_angle() {
        let first = rotated_image(DesktopIconKind::Folder, -12.);
        let second = rotated_image(DesktopIconKind::Folder, -12.);
        let third = rotated_image(DesktopIconKind::Folder, 8.);
        assert!(Arc::ptr_eq(&first, &second));
        assert!(!Arc::ptr_eq(&first, &third));
    }
}

fn render_image(image: Arc<Image>, size: f32) -> AnyElement {
    img(image)
        .w(px(size))
        .h(px(size))
        .object_fit(ObjectFit::Contain)
        .with_fallback(move || {
            // Resource failures still leave a quiet, correctly sized file mark.
            div()
                .w(px(size))
                .h(px(size))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(size * 0.56))
                        .h(px(size * 0.76))
                        .rounded(px(size * 0.04))
                        .bg(rgb(0xe9edf2))
                        .border_1()
                        .border_color(rgba(0xffffff80)),
                )
                .into_any_element()
        })
        .into_any_element()
}
