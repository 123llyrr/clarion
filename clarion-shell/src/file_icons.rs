// SPDX-License-Identifier: GPL-3.0-or-later
//! File-type artwork for the desktop, the Dock and its folder fan.
//!
//! One place decides what a path *is* (`FileKind`), which icon name the
//! freedesktop theme should supply for it, and how the artwork reaches the
//! screen:
//!
//! * the desktop's own preview (the freedesktop thumbnail cache Nautilus and
//!   the other Files apps write) for pictures, videos and PDFs,
//! * the file's own drawing for SVG documents,
//! * the icon theme's type mark (`mimes` category) for everything else, and
//! * the bundled artwork when no theme answers.
//!
//! Themes ship their MIME icons as full-colour SVGs; GPUI's `svg` element
//! flattens those to one colour, so `crate::icons` rasterizes them with the
//! same resvg build GPUI uses and this module draws the resulting PNG bytes.
//! Everything stays bytes until it is painted, so the fan's tilted cards
//! rotate the preview as easily as a type mark.
//!
//! Finder-style tags (`user.xdg.tags`) ride on top of the artwork as a small
//! row of coloured dots, the way the reference desktop marks them.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use gpui::prelude::*;
use gpui::*;

use crate::desktop_icons::{self, DesktopIconKind};
use crate::icons::{self, is_svg, RASTER};

/// Tags drawn on one icon; Finder shows a short row, not an endless strip.
const MAX_TAG_DOTS: usize = 3;
/// Rotated cards held in memory before the cache is dropped wholesale.
const CACHE_LIMIT: usize = 512;
/// Preview cards keep a 4:3 ratio, whatever the source frame was.
const PREVIEW_ASPECT: f32 = 4. / 3.;
/// How much larger than the icon box a preview card is drawn. The card is 4:3,
/// so letting it spill a little sideways keeps its visual weight level with
/// the square type marks around it.
const PREVIEW_SCALE: f32 = 1.1;
/// Corner radius of a preview card, as a fraction of its short side. Kept
/// small so a Dock-sized card (roughly 48 points before the stack shrinks it)
/// paints about two points of rounding, not a pill.
const PREVIEW_RADIUS: f32 = 0.06;
/// How much of the icon box a document page fills. The theme's own type marks
/// leave a similar margin, so a page preview keeps their visual weight.
const PAGE_FILL: f32 = 0.90;
/// Thumbnail cache buckets, sharpest first. `xx-large` is rare and only worth
/// a look when nothing smaller answered.
const THUMBNAIL_BUCKETS: [&str; 4] = ["x-large", "large", "normal", "xx-large"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileKind {
    Folder,
    Application,
    Image,
    Video,
    Audio,
    Pdf,
    Archive,
    Spreadsheet,
    Presentation,
    Document,
    Text,
    Code,
    Font,
    Executable,
    Unknown,
}

/// Classify a path. Folders and known extensions win over `mime_guess`, whose
/// answers are sometimes too coarse (`application/octet-stream` for anything
/// it does not know) to pick artwork.
pub(crate) fn kind(path: &Path, is_dir: bool) -> FileKind {
    if is_dir {
        return FileKind::Folder;
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "desktop" => FileKind::Application,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "avif"
        | "ico" | "svg" => FileKind::Image,
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "mpg" | "mpeg" | "flv" => FileKind::Video,
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "opus" | "aac" | "wma" | "aiff" => FileKind::Audio,
        "pdf" => FileKind::Pdf,
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "zst" | "lz4" => {
            FileKind::Archive
        }
        "ods" | "xlsx" | "xls" | "csv" | "tsv" | "numbers" => FileKind::Spreadsheet,
        "odp" | "pptx" | "ppt" | "key" => FileKind::Presentation,
        "odt" | "docx" | "doc" | "rtf" | "pages" | "epub" => FileKind::Document,
        "txt" | "md" | "markdown" | "log" | "rst" => FileKind::Text,
        "rs" | "py" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "c" | "h" | "cc" | "cpp" | "hpp"
        | "go" | "java" | "rb" | "sh" | "bash" | "zsh" | "fish" | "toml" | "yaml" | "yml"
        | "json" | "xml" | "html" | "htm" | "css" | "scss" | "sql" | "lua" | "nix" | "proto"
        | "gradle" | "cmake" | "diff" | "patch" => FileKind::Code,
        "ttf" | "otf" | "woff" | "woff2" | "ttc" => FileKind::Font,
        "appimage" | "run" | "bin" | "exe" | "msi" | "deb" | "rpm" | "flatpakref" => {
            FileKind::Executable
        }
        _ => match mime_guess::from_path(path).first_raw() {
            Some(mime) if mime.starts_with("image/") => FileKind::Image,
            Some(mime) if mime.starts_with("video/") => FileKind::Video,
            Some(mime) if mime.starts_with("audio/") => FileKind::Audio,
            Some(mime) if mime.starts_with("font/") => FileKind::Font,
            Some(mime) if mime.starts_with("text/") => FileKind::Text,
            _ => FileKind::Unknown,
        },
    }
}

/// The freedesktop icon name for the type. Every theme worth naming carries
/// these generic MIME marks.
pub(crate) fn icon_name(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Folder => "folder",
        FileKind::Application | FileKind::Executable => "application-x-executable",
        FileKind::Image => "image-x-generic",
        FileKind::Video => "video-x-generic",
        FileKind::Audio => "audio-x-generic",
        FileKind::Pdf => "application-pdf",
        FileKind::Archive => "package-x-generic",
        FileKind::Spreadsheet => "x-office-spreadsheet",
        FileKind::Presentation => "x-office-presentation",
        FileKind::Document => "x-office-document",
        FileKind::Text | FileKind::Code | FileKind::Unknown => "text-x-generic",
        FileKind::Font => "font-x-generic",
    }
}

