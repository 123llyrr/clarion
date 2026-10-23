//! Toolkit-independent Clarion desktop preferences. Preview never reads or writes
//! session preferences; legacy appearance switches are deliberately ignored.
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub type Environment = HashMap<String, String>;
const MAX_BYTES: u64 = 65_536;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DockPosition {
    #[default]
    Bottom,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MinimizeEffect {
    #[default]
    Genie,
    Scale,
}

pub(crate) const MAX_DOCK_FOLDERS: usize = 8;
/// Opacity below this leaves even high-contrast text floating over the
/// wallpaper. Frost strength is a separate 0..=1 blend.
pub const MIN_SURFACE_OPACITY: f64 = 0.30;
pub const MAX_SURFACE_OPACITY: f64 = 1.;
pub const DEFAULT_DOCK_OPACITY: f64 = 0.62;
pub const DEFAULT_SURFACE_OPACITY: f64 = 0.68;
pub const DEFAULT_FROST_STRENGTH: f64 = 0.82;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderDisplay {
    #[default]
    Stack,
    Folder,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderLayout {
    #[default]
    Auto,
    List,
    Grid,
    Fan,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderSort {
    #[default]
    Name,
    Modified,
    Kind,
    Size,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockFolderPreference {
    pub path: String,
    #[serde(default)]
    pub display: FolderDisplay,
    #[serde(default)]
    pub layout: FolderLayout,
    #[serde(default)]
    pub sort: FolderSort,
    #[serde(default)]
    pub descending: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preferences {
    pub theme: String,
    pub accent: String,
    pub reduce_motion: bool,
    /// None chooses installed defaults; Some([]) explicitly pins nothing.
    pub pinned_app_ids: Option<Vec<String>>,
    /// The arrangement of the pinned applications. None leaves the pinned
    /// group in the order the preferences file lists it.
    pub dock_order: Option<Vec<String>>,
    /// Folders pinned to the right of the app row, next to the trash.
    pub dock_folders: Vec<DockFolderPreference>,
    pub recent_app_ids: Vec<String>,
    pub bar_height: f64,
    pub bar_auto_hide: bool,
    pub show_battery_percentage: bool,
    pub clock_show_date: bool,
    pub clock_show_seconds: bool,
    pub dock_size: f64,
    /// The share of the Dock tray colour that stays visible. The remaining
    /// share lets the frosted wallpaper through.
    pub dock_opacity: f64,
    /// The same opacity for menus, popups and their inner containers.
    pub surface_opacity: f64,
    /// Compositor frost plus the translucent wash layered over every surface.
    /// Zero disables the background effect entirely.
    pub frost_strength: f64,
    pub dock_position: DockPosition,
    pub dock_auto_hide: bool,
    pub dock_hidden: bool,
    pub magnification: bool,
    pub magnification_scale: f64,
    pub show_running_indicators: bool,
    pub animate_opening_applications: bool,
    pub show_recent_applications: bool,
    pub show_launcher: bool,
    pub show_downloads: bool,
    pub minimize_into_app: bool,
    pub use_original_icons: bool,
    pub minimize_effect: MinimizeEffect,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            accent: "#0a84ff".into(),
            reduce_motion: false,
            pinned_app_ids: None,
            dock_order: None,
            dock_folders: Vec::new(),
            recent_app_ids: Vec::new(),
            bar_height: 24.,
            bar_auto_hide: false,
            show_battery_percentage: false,
            clock_show_date: true,
            clock_show_seconds: false,
            dock_size: 48.,
            // A cool, translucent slate keeps the desktop visible through the
            // tray while the frosted wash stops icon colours from fighting it.
            dock_opacity: DEFAULT_DOCK_OPACITY,
            surface_opacity: DEFAULT_SURFACE_OPACITY,
            frost_strength: DEFAULT_FROST_STRENGTH,
            dock_position: DockPosition::Bottom,
            dock_auto_hide: false,
            dock_hidden: false,
            magnification: true,
            magnification_scale: 1.45,
            show_running_indicators: true,
            animate_opening_applications: true,
            show_recent_applications: true,
            show_launcher: true,
            show_downloads: true,
            minimize_into_app: true,
            use_original_icons: true,
            minimize_effect: MinimizeEffect::Genie,
        }
    }
}

/// Toolkit-independent snapshot of the translucent surface controls. The
/// desktop window lives outside `ShellView`, so it receives this instead of
/// reaching into the shell palette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceAppearance {
    pub dark: bool,
    pub dock_opacity: f32,
    pub surface_opacity: f32,
    pub frost: f32,
}

impl Default for SurfaceAppearance {
    fn default() -> Self {
        let prefs = Preferences::default();
        Self {
            dark: true,
            dock_opacity: prefs.dock_opacity as f32,
            surface_opacity: prefs.surface_opacity as f32,
            frost: prefs.frost_strength as f32,
        }
    }
}

impl Preferences {
    pub fn surface_appearance(&self) -> SurfaceAppearance {
        SurfaceAppearance {
            dark: self.theme != "light",
            dock_opacity: self.dock_opacity as f32,
            surface_opacity: self.surface_opacity as f32,
            frost: self.frost_strength as f32,
        }
    }
}

pub fn config_directory(environment: &Environment) -> Result<PathBuf> {
    clarion_launcher::preferences::config_directory(environment)
}

pub fn preferences_path(environment: &Environment) -> Result<PathBuf> {
    Ok(config_directory(environment)?.join("preferences.json"))
}

fn read_object(path: &Path) -> Result<Map<String, Value>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(error).context("Open desktop preferences"),
    };
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= MAX_BYTES,
        "Desktop preferences must be a small regular file"
    );
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "Desktop preferences are too large"
    );
    let value: Value = serde_json::from_slice(&bytes).context("Parse desktop preferences")?;
    value
        .as_object()
        .cloned()
        .context("Desktop preferences must contain an object")
}

