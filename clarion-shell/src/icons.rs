// SPDX-License-Identifier: GPL-3.0-or-later
//! Icon theme lookup and artwork resolution for the whole shell.
//!
//! The desktop picks one icon theme (GTK settings first, then KDE, then
//! `hicolor`). This module is the only place that knows where themes live:
//! callers ask for a freedesktop name and receive
//!
//! * `resolve` — the file the theme ships for that name,
//! * `artwork` / `image` / `trash` — the same file as cached PNG bytes or a
//!   decoded GPUI image, rasterizing SVG first, and
//! * `asset` / `asset_svg` / `symbolic` — the bundled fallback files and the
//!   theme aliases they borrow.
//!
//! Full-colour theme art is SVG, while GPUI's `svg` element paints one flat
//! colour, so non-symbolic names travel through the raster cache. Symbolic
//! names stay vectors and recolour in the renderer. The light/dark variant is
//! switched once (`set_dark`), so resolvers never reason about the shell's
//! appearance themselves; every cache keys on the active theme and therefore
//! invalidates naturally when the desktop's theme changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{Image, ImageFormat};

/// The pixel size artwork is rasterized at. Comfortably above the largest
/// place the shell draws an icon (the desktop grid), so the GPU only ever
/// scales down.
pub(crate) const RASTER: f32 = 256.;
/// Artwork held in memory before a cache is dropped wholesale.
const CACHE_LIMIT: usize = 512;
/// How long a miss is trusted. Files apps may install the theme while the
/// shell is running, so a miss must not stick for the whole session.
const RETRY_AFTER: Duration = Duration::from_secs(30);

// --------------------------------------------------------------- theme ----

/// The icon theme the desktop currently selected: the GTK settings (both
/// majors) first, then KDE's kdeglobals, then the spec fallback theme.
fn configured() -> String {
    if let Ok(home) = std::env::var("HOME") {
        let config = PathBuf::from(&home).join(".config");
        for file in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
            if let Ok(text) = std::fs::read_to_string(config.join(file)) {
                if let Some(theme) = ini_value(&text, "gtk-icon-theme-name") {
                    return theme;
                }
            }
        }
        if let Ok(text) = std::fs::read_to_string(config.join("kdeglobals")) {
            if let Some(theme) = ini_value(&text, "Theme") {
                return theme;
            }
        }
    }
    "hicolor".to_owned()
}

/// The icon theme every resolution uses right now. The shell switches the
/// light/dark *variant* here once, when its appearance changes, so the icon
/// resolvers never have to reason about the appearance themselves.
static ACTIVE_THEME: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// The theme name every lookup keys its cache on.
pub(crate) fn theme() -> String {
    ACTIVE_THEME
        .read()
        .ok()
        .and_then(|theme| theme.clone())
        .unwrap_or_else(configured)
}

/// Point icon resolution at the light or dark variant of the desktop's icon
/// theme. Called when the shell appearance changes and once at startup.
pub(crate) fn set_dark(dark: bool) {
    let base = configured();
    let theme = variant(&base, dark, installed);
    if let Ok(mut active) = ACTIVE_THEME.write() {
        if active.as_deref() != Some(theme.as_str()) {
            *active = Some(theme);
        }
    }
}

/// The light/dark counterpart of a theme name. Light themes get a `-dark`
/// suffix; a dark theme chosen in the desktop settings strips it again.
/// The `exists` probe keeps a missing variant from blanking the icons.
fn variant(base: &str, dark: bool, exists: impl Fn(&str) -> bool) -> String {
    if dark {
        for suffix in ["-dark", "-Dark"] {
            if !base.ends_with(suffix) {
                let candidate = format!("{base}{suffix}");
                if exists(&candidate) {
                    return candidate;
                }
            }
        }
        base.to_owned()
    } else {
        for suffix in ["-dark", "-Dark"] {
            if let Some(light) = base.strip_suffix(suffix) {
                if exists(light) {
                    return light.to_owned();
                }
            }
        }
        base.to_owned()
    }
}

fn installed(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    theme_roots()
        .into_iter()
        .any(|root| root.join(name).join("index.theme").is_file())
}

/// One `key = value` pair out of an ini-style settings file.
fn ini_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        if let Some((name, value)) = line.split_once('=') {
            if name.trim() == key {
                let value = value.trim().trim_matches('"');
                if !value.is_empty() {
                    return Some(value.to_owned());
                }
            }
        }
    }
    None
}