/// The bundled artwork behind a type when no theme answers.
fn fallback(kind: FileKind) -> DesktopIconKind {
    match kind {
        FileKind::Folder => DesktopIconKind::Folder,
        FileKind::Application | FileKind::Executable => DesktopIconKind::Application,
        FileKind::Image | FileKind::Video => DesktopIconKind::Image,
        _ => DesktopIconKind::Document,
    }
}

/// The Finder-style tags of a path (`user.xdg.tags`, the xdg attribute the
/// Dolphin/Thunar family writes). Unknown or unreadable attributes are simply
/// no tags.
pub(crate) fn tags(path: &Path) -> Vec<String> {
    let Some(raw) = read_xattr(path, "user.xdg.tags") else {
        return Vec::new();
    };
    let raw = String::from_utf8_lossy(&raw);
    parse_tags(&raw)
}

fn parse_tags(raw: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in raw.split(',').map(str::trim).filter(|tag| !tag.is_empty()) {
        if tags.iter().any(|seen| seen == tag) {
            continue;
        }
        tags.push(tag.to_owned());
        if tags.len() >= 8 {
            break;
        }
    }
    tags
}

fn read_xattr(path: &Path, name: &str) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let name = std::ffi::CString::new(name).ok()?;
    // SAFETY: both pointers come from live `CString`s and the null buffer asks
    // for the size only, per getxattr(2).
    let size = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
    if size <= 0 {
        return None;
    }
    let mut buffer = vec![0u8; size as usize];
    // SAFETY: the buffer is exactly `size` bytes, the length getxattr asked
    // for, and both C strings outlive the call.
    let read = unsafe {
        libc::getxattr(
            path.as_ptr(),
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    if read < 0 {
        return None;
    }
    buffer.truncate(read as usize);
    Some(buffer)
}

/// The dot colour for a tag name. The Finder palette names are honoured in
/// both languages; anything else hashes onto the wheel so custom tags stay
/// stable from frame to frame.
pub(crate) fn tag_color(name: &str) -> Hsla {
    let name = name.trim().to_lowercase();
    let named = match name.as_str() {
        "red" | "红" | "红色" | "重要" => Some(0xff453a),
        "orange" | "橙" | "橙色" => Some(0xff9f0a),
        "yellow" | "黄" | "黄色" => Some(0xffd60a),
        "green" | "绿" | "绿色" => Some(0x30d158),
        "blue" | "蓝" | "蓝色" => Some(0x0a84ff),
        "purple" | "紫" | "紫色" => Some(0xbf5af2),
        "gray" | "grey" | "灰" | "灰色" => Some(0x8e8e93),
        _ => None,
    };
    if let Some(value) = named {
        return rgb(value).into();
    }
    let mut hash = 0u32;
    for character in name.chars() {
        hash = hash.wrapping_mul(31).wrapping_add(character as u32);
    }
    hsla((hash % 360) as f32 / 360., 0.72, 0.55, 1.)
}

/// A file icon: the desktop's preview when one exists, otherwise the theme's
/// type artwork, rotated as the surface needs, with its tags on the lower
/// corner. `path` is `None` for a bare folder.
///
/// Pictures and videos take a 4:3 card: the preview crops to the middle of the
/// frame so every icon in a Dock stack has the same weight. Folder contents use
/// [`whole_icon`], which keeps the frame's own shape instead.
pub(crate) fn icon(
    path: Option<&Path>,
    is_dir: bool,
    tags: &[String],
    size: f32,
    angle: f32,
) -> AnyElement {
    icon_fit(path, is_dir, tags, size, size, angle, Fit::Card)
}

/// The whole artwork inside the icon box. Pictures and videos keep their own
/// aspect ratio rather than being cropped to the 4:3 card, so a folder fan or
/// grid shows the complete frame; everything else looks like [`icon`].
pub(crate) fn whole_icon(
    path: Option<&Path>,
    is_dir: bool,
    tags: &[String],
    size: f32,
    angle: f32,
) -> AnyElement {
    icon_fit(path, is_dir, tags, size, size, angle, Fit::Whole)
}

/// [`whole_icon`] in a box that need not be square, so a folder grid can give
/// the picture the cell's full width and the height left over by its name.
pub(crate) fn whole_icon_in(
    path: Option<&Path>,
    is_dir: bool,
    tags: &[String],
    width: f32,
    height: f32,
    angle: f32,
) -> AnyElement {
    icon_fit(path, is_dir, tags, width, height, angle, Fit::Whole)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fit {
    /// The uniform 4:3 preview card.
    Card,
    /// The file's own frame, contained in the icon box.
    Whole,
}

fn icon_fit(
    path: Option<&Path>,
    is_dir: bool,
    tags: &[String],
    width: f32,
    height: f32,
    angle: f32,
    fit: Fit,
) -> AnyElement {
    let kind = match path {
        Some(path) => kind(path, is_dir),
        None => FileKind::Folder,
    };
    let (key, bytes, preview) = artwork(path, kind, fit);
    // Rasterize near the painted size: the sprite atlas has no mipmaps, so a
    // 256px raster minified onto a small fan card would alias its edges.
    let target = (width.max(height) * 2.).ceil() as u32;
    let artwork = rotated_artwork(&key, bytes, angle, target);
    // The rotation canvas is larger than the card so tilting never clips its
    // corners; scale it so the card itself keeps the size the surface asked
    // for. A whole preview fits its own frame inside the box instead.
    let (card_width, card_height) = match fit {
        Fit::Card => {
            let width = if preview { width * PREVIEW_SCALE } else { width };
            (width, if preview { width * 3. / 4. } else { height })
        }
        Fit::Whole => {
            let (source_width, source_height) = artwork.source;
            let scale = if source_width > 0 && source_height > 0 {
                (width / source_width as f32).min(height / source_height as f32)
            } else {
                1.
            };
            (
                source_width as f32 * scale,
                source_height as f32 * scale,
            )
        }
    };
    let scale = if artwork.source.0 > 0 {
        card_width / artwork.source.0 as f32
    } else {
        1.
    };
    let draw_width = artwork.canvas.0 as f32 * scale;
    let draw_height = artwork.canvas.1 as f32 * scale;
    let card_bottom = (height - card_height) / 2.;
    let unit = width.min(height);
    let mut stack = div().relative().w(px(width)).h(px(height)).child(
        img(artwork.image)
            .absolute()
            .left(px((width - draw_width) / 2.))
            .top(px((height - draw_height) / 2.))
            .w(px(draw_width))
            .h(px(draw_height))
            .object_fit(ObjectFit::Contain)
            .with_fallback(move || desktop_icons::icon(fallback(kind), unit * 0.86)),
    );
    if tags.is_empty() {
        return stack.into_any_element();
    }
    let dot = (unit * 0.18).clamp(5., 11.);
    let gap = (dot * 0.35).max(2.);
    let mut dots = div()
        .absolute()
        .left(px(width * 0.03))
        .bottom(px(card_bottom + height * 0.02))
        .flex()
        .items_center()
        .gap(px(gap));
    for tag in tags.iter().take(MAX_TAG_DOTS) {
        dots = dots.child(
            div()
                .size(px(dot))
                .rounded(px(dot / 2.))
                .bg(tag_color(tag))
                .border_1()
                .border_color(rgba(0xffffffd9)),
        );
    }
    stack = stack.child(dots);
    stack.into_any_element()
}

/// The image a native drag shows at the pointer: the entry's preview or type
/// artwork, scaled to 64px and padded so the cursor does not cover it.
pub(crate) fn drag_icon(
    path: Option<&Path>,
    is_dir: bool,
    tags: &[String],
) -> Option<(u32, u32, Vec<u8>)> {
    let kind = match path {
        Some(path) => kind(path, is_dir),
        None => FileKind::Folder,
    };
    let (_, bytes, _) = artwork(path, kind, Fit::Whole);
    let image =
        ::image::load_from_memory_with_format(&bytes, ::image::ImageFormat::Png).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let _ = tags;
    let scale = 64. / width.max(height) as f32;
    let target_width = ((width as f32 * scale).round() as u32).max(1);
    let target_height = ((height as f32 * scale).round() as u32).max(1);
    let image = ::image::imageops::resize(
        &image,
        target_width,
        target_height,
        ::image::imageops::FilterType::Triangle,
    );
    // The drag surface's origin is the pointer: pad so the icon trails it.
    let pad = 16;
    let mut canvas = ::image::RgbaImage::new(target_width + pad, target_height + pad);
    ::image::imageops::overlay(&mut canvas, &image, pad as i64, pad as i64);
    Some((canvas.width(), canvas.height(), canvas.into_raw()))
}

/// The artwork behind one entry: cache key plus PNG bytes.
fn artwork(path: Option<&Path>, kind: FileKind, fit: Fit) -> (String, Arc<Vec<u8>>, bool) {
    if let Some(path) = path.filter(|_| kind != FileKind::Folder) {
        // A vector document shows its own drawing: the theme's generic image
        // mark would hide the artwork the file exists for.
        if kind == FileKind::Image && is_svg(path) {
            let whole = fit == Fit::Whole;
            let key = if whole {
                format!("file:{}:whole", path.display())
            } else {
                format!("file:{}", path.display())
            };
            if let Some(bytes) = icons::cached(&key, || {
                read_svg(path).map(|png| {
                    if whole {
                        preview_whole_png(png)
                    } else {
                        preview_png(png)
                    }
                })
            }) {
                return (key, bytes, true);
            }
        }
        // Pictures, videos and PDFs get the desktop's own preview when the
        // freedesktop thumbnail cache holds a fresh one — the same cache
        // Nautilus fills, so the shell and the file manager agree.
        if matches!(kind, FileKind::Image | FileKind::Video | FileKind::Pdf) {
            // Pictures and videos fill the icon square like a photo card; a
            // PDF keeps its page shape so the sheet stays readable.
            let cover = matches!(kind, FileKind::Image | FileKind::Video);
            let whole = cover && fit == Fit::Whole;
            let key = if whole {
                format!("thumb:{}:whole", path.display())
            } else {
                format!("thumb:{}", path.display())
            };
            if let Some(bytes) = icons::cached(&key, || {
                thumbnail_png(path).map(|png| {
                    if !cover {
                        page_png(png)
                    } else if whole {
                        preview_whole_png(png)
                    } else {
                        preview_png(png)
                    }
                })
            }) {
                return (key, bytes, cover);
            }
        }
    }
    // Special folders keep their place emblem: Downloads, Pictures and the
    // rest resolve through the theme's `folder-*` artwork instead of the plain
    // folder mark. Ordinary folders fall back to `folder`.
    let name = match (path, kind) {
        (Some(path), FileKind::Folder) => icons::place(path).unwrap_or("folder"),
        _ => icon_name(kind),
    };
    // A theme that ships no place icon for the folder still has the plain one,
    // so the tile never falls all the way to the bundled PNG.
    let art = icons::artwork(name)
        .or_else(|| (name != "folder").then(|| icons::artwork("folder")).flatten());
    match art {
        Some(art) => (art.key, art.png, false),
        None => (
            format!("bundled:{name}"),
            embedded_bytes(fallback(kind)),
            false,
        ),
    }
}

/// A vector file rasterized at icon size, so the preview is the file itself.
fn read_svg(path: &Path) -> Option<Vec<u8>> {
    icons::render_svg(&std::fs::read(path).ok()?)
}

/// The freedesktop thumbnail for a path, verified against the file's mtime the
/// way the specification asks. `.cache/thumbnails/<bucket>/<md5(uri)>.png`.
fn thumbnail_png(path: &Path) -> Option<Vec<u8>> {
    let hash = md5_hex(file_uri(path)?.as_bytes());
    let root = cache_home()?.join("thumbnails");
    let modified = std::fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|age| age.as_secs());
    for bucket in THUMBNAIL_BUCKETS {
        let file = root.join(bucket).join(format!("{hash}.png"));
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        // A stale preview is worse than the type mark, so a recorded mtime has
        // to match the file on disk.
        if let Some(stored) = png_text(&bytes, "Thumb::MTime") {
            if stored.trim().parse::<u64>().ok() != modified {
                continue;
            }
        }
        return Some(bytes);
    }
    None
}

fn cache_home() -> Option<PathBuf> {
    if let Ok(cache) = std::env::var("XDG_CACHE_HOME") {
        if !cache.is_empty() {
            return Some(PathBuf::from(cache));
        }
    }
    Some(PathBuf::from(std::env::var("HOME").ok()?).join(".cache"))
}

/// `file://` URI for a path, percent-encoded the way `g_filename_to_uri`
/// writes it (which is what the thumbnail cache hashes).
fn file_uri(path: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_bytes() {
        let character = *byte as char;
        if byte.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '_' | '~' | '/') {
            uri.push(character);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    Some(uri)
}

/// The value of one `tEXt` chunk, keyed by its keyword. PNG metadata sits
/// before the pixel data, so the walk stops at `IDAT`.
fn png_text(bytes: &[u8], keyword: &str) -> Option<String> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 8 || &bytes[..8] != SIGNATURE {
        return None;
    }
    let mut offset = 8;
    while offset + 8 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().ok()?) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        let start = offset + 8;
        let end = start.checked_add(length)?;
        if end > bytes.len() {
            return None;
        }
        if kind == b"tEXt" {
            let data = &bytes[start..end];
            if let Some(split) = data.iter().position(|byte| *byte == 0) {
                if &data[..split] == keyword.as_bytes() {
                    return Some(String::from_utf8_lossy(&data[split + 1..]).into_owned());
                }
            }
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return None;
        }
        offset = end + 4;
    }
    None
}

