// SPDX-License-Identifier: GPL-3.0-or-later
//! Embedded desktop artwork. Each immutable image gets a stable GPUI asset key,
//! so drawing another file never reopens an icon or starts another decode.

use std::sync::{Arc, OnceLock};

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

/// The raw PNG for a kind, for callers that rotate or re-encode the artwork
/// before it reaches the screen.
pub(crate) fn bytes(kind: DesktopIconKind) -> &'static [u8] {
    embedded(kind)
}

pub(crate) fn image(kind: DesktopIconKind) -> Arc<Image> {
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

pub(crate) fn icon(kind: DesktopIconKind, size: f32) -> AnyElement {
    render_image(image(kind), size)
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
