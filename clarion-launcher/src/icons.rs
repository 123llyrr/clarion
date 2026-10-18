// SPDX-License-Identifier: GPL-3.0-or-later
//! Filesystem icon-theme resolution without starting another GUI toolkit.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Take};
use std::path::{Component, Path, PathBuf};

use walkdir::WalkDir;

const MAX_THEME_BYTES: u64 = 256 * 1024;
const MAX_INHERITED_THEMES: usize = 32;

type Ini = HashMap<String, HashMap<String, String>>;

/// Resolves PNG/SVG app artwork in the selected GTK theme and its inheritance chain.
/// XPM is deliberately excluded because GPUI's image decoder does not support it.
#[derive(Debug)]
pub struct IconResolver {
    theme: String,
    roots: Vec<PathBuf>,
    pixmaps: Vec<PathBuf>,
    themes: HashMap<String, ThemeIndex>,
    cache: HashMap<(String, u32), Option<PathBuf>>,
}

impl IconResolver {
    pub fn new(environment: &HashMap<String, String>) -> Self {
        let data_roots = crate::catalog::data_roots(environment);
        let mut roots = Vec::new();
        if let Some(home) = absolute_env(environment, "HOME") {
            roots.push(home.join(".icons"));
        }
        roots.extend(data_roots.iter().map(|root| root.join("icons")));
        deduplicate(&mut roots);
        let mut pixmaps = data_roots
            .iter()
            .map(|root| root.join("pixmaps"))
            .collect::<Vec<_>>();
        // The unthemed system fallback is independent of XDG_DATA_DIRS.
        pixmaps.push(PathBuf::from("/usr/share/pixmaps"));
        deduplicate(&mut pixmaps);
        Self {
            theme: selected_theme(environment),
            roots,
            pixmaps,
            themes: HashMap::new(),
            cache: HashMap::new(),
        }
    }

    /// `pixel_size` already includes display scale. It is never multiplied again.
    pub fn resolve(&mut self, name: &str, pixel_size: u32) -> Option<PathBuf> {
        let pixel_size = pixel_size.clamp(1, 512);
        let key = (name.to_owned(), pixel_size);
        if let Some(path) = self.cache.get(&key) {
            return path.clone();
        }
        let path = if Path::new(name).is_absolute() {
            supported_file(Path::new(name)).then(|| PathBuf::from(name))
        } else if single_component(name) {
            let mut visited = HashSet::new();
            let theme = self.theme.clone();
            self.resolve_theme(&theme, name, pixel_size, &mut visited)
                .or_else(|| self.resolve_theme("hicolor", name, pixel_size, &mut visited))
                .or_else(|| {
                    self.roots
                        .iter()
                        .chain(&self.pixmaps)
                        .find_map(|root| unthemed_icon(root, name, pixel_size))
                })
        } else {
            None
        };
        if self.cache.len() >= 4096 {
            self.cache.clear();
        }
        self.cache.insert(key, path.clone());
        path
    }

    fn resolve_theme(
        &mut self,
        theme: &str,
        name: &str,
        pixel_size: u32,
        visited: &mut HashSet<String>,
    ) -> Option<PathBuf> {
        if !single_component(theme)
            || visited.len() >= MAX_INHERITED_THEMES
            || !visited.insert(theme.to_owned())
        {
            return None;
        }
        let index = self
            .themes
            .entry(theme.to_owned())
            .or_insert_with(|| ThemeIndex::load(&self.roots, theme));
        // Keep the artwork from the first theme containing this icon, even if
        // an inherited theme has a larger version with a different design.
        if let Some(path) = index.find(name, pixel_size) {
            return Some(path);
        }
        let inherited = index.inherits.clone();
        for parent in inherited {
            if let Some(path) = self.resolve_theme(&parent, name, pixel_size, visited) {
                return Some(path);
            }
        }
        None
    }
}

#[derive(Debug, Default)]
struct ThemeIndex {
    inherits: Vec<String>,
    files: HashMap<String, Vec<Candidate>>,
}