/// A preview card: centre-cropped to 4:3 so every picture and video fills the
/// same fixed frame, scaled to `RASTER` wide and rounded like the reference
/// desktop's thumbnails.
fn preview_png(bytes: Vec<u8>) -> Vec<u8> {
    let Ok(image) = ::image::load_from_memory_with_format(&bytes, ::image::ImageFormat::Png) else {
        return bytes;
    };
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return bytes;
    }
    // Cover crop: keep the largest 4:3 window in the middle of the frame.
    let (crop_w, crop_h) = if width as f32 / height as f32 > PREVIEW_ASPECT {
        let crop_w = (height as f32 * PREVIEW_ASPECT).round() as u32;
        (crop_w.clamp(1, width), height)
    } else {
        let crop_h = (width as f32 / PREVIEW_ASPECT).round() as u32;
        (width, crop_h.clamp(1, height))
    };
    let cropped = ::image::imageops::crop_imm(
        &image,
        (width - crop_w) / 2,
        (height - crop_h) / 2,
        crop_w,
        crop_h,
    )
    .to_image();
    let raster_w = RASTER as u32;
    let raster_h = (RASTER * 3. / 4.).round() as u32;
    let mut scaled = ::image::imageops::resize(
        &cropped,
        raster_w,
        raster_h,
        ::image::imageops::FilterType::Triangle,
    );
    round_corners(&mut scaled, raster_h as f32 * PREVIEW_RADIUS);
    let mut encoded = Cursor::new(Vec::new());
    if scaled
        .write_to(&mut encoded, ::image::ImageFormat::Png)
        .is_ok()
    {
        encoded.into_inner()
    } else {
        bytes
    }
}

