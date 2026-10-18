// SPDX-License-Identifier: GPL-3.0-or-later
//! Read shared Shell preferences without rewriting them; persist only Apps view state.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub type Environment = HashMap<String, String>;
const MAX_JSON_BYTES: u64 = 65_536;

/// Match Flutter's authoritative canonical/legacy namespace selection.
pub fn config_directory(environment: &Environment) -> Result<PathBuf> {
    let root = match environment.get("XDG_CONFIG_HOME") {
        Some(root) if root.starts_with('/') => PathBuf::from(root),
        _ => {
            let home = environment
                .get("HOME")
                .filter(|home| home.starts_with('/'))
                .context("An absolute HOME directory is required for user files")?;
            PathBuf::from(home).join(".config")
        }
    };
    let canonical = root.join("clarion-shell");
    let legacy = root.join("realand-shell");
    if fs::symlink_metadata(&canonical)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        && fs::symlink_metadata(&legacy).is_ok()
    {
        return Ok(legacy);
    }
    Ok(canonical)
}

fn read_json(environment: &Environment, filename: &str) -> Option<serde_json::Value> {
    let path = config_directory(environment).ok()?.join(filename);
    // A replaced FIFO or device must not stall the UI thread. Inspect the
    // opened descriptor, then bound the read even if a regular file grows.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_JSON_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_JSON_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellSnapshot {
    pub theme: String,
    pub accent: String,
    pub reduce_motion: bool,
    pub recent_app_ids: Vec<String>,
}

impl Default for ShellSnapshot {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            accent: "#6750a4".into(),
            reduce_motion: false,
            recent_app_ids: Vec::new(),
        }
    }
}

impl ShellSnapshot {
    pub fn load(environment: &Environment) -> Self {
        let mut snapshot = Self::default();
        let Some(json) = read_json(environment, "preferences.json").filter(|json| json.is_object())
        else {
            return snapshot;
        };
        if let Some(theme @ ("light" | "dark")) = json["theme"].as_str() {
            snapshot.theme = theme.into();
        }
        if let Some(accent) = json["accent"].as_str() {
            if accent.len() == 7
                && accent.starts_with('#')
                && accent.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
            {
                snapshot.accent = accent.to_ascii_lowercase();
            }
        }
        snapshot.reduce_motion = json["reduce_motion"].as_bool() == Some(true);
        if let Some(recent) = json["recent_app_ids"].as_array() {
            let mut seen = HashSet::new();
            snapshot.recent_app_ids = recent
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty() && seen.insert((*id).to_owned()))
                .take(24)
                .map(str::to_owned)
                .collect();
        }
        snapshot
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    #[default]
    Category,
    Name,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    #[default]
    Icons,
    List,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AppsViewState {
    pub mode: ViewMode,
    pub display: DisplayMode,
    pub width: u32,
    pub height: u32,
}

impl Default for AppsViewState {
    fn default() -> Self {
        Self {
            mode: ViewMode::Category,
            display: DisplayMode::Icons,
            width: 980,
            height: 680,
        }
    }
}

impl AppsViewState {
    fn normalized(mut self) -> Self {
        if !(480..=3840).contains(&self.width) {
            self.width = 980;
        }
        if !(320..=2160).contains(&self.height) {
            self.height = 680;
        }
        self
    }

    pub fn load(environment: &Environment) -> Self {
        read_json(environment, "apps-view.json")
            .and_then(|json| serde_json::from_value::<Self>(json).ok())
            .unwrap_or_default()
            .normalized()
    }

    pub fn save(&self, environment: &Environment) -> Result<()> {
        let directory = config_directory(environment)?;
        fs::create_dir_all(&directory).context("Create Apps view-state directory")?;
        let mut staging = tempfile::NamedTempFile::new_in(&directory)
            .context("Create atomic Apps view-state staging file")?;
        serde_json::to_writer(&mut staging, &self.clone().normalized())?;
        staging.write_all(b"\n")?;
        staging.as_file().sync_all()?;
        staging
            .persist(directory.join("apps-view.json"))
            .context("Replace Apps view state atomically")?;
        File::open(directory)?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(root: &std::path::Path) -> Environment {
        [(
            "XDG_CONFIG_HOME".into(),
            root.to_string_lossy().into_owned(),
        )]
        .into()
    }

    #[test]
    fn canonical_namespace_is_authoritative_even_for_a_dangling_symlink() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path());
        let legacy = root.path().join("realand-shell");
        fs::create_dir(&legacy).unwrap();
        assert_eq!(config_directory(&env).unwrap(), legacy);
        let canonical = root.path().join("clarion-shell");
        std::os::unix::fs::symlink(root.path().join("missing"), &canonical).unwrap();
        assert_eq!(config_directory(&env).unwrap(), canonical);
    }

