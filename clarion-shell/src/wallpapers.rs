// SPDX-License-Identifier: GPL-3.0-or-later
//! The palette's wallpaper library.
//!
//! Clarion stores the selected wallpaper in `wallpaper.json`; the palette
//! scans the usual picture folders for other images. Decoding happens on a
//! worker thread and every entry carries a small PNG thumbnail, so the sheet
//! never blocks on a multi-megabyte photograph.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use image::{ImageFormat, ImageReader};
use walkdir::WalkDir;

const MAX_DEPTH: usize = 4;
const MAX_ENTRIES: usize = 160;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const THUMB_W: u32 = 360;
const THUMB_H: u32 = 240;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallpaperEntry {
    pub path: PathBuf,
    pub name: String,
    pub parent: PathBuf,
    pub modified: Option<SystemTime>,
    /// PNG bytes for the 360×240 preview, when the image decoded.
    pub thumbnail: Option<Vec<u8>>,
}

/// Folders scanned by the gallery, in priority order. The current wallpaper's
/// own directory is prepended so a photo chosen from anywhere stays visible.
pub fn roots(environment: &std::collections::HashMap<String, String>) -> Vec<PathBuf> {
    let Some(home) = environment.get("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    if let Some(current) = current_wallpaper(environment) {
        if let Some(parent) = current.parent() {
            roots.push(parent.to_path_buf());
        }
    }
    for relative in ["Pictures/Wallpapers", "Pictures/wallpapers", "Pictures", "壁纸"] {
        let candidate = home.join(relative);
        if candidate.is_dir() && !roots.contains(&candidate) {
            roots.push(candidate);
        }
    }
    roots
}

/// The wallpaper the shell last applied, read from its own config. The
/// gallery marks this entry as current; a missing or stale path is normal.
pub fn current_wallpaper(
    environment: &std::collections::HashMap<String, String>,
) -> Option<PathBuf> {
    let config = environment
        .get("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| environment.get("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    let bytes = std::fs::read(config.join("clarion-shell/wallpaper.json")).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let path = PathBuf::from(json["path"].as_str()?);
    path.is_file().then_some(path)
}

/// Scan every known picture folder and decode a thumbnail for each image.
/// Missing folders are normal; unreadable images are skipped individually.
pub fn scan(environment: &std::collections::HashMap<String, String>) -> Vec<WallpaperEntry> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for root in roots(environment) {
        for entry in WalkDir::new(&root)
            .max_depth(MAX_DEPTH)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0 || !entry.file_name().to_string_lossy().starts_with('.')
            })
            .flatten()
        {
            if entries.len() >= MAX_ENTRIES {
                break;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !is_image(path) {
                continue;
            }
            let Some(metadata) = entry.metadata().ok() else {
                continue;
            };
            if metadata.len() > MAX_FILE_BYTES {
                continue;
            }
            if !seen.insert(path.to_path_buf()) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            entries.push(WallpaperEntry {
                path: path.to_path_buf(),
                name: name.to_owned(),
                parent: path.parent().unwrap_or(&root).to_path_buf(),
                modified: metadata.modified().ok(),
                thumbnail: thumbnail(path).ok(),
            });
        }
    }
    entries.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    entries
}

pub fn is_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg")
    )
}

fn thumbnail(path: &Path) -> Result<Vec<u8>> {
    let reader = ImageReader::open(path)
        .context("open wallpaper")?
        .with_guessed_format()
        .context("guess wallpaper format")?;
    let format = reader.format().context("unknown wallpaper format")?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        anyhow::bail!("unsupported wallpaper format");
    }
    let image = reader.decode().context("decode wallpaper")?;
    // `thumbnail` would also blow a small image up to the preview box; only
    // shrink what is actually larger than it.
    let image = if image.width() > THUMB_W || image.height() > THUMB_H {
        image.thumbnail(THUMB_W, THUMB_H)
    } else {
        image
    };
    let mut output = std::io::Cursor::new(Vec::new());
    image.write_to(&mut output, ImageFormat::Png)?;
    Ok(output.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn environment(root: &Path) -> std::collections::HashMap<String, String> {
        [
            ("HOME".into(), root.display().to_string()),
            (
                "XDG_CONFIG_HOME".into(),
                root.join("config").display().to_string(),
            ),
        ]
        .into()
    }

    fn write_png(path: &Path, colour: [u8; 3]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        RgbImage::from_pixel(32, 24, Rgb(colour)).save(path).unwrap();
    }

    #[test]
    fn scan_collects_picture_folders_and_skips_hidden_or_non_images() {
        let root = tempfile::tempdir().unwrap();
        write_png(&root.path().join("Pictures/Wallpapers/a.png"), [10, 20, 30]);
        write_png(&root.path().join("Pictures/b.jpg"), [40, 50, 60]);
        write_png(&root.path().join("Pictures/.hidden.png"), [1, 2, 3]);
        std::fs::create_dir_all(root.path().join("Pictures/Wallpapers")).unwrap();
        std::fs::write(root.path().join("Pictures/Wallpapers/readme.txt"), b"no").unwrap();

        let entries = scan(&environment(root.path()));
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["a.png", "b.jpg"]);
        assert!(entries.iter().all(|entry| entry.thumbnail.is_some()));
        let thumbnail = image::load_from_memory(entries[0].thumbnail.as_ref().unwrap()).unwrap();
        assert_eq!((thumbnail.width(), thumbnail.height()), (32, 24));
    }

    #[test]
    fn the_saved_wallpaper_folder_is_scanned_even_outside_pictures() {
        let root = tempfile::tempdir().unwrap();
        write_png(&root.path().join("art/current.png"), [7, 8, 9]);
        let config = root.path().join("config/clarion-shell");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("wallpaper.json"),
            serde_json::json!({"version": 1, "path": root.path().join("art/current.png")})
                .to_string(),
        )
        .unwrap();
        let entries = scan(&environment(root.path()));
        assert!(entries.iter().any(|entry| entry.name == "current.png"));
    }

    #[test]
    fn a_missing_picture_folder_is_not_an_error() {
        let root = tempfile::tempdir().unwrap();
        assert!(scan(&environment(root.path())).is_empty());
    }
}