impl ThemeIndex {
    fn load(roots: &[PathBuf], theme: &str) -> Self {
        let metadata = roots
            .iter()
            .find_map(|root| read_small(&root.join(theme).join("index.theme")))
            .map(|source| parse_ini(&source))
            .unwrap_or_default();
        let theme_info = metadata.get("Icon Theme");
        let inherits = theme_info
            .and_then(|values| values.get("Inherits"))
            .map(|value| comma_list(value).map(str::to_owned).collect())
            .unwrap_or_default();
        let directories = ["Directories", "ScaledDirectories"]
            .iter()
            .filter_map(|key| theme_info.and_then(|values| values.get(*key)))
            .flat_map(|value| comma_list(value))
            .filter(|directory| safe_relative_path(directory))
            .map(|directory| {
                (
                    directory.to_owned(),
                    DirectorySize::from_metadata(metadata.get(directory)),
                )
            })
            .collect::<Vec<_>>();
        let mut index = Self {
            inherits,
            files: HashMap::new(),
        };
        for (priority, root) in roots.iter().enumerate() {
            let root = root.join(theme);
            if directories.is_empty() {
                // Some exported/application themes omit index.theme. Index once,
                // without following directory symlinks or recursively rescanning
                // the theme for each application.
                for entry in WalkDir::new(&root)
                    .follow_links(false)
                    .max_depth(5)
                    .into_iter()
                    .filter_map(Result::ok)
                {
                    if !entry.file_type().is_dir() {
                        let size = DirectorySize::from_path(entry.path());
                        index.insert(entry.into_path(), size, priority);
                    }
                }
            } else {
                index.scan_directory(&root, DirectorySize::default(), priority);
                for (directory, size) in &directories {
                    index.scan_directory(&root.join(directory), *size, priority);
                }
            }
        }
        index
    }

    fn scan_directory(&mut self, directory: &Path, size: DirectorySize, priority: usize) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            if entry.file_type().is_ok_and(|kind| !kind.is_dir()) {
                self.insert(entry.path(), size, priority);
            }
        }
    }

    fn insert(&mut self, path: PathBuf, size: DirectorySize, priority: usize) {
        if !supported_extension(&path) {
            return;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
            return;
        };
        let candidate = Candidate {
            path: path.clone(),
            size,
            priority,
        };
        self.files
            .entry(name.to_owned())
            .or_default()
            .push(candidate.clone());
        self.files
            .entry(stem.to_owned())
            .or_default()
            .push(candidate);
    }

    fn find(&self, name: &str, pixel_size: u32) -> Option<PathBuf> {
        self.files
            .get(name)?
            .iter()
            .filter_map(|candidate| {
                let rank = candidate.rank(pixel_size)?;
                Some((rank, &candidate.path))
            })
            .min()
            .map(|(_, path)| path.clone())
    }
}

#[derive(Debug, Clone, Copy)]
enum DirectoryKind {
    Fixed,
    Scalable { min: u32, max: u32 },
    Threshold(u32),
}

#[derive(Debug, Clone, Copy)]
struct DirectorySize {
    size: u32,
    scale: u32,
    kind: DirectoryKind,
}

impl Default for DirectorySize {
    fn default() -> Self {
        Self {
            size: 48,
            scale: 1,
            kind: DirectoryKind::Fixed,
        }
    }
}

impl DirectorySize {
    fn from_metadata(values: Option<&HashMap<String, String>>) -> Self {
        let Some(values) = values else {
            return Self::default();
        };
        let number = |key: &str, default| {
            values
                .get(key)
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(default)
        };
        let size = number("Size", 48).max(1);
        let kind = match values.get("Type").map(String::as_str) {
            Some("Fixed") => DirectoryKind::Fixed,
            Some("Scalable") => DirectoryKind::Scalable {
                min: number("MinSize", size),
                max: number("MaxSize", size),
            },
            _ => DirectoryKind::Threshold(number("Threshold", 2)),
        };
        Self {
            size,
            scale: number("Scale", 1).clamp(1, 16),
            kind,
        }
    }

    fn from_path(path: &Path) -> Self {
        for component in path.components().rev() {
            let Some(value) = component.as_os_str().to_str() else {
                continue;
            };
            if value == "scalable" || value == "symbolic" {
                return Self {
                    kind: DirectoryKind::Scalable { min: 1, max: 512 },
                    ..Self::default()
                };
            }
            if let Some((width, height)) = value.split_once('x') {
                if let (Ok(width), Ok(height)) = (width.parse::<u32>(), height.parse::<u32>()) {
                    return Self {
                        size: width.max(height).max(1),
                        ..Self::default()
                    };
                }
            }
        }
        Self::default()
    }