    #[test]
    fn relative_xdg_falls_back_only_to_explicit_absolute_home() {
        let env = [("XDG_CONFIG_HOME".into(), "relative".into())].into();
        assert!(config_directory(&env).is_err());
        let env = [
            ("HOME".into(), "/tmp/fixture-home".into()),
            ("XDG_CONFIG_HOME".into(), "relative".into()),
        ]
        .into();
        assert_eq!(
            config_directory(&env).unwrap(),
            PathBuf::from("/tmp/fixture-home/.config/clarion-shell")
        );
    }

    #[test]
    fn snapshot_matches_shell_validation_and_bounds_recent_apps() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path());
        let directory = config_directory(&env).unwrap();
        fs::create_dir(&directory).unwrap();
        let mut ids = vec![
            serde_json::json!(" one.desktop "),
            serde_json::json!("one.desktop"),
            serde_json::json!(null),
            serde_json::json!("  "),
        ];
        ids.extend((0..30).map(|i| serde_json::json!(format!("app{i}.desktop"))));
        fs::write(
            directory.join("preferences.json"),
            serde_json::to_vec(&serde_json::json!({
                "theme":"dark", "accent":"#Aa22Ff", "reduce_motion":true, "recent_app_ids":ids,
            }))
            .unwrap(),
        )
        .unwrap();
        let snapshot = ShellSnapshot::load(&env);
        assert_eq!(snapshot.theme, "dark");
        assert_eq!(snapshot.accent, "#aa22ff");
        assert!(snapshot.reduce_motion);
        assert_eq!(snapshot.recent_app_ids.len(), 24);
        assert_eq!(snapshot.recent_app_ids[0], "one.desktop");
    }

    #[test]
    fn malformed_large_or_non_regular_files_fall_back_without_blocking() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path());
        let directory = config_directory(&env).unwrap();
        fs::create_dir(&directory).unwrap();
        let path = directory.join("preferences.json");
        for bytes in [
            b"not json".as_slice(),
            &vec![b' '; MAX_JSON_BYTES as usize + 1],
        ] {
            fs::write(&path, bytes).unwrap();
            assert_eq!(ShellSnapshot::load(&env), ShellSnapshot::default());
        }
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(ShellSnapshot::load(&env), ShellSnapshot::default());
    }

    #[test]
    fn view_state_roundtrip_is_independent_and_replaces_atomically() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path());
        let directory = config_directory(&env).unwrap();
        fs::create_dir(&directory).unwrap();
        let shared = directory.join("preferences.json");
        fs::write(&shared, b"{\"theme\":\"dark\"}\n").unwrap();
        let state = AppsViewState {
            mode: ViewMode::Name,
            display: DisplayMode::List,
            width: 1200,
            height: 800,
        };
        state.save(&env).unwrap();
        assert_eq!(AppsViewState::load(&env), state);
        assert_eq!(fs::read(&shared).unwrap(), b"{\"theme\":\"dark\"}\n");
        assert!(!directory.join("launchpad-layout.json").exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("apps-view.json")).unwrap()).unwrap();
        assert_eq!(json["mode"], "name");
        assert_eq!(json["display"], "list");
    }

    #[test]
    fn invalid_view_dimensions_and_enum_values_have_bounded_defaults() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path());
        let directory = config_directory(&env).unwrap();
        fs::create_dir(&directory).unwrap();
        let path = directory.join("apps-view.json");
        fs::write(&path, br#"{"mode":"name","width":0,"height":4294967295}"#).unwrap();
        let state = AppsViewState::load(&env);
        assert_eq!((state.width, state.height), (980, 680));
        assert_eq!(state.mode, ViewMode::Name);
        fs::write(path, br#"{"mode":"folders"}"#).unwrap();
        assert_eq!(AppsViewState::load(&env), AppsViewState::default());
    }
}
