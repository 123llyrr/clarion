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
//! flattens those to one colour, so they are rasterized here with the same
//! resvg build GPUI uses, cached, and drawn as images. Everything stays PNG
//! bytes until it is painted, so the fan's tilted cards rotate the preview as
//! easily as a type mark.
//!
//! Finder-style tags (`user.xdg.tags`) ride on top of the artwork as a small
//! row of coloured dots, the way the reference desktop marks them.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use gpui::prelude::*;
use gpui::*;

use crate::desktop_icons::{self, DesktopIconKind};

/// The pixel size artwork is rasterized at. Comfortably above the largest
/// place the shell draws a file icon (the desktop grid) so the GPU only ever
/// scales down.
const RASTER: f32 = 256.;
/// Tags drawn on one icon; Finder shows a short row, not an endless strip.
const MAX_TAG_DOTS: usize = 3;
/// Artwork held in memory before the caches are dropped wholesale.
const CACHE_LIMIT: usize = 512;
/// How long a miss is trusted. Files apps fill the thumbnail cache while the
/// shell is running, so a miss must not stick for the whole session.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Preview cards keep a 4:3 ratio, whatever the source frame was.
const PREVIEW_ASPECT: f32 = 4. / 3.;
/// Corner radius of a preview card, as a fraction of its short side.
const PREVIEW_RADIUS: f32 = 0.11;
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
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "mpg" | "mpeg" | "flv" => {
            FileKind::Video
        }
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "opus" | "aac" | "wma" | "aiff" => {
            FileKind::Audio
        }
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
pub(crate) fn icon(
    path: Option<&Path>,
    is_dir: bool,
    tags: &[String],
    size: f32,
    angle: f32,
) -> AnyElement {
    let kind = match path {
        Some(path) => kind(path, is_dir),
        None => FileKind::Folder,
    };
    let (key, bytes, preview) = artwork(path, kind);
    let artwork = rotated_image(&key, bytes, angle);
    // A preview card is 4:3, so its lower edge sits above the square's.
    let card_height = if preview { size * 3. / 4. } else { size };
    let card_bottom = (size - card_height) / 2.;
    let mut stack = div()
        .relative()
        .w(px(size))
        .h(px(size))
        .child(
            img(artwork)
                .w(px(size))
                .h(px(size))
                .object_fit(ObjectFit::Contain)
                .with_fallback(move || {
                    desktop_icons::icon(fallback(kind), size * 0.86)
                }),
        );
    if tags.is_empty() {
        return stack.into_any_element();
    }
    let dot = (size * 0.18).clamp(5., 11.);
    let gap = (dot * 0.35).max(2.);
    let mut dots = div()
        .absolute()
        .left(px(size * 0.03))
        .bottom(px(card_bottom + size * 0.02))
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

/// The artwork behind one entry: cache key plus PNG bytes.
fn artwork(path: Option<&Path>, kind: FileKind) -> (String, Arc<Vec<u8>>, bool) {
    if let Some(path) = path.filter(|_| kind != FileKind::Folder) {
        // A vector document shows its own drawing: the theme's generic image
        // mark would hide the artwork the file exists for.
        if kind == FileKind::Image && is_svg(path) {
            let key = format!("file:{}", path.display());
            if let Some(bytes) = cached_bytes(&key, || read_svg(path).map(preview_png)) {
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
            let key = format!("thumb:{}", path.display());
            if let Some(bytes) = cached_bytes(&key, || {
                thumbnail_png(path).map(|png| {
                    if cover {
                        preview_png(png)
                    } else {
                        page_png(png)
                    }
                })
            }) {
                return (key, bytes, cover);
            }
        }
    }
    let name = icon_name(kind);
    let key = format!("theme:{name}");
    let bytes = cached_bytes(&key, || {
        resolve(name).and_then(|path| rasterize(&path))
    })
    .unwrap_or_else(|| embedded_bytes(fallback(kind)));
    (key, bytes, false)
}

fn is_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

/// A vector file rasterized at icon size, so the preview is the file itself.
fn read_svg(path: &Path) -> Option<Vec<u8>> {
    render_svg(&std::fs::read(path).ok()?)
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
    if scaled.write_to(&mut encoded, ::image::ImageFormat::Png).is_ok() {
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
    if canvas.write_to(&mut encoded, ::image::ImageFormat::Png).is_ok() {
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
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20,
        5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
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

/// The artwork key to the PNG bytes we draw for it, or a timed miss.
type ArtworkCache = Mutex<HashMap<String, (Option<Arc<Vec<u8>>>, Instant)>>;
/// The icon name to the theme file it resolves to.
type PathCache = Mutex<HashMap<String, Option<PathBuf>>>;
/// Rasterized artwork by key and rotation (tenths of a degree).
type RotationCache = Mutex<HashMap<(String, i16), Arc<Image>>>;
/// Bundled PNG bytes, decoded once per coarse kind.
type EmbeddedCache = Mutex<HashMap<DesktopIconKind, Arc<Vec<u8>>>>;

fn cached_bytes(key: &str, load: impl FnOnce() -> Option<Vec<u8>>) -> Option<Arc<Vec<u8>>> {
    static CACHE: OnceLock<ArtworkCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((bytes, at)) = cache.lock().unwrap().get(key) {
        if bytes.is_some() || at.elapsed() < RETRY_AFTER {
            return bytes.clone();
        }
    }
    let bytes = load().map(Arc::new);
    let mut cache = cache.lock().unwrap();
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key.to_owned(), (bytes.clone(), Instant::now()));
    bytes
}

fn resolve(name: &str) -> Option<PathBuf> {
    static PATHS: OnceLock<PathCache> = OnceLock::new();
    let cache = PATHS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = cache.lock().unwrap().get(name) {
        return cached.clone();
    }
    let resolved = crate::view::resolve_theme_icon(name);
    cache.lock().unwrap().insert(name.to_owned(), resolved.clone());
    resolved
}

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

/// Full-colour icon theme SVG → PNG, using the renderer GPUI already links.
fn rasterize(path: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    if is_svg(path) {
        render_svg(&bytes)
    } else {
        Some(bytes)
    }
}

fn render_svg(bytes: &[u8]) -> Option<Vec<u8>> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
    let source = tree.size();
    let longest = source.width().max(source.height());
    if longest <= 0. {
        return None;
    }
    let scale = RASTER / longest;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(RASTER as u32, RASTER as u32)?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(
        (RASTER - source.width() * scale) / 2.,
        (RASTER - source.height() * scale) / 2.,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    pixmap.encode_png().ok()
}

/// The decoded artwork for one key at one angle, cached. Rotation happens on
/// the PNG bytes, so the fan's tilted cards keep whatever artwork the surface
/// resolved — theme mark, file preview or bundled fallback.
fn rotated_image(key: &str, bytes: Arc<Vec<u8>>, angle: f32) -> Arc<Image> {
    static CACHE: OnceLock<RotationCache> = OnceLock::new();
    let tenths = (angle * 10.).round() as i16;
    let cache_key = (key.to_owned(), tenths);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = cache.lock().unwrap().get(&cache_key).cloned() {
        return cached;
    }
    let encoded = if tenths == 0 {
        bytes.as_ref().clone()
    } else {
        desktop_icons::rotate_png(&bytes, angle)
    };
    let image = Arc::new(Image::from_bytes(ImageFormat::Png, encoded));
    let mut cache = cache.lock().unwrap();
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(cache_key, image.clone());
    image
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
    fn the_theme_svg_rasterizes_to_png() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><rect width="24" height="24" fill="#ff0000"/></svg>"##;
        let png = render_svg(svg).expect("resvg renders a plain svg");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn document_pages_fit_inside_the_icon_box() {
        let source = ::image::RgbaImage::from_pixel(100, 141, ::image::Rgba([250, 250, 250, 255]));
        let mut encoded = Cursor::new(Vec::new());
        source.write_to(&mut encoded, ::image::ImageFormat::Png).unwrap();
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
    fn previews_fill_a_four_by_three_card_with_rounded_corners() {
        for (width, height) in [(16, 8), (8, 16), (16, 16)] {
            let source =
                ::image::RgbaImage::from_pixel(width, height, ::image::Rgba([200, 100, 50, 255]));
            let mut encoded = Cursor::new(Vec::new());
            source.write_to(&mut encoded, ::image::ImageFormat::Png).unwrap();
            let decoded = ::image::load_from_memory_with_format(
                &preview_png(encoded.into_inner()),
                ::image::ImageFormat::Png,
            )
            .unwrap()
            .to_rgba8();
            let (framed_width, framed_height) = decoded.dimensions();
            assert_eq!(framed_width, RASTER as u32);
            assert_eq!(framed_height, (RASTER * 3. / 4.).round() as u32);
            assert_eq!(decoded.get_pixel(framed_width / 2, framed_height / 2).0[3], 255);
            assert_eq!(decoded.get_pixel(0, 0).0[3], 0);
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