fn ids(value: Option<&Value>, limit: usize) -> Option<Vec<String>> {
    let array = value?.as_array()?;
    let mut seen = HashSet::new();
    Some(
        array
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty() && id.len() <= 1024 && seen.insert((*id).to_owned()))
            .take(limit)
            .map(str::to_owned)
            .collect(),
    )
}

fn dock_folder_preferences(value: Option<&Value>) -> Vec<DockFolderPreference> {
    let Some(array) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut folders = Vec::new();
    for item in array {
        if folders.len() >= MAX_DOCK_FOLDERS {
            break;
        }
        let Some(object) = item.as_object() else {
            continue;
        };
        let Some(path) = object.get("path").and_then(Value::as_str) else {
            continue;
        };
        let path = path.trim();
        if path.is_empty()
            || path.len() > 4096
            || path.contains('\0')
            || !Path::new(path).is_absolute()
            || !seen.insert(path.to_owned())
        {
            continue;
        }
        let display = match object.get("display").and_then(Value::as_str) {
            Some("folder") => FolderDisplay::Folder,
            _ => FolderDisplay::Stack,
        };
        let layout = match object.get("layout").and_then(Value::as_str) {
            Some("list") => FolderLayout::List,
            Some("grid") => FolderLayout::Grid,
            Some("fan") => FolderLayout::Fan,
            _ => FolderLayout::Auto,
        };
        let sort = match object.get("sort").and_then(Value::as_str) {
            Some("modified") => FolderSort::Modified,
            Some("kind") => FolderSort::Kind,
            Some("size") => FolderSort::Size,
            _ => FolderSort::Name,
        };
        folders.push(DockFolderPreference {
            path: path.to_owned(),
            display,
            layout,
            sort,
            descending: object
                .get("descending")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    folders
}

impl Preferences {
    pub fn load(environment: &Environment, preview: bool) -> Result<Self> {
        if preview {
            return Ok(Self::default());
        }
        Ok(Self::from_json(&Value::Object(read_object(
            &preferences_path(environment)?,
        )?)))
    }

    pub fn from_json(json: &Value) -> Self {
        let mut p = Self::default();
        let old = json.get("macos").and_then(Value::as_object);
        let get =
            |key: &str, legacy: &str| json.get(key).or_else(|| old.and_then(|m| m.get(legacy)));
        let number = |key, legacy, fallback: f64, min: f64, max: f64| {
            get(key, legacy)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && (min..=max).contains(v))
                .unwrap_or(fallback)
        };
        if let Some(theme @ ("system" | "light" | "dark")) =
            json.get("theme").and_then(Value::as_str)
        {
            p.theme = theme.into();
        }
        if let Some(accent) = json
            .get("accent")
            .and_then(Value::as_str)
            .filter(|s| valid_accent(s))
        {
            p.accent = accent.to_ascii_lowercase();
        }
        p.reduce_motion = json
            .get("reduce_motion")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        p.pinned_app_ids = ids(json.get("pinned_app_ids"), 256);
        p.dock_order = ids(json.get("dock_order"), 256);
        p.dock_folders = dock_folder_preferences(json.get("dock_folders"));
        p.recent_app_ids = ids(json.get("recent_app_ids"), 24).unwrap_or_default();
        p.bar_height = number("bar_height", "menu_bar_height", p.bar_height, 22., 48.).round();
        p.dock_size = number("dock_size", "dock_size", p.dock_size, 32., 64.).round();
        p.dock_opacity = number(
            "dock_opacity",
            "dock_opacity",
            p.dock_opacity,
            MIN_SURFACE_OPACITY,
            MAX_SURFACE_OPACITY,
        );
        p.surface_opacity = number(
            "surface_opacity",
            "surface_opacity",
            p.surface_opacity,
            MIN_SURFACE_OPACITY,
            MAX_SURFACE_OPACITY,
        );
        p.frost_strength = number("frost_strength", "frost_strength", p.frost_strength, 0., 1.);
        p.magnification_scale = number(
            "magnification_scale",
            "magnification_scale",
            p.magnification_scale,
            1.,
            2.,
        );
        macro_rules! boolean {
            ($field:ident, $legacy:literal) => {
                p.$field = get(stringify!($field), $legacy)
                    .and_then(Value::as_bool)
                    .unwrap_or(p.$field);
            };
        }
        boolean!(bar_auto_hide, "menu_bar_auto_hide");
        boolean!(show_battery_percentage, "show_battery_percentage");
        boolean!(clock_show_date, "clock_show_date");
        boolean!(clock_show_seconds, "clock_show_seconds");
        boolean!(dock_auto_hide, "dock_auto_hide");
        boolean!(dock_hidden, "dock_hidden");
        boolean!(magnification, "magnification");
        boolean!(show_running_indicators, "show_running_indicators");
        boolean!(animate_opening_applications, "animate_opening_applications");
        boolean!(show_recent_applications, "show_recent_applications");
        boolean!(show_launcher, "show_launchpad");
        boolean!(show_downloads, "show_downloads");
        boolean!(minimize_into_app, "minimize_into_app");
        boolean!(use_original_icons, "use_original_icons");
        p.dock_position = get("dock_position", "dock_position")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        p.minimize_effect = get("minimize_effect", "minimize_effect")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        p
    }

    /// Merge into the latest on-disk object so unrelated/legacy fields survive.
    /// Malformed or oversized existing preferences are never overwritten.
    pub fn save(&self, environment: &Environment, preview: bool) -> Result<()> {
        if preview {
            return Ok(());
        }
        anyhow::ensure!(
            matches!(self.theme.as_str(), "system" | "light" | "dark")
                && valid_accent(&self.accent),
            "Invalid desktop appearance"
        );
        anyhow::ensure!(
            self.bar_height.is_finite()
                && (22. ..=48.).contains(&self.bar_height)
                && self.dock_size.is_finite()
                && (32. ..=64.).contains(&self.dock_size)
                && self.dock_opacity.is_finite()
                && (MIN_SURFACE_OPACITY..=MAX_SURFACE_OPACITY).contains(&self.dock_opacity)
                && self.surface_opacity.is_finite()
                && (MIN_SURFACE_OPACITY..=MAX_SURFACE_OPACITY).contains(&self.surface_opacity)
                && self.frost_strength.is_finite()
                && (0. ..=1.).contains(&self.frost_strength)
                && self.magnification_scale.is_finite()
                && (1. ..=2.).contains(&self.magnification_scale),
            "Invalid desktop dimensions"
        );
        let path = preferences_path(environment)?;
        let mut object = read_object(&path)?;
        object.insert("version".into(), 4.into());
        macro_rules! field { ($($field:ident),+ $(,)?) => { $(object.insert(stringify!($field).into(), serde_json::to_value(&self.$field)?);)+ }; }
        field!(
            theme,
            accent,
            reduce_motion,
            pinned_app_ids,
            dock_order,
            dock_folders,
            recent_app_ids,
            bar_height,
            bar_auto_hide,
            show_battery_percentage,
            clock_show_date,
            clock_show_seconds,
            dock_size,
            dock_opacity,
            surface_opacity,
            frost_strength,
            dock_position,
            dock_auto_hide,
            dock_hidden,
            magnification,
            magnification_scale,
            show_running_indicators,
            animate_opening_applications,
            show_recent_applications,
            show_launcher,
            show_downloads,
            minimize_into_app,
            use_original_icons,
            minimize_effect
        );
        let mut bytes = serde_json::to_vec_pretty(&object)?;
        bytes.push(b'\n');
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_BYTES,
            "Desktop preferences are too large"
        );
        let directory = path.parent().context("Preferences directory")?;
        fs::create_dir_all(directory)?;
        let mut temp = tempfile::NamedTempFile::new_in(directory)?;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(&path)
            .context("Save desktop preferences atomically")?;
        File::open(directory)?.sync_all()?;
        Ok(())
    }
}

fn valid_accent(text: &str) -> bool {
    text.len() == 7
        && text.starts_with('#')
        && text.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn environment(root: &Path) -> Environment {
        [(
            "XDG_CONFIG_HOME".into(),
            root.to_string_lossy().into_owned(),
        )]
        .into()
    }
    #[test]
    fn legacy_fields_migrate_but_cannot_select_retired_implementations() {
        let p = Preferences::from_json(
            &serde_json::json!({"shell_style":"material", "use_external_launcher":false,
            "bar_position":"bottom", "pinned_app_ids":[], "macos":{"dock_size":60,"dock_position":"left","menu_bar_height":32,"show_launchpad":false}}),
        );
        assert_eq!(p.dock_size, 60.);
        assert_eq!(p.dock_position, DockPosition::Left);
        assert_eq!(p.bar_height, 32.);
        assert!(!p.show_launcher);
        assert_eq!(p.pinned_app_ids, Some(vec![]));
        assert_eq!(
            Preferences::from_json(&serde_json::json!({})).pinned_app_ids,
            None
        );
    }
    #[test]
    fn atomic_save_preserves_unknown_nested_values_and_reloads_new_fields() {
        let dir = tempfile::tempdir().unwrap();
        let env = environment(dir.path());
        let path = preferences_path(&env).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            br##"{"unknown":{"a":[1,2]},"macos":{"dock_size":64},"theme":"dark"}"##,
        )
        .unwrap();
        let mut p = Preferences::load(&env, false).unwrap();
        p.dock_size = 40.;
        p.save(&env, false).unwrap();
        let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["unknown"]["a"], serde_json::json!([1, 2]));
        assert_eq!(value["macos"]["dock_size"], 64);
        assert_eq!(Preferences::load(&env, false).unwrap().dock_size, 40.);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[test]
    fn dock_motion_defaults_migrate_and_save_without_losing_unrelated_preferences() {
        let defaults = Preferences::default();
        assert!(defaults.magnification);
        assert_eq!(defaults.magnification_scale, 1.45);
        assert!(defaults.animate_opening_applications);
        let dir = tempfile::tempdir().unwrap();
        let env = environment(dir.path());
        let path = preferences_path(&env).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, br##"{"unknown":{"keep":["value"]},"macos":{"magnification":false,"magnification_scale":1.8,"animate_opening_applications":false}}"##).unwrap();
        let mut prefs = Preferences::load(&env, false).unwrap();
        assert!(!prefs.magnification);
        assert_eq!(prefs.magnification_scale, 1.8);
        assert!(!prefs.animate_opening_applications);
        prefs.magnification = true;
        prefs.magnification_scale = 1.75;
        prefs.animate_opening_applications = true;
        prefs.save(&env, false).unwrap();
        let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["unknown"]["keep"], serde_json::json!(["value"]));
        assert_eq!(saved["macos"]["magnification_scale"], 1.8);
        assert_eq!(saved["magnification_scale"], 1.75);
        assert_eq!(saved["magnification"], true);
        assert_eq!(saved["animate_opening_applications"], true);
        assert_eq!(Preferences::load(&env, false).unwrap(), prefs);
        for invalid in [0.5, 2.1] {
            let parsed =
                Preferences::from_json(&serde_json::json!({"magnification_scale":invalid}));
            assert_eq!(parsed.magnification_scale, defaults.magnification_scale);
            prefs.magnification_scale = invalid;
            assert!(prefs.save(&env, false).is_err());
        }
    }
    #[test]
    fn translucent_surface_defaults_validate_and_roundtrip() {
        let defaults = Preferences::default();
        assert_eq!(defaults.dock_opacity, DEFAULT_DOCK_OPACITY);
        assert_eq!(defaults.surface_opacity, DEFAULT_SURFACE_OPACITY);
        assert_eq!(defaults.frost_strength, DEFAULT_FROST_STRENGTH);
        let appearance = defaults.surface_appearance();
        assert!(appearance.dark);
        assert!((appearance.dock_opacity - 0.62).abs() < 0.001);
        assert!((appearance.surface_opacity - 0.68).abs() < 0.001);
        assert!((appearance.frost - 0.82).abs() < 0.001);

        let parsed = Preferences::from_json(&serde_json::json!({
            "dock_opacity": 0.45,
            "surface_opacity": 0.71,
            "frost_strength": 0.35,
            "theme": "light"
        }));
        assert_eq!(parsed.dock_opacity, 0.45);
        assert_eq!(parsed.surface_opacity, 0.71);
        assert_eq!(parsed.frost_strength, 0.35);
        assert!(!parsed.surface_appearance().dark);

        for invalid in [
            serde_json::json!({"dock_opacity": 0.1}),
            serde_json::json!({"surface_opacity": 1.1}),
            serde_json::json!({"frost_strength": -0.1}),
            serde_json::json!({"frost_strength": 1.1}),
        ] {
            let parsed = Preferences::from_json(&invalid);
            assert_eq!(parsed.dock_opacity, defaults.dock_opacity);
            assert_eq!(parsed.surface_opacity, defaults.surface_opacity);
            assert_eq!(parsed.frost_strength, defaults.frost_strength);
        }

        let dir = tempfile::tempdir().unwrap();
        let env = environment(dir.path());
        let mut prefs = defaults.clone();
        prefs.dock_opacity = 0.51;
        prefs.surface_opacity = 0.73;
        prefs.frost_strength = 0.;
        prefs.save(&env, false).unwrap();
        assert_eq!(Preferences::load(&env, false).unwrap(), prefs);
        prefs.dock_opacity = 0.2;
        assert!(prefs.save(&env, false).is_err());
        prefs.dock_opacity = 0.51;
        prefs.frost_strength = 1.5;
        assert!(prefs.save(&env, false).is_err());
    }

    #[test]
    fn preview_does_not_read_invalid_preferences_or_write_session_data() {
        let dir = tempfile::tempdir().unwrap();
        let env = environment(dir.path());
        let path = preferences_path(&env).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"invalid").unwrap();
        assert_eq!(
            Preferences::load(&env, true).unwrap(),
            Preferences::default()
        );
        Preferences::default().save(&env, true).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"invalid");
        assert!(Preferences::default().save(&env, false).is_err());
    }
    #[test]
    fn canonical_directory_is_authoritative_even_without_its_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let env = environment(dir.path());
        fs::create_dir(dir.path().join("realand-shell")).unwrap();
        assert!(preferences_path(&env)
            .unwrap()
            .starts_with(dir.path().join("realand-shell")));
        fs::create_dir(dir.path().join("clarion-shell")).unwrap();
        assert!(preferences_path(&env)
            .unwrap()
            .starts_with(dir.path().join("clarion-shell")));
    }
}