    fn distance(self, pixels: u32) -> u32 {
        let (min, max) = match self.kind {
            DirectoryKind::Fixed => (self.size, self.size),
            DirectoryKind::Scalable { min, max } => (min.min(max), min.max(max)),
            DirectoryKind::Threshold(threshold) => (
                self.size.saturating_sub(threshold),
                self.size.saturating_add(threshold),
            ),
        };
        let min = min.saturating_mul(self.scale);
        let max = max.saturating_mul(self.scale);
        if pixels < min {
            min - pixels
        } else {
            pixels.saturating_sub(max)
        }
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    size: DirectorySize,
    priority: usize,
}

impl Candidate {
    fn rank(&self, pixels: u32) -> Option<(bool, u32, bool, usize)> {
        if !supported_file(&self.path) {
            return None;
        }
        let svg = self
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"));
        let undersized = !svg && png_extent(&self.path).is_none_or(|extent| extent < pixels);
        Some((undersized, self.size.distance(pixels), svg, self.priority))
    }
}

fn unthemed_icon(root: &Path, name: &str, pixel_size: u32) -> Option<PathBuf> {
    let paths = if supported_extension(Path::new(name)) {
        vec![root.join(name)]
    } else {
        vec![
            root.join(format!("{name}.png")),
            root.join(format!("{name}.svg")),
        ]
    };
    paths
        .into_iter()
        .filter_map(|path| {
            let candidate = Candidate {
                path,
                size: DirectorySize::default(),
                priority: 0,
            };
            let rank = candidate.rank(pixel_size)?;
            Some((rank, candidate.path))
        })
        .min()
        .map(|(_, path)| path)
}

fn supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("png") || extension.eq_ignore_ascii_case("svg")
        })
}