/// A preview that keeps the frame whole: the longest side scales to `RASTER`
/// and nothing is cropped, so a folder grid or fan shows the entire picture at
/// its own aspect ratio.
fn preview_whole_png(bytes: Vec<u8>) -> Vec<u8> {
    let Ok(image) = ::image::load_from_memory_with_format(&bytes, ::image::ImageFormat::Png) else {
        return bytes;
    };
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return bytes;
    }
    let scale = RASTER / width.max(height) as f32;
    let target_width = ((width as f32 * scale).round() as u32).max(1);
    let target_height = ((height as f32 * scale).round() as u32).max(1);
    let mut scaled = ::image::imageops::resize(
        &image,
        target_width,
        target_height,
        ::image::imageops::FilterType::Triangle,
    );
    round_corners(
        &mut scaled,
        target_width.min(target_height) as f32 * PREVIEW_RADIUS,
    );
    let mut encoded = Cursor::new(Vec::new());
    if scaled
        .write_to(&mut encoded, ::image::ImageFormat::Png)
        .is_ok()
    {
        encoded.into_inner()
    } else {
        bytes
    }
}

/// Soft rounded corners, one pixel of coverage at the edge.
fn round_corners(image: &mut ::image::RgbaImage, radius: f32) {
    let (width, height) = image.dimensions();
    let (w, h) = (width as f32, height as f32);
    let radius = radius.max(0.).min(w.min(h) / 2.);
    for y in 0..height {
        for x in 0..width {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = (radius - px).max(px - (w - radius)).max(0.);
            let dy = (radius - py).max(py - (h - radius)).max(0.);
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = (radius + 0.5 - distance).clamp(0., 1.);
            if coverage < 1. {
                let pixel = image.get_pixel_mut(x, y);
                pixel.0[3] = (pixel.0[3] as f32 * coverage).round() as u8;
            }
        }
    }
}