#[cfg(test)]
mod dock_folder_preference_tests {
    use super::*;

    #[test]
    fn dock_folders_roundtrip_with_defaults_and_unknown_top_level_fields() {
        let json = serde_json::json!({
            "dock_folders": [
                {"path": "/home/user/Projects", "display": "folder", "layout": "grid",
                 "sort": "modified", "descending": true},
                {"path": "/home/user/Pictures"}
            ],
            "unknown": {"keep": true}
        });
        let p = Preferences::from_json(&json);
        assert_eq!(p.dock_folders.len(), 2);
        assert_eq!(p.dock_folders[0].display, FolderDisplay::Folder);
        assert_eq!(p.dock_folders[0].layout, FolderLayout::Grid);
        assert_eq!(p.dock_folders[0].sort, FolderSort::Modified);
        assert!(p.dock_folders[0].descending);
        assert_eq!(p.dock_folders[1].display, FolderDisplay::Stack);
        assert_eq!(p.dock_folders[1].layout, FolderLayout::Auto);
        assert_eq!(p.dock_folders[1].sort, FolderSort::Name);
        assert!(!p.dock_folders[1].descending);
        assert_eq!(Preferences::from_json(&json).dock_folders, p.dock_folders);
    }

    #[test]
    fn dock_folders_reject_invalid_paths_dedupe_and_cap() {
        let mut folders = (0..12)
            .map(|index| serde_json::json!({"path": format!("/tmp/folder-{index}")}))
            .collect::<Vec<_>>();
        folders.extend([
            serde_json::json!({"path": "/tmp/folder-0"}),
            serde_json::json!({"path": "relative/path"}),
            serde_json::json!({"path": "/tmp/has\u{0}nul"}),
            serde_json::json!({"path": ""}),
        ]);
        let p = Preferences::from_json(&serde_json::json!({"dock_folders": folders}));
        assert_eq!(p.dock_folders.len(), MAX_DOCK_FOLDERS);
        assert_eq!(p.dock_folders[0].path, "/tmp/folder-0");
        assert_eq!(p.dock_folders[7].path, "/tmp/folder-7");
        assert_eq!(
            Preferences::from_json(&serde_json::json!({})).dock_folders,
            Vec::<DockFolderPreference>::new()
        );
        assert_eq!(
            Preferences::from_json(&serde_json::json!({"dock_folders": [
                {"path": "/tmp/ok", "layout": "nonsense", "sort": "nonsense",
                 "display": "nonsense"}
            ]}))
            .dock_folders[0]
                .layout,
            FolderLayout::Auto
        );
        assert_eq!(
            Preferences::from_json(&serde_json::json!({"dock_folders": [
                {"path": "/tmp/ok", "layout": "nonsense", "sort": "nonsense",
                 "display": "nonsense"}
            ]}))
            .dock_folders[0]
                .sort,
            FolderSort::Name
        );
        assert_eq!(
            Preferences::from_json(&serde_json::json!({"dock_folders": [
                {"path": "/tmp/ok", "layout": "nonsense", "sort": "nonsense",
                 "display": "nonsense"}
            ]}))
            .dock_folders[0]
                .display,
            FolderDisplay::Stack
        );
    }