fn supported_file(path: &Path) -> bool {
    supported_extension(path) && fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn png_extent(path: &Path) -> Option<u32> {
    let mut header = [0; 24];
    File::open(path).ok()?.read_exact(&mut header).ok()?;
    if &header[..8] != b"\x89PNG\r\n\x1a\n" || &header[12..16] != b"IHDR" {
        return None;
    }
    Some(
        u32::from_be_bytes(header[16..20].try_into().ok()?)
            .max(u32::from_be_bytes(header[20..24].try_into().ok()?)),
    )
}

fn selected_theme(environment: &HashMap<String, String>) -> String {
    if let Some(theme) = environment
        .get("GTK_ICON_THEME")
        .filter(|theme| single_component(theme))
    {
        return theme.clone();
    }
    let mut configs = Vec::new();
    if let Some(config) = absolute_env(environment, "XDG_CONFIG_HOME") {
        configs.push(config);
    } else if let Some(home) = absolute_env(environment, "HOME") {
        configs.push(home.join(".config"));
    }
    let system = environment
        .get("XDG_CONFIG_DIRS")
        .map(String::as_str)
        .unwrap_or("/etc/xdg");
    configs.extend(
        system
            .split(':')
            .filter(|path| Path::new(path).is_absolute())
            .map(PathBuf::from),
    );
    for config in configs {
        for version in ["gtk-3.0", "gtk-4.0"] {
            if let Some(source) = read_small(&config.join(version).join("settings.ini")) {
                if let Some(theme) = parse_ini(&source)
                    .get("Settings")
                    .and_then(|values| values.get("gtk-icon-theme-name"))
                {
                    let theme = theme.trim_matches(['\'', '"']);
                    if single_component(theme) {
                        return theme.to_owned();
                    }
                }
            }
        }
    }
    "Adwaita".to_owned()
}

fn absolute_env(environment: &HashMap<String, String>, key: &str) -> Option<PathBuf> {
    environment
        .get(key)
        .filter(|path| Path::new(path).is_absolute())
        .map(PathBuf::from)
}

fn single_component(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && Path::new(value)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn deduplicate(paths: &mut Vec<PathBuf>) {
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
}

fn comma_list(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn read_small(path: &Path) -> Option<String> {
    if !fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut file: Take<File> = File::open(path).ok()?.take(MAX_THEME_BYTES + 1);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_THEME_BYTES)
        .then(|| String::from_utf8(bytes).ok())
        .flatten()
}

fn parse_ini(source: &str) -> Ini {
    let mut sections = Ini::new();
    let mut section = String::new();
    for line in source.lines().map(str::trim) {
        if line.starts_with('#') || line.starts_with(';') || line.is_empty() {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            section = name.to_owned();
        } else if let Some((key, value)) = line.split_once('=') {
            sections
                .entry(section.clone())
                .or_default()
                .insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    sections
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    use super::*;

    fn environment(root: &TempDir) -> HashMap<String, String> {
        HashMap::from([
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            (
                "XDG_DATA_HOME".into(),
                root.path().join("data").to_string_lossy().into_owned(),
            ),
            (
                "XDG_DATA_DIRS".into(),
                root.path().join("system").to_string_lossy().into_owned(),
            ),
            (
                "XDG_CONFIG_DIRS".into(),
                root.path().join("config").to_string_lossy().into_owned(),
            ),
            ("GTK_ICON_THEME".into(), "Fixture".into()),
        ])
    }

    fn write(path: &Path, source: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }

    fn png(path: &Path, size: u32) {
        let mut header = *b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\0\0\0\0\0";
        header[16..20].copy_from_slice(&size.to_be_bytes());
        header[20..24].copy_from_slice(&size.to_be_bytes());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, header).unwrap();
    }

    #[test]
    fn prefers_adequate_artwork_in_selected_theme_before_inheritance() {
        let root = TempDir::new().unwrap();
        let theme = root.path().join("data/icons/Fixture");
        write(&theme.join("index.theme"), "[Icon Theme]\nDirectories=32x32/apps,256x256/apps\nInherits=Parent\n[32x32/apps]\nSize=32\nType=Fixed\n[256x256/apps]\nSize=256\nType=Fixed\n");
        let low = theme.join("32x32/apps/demo.png");
        let high = theme.join("256x256/apps/demo.png");
        png(&low, 32);
        png(&high, 256);
        let parent = root.path().join("data/icons/Parent/scalable/apps/demo.svg");
        write(&parent, "<svg/>");
        let mut resolver = IconResolver::new(&environment(&root));
        assert_eq!(resolver.resolve("demo", 128), Some(high));
        assert_eq!(resolver.resolve("demo", 24), Some(low));
        assert_eq!(
            resolver.resolve("demo", 512).unwrap().parent(),
            Some(theme.join("256x256/apps").as_path())
        );
    }

    #[test]
    fn reads_gtk_settings_and_follows_inheritance_cycles_then_hicolor() {
        let root = TempDir::new().unwrap();
        let mut env = environment(&root);
        env.remove("GTK_ICON_THEME");
        write(
            &root.path().join(".config/gtk-3.0/settings.ini"),
            "[Settings]\ngtk-icon-theme-name=Custom\n",
        );
        write(
            &root.path().join("data/icons/Custom/index.theme"),
            "[Icon Theme]\nInherits=Parent\n",
        );
        write(
            &root.path().join("data/icons/Parent/index.theme"),
            "[Icon Theme]\nInherits=Custom\n",
        );
        let inherited = root
            .path()
            .join("data/icons/Parent/scalable/apps/inherited.svg");
        write(&inherited, "<svg/>");
        let fallback = root
            .path()
            .join("system/icons/hicolor/scalable/apps/fallback.svg");
        write(&fallback, "<svg/>");
        let mut resolver = IconResolver::new(&env);
        assert_eq!(resolver.resolve("inherited", 128), Some(inherited));
        assert_eq!(resolver.resolve("fallback", 128), Some(fallback));
    }

    #[test]
    fn honors_physical_sizes_and_preserves_user_theme_priority() {
        let root = TempDir::new().unwrap();
        let theme = root.path().join("data/icons/Fixture");
        write(&theme.join("index.theme"), "[Icon Theme]\nDirectories=64x64@2/apps,128x128/apps\n[64x64@2/apps]\nSize=64\nScale=2\nType=Fixed\n[128x128/apps]\nSize=128\nType=Fixed\n");
        let user = theme.join("64x64@2/apps/demo.png");
        png(&user, 128);
        png(
            &root
                .path()
                .join("system/icons/Fixture/64x64@2/apps/demo.png"),
            128,
        );
        let mut resolver = IconResolver::new(&environment(&root));
        assert_eq!(resolver.resolve("demo", 128), Some(user));
    }

    #[test]
    fn resolves_absolute_and_exported_symlink_files_and_pixmaps() {
        let root = TempDir::new().unwrap();
        let original = root.path().join("original.svg");
        write(&original, "<svg/>");
        let exported = root
            .path()
            .join("data/icons/hicolor/scalable/apps/exported.svg");
        fs::create_dir_all(exported.parent().unwrap()).unwrap();
        symlink(&original, &exported).unwrap();
        let fallback = root.path().join("system/pixmaps/other.png");
        png(&fallback, 128);
        let mut resolver = IconResolver::new(&environment(&root));
        assert_eq!(
            resolver.resolve(original.to_str().unwrap(), 128),
            Some(original)
        );
        assert_eq!(resolver.resolve("exported", 128), Some(exported));
        assert_eq!(resolver.resolve("other", 128), Some(fallback));
        assert_eq!(resolver.resolve("../original", 128), None);
        assert_eq!(resolver.resolve("not-present", 128), None);
    }
}