/// A document page (a PDF preview) fitted into the icon box at the same
/// visual weight as the theme's type marks: whole page, centred, never
/// overflowing the square.
fn page_png(bytes: Vec<u8>) -> Vec<u8> {
    let Ok(image) = ::image::load_from_memory_with_format(&bytes, ::image::ImageFormat::Png) else {
        return bytes;
    };
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return bytes;
    }
    let raster = RASTER as u32;
    let longest = width.max(height) as f32;
    let scale = (raster as f32 * PAGE_FILL) / longest;
    let target_width = ((width as f32 * scale).round() as u32).max(1);
    let target_height = ((height as f32 * scale).round() as u32).max(1);
    let page = ::image::imageops::resize(
        &image,
        target_width,
        target_height,
        ::image::imageops::FilterType::Triangle,
    );
    let mut canvas = ::image::RgbaImage::new(raster, raster);
    ::image::imageops::overlay(
        &mut canvas,
        &page,
        ((raster - target_width) / 2) as i64,
        ((raster - target_height) / 2) as i64,
    );
    let mut encoded = Cursor::new(Vec::new());
    if canvas
        .write_to(&mut encoded, ::image::ImageFormat::Png)
        .is_ok()
    {
        encoded.into_inner()
    } else {
        bytes
    }
}