/// Where themes live, in precedence order: the user's own installs win over
/// the system ones.
fn theme_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join(".icons"));
        roots.push(home.join(".local/share/icons"));
    }
    roots.push(PathBuf::from("/usr/share/icons"));
    roots
}

// ---------------------------------------------------------- resolution ----

/// Resolve a freedesktop icon name to a real file on the theme paths.
///
/// The desktop's chosen theme wins; `hicolor` is the spec fallback; every
/// other installed theme follows in deterministic order because vendors ship
/// device glyphs in their own theme (breeze, blueman, ...). Absolute paths are
/// accepted as-is, which is how notification payloads name their icons.
///
/// Answers are cached per theme, so callers may ask every frame.
pub(crate) fn resolve(name: &str) -> Option<PathBuf> {
    static PATHS: OnceLock<Mutex<HashMap<String, Option<PathBuf>>>> = OnceLock::new();
    let key = format!("{}:{name}", theme());
    let cache = PATHS.get_or_init(Default::default);
    if let Some(cached) = cache.lock().unwrap().get(&key) {
        return cached.clone();
    }
    let resolved = resolve_uncached(name);
    let mut cache = cache.lock().unwrap();
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, resolved.clone());
    resolved
}

fn resolve_uncached(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains("..") {
        return None;
    }
    let absolute = PathBuf::from(name);
    if absolute.is_absolute() {
        return absolute.is_file().then_some(absolute);
    }
    let current = theme();
    let roots = theme_roots();
    let mut themes: Vec<PathBuf> = Vec::new();
    for theme in [Some(current.as_str()), (current != "hicolor").then_some("hicolor")]
        .into_iter()
        .flatten()
    {
        themes.extend(roots.iter().map(|root| root.join(theme)));
    }
    // System themes come first in the fallback scan to keep the old
    // deterministic order; the user roots follow.
    let mut scan = vec![PathBuf::from("/usr/share/icons")];
    scan.extend(
        roots
            .iter()
            .filter(|root| root.as_path() != Path::new("/usr/share/icons"))
            .cloned(),
    );
    let mut others: Vec<PathBuf> = Vec::new();
    for root in scan {
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let path = entry.path();
                let theme = path.file_name().map(|n| n.to_string_lossy().into_owned());
                if path.is_dir()
                    && theme
                        .as_deref()
                        .is_none_or(|theme| theme != current && theme != "hicolor")
                {
                    others.push(path);
                }
            }
        }
    }
    others.sort();
    themes.extend(others);
    themes.push(PathBuf::from("/usr/share/pixmaps"));
    for theme in &themes {
        if theme.ends_with("pixmaps") {
            for extension in ["svg", "png"] {
                let path = theme.join(format!("{name}.{extension}"));
                if path.is_file() {
                    return Some(path);
                }
            }
            continue;
        }
        if let Some(path) = probe_theme(theme, name) {
            return Some(path);
        }
    }
    None
}

