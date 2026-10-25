// SPDX-License-Identifier: GPL-3.0-or-later
//! Launch history used by the palette's usage-aware application order.
//!
//! The file is shell state, not configuration: it lives under
//! `$XDG_STATE_HOME/clarion-shell/` (or `~/.local/state/clarion-shell/`). A
//! missing, oversized or malformed file is treated as an empty history so a
//! damaged store can never keep the launcher from opening.
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::preferences::Environment;
use crate::search::AppOrder;

/// Wall-clock milliseconds since the Unix epoch. A pre-1970 clock counts as 0
/// instead of panicking, which only makes recency ties fall back to names.
pub fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

pub const SCHEMA_VERSION: u32 = 1;
/// The store keeps at most this many applications. Older entries beyond the
/// bound are dropped on the next save instead of growing the file forever.
pub const ENTRY_LIMIT: usize = 512;
const MAX_BYTES: u64 = 256 * 1024;
const MAX_ID_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchRecord {
    #[serde(rename = "launchCount", default)]
    pub launch_count: u64,
    #[serde(rename = "lastLaunchedAt", default)]
    pub last_launched_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchHistory {
    #[serde(default)]
    applications: HashMap<String, LaunchRecord>,
}

/// Usage contribution for one application. Never negative, never NaN: the
/// rank sort compares it only after textual scores tie.
pub fn usage_score(order: AppOrder, record: Option<&LaunchRecord>, now_ms: u64) -> f64 {
    let record = record.copied().unwrap_or_default();
    match order {
        AppOrder::MostUsed => record.launch_count as f64,
        AppOrder::RecentlyUsed => record.last_launched_at as f64,
        AppOrder::Name => 0.,
        AppOrder::Smart => {
            if record.launch_count == 0 {
                return 0.;
            }
            let hours = now_ms.saturating_sub(record.last_launched_at) as f64 / 3_600_000.;
            let bonus = if record.last_launched_at == 0 {
                0.
            } else if hours < 1. {
                8.
            } else if hours < 24. {
                6.
            } else if hours < 168. {
                4.
            } else if hours < 720. {
                2.
            } else {
                0.
            };
            ((record.launch_count as f64 + 1.).log2()) + bonus
        }
    }
}

fn state_root(environment: &Environment) -> Result<PathBuf> {
    if let Some(root) = environment
        .get("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Ok(root);
    }
    let home = environment
        .get("HOME")
        .filter(|home| home.starts_with('/'))
        .context("An absolute HOME directory is required for launch history")?;
    Ok(PathBuf::from(home).join(".local/state"))
}

pub fn history_path(environment: &Environment) -> Result<PathBuf> {
    Ok(state_root(environment)?.join("clarion-shell/launch-history.json"))
}

fn valid_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= MAX_ID_BYTES && !id.contains('\0')
}

impl LaunchHistory {
    pub fn get(&self, id: &str) -> Option<LaunchRecord> {
        self.applications.get(id).copied()
    }

    /// Read-only view for ranking.
    pub fn records(&self) -> &HashMap<String, LaunchRecord> {
        &self.applications
    }

    pub fn len(&self) -> usize {
        self.applications.len()
    }

    pub fn is_empty(&self) -> bool {
        self.applications.is_empty()
    }

    /// Count one successful launch. Launch counts saturate instead of wrapping.
    pub fn record_launch(&mut self, id: &str, now_ms: u64) {
        if !valid_id(id) {
            return;
        }
        let record = self.applications.entry(id.to_owned()).or_default();
        record.launch_count = record.launch_count.saturating_add(1);
        record.last_launched_at = record.last_launched_at.max(now_ms);
        if self.applications.len() > ENTRY_LIMIT {
            self.trim();
        }
    }

    /// Keep the most recently used `ENTRY_LIMIT` applications.
    fn trim(&mut self) {
        let mut entries: Vec<(String, LaunchRecord)> = self
            .applications
            .iter()
            .map(|(id, record)| (id.clone(), *record))
            .collect();
        entries.sort_by(|(left_id, left), (right_id, right)| {
            right
                .last_launched_at
                .cmp(&left.last_launched_at)
                .then_with(|| left_id.cmp(right_id))
        });
        entries.truncate(ENTRY_LIMIT);
        self.applications = entries.into_iter().collect();
    }

    pub fn from_json(text: &str) -> Self {
        #[derive(Deserialize)]
        struct File {
            #[serde(default)]
            #[serde(rename = "schemaVersion")]
            schema_version: u32,
            #[serde(default)]
            applications: HashMap<String, LaunchRecord>,
        }
        let Ok(file) = serde_json::from_str::<File>(text) else {
            return Self::default();
        };
        if file.schema_version != SCHEMA_VERSION {
            return Self::default();
        }
        let mut history = Self::default();
        for (id, record) in file.applications {
            if !valid_id(&id) || history.applications.len() >= ENTRY_LIMIT {
                continue;
            }
            if record.launch_count == 0 {
                continue;
            }
            history.applications.insert(id, record);
        }
        history
    }