/// MD5 of a string, needed only to name thumbnail cache entries.
fn md5_hex(bytes: &[u8]) -> String {
    let digest = md5(bytes);
    let mut out = String::with_capacity(32);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn md5(input: &[u8]) -> [u8; 16] {
    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let mut constants = [0u32; 64];
    for (index, constant) in constants.iter_mut().enumerate() {
        *constant = ((index as f64 + 1.).sin().abs() * 4_294_967_296.) as u32;
    }
    let mut state = [0x6745_2301u32, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    let mut message = input.to_vec();
    let bit_length = (input.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_le_bytes());
    for chunk in message.as_chunks::<64>().0 {
        let mut words = [0u32; 16];
        for (index, word) in words.iter_mut().enumerate() {
            *word = u32::from_le_bytes(chunk[index * 4..index * 4 + 4].try_into().unwrap());
        }
        let [mut a, mut b, mut c, mut d] = state;
        for step in 0..64 {
            let (function, word) = match step / 16 {
                0 => ((b & c) | (!b & d), step),
                1 => ((d & b) | (!d & c), (5 * step + 1) % 16),
                2 => (b ^ c ^ d, (3 * step + 5) % 16),
                _ => (c ^ (b | !d), (7 * step) % 16),
            };
            let rotated = a
                .wrapping_add(function)
                .wrapping_add(constants[step])
                .wrapping_add(words[word])
                .rotate_left(SHIFTS[step]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(rotated);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
    }
    let mut digest = [0u8; 16];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    digest
}

/// Rasterized artwork by key, rotation (whole degrees) and raster bucket.
type RotationCache = Mutex<HashMap<(String, i16, u32), Rotated>>;
/// Bundled PNG bytes, decoded once per coarse kind.
type EmbeddedCache = Mutex<HashMap<DesktopIconKind, Arc<Vec<u8>>>>;

fn embedded_bytes(kind: DesktopIconKind) -> Arc<Vec<u8>> {
    static CACHE: OnceLock<EmbeddedCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = cache.lock().unwrap().get(&kind).cloned() {
        return cached;
    }
    let bytes = Arc::new(desktop_icons::bytes(kind).to_vec());
    cache.lock().unwrap().insert(kind, bytes.clone());
    bytes
}

/// One piece of artwork ready to paint: the encoded canvas, the card size it
/// was made from, and the canvas it now occupies.
#[derive(Clone)]
struct Rotated {
    image: Arc<Image>,
    /// Pixel size of the card before rotation.
    source: (u32, u32),
    /// Pixel size of the canvas after rotation.
    canvas: (u32, u32),
}

/// The artwork for one key at one angle, cached.
///
/// The source is first fitted into a raster bucket sized from the card being
/// painted (about twice its logical pixels) and only then rotated with bilinear
/// sampling. GPUI's sprite atlas has no mipmaps, so rotating the full 256px
/// artwork and shrinking it to a 50px fan card undersamples its edges and
/// produces hard jaggies; rasterizing near the painted size keeps the GPU's
/// minification close to 1:1.
///
/// Angles are quantized to whole degrees because the surfaces animate the tilt
/// continuously. The bucket is a multiple of 32px, so the icon's grow-in also
/// reuses entries instead of producing one raster per animation frame.
fn rotated_artwork(key: &str, bytes: Arc<Vec<u8>>, angle: f32, target: u32) -> Rotated {
    static CACHE: OnceLock<RotationCache> = OnceLock::new();
    let degrees = angle.round() as i16;
    let source = png_size(&bytes).unwrap_or((RASTER as u32, RASTER as u32));
    let longest = source.0.max(source.1).max(1);
    let bucket = target
        .max(24)
        .div_ceil(32)
        .saturating_mul(32)
        .clamp(32, longest.max(32));
    let cache_key = (key.to_owned(), degrees, bucket);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = cache.lock().unwrap().get(&cache_key).cloned() {
        return cached;
    }
    let rotated = match render_artwork(&bytes, degrees as f32, bucket) {
        Some((png, source, canvas)) => Rotated {
            image: Arc::new(Image::from_bytes(ImageFormat::Png, png)),
            source,
            canvas,
        },
        None => Rotated {
            image: Arc::new(Image::from_bytes(ImageFormat::Png, bytes.as_ref().clone())),
            source,
            canvas: source,
        },
    };
    let mut cache = cache.lock().unwrap();
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(cache_key, rotated.clone());
    rotated
}

/// The pixel size recorded in a PNG header.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

/// The encoded PNG, the card size it was made from and the canvas size.
type RenderedArtwork = (Vec<u8>, (u32, u32), (u32, u32));

/// Fit the source into `bucket` pixels on its longest side, then rotate it into
/// a canvas that holds the whole tilted card with a pixel of breathing room.
fn render_artwork(bytes: &[u8], degrees: f32, bucket: u32) -> Option<RenderedArtwork> {
    let image = ::image::load_from_memory_with_format(bytes, ::image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let image = if width.max(height) > bucket {
        let scale = bucket as f32 / width.max(height) as f32;
        let target_width = ((width as f32 * scale).round() as u32).max(1);
        let target_height = ((height as f32 * scale).round() as u32).max(1);
        ::image::imageops::resize(
            &image,
            target_width,
            target_height,
            ::image::imageops::FilterType::CatmullRom,
        )
    } else {
        image
    };
    let (width, height) = image.dimensions();
    if degrees == 0. {
        return Some((encode_png(&image)?, (width, height), (width, height)));
    }
    let radians = degrees.to_radians();
    let (sin, cos) = radians.sin_cos();
    let (sin, cos) = (sin.abs(), cos.abs());
    let canvas_width = (width as f32 * cos + height as f32 * sin).ceil() as u32 + 2;
    let canvas_height = (width as f32 * sin + height as f32 * cos).ceil() as u32 + 2;
    let mut canvas = ::image::RgbaImage::new(canvas_width, canvas_height);
    let center_x = canvas_width as f32 / 2.;
    let center_y = canvas_height as f32 / 2.;
    let source_x = width as f32 / 2.;
    let source_y = height as f32 / 2.;
    for y in 0..canvas_height {
        let dy = y as f32 + 0.5 - center_y;
        for x in 0..canvas_width {
            let dx = x as f32 + 0.5 - center_x;
            let sx = cos * dx + sin * dy + source_x;
            let sy = -sin * dx + cos * dy + source_y;
            canvas.put_pixel(x, y, sample(&image, sx, sy));
        }
    }
    Some((
        encode_png(&canvas)?,
        (width, height),
        (canvas_width, canvas_height),
    ))
}

fn encode_png(image: &::image::RgbaImage) -> Option<Vec<u8>> {
    let mut encoded = Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, ::image::ImageFormat::Png)
        .ok()?;
    Some(encoded.into_inner())
}

/// Bilinear sample, transparent outside the source.
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

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;

    #[test]
    fn extensions_map_onto_the_artwork_families() {
        let kind_of = |name: &str| kind(Path::new(name), false);
        assert_eq!(kind_of("holiday.png"), FileKind::Image);
        assert_eq!(kind_of("clip.MP4"), FileKind::Video);
        assert_eq!(kind_of("song.flac"), FileKind::Audio);
        assert_eq!(kind_of("paper.pdf"), FileKind::Pdf);
        assert_eq!(kind_of("src.tar.gz"), FileKind::Archive);
        assert_eq!(kind_of("budget.xlsx"), FileKind::Spreadsheet);
        assert_eq!(kind_of("deck.key"), FileKind::Presentation);
        assert_eq!(kind_of("notes.md"), FileKind::Text);
        assert_eq!(kind_of("main.rs"), FileKind::Code);
        assert_eq!(kind_of("font.woff2"), FileKind::Font);
        assert_eq!(kind_of("app.desktop"), FileKind::Application);
        assert_eq!(kind_of("tool.appimage"), FileKind::Executable);
        assert_eq!(kind_of("mystery"), FileKind::Unknown);
        assert_eq!(kind(Path::new("anything"), true), FileKind::Folder);
        assert_eq!(icon_name(FileKind::Pdf), "application-pdf");
        assert_eq!(icon_name(FileKind::Audio), "audio-x-generic");
        assert_eq!(icon_name(FileKind::Folder), "folder");
    }

    #[test]
    fn tags_parse_into_a_short_deduplicated_list() {
        assert_eq!(
            parse_tags("工作, 重要 ,工作,,"),
            vec!["工作".to_owned(), "重要".to_owned()]
        );
        assert!(parse_tags("  ").is_empty());
    }

    #[test]
    fn tag_colours_are_named_or_stable() {
        assert_eq!(tag_color("Red"), tag_color("红"));
        assert_eq!(tag_color("自定义"), tag_color("自定义"));
        assert_ne!(tag_color("自定义"), tag_color("另一个"));
    }

    #[test]
    fn rotation_keeps_the_whole_card_so_no_octagon_appears() {
        // An opaque 4:3 card: rotating it in a same-size canvas would shave the
        // corners off (the octagon), so the canvas has to grow with the tilt.
        let card = ::image::RgbaImage::from_pixel(64, 48, ::image::Rgba([200, 100, 50, 255]));
        let mut encoded = Cursor::new(Vec::new());
        card.write_to(&mut encoded, ::image::ImageFormat::Png)
            .unwrap();
        let degrees = 16.;
        let (png, (source_width, source_height), (canvas_width, canvas_height)) =
            render_artwork(&encoded.into_inner(), degrees, 64).expect("the card rotates");
        assert_eq!((source_width, source_height), (64, 48));
        assert!(canvas_width > source_width && canvas_height > source_height);
        assert!(canvas_width > 64 && canvas_height > 48);
        let rotated = ::image::load_from_memory_with_format(&png, ::image::ImageFormat::Png)
            .unwrap()
            .to_rgba8();
        // Where the card's own top-left corner lands in the bigger canvas.
        let radians = degrees.to_radians();
        let (sin, cos) = radians.sin_cos();
        let corner_x = canvas_width as f32 / 2. - 32. * cos + 24. * sin;
        let corner_y = canvas_height as f32 / 2. - 32. * sin - 24. * cos;
        let ink_near_corner = (0..3).any(|dx| {
            (0..3).any(|dy| {
                rotated
                    .get_pixel(corner_x as u32 + dx, corner_y as u32 + dy)
                    .0[3]
                    > 0
            })
        });
        assert!(ink_near_corner, "the card's corner survives the rotation");
        assert_eq!(
            rotated.get_pixel(0, 0).0[3],
            0,
            "the canvas corner stays empty"
        );
    }

    #[test]
    fn rotation_rasters_are_fitted_to_the_painted_size() {
        // A 256px source drawn on a 48pt fan card must be fitted to the bucket
        // before it is rotated, or the GPU minifies five texels into one and
        // the tilted edges alias.
        let source = ::image::RgbaImage::from_pixel(256, 128, ::image::Rgba([200, 100, 50, 255]));
        let mut encoded = Cursor::new(Vec::new());
        source
            .write_to(&mut encoded, ::image::ImageFormat::Png)
            .unwrap();
        let bytes = encoded.into_inner();
        let (_, card, canvas) = render_artwork(&bytes, 0., 64).expect("the card renders");
        assert_eq!(card, (64, 32));
        assert_eq!(canvas, (64, 32));
        let (_, card, canvas) = render_artwork(&bytes, 20., 64).expect("the card rotates");
        assert_eq!(card, (64, 32));
        assert!(canvas.0 > 64 && canvas.1 > 32);
    }

    #[test]
    fn document_pages_fit_inside_the_icon_box() {
        let source = ::image::RgbaImage::from_pixel(100, 141, ::image::Rgba([250, 250, 250, 255]));
        let mut encoded = Cursor::new(Vec::new());
        source
            .write_to(&mut encoded, ::image::ImageFormat::Png)
            .unwrap();
        let decoded = ::image::load_from_memory_with_format(
            &page_png(encoded.into_inner()),
            ::image::ImageFormat::Png,
        )
        .unwrap()
        .to_rgba8();
        assert_eq!(decoded.dimensions(), (RASTER as u32, RASTER as u32));
        let opaque_rows = (0..RASTER as u32)
            .filter(|y| decoded.get_pixel(RASTER as u32 / 2, *y).0[3] > 0)
            .count();
        assert!(
            (opaque_rows as f32) <= RASTER * PAGE_FILL + 1.,
            "the page fills at most {PAGE_FILL} of the box, got {opaque_rows}"
        );
        assert!(
            (opaque_rows as f32) >= RASTER * PAGE_FILL - 1.,
            "the page still fills the box, got {opaque_rows}"
        );
        assert_eq!(decoded.get_pixel(0, 0).0[3], 0);
    }

    #[test]
    fn previews_read_about_two_points_of_rounding_on_a_dock_card() {
        // The folder stack draws a probe at 0.85 of the 48-point Dock icon,
        // and previews spill 1.1x past that box.
        let card_width = 48. * 0.85 * PREVIEW_SCALE;
        let raster_radius = RASTER * 3. / 4. * PREVIEW_RADIUS;
        let screen_radius = raster_radius * card_width / RASTER;
        assert!((screen_radius - 2.).abs() < 0.2, "{screen_radius}");
    }

    #[test]
    fn previews_fill_a_four_by_three_card_with_rounded_corners() {
        for (width, height) in [(16, 8), (8, 16), (16, 16)] {
            let source =
                ::image::RgbaImage::from_pixel(width, height, ::image::Rgba([200, 100, 50, 255]));
            let mut encoded = Cursor::new(Vec::new());
            source
                .write_to(&mut encoded, ::image::ImageFormat::Png)
                .unwrap();
            let decoded = ::image::load_from_memory_with_format(
                &preview_png(encoded.into_inner()),
                ::image::ImageFormat::Png,
            )
            .unwrap()
            .to_rgba8();
            let (framed_width, framed_height) = decoded.dimensions();
            assert_eq!(framed_width, RASTER as u32);
            assert_eq!(framed_height, (RASTER * 3. / 4.).round() as u32);
            assert_eq!(
                decoded.get_pixel(framed_width / 2, framed_height / 2).0[3],
                255
            );
            assert_eq!(decoded.get_pixel(0, 0).0[3], 0);
        }
    }

    #[test]
    fn whole_previews_keep_the_frames_own_aspect_ratio() {
        for (width, height, expected) in [
            (16u32, 8u32, (RASTER as u32, RASTER as u32 / 2)),
            (8, 16, (RASTER as u32 / 2, RASTER as u32)),
            (16, 16, (RASTER as u32, RASTER as u32)),
            (25, 9, (RASTER as u32, (RASTER * 9. / 25.).round() as u32)),
        ] {
            let source =
                ::image::RgbaImage::from_pixel(width, height, ::image::Rgba([200, 100, 50, 255]));
            let mut encoded = Cursor::new(Vec::new());
            source
                .write_to(&mut encoded, ::image::ImageFormat::Png)
                .unwrap();
            let decoded = ::image::load_from_memory_with_format(
                &preview_whole_png(encoded.into_inner()),
                ::image::ImageFormat::Png,
            )
            .unwrap()
            .to_rgba8();
            let (framed_width, framed_height) = decoded.dimensions();
            assert_eq!(
                (framed_width, framed_height),
                expected,
                "a {width}x{height} frame must not be cropped"
            );
            assert_eq!(
                decoded.get_pixel(framed_width / 2, framed_height / 2).0[3],
                255,
                "the middle of the frame stays painted"
            );
        }
    }

    #[test]
    fn md5_matches_the_reference_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
    }

    #[test]
    fn file_uris_percent_encode_the_path_the_way_the_cache_hashes_it() {
        assert_eq!(
            file_uri(Path::new("/home/u/Pictures/a b.png")).unwrap(),
            "file:///home/u/Pictures/a%20b.png"
        );
        assert_eq!(
            file_uri(Path::new("/home/u/微信图片.png")).unwrap(),
            "file:///home/u/%E5%BE%AE%E4%BF%A1%E5%9B%BE%E7%89%87.png"
        );
        assert_eq!(
            file_uri(Path::new("/tmp/x,y(1)~.png")).unwrap(),
            "file:///tmp/x%2Cy%281%29~.png"
        );
    }

    #[test]
    fn png_text_reads_the_thumbnail_metadata() {
        // 1x1 PNG carrying `Thumb::MTime` before the pixel data.
        let mut png: Vec<u8> = b"\x89PNG\r\n\x1a\n".to_vec();
        let text = b"Thumb::MTime\0 1700000000";
        png.extend_from_slice(&(text.len() as u32).to_be_bytes());
        png.extend_from_slice(b"tEXt");
        png.extend_from_slice(text);
        png.extend_from_slice(&[0, 0, 0, 0]);
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IDAT");
        png.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(
            png_text(&png, "Thumb::MTime").as_deref(),
            Some(" 1700000000")
        );
        assert_eq!(png_text(&png, "Thumb::URI"), None);
        assert_eq!(png_text(b"not a png", "Thumb::MTime"), None);
    }
}