/// Probe one theme directory for the icon. Themes use two layouts —
/// `size/category` (hicolor) and `category/size` (MacTahoe) — so try both.
fn probe_theme(dir: &Path, name: &str) -> Option<PathBuf> {
    const SIZES: &[&str] = &[
        "scalable", "symbolic", "256", "256x256", "128", "128x128", "96", "96x96", "64", "64x64",
        "48", "48x48", "32", "32x32", "24", "24x24", "22", "22x22", "16", "16x16",
    ];
    const CATEGORIES: &[&str] = &[
        "devices",
        "status",
        "actions",
        "apps",
        "panel",
        "places",
        // File-type marks live in `mimes`; without it every document would
        // fall back to the bundled artwork instead of the desktop's theme.
        "mimes",
        "categories",
        "",
    ];
    for size in SIZES {
        for category in CATEGORIES {
            // Full-colour art ships as gradients the single-palette svg
            // rasterizer cannot honour (it paints them one flat colour), so
            // bitmaps win for non-symbolic names; symbolic glyphs only exist
            // as vectors and recolor by design.
            let extensions: &[&str] = if name.ends_with("symbolic") {
                &["svg", "png"]
            } else {
                &["png", "svg"]
            };
            for extension in extensions {
                let path = dir
                    .join(size)
                    .join(category)
                    .join(format!("{name}.{extension}"));
                if path.is_file() {
                    return Some(path);
                }
                let path = dir
                    .join(category)
                    .join(size)
                    .join(format!("{name}.{extension}"));
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

// ------------------------------------------------------------ fallbacks ----

/// A shell-owned asset, resolved from the install prefix or the source tree.
pub(crate) fn asset(filename: &str) -> PathBuf {
    let prefix = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()));
    let executable = prefix.as_ref().map(|p| p.join("assets/icons"));
    let system = prefix.map(|p| p.join("share/clarion-shell/icons"));
    for root in executable.into_iter().chain(system).chain([
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/icons"),
        PathBuf::from("/usr/share/clarion-shell/icons"),
        PathBuf::from("/usr/local/share/clarion-shell/icons"),
    ]) {
        let path = root.join(filename);
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from(filename)
}

/// The bundled vector behind a name, e.g. `bell-off` → `bell-off.svg`.
pub(crate) fn asset_svg(name: &str) -> PathBuf {
    asset(&format!("{name}.svg"))
}

/// The symbolic icon the desktop theme provides for a bundled glyph, or
/// `None` when the glyph has no common theme counterpart and always draws
/// from the bundled assets.
pub(crate) fn symbolic(asset: &str) -> Option<&'static str> {
    Some(match asset {
        "x.svg" => "window-close-symbolic",
        "wine.svg" => "wine-symbolic",
        "launchpad.svg" => "software-store-symbolic",
        "grid.svg" => "view-app-grid-symbolic",
        "chevron-down.svg" => "pan-down-symbolic",
        "chevron-right.svg" => "pan-end-symbolic",
        "bell.svg" => "bell-symbolic",
        "bell" => "notifications-symbolic",
        "bell-off" => "notifications-disabled-symbolic",
        "sun.svg" => "weather-clear-symbolic",
        "moon.svg" => "weather-clear-night-symbolic",
        "speaker.svg" => "audio-volume-high-symbolic",
        "speaker-muted.svg" => "audio-volume-muted-symbolic",
        "sliders.svg" => "sliders-symbolic",
        "settings.svg" => "settings-symbolic",
        "clipboard.svg" => "edit-paste-symbolic",
        "search.svg" => "system-search-symbolic",
        "display.svg" => "display-symbolic",
        "home.svg" => "user-home-symbolic",
        "downloads.svg" => "folder-download-symbolic",
        _ => return None,
    })
}

// -------------------------------------------------------- well-known dirs ----

/// The freedesktop place names of the well-known directories. `user-dirs.dirs`
/// names the actual paths; the spec's default names cover the keys the file
/// leaves out, so a minimal setup still finds Downloads, Pictures and friends.
const WELL_KNOWN_FOLDERS: [(&str, &str, &str); 8] = [
    ("XDG_DESKTOP_DIR", "Desktop", "folder-desktop"),
    ("XDG_DOWNLOAD_DIR", "Downloads", "folder-download"),
    ("XDG_TEMPLATES_DIR", "Templates", "folder-templates"),
    ("XDG_PUBLICSHARE_DIR", "Public", "folder-publicshare"),
    ("XDG_DOCUMENTS_DIR", "Documents", "folder-documents"),
    ("XDG_MUSIC_DIR", "Music", "folder-music"),
    ("XDG_PICTURES_DIR", "Pictures", "folder-pictures"),
    ("XDG_VIDEOS_DIR", "Videos", "folder-videos"),
];

/// The place icon of a well-known directory, or `None` for every ordinary
/// folder. The lookup canonicalizes once per path and then remembers the
/// answer, because icons are built every frame.
pub(crate) fn place(path: &Path) -> Option<&'static str> {
    static PLACES: OnceLock<Mutex<HashMap<PathBuf, Option<&'static str>>>> = OnceLock::new();
    let places = PLACES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(known) = places.lock().unwrap().get(path) {
        return *known;
    }
    let resolved = std::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|home| place_name(path, &home, &user_dirs(&home)));
    places.lock().unwrap().insert(path.to_owned(), resolved);
    resolved
}

/// The pure half of `place`: the home folder and the XDG user directories,
/// with `dirs` overriding the spec defaults.
fn place_name(path: &Path, home: &Path, dirs: &[(String, String)]) -> Option<&'static str> {
    let key = comparable(path);
    if key == comparable(home) {
        return Some("user-home");
    }
    for (name, default, icon) in WELL_KNOWN_FOLDERS {
        let dir = dirs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| PathBuf::from(value))
            .unwrap_or_else(|| home.join(default));
        if comparable(&dir) == key {
            return Some(icon);
        }
    }
    None
}