    #[test]
    fn dock_folders_save_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let env: Environment = [(
            "XDG_CONFIG_HOME".into(),
            dir.path().to_string_lossy().into_owned(),
        )]
        .into();
        let mut prefs = Preferences::default();
        prefs.dock_folders = vec![DockFolderPreference {
            path: "/tmp/Projects".into(),
            display: FolderDisplay::Folder,
            layout: FolderLayout::Fan,
            sort: FolderSort::Size,
            descending: true,
        }];
        prefs.save(&env, false).unwrap();
        assert_eq!(
            Preferences::load(&env, false).unwrap().dock_folders,
            prefs.dock_folders
        );
    }
}

#[cfg(test)]
mod dock_order_tests {
    use super::*;
    #[test]
    fn dock_order_roundtrips_and_defaults_to_none() {
        let parsed = Preferences::from_json(
            &serde_json::json!({"dock_order":["beta.desktop","alpha.desktop"]}),
        );
        assert_eq!(
            parsed.dock_order,
            Some(vec!["beta.desktop".to_owned(), "alpha.desktop".to_owned()])
        );
        assert_eq!(
            Preferences::from_json(&serde_json::json!({})).dock_order,
            None,
            "an absent field keeps the unpersisted arrangement"
        );
        let dir = tempfile::tempdir().unwrap();
        let env: Environment = [(
            "XDG_CONFIG_HOME".into(),
            dir.path().to_string_lossy().into_owned(),
        )]
        .into();
        let mut prefs = Preferences::default();
        prefs.dock_order = Some(vec!["beta.desktop".to_owned()]);
        prefs.save(&env, false).unwrap();
        assert_eq!(
            Preferences::load(&env, false).unwrap().dock_order,
            Some(vec!["beta.desktop".to_owned()])
        );
    }
}