    pub fn to_json(&self) -> String {
        #[derive(Serialize)]
        struct File<'a> {
            #[serde(rename = "schemaVersion")]
            schema_version: u32,
            applications: std::collections::BTreeMap<&'a str, LaunchRecord>,
        }
        let applications: std::collections::BTreeMap<&str, LaunchRecord> = self
            .applications
            .iter()
            .map(|(id, record)| (id.as_str(), *record))
            .collect();
        serde_json::to_string(&File {
            schema_version: SCHEMA_VERSION,
            applications,
        })
        .unwrap_or_else(|_| "{}".into())
    }

    pub fn load(environment: &Environment) -> Self {
        let Ok(path) = history_path(environment) else {
            return Self::default();
        };
        Self::read(&path).unwrap_or_default()
    }

    fn read(path: &Path) -> Result<Self> {
        let file = match fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(error).context("Open launch history"),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_BYTES {
            return Err(anyhow::anyhow!("Launch history must be a small file"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(anyhow::anyhow!("Launch history is too large"));
        }
        let text = std::str::from_utf8(&bytes).context("Launch history is not UTF-8")?;
        Ok(Self::from_json(text))
    }

    pub fn save(&self, environment: &Environment) -> Result<()> {
        let path = history_path(environment)?;
        let directory = path.parent().context("Launch history directory")?;
        fs::create_dir_all(directory)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        let mut bytes = self.to_json().into_bytes();
        bytes.push(b'\n');
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_BYTES,
            "Launch history is too large"
        );
        let mut temp = tempfile::NamedTempFile::new_in(directory)?;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(&path)
            .map_err(|error| error.error)
            .context("Replace launch history")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(root: &Path) -> Environment {
        [(
            "XDG_STATE_HOME".to_owned(),
            root.to_string_lossy().into_owned(),
        )]
        .into()
    }

    #[test]
    fn recording_launches_counts_and_tracks_the_newest_time() {
        let mut history = LaunchHistory::default();
        history.record_launch("firefox.desktop", 100);
        history.record_launch("firefox.desktop", 90);
        let record = history.get("firefox.desktop").unwrap();
        assert_eq!(record.launch_count, 2);
        assert_eq!(record.last_launched_at, 100);
        // Invalid ids never enter the store.
        history.record_launch("  ", 100);
        history.record_launch("bad\0id", 100);
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn the_store_round_trips_through_json() {
        let mut history = LaunchHistory::default();
        history.record_launch("a.desktop", 7);
        history.record_launch("b.desktop", 11);
        let parsed = LaunchHistory::from_json(&history.to_json());
        assert_eq!(parsed, history);
    }

    #[test]
    fn malformed_or_foreign_stores_read_as_empty() {
        assert_eq!(LaunchHistory::from_json("not json"), LaunchHistory::default());
        assert_eq!(
            LaunchHistory::from_json(r#"{"schemaVersion":2,"applications":{"a":{"launchCount":1}}}"#),
            LaunchHistory::default()
        );
        assert_eq!(
            LaunchHistory::from_json(r#"{"schemaVersion":1,"applications":{"": {"launchCount":3}}}"#),
            LaunchHistory::default()
        );
        assert_eq!(
            LaunchHistory::from_json(r#"{"schemaVersion":1,"applications":{"a": {"launchCount":0}}}"#),
            LaunchHistory::default()
        );
    }

    #[test]
    fn saving_and_loading_uses_the_state_namespace() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path());
        let mut history = LaunchHistory::default();
        history.record_launch("firefox.desktop", 1234);
        history.save(&env).unwrap();
        let path = history_path(&env).unwrap();
        assert_eq!(path, root.path().join("clarion-shell/launch-history.json"));
        assert_eq!(LaunchHistory::load(&env), history);
    }

    #[test]
    fn a_missing_file_is_an_empty_history() {
        let root = tempfile::tempdir().unwrap();
        assert!(LaunchHistory::load(&environment(root.path())).is_empty());
    }

    #[test]
    fn smart_order_rewards_frequency_and_recency_without_overflow() {
        let now = 1_000_000_000;
        let fresh = LaunchRecord {
            launch_count: 4,
            last_launched_at: now - 60_000,
        };
        let stale = LaunchRecord {
            launch_count: 4,
            last_launched_at: now.saturating_sub(40 * 24 * 3_600_000),
        };
        assert!(usage_score(AppOrder::Smart, Some(&fresh), now) > usage_score(AppOrder::Smart, Some(&stale), now));
        assert_eq!(usage_score(AppOrder::Name, Some(&fresh), now), 0.);
        assert_eq!(usage_score(AppOrder::MostUsed, Some(&fresh), now), 4.);
        assert_eq!(usage_score(AppOrder::RecentlyUsed, Some(&fresh), now), fresh.last_launched_at as f64);
        assert!(usage_score(AppOrder::Smart, None, now).is_finite());
    }

    #[test]
    fn trimming_keeps_the_most_recent_entries() {
        let mut history = LaunchHistory::default();
        for index in 0..ENTRY_LIMIT + 4 {
            history.record_launch(&format!("app{index}.desktop"), index as u64 + 1);
        }
        assert_eq!(history.len(), ENTRY_LIMIT);
        assert!(history.get("app0.desktop").is_none());
        assert!(history.get(&format!("app{}.desktop", ENTRY_LIMIT + 3)).is_some());
    }
}