/// The form path comparisons use: the canonical path when the directory
/// exists, the path itself otherwise.
fn comparable(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `XDG_*_DIR="value"` entries from `user-dirs.dirs`, `$HOME` expanded.
fn user_dirs(home: &Path) -> Vec<(String, String)> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    match std::fs::read_to_string(config.join("user-dirs.dirs")) {
        Ok(text) => parse_user_dirs(&text, home),
        Err(_) => Vec::new(),
    }
}

fn parse_user_dirs(text: &str, home: &Path) -> Vec<(String, String)> {
    let home = home.to_string_lossy();
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let value = value.trim().trim_matches('"');
            let value = value.replace("${HOME}", &home).replace("$HOME", &home);
            Some((key.trim().to_owned(), value))
        })
        .collect()
}

// -------------------------------------------------------------- artwork ----

/// One themed icon as encoded PNG bytes, ready for the rotation and preview
/// pipeline. The key already names the theme, so derived caches can use it.
#[derive(Clone)]
pub(crate) struct Artwork {
    pub key: String,
    pub png: Arc<Vec<u8>>,
}

/// A derived-artwork key to its bytes, or a timed miss.
type ArtworkCache = Mutex<HashMap<String, (Option<Arc<Vec<u8>>>, Instant)>>;

/// Memoize derived artwork bytes under a caller-chosen key. A miss is retried
/// after `RETRY_AFTER` because files apps fill caches while the shell runs.
pub(crate) fn cached(
    key: &str,
    load: impl FnOnce() -> Option<Vec<u8>>,
) -> Option<Arc<Vec<u8>>> {
    static CACHE: OnceLock<ArtworkCache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
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

/// The theme's artwork for one freedesktop name, rasterized to PNG and cached.
pub(crate) fn artwork(name: &str) -> Option<Artwork> {
    let key = format!("theme:{}:{name}", theme());
    let png = cached(&key, || resolve(name).and_then(|path| rasterize(&path)))?;
    Some(Artwork { key, png })
}

/// The same artwork decoded for GPUI. Decoding is cached separately because
/// `Image::from_bytes` takes ownership of its bytes.
pub(crate) fn image(name: &str) -> Option<Arc<Image>> {
    static IMAGES: OnceLock<Mutex<HashMap<String, Arc<Image>>>> = OnceLock::new();
    let artwork = artwork(name)?;
    let images = IMAGES.get_or_init(Default::default);
    if let Some(cached) = images.lock().unwrap().get(&artwork.key) {
        return Some(cached.clone());
    }
    let image = Arc::new(Image::from_bytes(
        ImageFormat::Png,
        artwork.png.as_ref().clone(),
    ));
    let mut images = images.lock().unwrap();
    if images.len() >= CACHE_LIMIT {
        images.clear();
    }
    images.insert(artwork.key, image.clone());
    Some(image)
}

/// The Dock's trash from the theme's places (`user-trash[-full]`). A theme
/// that only marks the empty state keeps that mark when full; `None` lets the
/// caller keep its bundled PNG.
pub(crate) fn trash(full: bool) -> Option<Arc<Image>> {
    let name = if full { "user-trash-full" } else { "user-trash" };
    image(name).or_else(|| image("user-trash"))
}

pub(crate) fn is_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
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

pub(crate) fn render_svg(bytes: &[u8]) -> Option<Vec<u8>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_svg_rasterizes_to_png() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><rect width="24" height="24" fill="#ff0000"/></svg>"##;
        let png = render_svg(svg).expect("resvg renders a plain svg");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn embedded_raster_artwork_renders() {
        // Some MacTahoe place icons wrap an embedded PNG in an SVG `<image>`
        // (`user-trash-full` is one). Without resvg's `raster-images` feature
        // the wrapper parses fine but paints nothing, so the trash vanished
        // the moment it had contents.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="64" height="64"><image xlink:href="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACAQMAAABIeJ9nAAAAIGNIUk0AAHomAACAhAAA+gAAAIDoAAB1MAAA6mAAADqYAAAXcJy6UTwAAAADUExURf8AABniCTcAAAAHdElNRQfqCw0OCybF1gq0AAAAJXRFWHRkYXRlOmNyZWF0ZQAyMDI2LTExLTEzVDE0OjExOjM4KzAwOjAwrByORwAAACV0RVh0ZGF0ZTptb2RpZnkAMjAyNi0xMS0xM1QxNDoxMTozOCswMDowMN1BNvsAAAAodEVYdGRhdGU6dGltZXN0YW1wADIwMjYtMTEtMTNUMTQ6MTE6MzgrMDA6MDCKVBckAAAADElEQVQI12NgYGAAAAAEAAEnNCcKAAAAAElFTkSuQmCC" width="64" height="64"/></svg>"##;
        let png = render_svg(svg).expect("the wrapper parses");
        let image = ::image::load_from_memory_with_format(&png, ::image::ImageFormat::Png)
            .expect("the renderer emits a png")
            .to_rgba8();
        assert!(
            image.pixels().any(|pixel| pixel.0[3] > 0),
            "an embedded PNG must contribute pixels, not a transparent square"
        );
    }

    #[test]
    fn icon_theme_variant_follows_the_shell_appearance() {
        let installed = |name: &str| {
            matches!(
                name,
                "MacTahoe" | "MacTahoe-dark" | "Papirus" | "Papirus-Dark"
            )
        };
        assert_eq!(variant("MacTahoe", true, installed), "MacTahoe-dark");
        assert_eq!(variant("MacTahoe", false, installed), "MacTahoe");
        // A desktop that already selected the dark variant comes back to the
        // light one when the shell switches appearance.
        assert_eq!(variant("MacTahoe-dark", false, installed), "MacTahoe");
        assert_eq!(variant("MacTahoe-dark", true, installed), "MacTahoe-dark");
        // Both common spellings of the dark suffix are understood.
        assert_eq!(variant("Papirus", true, installed), "Papirus-Dark");
        assert_eq!(variant("Papirus-Dark", false, installed), "Papirus");
        // A theme without a dark variant is used exactly as installed.
        assert_eq!(variant("Adwaita", true, |_| false), "Adwaita");
    }

    #[test]
    fn bundled_glyphs_borrow_their_theme_counterpart() {
        assert_eq!(symbolic("bell"), Some("notifications-symbolic"));
        assert_eq!(
            symbolic("bell-off"),
            Some("notifications-disabled-symbolic")
        );
        assert!(
            asset_svg("bell-off").is_file(),
            "themes without a disabled bell need the bundled fallback"
        );
        // Power profiles keep their bundled marks: the theme's 16 px status
        // glyphs turn to mush at the popup's size.
        for asset in ["power-saver.svg", "balanced.svg", "performance.svg"] {
            assert_eq!(symbolic(asset), None, "{asset} must stay bundled");
        }
    }

    #[test]
    fn resolution_rejects_names_that_escape_the_theme() {
        assert_eq!(resolve_uncached(""), None);
        assert_eq!(resolve_uncached("../etc/passwd"), None);
        assert_eq!(resolve_uncached("places/../../etc/passwd"), None);
    }

    #[test]
    fn well_known_folders_use_their_place_icons() {
        let home = Path::new("/home/me");
        assert_eq!(place_name(Path::new("/home/me"), home, &[]), Some("user-home"));
        assert_eq!(
            place_name(Path::new("/home/me/Downloads"), home, &[]),
            Some("folder-download")
        );
        assert_eq!(
            place_name(Path::new("/home/me/Pictures"), home, &[]),
            Some("folder-pictures")
        );
        assert_eq!(
            place_name(Path::new("/home/me/Desktop"), home, &[]),
            Some("folder-desktop")
        );
        // `user-dirs.dirs` moves the folder; the default name must not keep
        // answering for the old path.
        let dirs = vec![(
            "XDG_DOWNLOAD_DIR".to_owned(),
            "/mnt/hoard/dl".to_owned(),
        )];
        assert_eq!(
            place_name(Path::new("/mnt/hoard/dl"), home, &dirs),
            Some("folder-download")
        );
        assert_eq!(
            place_name(Path::new("/home/me/Downloads"), home, &dirs),
            None
        );
        // Ordinary folders keep the plain folder mark.
        assert_eq!(place_name(Path::new("/home/me/Projects"), home, &[]), None);
    }

    #[test]
    fn user_dirs_entries_expand_the_home_macro() {
        let text = "# written by xdg-user-dirs-update\n\
                    XDG_DOCUMENTS_DIR=\"$HOME/Documents\"\n\
                    XDG_DOWNLOAD_DIR=\"${HOME}/dl\"\n";
        assert_eq!(
            parse_user_dirs(text, Path::new("/home/me")),
            vec![
                (
                    "XDG_DOCUMENTS_DIR".to_owned(),
                    "/home/me/Documents".to_owned()
                ),
                ("XDG_DOWNLOAD_DIR".to_owned(), "/home/me/dl".to_owned()),
            ]
        );
    }
}
