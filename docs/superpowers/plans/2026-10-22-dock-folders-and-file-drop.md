# Dock Folders and File Drop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add pinned Dock folders with list/grid/fan layouts and let files dropped on Dock app icons open in that app when its `MimeType` list supports every path.

**Architecture:** Persist folder paths and per-folder display preferences, scan directories on a bounded background worker into immutable snapshots, add a folder slot group between overflow and trash, render a full-output transparent `Panel::DockFolder` with list/grid/fan layouts, and reuse GPUI's internal drag/drop plus `ExternalPaths` for opening files with a target app through `gio launch`.

**Tech Stack:** Rust 2021, GPUI (vendored), Wayland layer-shell, `serde_json`, `mime_guess`, GIO `gio launch`, existing Clarion IPC/backend.

**Spec:** `docs/superpowers/specs/2026-10-22-dock-folders-and-file-drop-design.md`

## Global Constraints

- `MAX_DOCK_FOLDERS = 8`.
- `MAX_FOLDER_ENTRIES = 256`, `MAX_VISITED = 4096`.
- A single drop accepts at most 16 paths.
- All dropped paths must be supported by the target app; never open a partial set.
- No file move/copy/delete/rename from Dock folder features.
- No full-screen dim, full-screen blur, or painted hole around the Dock.
- List/grid popups use the existing popup outline + arrow; fan has no rectangular body or arrow.
- All names and titles are single-line with ellipsis; no wrapping that changes row height.
- Preserve unknown top-level preference fields and atomic 0600 preference writes.
- Keep existing Dock preferences, overflow, recent apps, reorder, minimize targets, and tests passing.
- Do not implement the dormant `show_downloads` stack in this plan.

## Review Focus

- Dropping a folder on an app that does not support directories must not pin a folder; the app tile consumes the drop.
- Dropping a folder on trash must not pin a folder.
- A very long file/folder/window name must not increase a row's height or push controls outside the popup.
- Fan items, labels, and folder-entry drop zones must stay inside the output and must not block outside-click dismissal.
- Mixed file drops where only some paths are supported must produce no launch command.
- Missing or unreadable pinned folders must render a fallback tile and an in-popup error, never panic or block the Dock.
- Pinned folder count at the cap must be ignored, not silently evict another folder.

---

### Task 1: Preference model for Dock folders

**Files:**
- Modify: `clarion-shell/src/preferences.rs`
- Test: `clarion-shell/src/preferences.rs` (`mod tests` and `mod dock_order_tests`)

**Interfaces:**
- Produces:
  - `pub enum FolderDisplay { Stack, Folder }`
  - `pub enum FolderLayout { Auto, List, Grid, Fan }`
  - `pub enum FolderSort { Name, Modified, Kind, Size }`
  - `pub struct DockFolderPreference { path: String, display: FolderDisplay, layout: FolderLayout, sort: FolderSort, descending: bool }`
  - `Preferences.dock_folders: Vec<DockFolderPreference>`
  - `pub(in crate) const MAX_DOCK_FOLDERS: usize = 8;`

- [ ] **Step 1: Write the failing round-trip and validation tests**

Add to `clarion-shell/src/preferences.rs` tests:

```rust
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
}

#[test]
fn dock_folders_reject_invalid_paths_dedupe_and_cap() {
    let folders = (0..12)
        .map(|i| serde_json::json!({"path": format!("/tmp/folder-{i}")}))
        .chain([
            serde_json::json!({"path": "/tmp/folder-0"}),
            serde_json::json!({"path": "relative/path"}),
            serde_json::json!({"path": "/tmp/has\u{0}nul"}),
            serde_json::json!({"path": ""}),
            serde_json::json!({"path": "/tmp/folder-99", "layout": "nonsense"}),
        ])
        .collect::<Vec<_>>();
    let p = Preferences::from_json(&serde_json::json!({"dock_folders": folders}));
    assert_eq!(p.dock_folders.len(), 8);
    assert_eq!(p.dock_folders[0].path, "/tmp/folder-0");
    assert_eq!(p.dock_folders[7].path, "/tmp/folder-7");
    assert_eq!(
        Preferences::from_json(&serde_json::json!({})).dock_folders,
        Vec::<DockFolderPreference>::new()
    );
    assert_eq!(
        Preferences::from_json(&serde_json::json!({"dock_folders": [
            {"path": "/tmp/ok", "layout": "nonsense"}
        ]}))
        .dock_folders[0]
            .layout,
        FolderLayout::Auto
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
cargo test -p clarion-shell dock_folders_ -- --nocapture
```

Expected: compile error because `dock_folders`, `DockFolderPreference`, `FolderDisplay`, `FolderLayout`, and `FolderSort` do not exist.

- [ ] **Step 3: Add the enums, struct, field, parser, and saver**

Add near the other preference enums:

```rust
pub(crate) const MAX_DOCK_FOLDERS: usize = 8;

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
```

Add `pub dock_folders: Vec<DockFolderPreference>` to `Preferences` and `Vec::new()` to `Default`.

Add parsing in `from_json`:

```rust
p.dock_folders = dock_folder_preferences(json.get("dock_folders"));
```

Add the parser before `impl Preferences`:

```rust
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
        let enum_or_default = |key: &str, default: fn(&str) -> Option<FolderLayout>| {
            object
                .get(key)
                .and_then(Value::as_str)
                .and_then(default)
                .unwrap_or_default()
        };
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
```

Delete the unused `enum_or_default` helper from the sketch before compiling.

In `save`, add `dock_folders` to the `field!` list and set version to `3`:

```rust
object.insert("version".into(), 3.into());
```

- [ ] **Step 4: Run the preference tests**

Run:

```bash
cargo test -p clarion-shell dock_folders_ -- --nocapture
cargo test -p clarion-shell preferences
```

Expected: the new tests pass and existing preference tests still pass.

- [ ] **Step 5: Commit**

```bash
git add clarion-shell/src/preferences.rs
git commit -m "feat: persist dock folder preferences"
```

---

### Task 2: Bounded folder scanning and sorting

**Files:**
- Create: `clarion-shell/src/dock_folder.rs`
- Modify: `clarion-shell/src/main.rs` if modules are declared there; otherwise `clarion-shell/src/app.rs`
- Test: inline `#[cfg(test)] mod tests` in `clarion-shell/src/dock_folder.rs`

**Interfaces:**
- Produces:
  - `pub const MAX_FOLDER_ENTRIES: usize = 256;`
  - `pub const MAX_VISITED: usize = 4096;`
  - `pub struct FolderSnapshot { pub entries: Vec<FolderEntry>, pub truncated: bool }`
  - `pub struct FolderEntry { pub path: PathBuf, pub name: String, pub kind: FolderEntryKind, pub modified: Option<SystemTime>, pub size: u64 }`
  - `pub enum FolderEntryKind { Directory, Application, Image, Document }`
  - `pub fn scan_folder(path: &Path, sort: FolderSort, descending: bool) -> anyhow::Result<FolderSnapshot>`
  - `pub fn sort_folder_entries(entries: &mut [FolderEntry], sort: FolderSort, descending: bool)`

- [ ] **Step 1: Write failing scan/sort tests**

Create `clarion-shell/src/dock_folder.rs` with tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn touch(path: &Path, bytes: usize, modified: SystemTime) {
        fs::write(path, vec![b'x'; bytes]).unwrap();
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(modified).unwrap();
    }

    #[test]
    fn scan_skips_hidden_entries_and_puts_directories_first() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("z-dir")).unwrap();
        fs::create_dir(root.path().join(".hidden-dir")).unwrap();
        touch(&root.path().join("b.txt"), 2, SystemTime::now());
        touch(&root.path().join("a.txt"), 1, SystemTime::now());
        fs::write(root.path().join(".hidden"), b"x").unwrap();
        let snapshot = scan_folder(root.path(), FolderSort::Name, false).unwrap();
        let names: Vec<_> = snapshot.entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["z-dir", "a.txt", "b.txt"]);
        assert!(snapshot.entries.iter().all(|entry| !entry.name.starts_with('.')));
    }

    #[test]
    fn natural_name_sort_is_case_insensitive_and_numeric() {
        let root = tempfile::tempdir().unwrap();
        for name in ["item10.txt", "Item2.txt", "item1.txt"] {
            fs::write(root.path().join(name), b"x").unwrap();
        }
        let snapshot = scan_folder(root.path(), FolderSort::Name, false).unwrap();
        let names: Vec<_> = snapshot.entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["item1.txt", "Item2.txt", "item10.txt"]);
        let snapshot = scan_folder(root.path(), FolderSort::Name, true).unwrap();
        let names: Vec<_> = snapshot.entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["item10.txt", "Item2.txt", "item1.txt"]);
    }

    #[test]
    fn modified_kind_and_size_sorts_are_stable_and_folders_first() {
        let root = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        fs::create_dir(root.path().join("dir")).unwrap();
        touch(&root.path().join("small.txt"), 1, now - Duration::from_secs(20));
        touch(&root.path().join("large.bin"), 30, now);
        fs::write(root.path().join("icon.png"), b"png").unwrap();
        fs::write(root.path().join("app.desktop"), b"[Desktop Entry]\n").unwrap();

        let snapshot = scan_folder(root.path(), FolderSort::Size, false).unwrap();
        assert_eq!(snapshot.entries[0].kind, FolderEntryKind::Directory);
        assert_eq!(snapshot.entries.last().unwrap().name, "large.bin");

        let snapshot = scan_folder(root.path(), FolderSort::Modified, false).unwrap();
        assert_eq!(snapshot.entries[0].kind, FolderEntryKind::Directory);
        assert_eq!(snapshot.entries.last().unwrap().name, "large.bin");

        let snapshot = scan_folder(root.path(), FolderSort::Kind, false).unwrap();
        assert_eq!(snapshot.entries[0].kind, FolderEntryKind::Directory);
        assert_eq!(snapshot.entries[1].kind, FolderEntryKind::Application);
        assert!(snapshot
            .entries
            .iter()
            .any(|entry| entry.kind == FolderEntryKind::Image));
    }

    #[test]
    fn scan_is_bounded_and_reports_more_entries() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..(MAX_FOLDER_ENTRIES + 5) {
            fs::write(root.path().join(format!("{index:04}.txt")), b"x").unwrap();
        }
        let snapshot = scan_folder(root.path(), FolderSort::Name, false).unwrap();
        assert_eq!(snapshot.entries.len(), MAX_FOLDER_ENTRIES);
        assert!(snapshot.truncated);
    }

    #[test]
    fn missing_and_unreadable_paths_return_errors() {
        let root = tempfile::tempdir().unwrap();
        assert!(scan_folder(&root.path().join("missing"), FolderSort::Name, false).is_err());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p clarion-shell dock_folder::tests -- --nocapture
```

Expected: compile error because the module types/functions do not exist.

- [ ] **Step 3: Implement scanning and sorting**

In `clarion-shell/src/dock_folder.rs`:

```rust
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context as _;
use crate::preferences::FolderSort;

pub const MAX_FOLDER_ENTRIES: usize = 256;
pub const MAX_VISITED: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderEntryKind {
    Directory,
    Application,
    Image,
    Document,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: FolderEntryKind,
    pub modified: Option<SystemTime>,
    pub size: u64,
}

#[derive(Clone, Debug, Default)]
pub struct FolderSnapshot {
    pub entries: Vec<FolderEntry>,
    pub truncated: bool,
}

pub fn scan_folder(
    path: &Path,
    sort: FolderSort,
    descending: bool,
) -> anyhow::Result<FolderSnapshot> {
    let directory = fs::read_dir(path).with_context(|| format!("读取文件夹 {}", path.display()))?;
    let mut entries = Vec::new();
    let mut visited = 0usize;
    let mut truncated = false;
    for item in directory {
        if visited >= MAX_VISITED || entries.len() >= MAX_FOLDER_ENTRIES {
            truncated = entries.len() >= MAX_FOLDER_ENTRIES;
            break;
        }
        visited += 1;
        let Ok(item) = item else {
            continue;
        };
        let entry_path = item.path();
        let Some(file_name) = entry_path.file_name() else {
            continue;
        };
        if file_name.as_encoded_bytes().starts_with(b".") {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&entry_path) else {
            continue;
        };
        let kind = if metadata.is_dir() {
            FolderEntryKind::Directory
        } else if entry_path.extension().is_some_and(|ext| ext == "desktop") {
            FolderEntryKind::Application
        } else if mime_guess::from_path(&entry_path)
            .first_raw()
            .is_some_and(|mime| mime.starts_with("image/"))
        {
            FolderEntryKind::Image
        } else {
            FolderEntryKind::Document
        };
        entries.push(FolderEntry {
            path: entry_path,
            name: file_name.to_string_lossy().into_owned(),
            kind,
            modified: metadata.modified().ok(),
            size: if metadata.is_file() { metadata.len() } else { 0 },
        });
    }
    sort_folder_entries(&mut entries, sort, descending);
    Ok(FolderSnapshot { entries, truncated })
}

pub fn sort_folder_entries(entries: &mut [FolderEntry], sort: FolderSort, descending: bool) {
    entries.sort_by(|a, b| {
        let group = (a.kind != FolderEntryKind::Directory)
            .cmp(&(b.kind != FolderEntryKind::Directory));
        if group != Ordering::Equal {
            return group;
        }
        let primary = match sort {
            FolderSort::Name => Ordering::Equal,
            FolderSort::Modified => compare_optional_time(a.modified, b.modified),
            FolderSort::Kind => kind_rank(a.kind).cmp(&kind_rank(b.kind)),
            FolderSort::Size => a.size.cmp(&b.size),
        };
        let primary = if descending { primary.reverse() } else { primary };
        primary
            .then_with(|| natural_name_cmp(&a.name, &b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
}

fn compare_optional_time(a: Option<SystemTime>, b: Option<SystemTime>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn kind_rank(kind: FolderEntryKind) -> u8 {
    match kind {
        FolderEntryKind::Directory => 0,
        FolderEntryKind::Application => 1,
        FolderEntryKind::Image => 2,
        FolderEntryKind::Document => 3,
    }
}

fn natural_name_cmp(a: &str, b: &str) -> Ordering {
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    let mut a_chars = a.chars().peekable();
    let mut b_chars = b.chars().peekable();
    loop {
        match (a_chars.peek().copied(), b_chars.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ac), Some(bc)) if ac.is_ascii_digit() && bc.is_ascii_digit() => {
                let a_number = take_number(&mut a_chars);
                let b_number = take_number(&mut b_chars);
                match a_number.cmp(&b_number) {
                    Ordering::Equal => {}
                    other => return other,
                }
            }
            (Some(ac), Some(bc)) => {
                a_chars.next();
                b_chars.next();
                match ac.cmp(&bc) {
                    Ordering::Equal => {}
                    other => return other,
                }
            }
        }
    }
}

fn take_number(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> u64 {
    let mut value = 0u64;
    while let Some(ch) = chars.peek().copied() {
        if !ch.is_ascii_digit() {
            break;
        }
        value = value.saturating_mul(10).saturating_add(ch as u64 - b'0' as u64);
        chars.next();
    }
    value
}
```

Add `mime_guess = "2.0.5"` to `clarion-shell/Cargo.toml` now, because this module uses it.

Add `mod dock_folder;` to the shell crate root (`clarion-shell/src/main.rs` if it declares modules; otherwise wherever `mod dock_motion;` is declared).

- [ ] **Step 4: Run the tests**

```bash
cargo test -p clarion-shell dock_folder::tests -- --nocapture
```

Expected: all scan/sort tests pass.

- [ ] **Step 5: Commit**

```bash
git add clarion-shell/Cargo.toml Cargo.lock clarion-shell/src/main.rs clarion-shell/src/dock_folder.rs
git commit -m "feat: scan and sort dock folder contents"
```

---

### Task 3: Host folder cache, pin/unpin, navigation, and actions

**Files:**
- Create: `clarion-shell/src/app/dock_folders.rs`
- Modify: `clarion-shell/src/app.rs`
- Test: `clarion-shell/src/app/dock_folder_tests.rs` and add `mod dock_folder_tests;` under `#[cfg(test)]` in `app.rs`

**Interfaces:**
- Consumes: Task 1 preference types, Task 2 `scan_folder`, `FolderSnapshot`, `FolderEntry`.
- Produces:
  - `Host.dock_folder_cache: HashMap<PathBuf, Arc<FolderSnapshot>>`
  - `Host.dock_folder_errors: HashMap<PathBuf, String>`
  - `Host.folder_navigation: Option<FolderNavigation>`
  - `ViewState.dock_folders: Vec<DockFolderView>`
  - `ViewState.folder_navigation: Option<FolderNavigation>`
  - `Action::PinFolder(PathBuf)`, `UnpinFolder(PathBuf)`, `FolderOrder(Vec<String>)`, `OpenFolder(PathBuf)`, `NavigateFolder(PathBuf)`, `FolderBack`, `RevealFolder(PathBuf)`, `SetFolderDisplay(...)`, `SetFolderLayout(...)`, `SetFolderSort(...)`, `SetFolderDescending(...)`.
  - `Host::pin_folder(&mut self, path: PathBuf, cx: &mut Context<Self>)`
  - `Host::request_folder_scan(&mut self, path: PathBuf, cx: &mut Context<Self>)`

- [ ] **Step 1: Write failing Host action tests**

Create `clarion-shell/src/app/dock_folder_tests.rs`:

```rust
use std::collections::HashMap;
use std::fs;
use gpui::TestAppContext;

use super::{testing, Action, AppCatalog, Host};
use crate::preferences::{FolderDisplay, FolderLayout, FolderSort};

#[gpui::test]
fn pinning_a_folder_adds_it_and_rejects_duplicates_and_non_directories(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("Projects");
    fs::create_dir(&folder).unwrap();
    let file = root.path().join("file.txt");
    fs::write(&file, b"x").unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(folder.clone()), cx);
        assert_eq!(host.prefs.dock_folders.len(), 1);
        host.handle(Action::PinFolder(folder.clone()), cx);
        assert_eq!(host.prefs.dock_folders.len(), 1);
        host.handle(Action::PinFolder(file), cx);
        assert_eq!(host.prefs.dock_folders.len(), 1);
    });
}

#[gpui::test]
fn folder_count_is_capped_at_eight(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        for index in 0..10 {
            let path = root.path().join(format!("folder-{index}"));
            fs::create_dir(&path).unwrap();
            host.handle(Action::PinFolder(path), cx);
        }
        assert_eq!(host.prefs.dock_folders.len(), 8);
    });
}

#[gpui::test]
fn folder_configuration_and_order_are_persisted_and_snapshot_exposes_views(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(a.clone()), cx);
        host.handle(Action::PinFolder(b.clone()), cx);
        host.handle(Action::FolderOrder(vec![b.to_string_lossy().into_owned(), a.to_string_lossy().into_owned()]), cx);
        host.handle(Action::SetFolderDisplay(a.clone(), FolderDisplay::Folder), cx);
        host.handle(Action::SetFolderLayout(a.clone(), FolderLayout::Grid), cx);
        host.handle(Action::SetFolderSort(a.clone(), FolderSort::Size), cx);
        host.handle(Action::SetFolderDescending(a.clone(), true), cx);
        let state = host.snapshot();
        assert_eq!(state.dock_folders.len(), 2);
        assert!(state.dock_folders[0].path.ends_with("b"));
        assert_eq!(state.dock_folders[1].display, FolderDisplay::Folder);
        assert_eq!(state.dock_folders[1].layout, FolderLayout::Grid);
        assert_eq!(state.dock_folders[1].sort, FolderSort::Size);
        assert!(state.dock_folders[1].descending);
    });
}

#[gpui::test]
fn opening_and_navigating_a_folder_tracks_history(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let child = root.path().join("child");
    fs::create_dir_all(&child).unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::OpenFolder(root.path().to_path_buf()), cx);
        let nav = host.folder_navigation.as_ref().unwrap();
        assert_eq!(nav.current, root.path());
        host.handle(Action::NavigateFolder(child.clone()), cx);
        let nav = host.folder_navigation.as_ref().unwrap();
        assert_eq!(nav.current, child);
        assert_eq!(nav.history, vec![root.path().to_path_buf()]);
        host.handle(Action::FolderBack, cx);
        assert_eq!(host.folder_navigation.as_ref().unwrap().current, root.path());
    });
}

#[gpui::test]
fn unpinning_a_folder_does_not_touch_the_filesystem(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("Keep");
    fs::create_dir(&folder).unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(folder.clone()), cx);
        host.handle(Action::UnpinFolder(folder.clone()), cx);
        assert!(host.prefs.dock_folders.is_empty());
        assert!(folder.is_dir());
    });
}
```

Keep the test file free of unused imports.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p clarion-shell dock_folder_tests -- --nocapture
```

Expected: compile errors for the new actions and Host fields.

- [ ] **Step 3: Add Host state and snapshot**

In `clarion-shell/src/app.rs`:

- Add `mod dock_folders;` and the test module declaration.
- Add imports:

```rust
use crate::preferences::{
    DockFolderPreference, FolderDisplay, FolderLayout, FolderSort,
};
use std::path::PathBuf;
```

- Add to `Host`:

```rust
dock_folder_cache: HashMap<PathBuf, Arc<crate::dock_folder::FolderSnapshot>>,
dock_folder_errors: HashMap<PathBuf, String>,
folder_navigation: Option<FolderNavigation>,
folder_scan_generation: HashMap<PathBuf, u64>,
```

- Add to `ViewState`:

```rust
pub dock_folders: Vec<DockFolderView>,
pub folder_navigation: Option<FolderNavigation>,
```

- Add the public view types:

```rust
#[derive(Clone)]
pub struct DockFolderView {
    pub path: PathBuf,
    pub name: String,
    pub display: FolderDisplay,
    pub layout: FolderLayout,
    pub sort: FolderSort,
    pub descending: bool,
    pub snapshot: Option<Arc<crate::dock_folder::FolderSnapshot>>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderNavigation {
    pub root: PathBuf,
    pub current: PathBuf,
    pub history: Vec<PathBuf>,
}
```

- Initialize the new `Host` fields in `run`, and in `snapshot()`:

```rust
dock_folders: self.dock_folder_views(),
folder_navigation: self.folder_navigation.clone(),
```

- Add the actions to `Action`.

- [ ] **Step 4: Implement the folder module**

Create `clarion-shell/src/app/dock_folders.rs`:

```rust
use super::*;
use crate::dock_folder::{scan_folder, FolderSnapshot};
use crate::preferences::MAX_DOCK_FOLDERS;
use std::path::{Path, PathBuf};
use std::sync::Arc;

impl Host {
    pub(super) fn dock_folder_views(&self) -> Vec<DockFolderView> {
        self.prefs
            .dock_folders
            .iter()
            .map(|folder| {
                let path = PathBuf::from(&folder.path);
                DockFolderView {
                    name: path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| folder.path.clone()),
                    path: path.clone(),
                    display: folder.display,
                    layout: folder.layout,
                    sort: folder.sort,
                    descending: folder.descending,
                    snapshot: self.dock_folder_cache.get(&path).cloned(),
                    error: self.dock_folder_errors.get(&path).cloned(),
                }
            })
            .collect()
    }

    pub(super) fn folder_preference_mut(
        &mut self,
        path: &Path,
    ) -> Option<&mut DockFolderPreference> {
        self.prefs
            .dock_folders
            .iter_mut()
            .find(|folder| Path::new(&folder.path) == path)
    }

    pub(super) fn pin_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.prefs.dock_folders.len() >= MAX_DOCK_FOLDERS {
            return;
        }
        let Ok(path) = std::fs::canonicalize(&path) else {
            return;
        };
        let Ok(metadata) = std::fs::metadata(&path) else {
            return;
        };
        if !metadata.is_dir()
            || self
                .prefs
                .dock_folders
                .iter()
                .any(|folder| Path::new(&folder.path) == path)
        {
            return;
        }
        self.prefs.dock_folders.push(DockFolderPreference {
            path: path.to_string_lossy().into_owned(),
            display: FolderDisplay::Stack,
            layout: FolderLayout::Auto,
            sort: FolderSort::Name,
            descending: false,
        });
        self.request_folder_scan(path, cx);
        self.save(cx);
        self.changed(cx);
    }

    pub(super) fn request_folder_scan(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(preference) = self
            .prefs
            .dock_folders
            .iter()
            .find(|folder| Path::new(&folder.path) == path)
        else {
            return;
        };
        let sort = preference.sort;
        let descending = preference.descending;
        let generation = self
            .folder_scan_generation
            .entry(path.clone())
            .and_modify(|value| *value = value.wrapping_add(1))
            .or_insert(1);
        let generation = *generation;
        let scan_path = path.clone();
        cx.spawn(async move |host, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { scan_folder(&scan_path, sort, descending) })
                .await;
            let _ = host.update(cx, |host, cx| {
                if host.folder_scan_generation.get(&path) != Some(&generation) {
                    return;
                }
                match result {
                    Ok(snapshot) => {
                        host.dock_folder_cache.insert(path.clone(), Arc::new(snapshot));
                        host.dock_folder_errors.remove(&path);
                    }
                    Err(error) => {
                        host.dock_folder_cache.remove(&path);
                        host.dock_folder_errors.insert(path.clone(), error.to_string());
                    }
                }
                host.changed(cx);
            });
        })
        .detach();
    }
}
```

Add action handling in `Host::handle`:

```rust
Action::PinFolder(path) => self.pin_folder(path, cx),
Action::UnpinFolder(path) => {
    self.prefs
        .dock_folders
        .retain(|folder| Path::new(&folder.path) != path);
    self.dock_folder_cache.remove(&path);
    self.dock_folder_errors.remove(&path);
    if self.folder_navigation.as_ref().is_some_and(|nav| nav.root == path) {
        self.folder_navigation = None;
        self.close(cx);
    }
    self.save(cx);
    self.changed(cx);
}
Action::FolderOrder(order) => {
    let mut seen = HashSet::new();
    let mut next = Vec::new();
    for path in order {
        let Some(folder) = self
            .prefs
            .dock_folders
            .iter()
            .find(|folder| folder.path == path)
        else {
            continue;
        };
        if seen.insert(folder.path.clone()) {
            next.push(folder.clone());
        }
    }
    for folder in &self.prefs.dock_folders {
        if !seen.contains(&folder.path) {
            next.push(folder.clone());
        }
    }
    self.prefs.dock_folders = next;
    self.save(cx);
    self.changed(cx);
}
Action::OpenFolder(path) => {
    self.folder_navigation = Some(FolderNavigation {
        root: path.clone(),
        current: path.clone(),
        history: Vec::new(),
    });
    self.request_folder_scan(path.clone(), cx);
    self.open_panel(Panel::DockFolder { root: path }, cx);
}
Action::NavigateFolder(path) => {
    if let Some(nav) = &mut self.folder_navigation {
        let Ok(metadata) = std::fs::metadata(&path) else {
            return;
        };
        if !metadata.is_dir() {
            return;
        }
        nav.history.push(nav.current.clone());
        nav.current = path.clone();
        self.request_folder_scan(path, cx);
        self.changed(cx);
    }
}
Action::FolderBack => {
    if let Some(nav) = &mut self.folder_navigation {
        if let Some(previous) = nav.history.pop() {
            nav.current = previous;
            self.request_folder_scan(nav.current.clone(), cx);
            self.changed(cx);
        }
    }
}
Action::RevealFolder(path) => {
    let _ = self.backend.send(backend::Command::OpenPath(path));
    self.close(cx);
}
Action::SetFolderDisplay(path, display) => {
    if let Some(folder) = self.folder_preference_mut(&path) {
        folder.display = display;
    }
    self.save(cx);
    self.changed(cx);
}
Action::SetFolderLayout(path, layout) => {
    if let Some(folder) = self.folder_preference_mut(&path) {
        folder.layout = layout;
    }
    self.save(cx);
    self.changed(cx);
}
Action::SetFolderSort(path, sort) => {
    if let Some(folder) = self.folder_preference_mut(&path) {
        folder.sort = sort;
    }
    self.save(cx);
    self.changed(cx);
}
Action::SetFolderDescending(path, descending) => {
    if let Some(folder) = self.folder_preference_mut(&path) {
        folder.descending = descending;
    }
    self.save(cx);
    self.changed(cx);
}
```

Call `request_folder_scan` once per pinned folder after the catalog and host are ready. Add a small loop near the existing initialization that runs when `self.ready` becomes true:

```rust
if !self.ready && !self.outputs.is_empty() {
    self.ready = true;
    for folder in self.prefs.dock_folders.clone() {
        self.request_folder_scan(PathBuf::from(folder.path), cx);
    }
    // existing ready work follows
}
```

`open_panel` may be named differently in this codebase; use the existing panel-opening helper used by `Action::Panel` rather than duplicating window creation. If `Host::open_panel` is private, call `self.handle(Action::Panel(Panel::DockFolder { root: path }), cx)` instead.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p clarion-shell dock_folder_tests -- --nocapture
cargo test -p clarion-shell preferences
```

Expected: all new Host tests and preference tests pass.

- [ ] **Step 6: Commit**

```bash
git add clarion-shell/src/app.rs clarion-shell/src/app/dock_folders.rs clarion-shell/src/app/dock_folder_tests.rs
git commit -m "feat: add dock folder host state and actions"
```

---

### Task 4: DockMetrics folder slots and shared surface origin

**Files:**
- Modify: `clarion-shell/src/dock_motion.rs`
- Modify: `clarion-shell/src/app/panels.rs`
- Modify: `clarion-shell/src/app.rs` (`targets`)
- Modify: `clarion-shell/src/view/macos_launchpad.rs` (`dock_cutout`)
- Test: inline tests in `dock_motion.rs`, `panels.rs`, `view/launchpad_tests.rs`

**Interfaces:**
- Produces:
  - `pub fn DockMetrics::with_folders(app_count, icon_size, scale, magnification_enabled, show_launcher, folder_count, vertical, output_width, output_height) -> Self`
  - `DockMetrics.folder_count: usize`
  - `pub enum DockSlotKind { Launcher, App(usize), Overflow, Folder(usize), Trash }`
  - `pub fn dock_slot_kind(metrics: &DockMetrics, launcher: usize, slot: usize) -> DockSlotKind`
  - `pub fn dock_surface_origin(state: &ViewState, metrics: &DockMetrics, w: f32, h: f32) -> (f32, f32)`
  - Existing `DockMetrics::new` remains a wrapper with `folder_count = 0`.

- [ ] **Step 1: Write failing slot and origin tests**

Add to `clarion-shell/src/dock_motion.rs` tests:

```rust
#[test]
fn folders_are_utilities_between_overflow_and_trash() {
    let mut metrics = DockMetrics::with_folders(3, 48., 1.45, true, true, 2, false, 1440., 900.);
    metrics.restrict_overflow(3, 0, 0);
    assert_eq!(metrics.folder_count, 2);
    assert_eq!(metrics.slot_count, 1 + 3 + 2 + 1);
    assert_eq!(dock_slot_kind(&metrics, 1, 0), DockSlotKind::Launcher);
    assert_eq!(dock_slot_kind(&metrics, 1, 1), DockSlotKind::App(0));
    assert_eq!(dock_slot_kind(&metrics, 1, 2), DockSlotKind::App(1));
    assert_eq!(dock_slot_kind(&metrics, 1, 3), DockSlotKind::App(2));
    assert_eq!(dock_slot_kind(&metrics, 1, 4), DockSlotKind::Folder(0));
    assert_eq!(dock_slot_kind(&metrics, 1, 5), DockSlotKind::Folder(1));
    assert_eq!(dock_slot_kind(&metrics, 1, 6), DockSlotKind::Trash);
}

#[test]
fn folders_are_kept_before_apps_overflow() {
    let mut metrics = DockMetrics::with_folders(60, 64., 1.45, true, true, 8, false, 1280., 800.);
    metrics.restrict_overflow(60, 0, 0);
    assert_eq!(metrics.folder_count, 8);
    assert!(metrics.visible_apps + metrics.hidden_apps == 60);
    assert_eq!(metrics.slot_count, metrics.visible_apps + 8 + 2 + 1);
}
```

Add to `clarion-shell/src/app/panels.rs` tests:

```rust
#[gpui::test]
fn folder_tile_centers_follow_the_dock_surface_origin(cx: &mut TestAppContext) {
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    let state = host.read_with(cx, |host, _| host.snapshot());
    for position in [DockPosition::Bottom, DockPosition::Left, DockPosition::Right] {
        let mut state = state.clone();
        state.prefs.dock_position = position;
        let metrics = DockMetrics::with_folders(2, 48., 1.45, true, true, 2, position != DockPosition::Bottom, 1440., 900.);
        let (x, y) = dock_tile_center(&state, &metrics, 3, 1440., 900.);
        assert!(x.is_finite() && y.is_finite());
        assert!(x > 0. && x < 1440. && y > 0. && y < 900.);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p clarion-shell folders_are_ -- --nocapture
cargo test -p clarion-shell folder_tile_centers -- --nocapture
```

Expected: compile errors for `with_folders`, `DockSlotKind`, and `dock_slot_kind`.

- [ ] **Step 3: Implement DockMetrics and slot helpers**

In `dock_motion.rs`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockSlotKind {
    Launcher,
    App(usize),
    Overflow,
    Folder(usize),
    Trash,
}

pub fn dock_slot_kind(
    metrics: &DockMetrics,
    launcher: usize,
    slot: usize,
) -> DockSlotKind {
    if slot == 0 && launcher == 1 {
        return DockSlotKind::Launcher;
    }
    let app_start = launcher;
    let app_end = app_start + metrics.visible_apps;
    if slot < app_end {
        return DockSlotKind::App(slot - app_start);
    }
    let mut next = app_end;
    if metrics.hidden_apps > 0 {
        if slot == next {
            return DockSlotKind::Overflow;
        }
        next += 1;
    }
    let folder_end = next + metrics.folder_count;
    if slot < folder_end {
        return DockSlotKind::Folder(slot - next);
    }
    DockSlotKind::Trash
}
```

Add `folder_count` to `DockMetrics`, add `with_folders`, and keep `new`:

```rust
pub fn new(
    app_count: usize,
    icon_size: f32,
    scale: f32,
    magnification_enabled: bool,
    show_launcher: bool,
    vertical: bool,
    output_width: f32,
    output_height: f32,
) -> Self {
    Self::with_folders(
        app_count,
        icon_size,
        scale,
        magnification_enabled,
        show_launcher,
        0,
        vertical,
        output_width,
        output_height,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn with_folders(
    app_count: usize,
    icon_size: f32,
    scale: f32,
    magnification_enabled: bool,
    show_launcher: bool,
    folder_count: usize,
    vertical: bool,
    output_width: f32,
    output_height: f32,
) -> Self {
    // Move the existing body here.
    // Replace:
    // let utilities = 1 + usize::from(show_launcher);
    // with:
    let folder_count = folder_count.min(crate::preferences::MAX_DOCK_FOLDERS);
    let utilities = 1 + usize::from(show_launcher) + folder_count;
    // Add folder_count to the returned struct.
}
```

- [ ] **Step 4: Add the shared surface origin and use it**

In `clarion-shell/src/app/panels.rs`:

```rust
pub(crate) fn dock_surface_origin(
    state: &ViewState,
    metrics: &crate::dock_motion::DockMetrics,
    w: f32,
    h: f32,
) -> (f32, f32) {
    match state.prefs.dock_position {
        DockPosition::Bottom => ((w - metrics.surface_width) / 2., h - 8. - metrics.surface_height),
        DockPosition::Left | DockPosition::Right => {
            let bar = state.prefs.bar_height as f32;
            let y = bar + (h - bar - metrics.surface_height) / 2.;
            if state.prefs.dock_position == DockPosition::Left {
                (8., y)
            } else {
                (w - 8. - metrics.surface_width, y)
            }
        }
    }
}
```

Rewrite `dock_tile_center` to use it:

```rust
let (surface_x, surface_y) = dock_surface_origin(state, metrics, w, h);
match state.prefs.dock_position {
    DockPosition::Bottom => (surface_x + main, surface_y + metrics.cross_padding + cross),
    DockPosition::Left => (surface_x + cross, surface_y + main),
    DockPosition::Right => (surface_x + metrics.surface_width - cross, surface_y + main),
}
```

In `Host::targets`, replace the per-position origin math with `dock_surface_origin` plus `metrics.main_padding` / `metrics.end_gap`:

```rust
let (surface_x, surface_y) = panels::dock_surface_origin(&self.snapshot(), &metrics, output.width, output.height);
let main = metrics.main_padding + metrics.end_gap + (index as f32 + launcher) * metrics.item;
let cross = metrics.inset;
let (x, y) = match self.prefs.dock_position {
    DockPosition::Bottom => (surface_x + main, surface_y + metrics.cross_padding + cross),
    DockPosition::Left => (surface_x + cross, surface_y + main),
    DockPosition::Right => (surface_x + metrics.surface_width - cross, surface_y + main),
};
```

Use `dock_surface_origin` in `macos_launchpad::dock_cutout`, and call `DockMetrics::with_folders(..., host.dock_folders.len(), ...)`.

- [ ] **Step 5: Run the geometry tests**

```bash
cargo test -p clarion-shell folders_are_ -- --nocapture
cargo test -p clarion-shell folder_tile_centers -- --nocapture
cargo test -p clarion-shell the_dock_ -- --nocapture
cargo test -p clarion-shell launchpad -- --nocapture
```

Expected: new tests pass; existing Dock/launchpad tests pass.

- [ ] **Step 6: Commit**

```bash
git add clarion-shell/src/dock_motion.rs clarion-shell/src/app/panels.rs clarion-shell/src/app.rs clarion-shell/src/view/macos_launchpad.rs
git commit -m "feat: include folders in dock metrics and slot geometry"
```

---

### Task 5: Folder tiles, previews, separators, and tooltips

**Files:**
- Modify: `clarion-shell/src/view/macos_dock.rs`
- Modify: `clarion-shell/src/view/dock_tests.rs`
- Test: `clarion-shell/src/view/dock_tests.rs`

**Interfaces:**
- Consumes: `DockFolderView`, `dock_slot_kind`, `DockMetrics.folder_count`, `desktop_icons`.
- Produces: folder tile selector `dock-folder-<index>`; folder drag selectors `dock-folder-drag-<index>`.

- [ ] **Step 1: Write failing folder tile tests**

Add to `dock_tests.rs`:

```rust
#[gpui::test]
fn folder_tiles_render_between_overflow_and_trash_with_a_preview(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("Projects");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("note.txt"), b"hello").unwrap();
    let mut fixture = Fixture::with_apps(cx, false, &[APP_A]);
    fixture.visual.cx.update(|cx| {
        fixture.host.update(cx, |host, cx| {
            host.prefs.dock_folders = vec![crate::preferences::DockFolderPreference {
                path: folder.to_string_lossy().into_owned(),
                display: crate::preferences::FolderDisplay::Stack,
                layout: crate::preferences::FolderLayout::Auto,
                sort: crate::preferences::FolderSort::Name,
                descending: false,
            }];
            host.changed(cx);
        });
    });
    fixture.visual.update(|window, _| window.refresh());
    fixture.visual.run_until_parked();
    assert!(fixture.visual.debug_bounds("dock-folder-0").is_some());
    let trash = fixture.visual.debug_bounds("dock-trash").unwrap();
    let folder_bounds = fixture.visual.debug_bounds("dock-folder-0").unwrap();
    assert!(folder_bounds.right() <= trash.left() + px(0.01));
}
```

Add a long folder name test:

```rust
#[gpui::test]
fn a_long_folder_name_does_not_change_the_folder_tile_height(cx: &mut TestAppContext) {
    // Create a folder with a very long name, pin it, and assert the tile is
    // exactly metrics.icon_size tall and wide, as the app tile test does.
}
```

Use the existing `Fixture::with_apps` helper and the `debug_bounds` selector. The host must be updated before the view snapshot is refreshed; follow the pattern in `the_icon_is_centered_with_the_same_clearance_on_every_side`.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p clarion-shell folder_tiles_render -- --nocapture
cargo test -p clarion-shell a_long_folder_name -- --nocapture
```

Expected: `debug_bounds` returns `None`.

- [ ] **Step 3: Render folder tiles**

In `macos_dock`, pass the folder count to `DockMetrics::with_folders`:

```rust
let folders = host.dock_folders.len();
let mut metrics = DockMetrics::with_folders(
    apps.len(),
    host.prefs.dock_size as f32,
    host.prefs.magnification_scale as f32,
    host.prefs.magnification && !host.prefs.reduce_motion,
    host.prefs.show_launcher,
    folders,
    vertical,
    width,
    height,
);
```

Replace the manual slot classification in the render loop with `dock_slot_kind(&metrics, launcher_offset, index)`. For `DockSlotKind::Folder(folder_index)`:

```rust
let folder = host.dock_folders.get(folder_index);
let id = format!("dock-folder-{folder_index}");
let mut tile = dock_slot(SharedString::from(id), slot.size)
    .left(px(x))
    .top(px(y));
tile = tile.child(folder_tile_icon(folder, slot.size));
tile = tile
    .on_click(move |_, _, cx| {
        let path = folder.as_ref().map(|folder| folder.path.clone());
        if let Some(path) = path {
            h.update(cx, |host, cx| host.handle(Action::OpenFolder(path), cx));
        }
    })
    .on_mouse_down(MouseButton::Right, {
        let h = self.host.clone();
        move |_, _, cx| {
            if let Some(folder) = folder.clone() {
                h.update(cx, |host, cx| {
                    host.handle(Action::Panel(Panel::DockFolderMenu { path: folder.path }), cx)
                });
            }
        }
    })
    .on_hover(cx.listener({
        let name = folder.as_ref().map(|folder| folder.name.clone()).unwrap_or_default();
        move |this, hovered, _, cx| {
            this.dock_motion.hover_tooltip(
                if *hovered { Some(name.as_str()) } else { None },
                this.dock_motion.epoch.elapsed(),
            );
            cx.notify();
        }
    }));
content = content.child(tile);
```

Add `folder_tile_icon` in `macos_dock.rs`:

```rust
fn folder_tile_icon(folder: Option<&DockFolderView>, size: f32) -> AnyElement {
    let Some(folder) = folder else {
        return crate::desktop_icons::icon(crate::desktop_icons::DesktopIconKind::Folder, size);
    };
    if folder.display == crate::preferences::FolderDisplay::Folder
        || folder.snapshot.as_ref().is_none_or(|snapshot| snapshot.entries.is_empty())
    {
        return crate::desktop_icons::icon(crate::desktop_icons::DesktopIconKind::Folder, size);
    }
    let entries = &folder.snapshot.as_ref().unwrap().entries;
    let mut stack = div().relative().size(px(size));
    for (index, entry) in entries.iter().take(3).enumerate().rev() {
        let offset = 0.16 + index as f32 * 0.08;
        stack = stack.child(
            div()
                .absolute()
                .left(px(offset * size))
                .top(px(offset * size))
                .child(crate::desktop_icons::icon(folder_entry_icon(entry.kind), size * 0.56)),
        );
    }
    stack.into_any_element()
}

fn folder_entry_icon(kind: crate::dock_folder::FolderEntryKind) -> crate::desktop_icons::DesktopIconKind {
    use crate::dock_folder::FolderEntryKind;
    use crate::desktop_icons::DesktopIconKind;
    match kind {
        FolderEntryKind::Directory => DesktopIconKind::Folder,
        FolderEntryKind::Application => DesktopIconKind::Application,
        FolderEntryKind::Image => DesktopIconKind::Image,
        FolderEntryKind::Document => DesktopIconKind::Document,
    }
}
```

Add the folder group separator. The existing `separate` condition should include the first folder:

```rust
let folder = matches!(kind, DockSlotKind::Folder(_));
let first_folder = folder && !folder_started;
folder_started |= folder;
let separate = trash || first_folder || (recent && !recent_started);
```

When folders exist, the trash still gets its own separator only if there was no folder separator immediately before it; use `folder_started` to avoid a double line.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p clarion-shell folder_tiles_render -- --nocapture
cargo test -p clarion-shell a_long_folder_name -- --nocapture
cargo test -p clarion-shell view::dock_tests -- --nocapture
```

Expected: folder tiles render, separator and tooltip tests pass, existing Dock tests pass.

- [ ] **Step 5: Commit**

```bash
git add clarion-shell/src/view/macos_dock.rs clarion-shell/src/view/dock_tests.rs
git commit -m "feat: render dock folder tiles and previews"
```

---

### Task 6: Folder tile drag reorder

**Files:**
- Modify: `clarion-shell/src/view/macos_dock.rs`
- Modify: `clarion-shell/src/app.rs` (`DockViewMotion` is in `macos_dock.rs`)
- Modify: `clarion-shell/src/view/dock_tests.rs`
- Test: `clarion-shell/src/view/dock_tests.rs`

**Interfaces:**
- Produces:
  - `DockViewMotion.folder_drag: Option<DockFolderDrag>`
  - `DockFolderDrag { path: PathBuf, grab: f32, pointer: f32, order: Vec<String>, insert: usize }`
  - `Action::FolderOrder(Vec<String>)` already exists from Task 3.
  - `DockGeometry.folder_base: f32`.

- [ ] **Step 1: Write the failing reorder test**

```rust
#[gpui::test]
fn dragging_a_folder_tile_reorders_only_the_folder_group(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("A");
    let b = root.path().join("B");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let mut fixture = Fixture::with_apps(cx, false, &[APP_A]);
    let first = fixture.visual.debug_bounds("dock-folder-0").unwrap();
    let second = fixture.visual.debug_bounds("dock-folder-1").unwrap();
    fixture.visual.simulate_mouse_down(first.center(), MouseButton::Left, Modifiers::default());
    fixture.visual.simulate_mouse_move(
        second.center() + point(px(48.), px(0.)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    fixture.visual.simulate_mouse_up(
        second.center() + point(px(48.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    fixture.visual.run_until_parked();
    fixture.host.read_with(&fixture.visual, |host, _| {
        assert_eq!(host.prefs.dock_folders[0].path, b.to_string_lossy());
        assert_eq!(host.prefs.dock_folders[1].path, a.to_string_lossy());
    });
}
```

Set the pinned folders before rendering with the same host update used in Task 5.

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p clarion-shell dragging_a_folder_tile -- --nocapture
```

Expected: the saved order stays A, B.

- [ ] **Step 3: Implement folder drag**

Add `folder_drag` to `DockViewMotion` and `folder_base` to `DockGeometry`. Add methods:

```rust
pub(super) fn press_folder(&mut self, path: String, pointer: f32) {
    self.folder_press = Some(DockFolderPress { path, pointer });
}

pub(super) fn begin_folder_drag(&mut self, pointer: f32, folders: &[DockFolderView]) -> bool {
    let Some(press) = self.folder_press.clone() else { return false; };
    if (pointer - press.pointer).abs() < crate::dock_motion::DRAG_THRESHOLD {
        return false;
    }
    let Some(index) = folders.iter().position(|folder| folder.path == Path::new(&press.path)) else {
        return false;
    };
    self.folder_drag = Some(DockFolderDrag {
        path: press.path,
        grab: press.pointer - (self.geometry.folder_base + index as f32 * self.geometry.item),
        pointer,
        order: folders.iter().map(|folder| folder.path.to_string_lossy().into_owned()).collect(),
        insert: index,
    });
    self.folder_press = None;
    self.strength.snap(0.);
    true
}
```

Wire folder tile `on_mouse_down` to `press_folder`, `on_mouse_move` to a helper that calls `begin_folder_drag`/`track_folder_drag`, and `on_mouse_up` to emit `Action::FolderOrder`. Use the same pointer coordinate as the app drag (main axis). Use a separate `folder_reorder: HashMap<String, Motion>` keyed by path so app and folder animations cannot collide.

Set `DockGeometry.folder_base` in render:

```rust
let folder_start = launcher_offset + metrics.visible_apps + usize::from(metrics.hidden_apps > 0);
self.dock_motion.geometry.folder_base =
    metrics.main_padding + metrics.end_gap + folder_start as f32 * metrics.item;
```

During a folder drag, skip painting the dragged folder at its slot and paint a placeholder plus a floating folder tile at `folder_drag.pointer - grab`, clamped to the folder group span. On release, call `host.handle(Action::FolderOrder(order), cx)`.

- [ ] **Step 4: Run the reorder test and existing drag tests**

```bash
cargo test -p clarion-shell dragging_a_folder_tile -- --nocapture
cargo test -p clarion-shell dragging_a_pinned -- --nocapture
```

Expected: folder order changes; existing app reorder tests still pass.

- [ ] **Step 5: Commit**

```bash
git add clarion-shell/src/view/macos_dock.rs clarion-shell/src/app.rs clarion-shell/src/view/dock_tests.rs
git commit -m "feat: reorder dock folder tiles"
```

---

### Task 7: External folder drop onto the Dock

**Files:**
- Modify: `third_party/gpui/src/interactive.rs`
- Modify: `third_party/gpui/CLARION_PATCHES.md`
- Modify: `clarion-shell/src/view/macos_dock.rs`
- Modify: `clarion-shell/src/view/dock_tests.rs`
- Test: `clarion-shell/src/view/dock_tests.rs`

**Interfaces:**
- Consumes: `ExternalPaths` (GPUI), `Action::PinFolder`.
- Produces: `ExternalPaths::new(paths: impl IntoIterator<Item = PathBuf>) -> Self`.

- [ ] **Step 1: Add the test constructor and write failing drop tests**

In `third_party/gpui/src/interactive.rs`:

```rust
impl ExternalPaths {
    pub fn new(paths: impl IntoIterator<Item = std::path::PathBuf>) -> Self {
        Self(paths.into_iter().collect())
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.0
    }
}
```

Add a note to `third_party/gpui/CLARION_PATCHES.md` describing the public test/embedding constructor.

Add to `dock_tests.rs`:

```rust
#[gpui::test]
fn dropping_a_folder_on_the_dock_pins_it(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("Pinned");
    std::fs::create_dir(&folder).unwrap();
    let mut fixture = Fixture::new(cx, false);
    let bounds = fixture.visual.debug_bounds("dock-trash").unwrap();
    let position = bounds.origin + point(px(-40.), px(24.));
    fixture.visual.simulate_event(gpui::FileDropEvent::Entered {
        position,
        paths: gpui::ExternalPaths::new([folder.clone()]),
    });
    fixture.visual.simulate_event(gpui::FileDropEvent::Submit { position });
    fixture.visual.run_until_parked();
    fixture.host.read_with(&fixture.visual, |host, _| {
        assert_eq!(host.prefs.dock_folders.len(), 1);
        assert_eq!(host.prefs.dock_folders[0].path, folder.to_string_lossy());
    });
}

#[gpui::test]
fn dropping_a_folder_on_trash_does_not_pin_it(cx: &mut TestAppContext) {
    // Same setup, but drop at trash.center(); assert dock_folders stays empty.
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p clarion-shell dropping_a_folder -- --nocapture
```

Expected: dock_folders remains empty.

- [ ] **Step 3: Add the Dock drop handler and ghost**

In `macos_dock`, wrap the tray content root with:

```rust
let host = self.host.clone();
content = content
    .can_drop(|value, _, _| {
        value
            .downcast_ref::<gpui::ExternalPaths>()
            .is_some_and(|paths| paths.paths().len() == 1)
    })
    .on_drop::<gpui::ExternalPaths>(move |paths, _, cx| {
        let Some(path) = paths.paths().first().cloned() else { return; };
        host.update(cx, |host, cx| host.handle(Action::PinFolder(path), cx));
    })
    .drag_over::<gpui::ExternalPaths>(|style, paths, _, _| {
        if paths.paths().len() == 1 {
            style.border_2().border_color(rgba(0x0a84ffff))
        } else {
            style
        }
    });
```

The `drag_over` style is applied to the full content; if the tray outline is not visually useful, instead render a small `folder-drop-hint` absolute tile at the folder-group end while `cx.has_active_drag()` is true, and apply the same style to that hint. Keep the content drop handler for the actual pin.

Add a trash drop consumer so folder drops cannot bubble into pinning:

```rust
tile = tile
    .can_drop(|_, _, _| true)
    .on_drop::<gpui::ExternalPaths>(|_, _, cx| cx.stop_propagation());
```

Add the same consuming listener to app tiles now, with no action; Task 17 replaces it with the launch action.

- [ ] **Step 4: Run the drop tests**

```bash
cargo test -p clarion-shell dropping_a_folder -- --nocapture
cargo test -p clarion-shell view::dock_tests -- --nocapture
```

Expected: folder drop pins once; trash drop and app drop do not pin; existing tests pass.

- [ ] **Step 5: Commit**

```bash
git add third_party/gpui/src/interactive.rs third_party/gpui/CLARION_PATCHES.md clarion-shell/src/view/macos_dock.rs clarion-shell/src/view/dock_tests.rs
git commit -m "feat: pin folders dropped on the dock"
```

---

### Task 8: Folder popup placement and dimensions

**Files:**
- Modify: `clarion-shell/src/app.rs` (`Panel` enum)
- Modify: `clarion-shell/src/app/panels.rs`
- Test: `clarion-shell/src/app/panels.rs`

**Interfaces:**
- Produces:
  - `Panel::DockFolder { root: PathBuf }`
  - `Panel::DockFolderMenu { path: PathBuf }`
  - `pub(crate) fn folder_layout(state: &ViewState, root: &Path) -> FolderLayout`
  - `pub(crate) fn folder_popup_size(state: &ViewState, root: &Path, w: f32, h: f32) -> (f32, f32)`
  - `pub(crate) fn folder_tile_rect(state: &ViewState, root: &Path, w: f32, h: f32) -> Option<Bounds<Pixels>>`

- [ ] **Step 1: Write failing placement tests**

Add to `panels.rs` tests:

```rust
#[gpui::test]
fn folder_popups_stay_inside_the_output_and_point_at_the_folder(cx: &mut TestAppContext) {
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    let state = host.read_with(cx, |host, _| host.snapshot());
    for position in [DockPosition::Bottom, DockPosition::Left, DockPosition::Right] {
        let mut state = state.clone();
        state.prefs.dock_position = position;
        let placement = placement(
            &state,
            &Panel::DockFolder { root: PathBuf::from("/tmp/Projects") },
            1440.,
            900.,
        );
        assert_eq!(f32::from(placement.bounds.size.width), 1440.);
        assert_eq!(f32::from(placement.bounds.size.height), 900.);
        if let Some(popup) = placement.popup {
            assert!(popup.origin.x >= px(0.) && popup.origin.y >= px(0.));
            assert!(popup.right() <= px(1440.) && popup.bottom() <= px(900.));
        }
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p clarion-shell folder_popups_stay -- --nocapture
```

Expected: compile error for the missing `Panel` variants.

- [ ] **Step 3: Implement placement**

Add the variants:

```rust
DockFolder { root: PathBuf },
DockFolderMenu { path: PathBuf },
```

Add helpers in `panels.rs`:

```rust
pub(crate) fn folder_layout(state: &ViewState, root: &Path) -> FolderLayout {
    state
        .dock_folders
        .iter()
        .find(|folder| folder.path == root)
        .map(|folder| folder.layout)
        .unwrap_or(FolderLayout::Auto)
}
```

Resolve `Auto` before placement:

```rust
fn folder_tile_center(
    state: &ViewState,
    root: &Path,
    w: f32,
    h: f32,
) -> Option<(f32, f32)> {
    let metrics = dock_metrics(state, &state.dock_apps(), w, h);
    let index = state
        .dock_folders
        .iter()
        .position(|folder| folder.path == root)?;
    let slot = usize::from(state.prefs.show_launcher)
        + metrics.visible_apps
        + usize::from(metrics.hidden_apps > 0)
        + index;
    Some(dock_tile_center(state, &metrics, slot, w, h))
}

fn fan_fits(state: &ViewState, root: &Path, count: usize, w: f32, h: f32) -> bool {
    if count == 0 || count > 8 {
        return false;
    }
    let Some((x, y)) = folder_tile_center(state, root, w, h) else {
        return false;
    };
    let icon = state.prefs.dock_size as f32;
    let radius = (icon * 3.2).clamp(120., 240.);
    let reach = radius + icon + 112.;
    let margin = 16.;
    match state.prefs.dock_position {
        DockPosition::Bottom => {
            y >= reach + margin
                && x >= radius * 0.55 + margin
                && w - x >= radius * 0.55 + margin
        }
        DockPosition::Left => {
            x <= w - reach - margin
                && y >= reach / 2. + margin
                && h - y >= reach / 2. + margin
        }
        DockPosition::Right => {
            x >= reach + margin
                && y >= reach / 2. + margin
                && h - y >= reach / 2. + margin
        }
    }
}

fn resolved_folder_layout(state: &ViewState, root: &Path, w: f32, h: f32) -> FolderLayout {
    let view = state.dock_folders.iter().find(|folder| folder.path == root);
    let count = view
        .and_then(|folder| folder.snapshot.as_ref())
        .map(|snapshot| snapshot.entries.len())
        .unwrap_or(0);
    match folder_layout(state, root) {
        FolderLayout::Auto => match state.prefs.dock_position {
            DockPosition::Bottom
                if count > 0 && count <= 8 && fan_fits(state, root, count, w, h) =>
            {
                FolderLayout::Fan
            }
            DockPosition::Bottom => FolderLayout::Grid,
            _ if count > 0 && count <= 12 => FolderLayout::List,
            _ => FolderLayout::Grid,
        },
        layout => layout,
    }
}
```

For list/grid:

```rust
let (menu_w, menu_h) = folder_popup_size(state, root, w, h);
let (tile_x, tile_y) = folder_tile_center(state, root, w, h)?;
let edge = metrics.thickness + 18.;
match state.prefs.dock_position {
    DockPosition::Bottom => {
        let x = (tile_x - menu_w / 2.).clamp(8., (w - menu_w - 8.).max(8.));
        let y = tile_y - edge - menu_h;
        (menu_w, menu_h, x, y, ArrowEdge::Bottom, tile_x - x)
    }
    DockPosition::Left => {
        let x = tile_x + edge;
        let y = (tile_y - menu_h / 2.).clamp(top, (h - menu_h - 8.).max(top));
        (menu_w, menu_h, x, y, ArrowEdge::Left, tile_y - y)
    }
    DockPosition::Right => {
        let x = tile_x - edge - menu_w;
        let y = (tile_y - menu_h / 2.).clamp(top, (h - menu_h - 8.).max(top));
        (menu_w, menu_h, x, y, ArrowEdge::Right, tile_y - y)
    }
}
```

For fan, return `arrow = None` and `popup = None`, with `bounds` full output.

`folder_popup_size`:

```rust
List => (300., (content_height + 88.).min(h * 0.7)),
Grid => (380., (grid_height + 88.).min(h * 0.7)),
```

Clamp width to `w - 24`.

`folder_tile_rect` uses `dock_slot_kind` and `dock_tile_center`:

```rust
let metrics = dock_metrics(state, &state.dock_apps(), w, h);
let index = state.dock_folders.iter().position(|folder| folder.path == root)?;
let slot = usize::from(state.prefs.show_launcher)
    + metrics.visible_apps
    + usize::from(metrics.hidden_apps > 0)
    + index;
let (x, y) = dock_tile_center(state, &metrics, slot, w, h);
Some(Bounds::new(point(px(x - metrics.icon_size / 2.), px(y - metrics.icon_size / 2.)), size(px(metrics.icon_size), px(metrics.icon_size))))
```

- [ ] **Step 4: Run the placement tests**

```bash
cargo test -p clarion-shell folder_popups_stay -- --nocapture
cargo test -p clarion-shell the_dock_menu_is_as_tall -- --nocapture
```

Expected: the new placement test passes and existing menu placement tests stay green.

- [ ] **Step 5: Commit**

```bash
git add clarion-shell/src/app.rs clarion-shell/src/app/panels.rs
git commit -m "feat: place dock folder popups"
```

---

### Task 9: Folder popup list/grid rendering, navigation, and scrolling

**Files:**
- Create: `clarion-shell/src/view/dock_folder.rs`
- Modify: `clarion-shell/src/view.rs`
- Modify: `clarion-shell/src/app.rs` (module declaration and panel opening)
- Test: `clarion-shell/src/view/dock_folder_tests.rs`

**Interfaces:**
- Consumes: `Panel::DockFolder`, `FolderNavigation`, `DockFolderView`, `FolderEntry`.
- Produces:
  - `pub(super) fn folder_popup_content(host: &ViewState, layout: FolderLayout, p: Palette) -> AnyElement`
  - `pub(super) fn folder_menu_content(host: &ViewState, path: &Path, p: Palette) -> AnyElement`
  - `pub(super) fn folder_panel_overlay(...) -> AnyElement` (fan in Task 10)

- [ ] **Step 1: Write failing navigation and row-height tests**

Create `clarion-shell/src/view/dock_folder_tests.rs`:

```rust
use super::*;
use crate::app::{testing, Action, Panel};
use crate::preferences::{FolderLayout, FolderSort};
use gpui::{px, Modifiers, MouseButton, TestAppContext};

#[gpui::test]
fn list_and_grid_render_entries_without_wrapping_long_names(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("this-is-a-very-long-file-name-that-must-not-wrap.txt"),
        b"x",
    )
    .unwrap();
    std::fs::write(root.path().join("short.txt"), b"x").unwrap();
    let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.handle(Action::PinFolder(root.path().to_path_buf()), cx);
        host.handle(
            Action::SetFolderLayout(root.path().to_path_buf(), FolderLayout::List),
            cx,
        );
        host.handle(Action::OpenFolder(root.path().to_path_buf()), cx);
    });
    cx.run_until_parked();
    let window = cx.update(|cx| testing::open_panel(&host, Panel::DockFolder { root: root.path().to_path_buf() }, cx));
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.run_until_parked();
    let long = visual.debug_bounds("folder-entry-0").unwrap();
    assert!(long.size.height <= px(34.01));
    assert!(long.right() <= px(300.01));
}

#[gpui::test]
fn clicking_a_subfolder_navigates_and_back_returns(cx: &mut TestAppContext) {
    // Create root/child, open root, click the child's debug selector,
    // assert host.folder_navigation.current == child, click "folder-back",
    // assert current == root.
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p clarion-shell list_and_grid_render -- --nocapture
cargo test -p clarion-shell clicking_a_subfolder -- --nocapture
```

Expected: `open_panel` lacks the new variant or debug bounds are missing.

- [ ] **Step 3: Render list/grid content**

Create `clarion-shell/src/view/dock_folder.rs` with:

```rust
use super::*;
use crate::dock_folder::{FolderEntry, FolderEntryKind};

pub(super) fn folder_entries(host: &ViewState, root: &Path) -> &[FolderEntry] {
    host.dock_folders
        .iter()
        .find(|folder| folder.path == root)
        .and_then(|folder| folder.snapshot.as_ref())
        .map(|snapshot| snapshot.entries.as_slice())
        .unwrap_or(&[])
}

pub(super) fn folder_header(host: &ViewState, p: Palette) -> AnyElement {
    let nav = host.folder_navigation.as_ref();
    let current = nav.map(|nav| nav.current.clone());
    let can_back = nav.is_some_and(|nav| !nav.history.is_empty());
    let title = current
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "文件夹".into());
    let mut row = div().flex().items_center().gap_2().min_w_0();
    if can_back {
        let h = self.host.clone();
        row = row.child(self.glyph_button("folder-back", "chevron-left", Action::FolderBack, p));
    }
    row = row
        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(title));
    if let Some(current) = current {
        let h = self.host.clone();
        row = row.child(self.glyph_button("folder-reveal", "folder", Action::RevealFolder(current), p));
    }
    row.into_any_element()
}
```

The snippet uses `self` in free functions; make these methods on `ShellView` instead:

```rust
impl ShellView {
    pub(super) fn folder_header(&self, host: &ViewState, p: Palette) -> AnyElement { ... }
    pub(super) fn folder_list(&self, host: &ViewState, p: Palette) -> AnyElement { ... }
    pub(super) fn folder_grid(&self, host: &ViewState, p: Palette) -> AnyElement { ... }
}
```

Use `self.glyph_button`, `self.icon`, and `self.host` exactly like `dock_menu`.

List row:

```rust
let h = self.host.clone();
let row = div()
    .id(SharedString::from(format!("folder-entry-{index}")))
    .debug_selector(move || format!("folder-entry-{index}"))
    .flex()
    .items_center()
    .gap_2()
    .h(px(34.))
    .px(px(6.))
    .rounded(px(5.))
    .cursor_pointer()
    .hover(move |s| s.bg(p.line))
    .child(crate::desktop_icons::icon(entry_icon(entry.kind), 24.))
    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(entry.name.clone()))
    .on_click(move |_, _, cx| {
        let path = entry.path.clone();
        h.update(cx, |host, cx| {
            if entry.kind == FolderEntryKind::Directory {
                host.handle(Action::NavigateFolder(path.clone()), cx);
            } else {
                host.handle(Action::Backend(backend::Command::OpenPath(path.clone())), cx);
                host.close(cx);
            }
        });
    })
    .on_mouse_down(MouseButton::Right, {
        let h = self.host.clone();
        let path = entry.path.clone();
        let reveal = if entry.kind == FolderEntryKind::Directory {
            path.clone()
        } else {
            path.parent().unwrap_or(&path).to_path_buf()
        };
        move |_, _, cx| {
            h.update(cx, |host, cx| host.handle(Action::RevealFolder(reveal.clone()), cx));
        }
    });
```

Single click navigates into a subfolder. Right-click reveals it externally. The `folder-header` reveal button covers the current folder. Do not use double-click for navigation.

Body:

```rust
div()
    .id("folder-scroll")
    .flex_1()
    .min_h_0()
    .overflow_y_scroll()
    .flex()
    .flex_col()
    .gap_1()
    .children(rows)
```

Footer:

```rust
let h = self.host.clone();
let current = host.folder_navigation.as_ref().map(|nav| nav.current.clone());
div().flex().items_center().justify_between().child(
    self.button("folder-reveal-footer", "在文件管理器中显示", Action::RevealFolder(current.unwrap_or_default()), p)
)
```

Empty/error states use the existing `self.empty` helper.

Grid: 4 columns, each cell 84x84, icon 40, name one line. Use the same click/right-click handlers. Wrap in a scroll container.

- [ ] **Step 4: Wire `Panel::DockFolder` into `panel()`**

In `panel()`:

```rust
if modal && matches!(&panel, Panel::DockFolder { .. }) {
    let root = match &panel { Panel::DockFolder { root } => root.clone(), _ => unreachable!() };
    let layout = resolved_folder_layout(host, &root, width, height);
    let content = match layout {
        FolderLayout::List => self.folder_list(host, p),
        FolderLayout::Grid => self.folder_grid(host, p),
        FolderLayout::Fan => self.folder_fan(host, &root, width, height, p),
        FolderLayout::Auto => unreachable!("auto is resolved before rendering"),
    };
    // For list/grid, use the existing arrow popup branch with `content`.
    // For fan, return the transparent overlay branch (Task 10).
}
```

Keep `placement.popup` for list/grid. For fan, `placement.popup` is `None`; render a full-screen `relative` overlay with a transparent backdrop and the fan content.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p clarion-shell list_and_grid_render -- --nocapture
cargo test -p clarion-shell clicking_a_subfolder -- --nocapture
cargo test -p clarion-shell view::dock_folder_tests -- --nocapture
```

Expected: list/grid entries render, long names stay one line, navigation works, Esc/outside click paths still work.

- [ ] **Step 6: Commit**

```bash
git add clarion-shell/src/view/dock_folder.rs clarion-shell/src/view/dock_folder_tests.rs clarion-shell/src/view.rs clarion-shell/src/app.rs
git commit -m "feat: render dock folder list and grid popups"
```

---

### Task 10: Fan geometry, rendering, and scrolling

**Files:**
- Modify: `clarion-shell/src/view/dock_folder.rs`
- Test: `clarion-shell/src/view/dock_folder_tests.rs`

**Interfaces:**
- Produces:
  - `pub(super) struct FanItem { index: usize, angle: f32, icon: Bounds<Pixels>, label: Bounds<Pixels> }`
  - `pub(super) fn fan_geometry(anchor: Point<Pixels>, position: DockPosition, count: usize, visible: usize, offset: usize, icon: f32, viewport: Size<Pixels>) -> (Vec<FanItem>, Bounds<Pixels>)`
  - `pub(super) fn fan_capacity(anchor: Point<Pixels>, position: DockPosition, icon: f32, viewport: Size<Pixels>) -> usize`

- [ ] **Step 1: Write failing geometry tests**

```rust
#[test]
fn fan_items_stay_inside_every_output_edge() {
    let viewport = size(px(1440.), px(900.));
    for position in [DockPosition::Bottom, DockPosition::Left, DockPosition::Right] {
        let anchor = match position {
            DockPosition::Bottom => point(px(720.), px(860.)),
            DockPosition::Left => point(px(40.), px(450.)),
            DockPosition::Right => point(px(1400.), px(450.)),
        };
        let (items, bounds) = fan_geometry(anchor, position, 8, 8, 0, 48., viewport);
        assert_eq!(items.len(), 8);
        assert!(bounds.left() >= px(0.) && bounds.top() >= px(0.));
        assert!(bounds.right() <= viewport.width && bounds.bottom() <= viewport.height);
        for item in items {
            assert!(item.icon.left() >= px(0.) && item.icon.right() <= viewport.width);
            assert!(item.label.left() >= px(0.) && item.label.right() <= viewport.width);
        }
    }
}

#[test]
fn fan_window_scrolls_only_when_there_are_more_entries() {
    let viewport = size(px(1440.), px(900.));
    let (items, _) = fan_geometry(point(px(720.), px(860.)), DockPosition::Bottom, 24, 8, 12, 48., viewport);
    assert_eq!(items.first().unwrap().index, 12);
    assert_eq!(items.last().unwrap().index, 19);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p clarion-shell fan_items_stay -- --nocapture
cargo test -p clarion-shell fan_window_scrolls -- --nocapture
```

Expected: missing `fan_geometry`.

- [ ] **Step 3: Implement fan geometry and rendering**

Fan points:

- Bottom: center = anchor + (0, -radius), angles from 200° to 340° measured from +x; each item at radius, pointing upward.
- Left: center = anchor + (radius, 0), angles from 290° to 70°; items open to the right.
- Right: center = anchor + (-radius, 0), angles from 110° to 250°; items open to the left.

Use `radius = (icon * 3.2).clamp(120., 240.)`, `span = 150_f32.to_radians()`, and calculate capacity from arc length:

```rust
let arc = radius * span;
let capacity = ((arc / (icon + 18.)).floor() as usize).clamp(1, 8);
```

Clamp radius down until the computed bounding box fits the viewport with a 16px margin. Return `(Vec<FanItem>, bounds)` where `bounds` encloses all icons and labels.

Rendering:

- A full-screen `relative` container with a transparent backdrop (`bg(rgba(0x00000001))`) and outside-click close.
- A fixed `folder fan` child positioned absolutely; each item is a `div` with `.absolute().left(...).top(...)` containing the icon and a capsule label. Use `overflow_hidden().whitespace_nowrap().text_ellipsis()` on labels.
- `on_scroll_wheel` updates a `fan_offset` field on `ShellView` (add `folder_fan_offset: usize`) clamped to `count.saturating_sub(visible)`. Call `cx.notify()`.
- Fixed "返回" and "在文件管理器中显示" capsules are placed at the arc ends; they are not part of the scroll window.
- Fan has no `set_glass_region`; the Render code must explicitly set `window.set_glass_region(None)` for this panel.

- [ ] **Step 4: Run the fan tests**

```bash
cargo test -p clarion-shell fan_items_stay -- --nocapture
cargo test -p clarion-shell fan_window_scrolls -- --nocapture
cargo test -p clarion-shell view::dock_folder_tests -- --nocapture
```

Expected: geometry tests pass and list/grid tests still pass.

- [ ] **Step 5: Commit**

```bash
git add clarion-shell/src/view/dock_folder.rs clarion-shell/src/view/dock_folder_tests.rs clarion-shell/src/view.rs
git commit -m "feat: add dock folder fan layout"
```

---

### Task 11: Folder context menu and configuration actions

**Files:**
- Modify: `clarion-shell/src/view/dock_folder.rs`
- Modify: `clarion-shell/src/view.rs`
- Modify: `clarion-shell/src/app/panels.rs`
- Test: `clarion-shell/src/view/dock_folder_tests.rs`

**Interfaces:**
- Consumes: `Panel::DockFolderMenu`, `Action::SetFolder*`, `Action::UnpinFolder`.
- Produces: selectors `folder-display-stack`, `folder-display-folder`, `folder-layout-list`, `folder-layout-grid`, `folder-layout-fan`, `folder-layout-auto`, `folder-sort-name`, `folder-sort-modified`, `folder-sort-kind`, `folder-sort-size`, `folder-direction`, `folder-remove`.

- [ ] **Step 1: Write failing menu tests**

```rust
#[gpui::test]
fn folder_menu_changes_display_layout_and_sort(cx: &mut TestAppContext) {
    // Pin a folder, open Panel::DockFolderMenu, click folder-layout-grid,
    // assert preferences layout == Grid, then click folder-sort-size and
    // folder-direction, assert sort == Size and descending == true.
}

#[gpui::test]
fn folder_menu_remove_unpins_without_deleting_the_directory(cx: &mut TestAppContext) {
    // Pin a folder, open the menu, click folder-remove, assert prefs empty
    // and directory still exists.
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p clarion-shell folder_menu_changes -- --nocapture
cargo test -p clarion-shell folder_menu_remove -- --nocapture
```

Expected: menu selectors missing.

- [ ] **Step 3: Implement the menu panel**

Add `Panel::DockFolderMenu { path }` placement using the same `folder_tile_rect`, width 360, arrow to the folder tile. Content:

```rust
let folder = host.dock_folders.iter().find(|folder| folder.path == path);
let descending = folder.is_some_and(|folder| folder.descending);
let mut rows = div().flex().flex_col().gap_2();
rows = rows
    .child(section_label("文件夹", p))
    .child(self.button("folder-open", "打开文件夹", Action::OpenFolder(path.to_path_buf()), p))
    .child(self.button("folder-reveal-menu", "在文件管理器中显示", Action::RevealFolder(path.to_path_buf()), p))
    .child(self.button("folder-refresh", "刷新", Action::RefreshFolder(path.to_path_buf()), p))
    .child(section_label("显示", p))
    .child(self.choice_row(
        vec![
            ("folder-display-stack", "图标叠加", Action::SetFolderDisplay(path.to_path_buf(), FolderDisplay::Stack), folder.is_some_and(|folder| folder.display == FolderDisplay::Stack)),
            ("folder-display-folder", "文件夹", Action::SetFolderDisplay(path.to_path_buf(), FolderDisplay::Folder), folder.is_some_and(|folder| folder.display == FolderDisplay::Folder)),
        ],
        p,
    ))
    .child(section_label("布局", p))
    .child(self.choice_row(
        vec![
            ("folder-layout-list", "列表", Action::SetFolderLayout(path.to_path_buf(), FolderLayout::List), folder.is_some_and(|folder| folder.layout == FolderLayout::List)),
            ("folder-layout-grid", "网格", Action::SetFolderLayout(path.to_path_buf(), FolderLayout::Grid), folder.is_some_and(|folder| folder.layout == FolderLayout::Grid)),
            ("folder-layout-fan", "扇形", Action::SetFolderLayout(path.to_path_buf(), FolderLayout::Fan), folder.is_some_and(|folder| folder.layout == FolderLayout::Fan)),
            ("folder-layout-auto", "自动", Action::SetFolderLayout(path.to_path_buf(), FolderLayout::Auto), folder.is_some_and(|folder| folder.layout == FolderLayout::Auto)),
        ],
        p,
    ))
    .child(section_label("排序", p))
    .child(self.choice_row(
        vec![
            ("folder-sort-name", "名称", Action::SetFolderSort(path.to_path_buf(), FolderSort::Name), folder.is_some_and(|folder| folder.sort == FolderSort::Name)),
            ("folder-sort-modified", "修改日期", Action::SetFolderSort(path.to_path_buf(), FolderSort::Modified), folder.is_some_and(|folder| folder.sort == FolderSort::Modified)),
            ("folder-sort-kind", "类型", Action::SetFolderSort(path.to_path_buf(), FolderSort::Kind), folder.is_some_and(|folder| folder.sort == FolderSort::Kind)),
            ("folder-sort-size", "大小", Action::SetFolderSort(path.to_path_buf(), FolderSort::Size), folder.is_some_and(|folder| folder.sort == FolderSort::Size)),
        ],
        p,
    ))
    .child(self.choice_row(
        vec![(
            "folder-direction",
            if descending { "降序 ↓" } else { "升序 ↑" },
            Action::SetFolderDescending(path.to_path_buf(), !descending),
            false,
        )],
        p,
    ))
    .child(self.button("folder-remove", "从 Dock 移除", Action::UnpinFolder(path.to_path_buf()), p));
```

`choice_row` is a flex row with equal-width `choice_button` children. Add:

```rust
fn section_label(label: &'static str, p: Palette) -> AnyElement {
    div()
        .text_size(px(11.))
        .text_color(p.muted)
        .child(label)
        .into_any_element()
}

fn choice_row(
    &self,
    choices: Vec<(&'static str, &'static str, Action, bool)>,
    p: Palette,
) -> AnyElement {
    let mut row = div().flex().gap_1();
    for (id, label, action, selected) in choices {
        row = row.child(self.choice_button(id, label, action, selected, p));
    }
    row.into_any_element()
}

fn choice_button(
    &self,
    id: &'static str,
    label: &'static str,
    action: Action,
    selected: bool,
    p: Palette,
) -> AnyElement {
    let host = self.host.clone();
    div()
        .id(id)
        .debug_selector(move || SharedString::from(id))
        .flex_1()
        .min_w_0()
        .px(px(6.))
        .py(px(5.))
        .rounded(px(5.))
        .text_size(px(12.))
        .text_center()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .when(selected, |s| s.bg(p.accent).text_color(rgb(0xffffffff)))
        .when(!selected, |s| s.bg(p.card).hover(move |s| s.bg(p.line)))
        .cursor_pointer()
        .child(label)
        .on_click(move |_, _, cx| host.update(cx, |host, cx| host.handle(action.clone(), cx)))
        .into_any_element()
}
```

Add `Action::RefreshFolder(PathBuf)` and handle it with `request_folder_scan`.

- [ ] **Step 4: Run the menu tests**

```bash
cargo test -p clarion-shell folder_menu_changes -- --nocapture
cargo test -p clarion-shell folder_menu_remove -- --nocapture
cargo test -p clarion-shell view::dock_folder_tests -- --nocapture
```

Expected: menu changes persist and removal leaves the directory intact.

- [ ] **Step 5: Commit**

```bash
git add clarion-shell/src/view/dock_folder.rs clarion-shell/src/view.rs clarion-shell/src/app/panels.rs clarion-shell/src/app.rs clarion-shell/src/view/dock_folder_tests.rs
git commit -m "feat: configure dock folders from the context menu"
```

---

### Task 12: Parse application `MimeType`

**Files:**
- Modify: `clarion-launcher/src/catalog.rs`
- Test: inline `#[cfg(test)] mod tests` in `clarion-launcher/src/catalog.rs`

**Interfaces:**
- Produces: `DesktopApp.mime_types: Vec<String>`.

- [ ] **Step 1: Write the failing catalog test**

Add:

```rust
#[test]
fn desktop_entries_parse_deduplicate_and_normalize_mime_types() {
    let root = tempfile::tempdir().unwrap();
    let applications = root.path().join("applications");
    std::fs::create_dir_all(&applications).unwrap();
    std::fs::write(
        applications.join("viewer.desktop"),
        "[Desktop Entry]\nType=Application\nName=Viewer\nDBusActivatable=true\nMimeType=text/plain; image/png;TEXT/PLAIN;;\n",
    )
    .unwrap();
    let env = [
        ("XDG_DATA_HOME".into(), root.path().to_string_lossy().into_owned()),
        ("XDG_DATA_DIRS".into(), root.path().join("none").to_string_lossy().into_owned()),
        ("HOME".into(), root.path().to_string_lossy().into_owned()),
        ("XDG_CURRENT_DESKTOP".into(), "clarion".into()),
    ]
    .into();
    let catalog = AppCatalog::load(&env).unwrap();
    let app = catalog.get("viewer.desktop").unwrap();
    assert_eq!(app.mime_types, ["text/plain", "image/png"]);
}

#[test]
fn missing_mime_type_is_an_empty_list() {
    // Load an entry without MimeType and assert mime_types.is_empty().
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p clarion-launcher desktop_entries_parse -- --nocapture
```

Expected: compile error because `mime_types` does not exist.

- [ ] **Step 3: Implement parsing**

Add to `DesktopApp`:

```rust
pub mime_types: Vec<String>,
```

In `AppCatalog::load`, after `categories`:

```rust
let mime_types = string_list(
    values
        .get("MimeType")
        .map(String::as_str)
        .unwrap_or_default(),
)
.into_iter()
.map(|mime| mime.to_ascii_lowercase())
.filter(|mime| {
    !mime.is_empty()
        && mime.len() <= 256
        && mime.contains('/')
        && !mime.chars().any(char::is_whitespace)
})
.collect::<Vec<_>>();
let mut seen_mimes = HashSet::new();
let mime_types = mime_types
    .into_iter()
    .filter(|mime| seen_mimes.insert(mime.clone()))
    .take(128)
    .collect();
```

Set `mime_types` in the `DesktopApp` initializer.

- [ ] **Step 4: Run the catalog tests**

```bash
cargo test -p clarion-launcher desktop_entries_parse -- --nocapture
cargo test -p clarion-launcher catalog -- --nocapture
```

Expected: new and existing catalog tests pass.

- [ ] **Step 5: Commit**

```bash
git add clarion-launcher/src/catalog.rs
git commit -m "feat: parse application mime types"
```

---

### Task 13: MIME matching for dropped paths

**Files:**
- Create: `clarion-shell/src/mime.rs`
- Modify: `clarion-shell/src/main.rs` or module root
- Test: inline tests in `clarion-shell/src/mime.rs`

**Interfaces:**
- Produces:
  - `pub fn path_mime(path: &Path) -> Option<String>`
  - `pub fn app_supports_mime(app: &DesktopApp, mime: &str) -> bool`
  - `pub fn app_accepts_paths(app: &DesktopApp, paths: &[PathBuf]) -> bool`

- [ ] **Step 1: Write failing MIME tests**

```rust
#[test]
fn directories_and_extensions_match_exact_wildcard_and_all_mime_types() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("folder");
    std::fs::create_dir(&dir).unwrap();
    let file = root.path().join("photo.PNG");
    std::fs::write(&file, b"png").unwrap();
    assert_eq!(path_mime(&dir).as_deref(), Some("inode/directory"));
    assert_eq!(path_mime(&file).as_deref(), Some("image/png"));

    let mut app = test_app(&["inode/directory", "image/*"]);
    assert!(app_accepts_paths(&app, &[dir.clone(), file.clone()]));

    app.mime_types = vec!["text/*".into()];
    assert!(!app_accepts_paths(&app, &[dir.clone(), file.clone()]));

    app.mime_types = vec!["*/*".into()];
    assert!(app_accepts_paths(&app, &[file]));
}

#[test]
fn a_mixed_or_empty_drop_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let text = root.path().join("a.txt");
    let image = root.path().join("a.png");
    std::fs::write(&text, b"x").unwrap();
    std::fs::write(&image, b"x").unwrap();
    let app = test_app(&["text/plain"]);
    assert!(!app_accepts_paths(&app, &[text.clone(), image]));
    assert!(!app_accepts_paths(&app, &[]));
    assert!(!app_accepts_paths(&app, &[root.path().join("missing.txt")]));
}
```

Implement `test_app` in the test module with a `DesktopApp` containing empty fields and the given MIME list.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p clarion-shell mime::tests -- --nocapture
```

Expected: module missing.

- [ ] **Step 3: Implement the module**

```rust
use clarion_launcher::catalog::DesktopApp;
use std::fs;
use std::path::{Path, PathBuf};

pub fn path_mime(path: &Path) -> Option<String> {
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return None;
    }
    if fs::metadata(path).ok().is_some_and(|metadata| metadata.is_dir()) {
        return Some("inode/directory".into());
    }
    Some(
        mime_guess::from_path(path)
            .first_raw()
            .unwrap_or("application/octet-stream")
            .to_ascii_lowercase(),
    )
}

pub fn app_supports_mime(app: &DesktopApp, mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    let (kind, _) = mime.split_once('/').unwrap_or((mime.as_str(), ""));
    app.mime_types.iter().any(|supported| {
        let supported = supported.to_ascii_lowercase();
        supported == mime
            || supported == "*/*"
            || (supported.ends_with("/*")
                && supported.strip_suffix("/*").is_some_and(|prefix| prefix == kind))
    })
}

pub fn app_accepts_paths(app: &DesktopApp, paths: &[PathBuf]) -> bool {
    if paths.is_empty() || paths.len() > 16 {
        return false;
    }
    paths.iter().all(|path| {
        path.is_absolute()
            && !path.as_os_str().as_encoded_bytes().contains(&0)
            && path.exists()
            && path_mime(path)
                .as_deref()
                .is_some_and(|mime| app_supports_mime(app, mime))
    })
}
```

Add `mod mime;` to the module root.

- [ ] **Step 4: Run the MIME tests**

```bash
cargo test -p clarion-shell mime::tests -- --nocapture
```

Expected: all MIME tests pass.

- [ ] **Step 5: Commit**

```bash
git add clarion-shell/src/main.rs clarion-shell/src/mime.rs
git commit -m "feat: match dropped paths against app mime types"
```

---

### Task 14: Launch target app with file paths

**Files:**
- Modify: `clarion-launcher/src/launch.rs`
- Modify: `clarion-shell/src/backend.rs`
- Modify: `clarion-shell/src/app.rs` (`Action::OpenPathsWithApp`)
- Test: `clarion-launcher/src/launch.rs` tests and `clarion-shell/src/app/dock_folder_tests.rs`

**Interfaces:**
- Consumes: `app_accepts_paths`, `DesktopApp.mime_types`.
- Produces:
  - `pub fn launch_desktop_file_with_paths(path: &Path, files: &[PathBuf], env: &HashMap<String, String>) -> Result<()>`
  - `backend::Command::LaunchDesktopWithPaths(PathBuf, Vec<PathBuf>)`
  - `Action::OpenPathsWithApp { app_id: String, paths: Vec<PathBuf> }`

- [ ] **Step 1: Write failing launch argument tests**

Add to `launch.rs` tests:

```rust
#[test]
fn launch_with_paths_appends_each_file_after_the_desktop_file() {
    let mut runner = FakeRunner::default();
    let env: HashMap<_, _> = [(
        "XDG_RUNTIME_DIR".into(),
        tempfile::tempdir().unwrap().path().to_string_lossy().into_owned(),
    )]
    .into();
    launch_desktop_file_with_paths(
        Path::new("/apps/viewer.desktop"),
        &[PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.png")],
        &env,
        &mut runner,
    )
    .unwrap();
    let args = &runner.launches.last().unwrap().1;
    assert_eq!(
        &args[args.len() - 4..],
        &[
            OsString::from("/apps/viewer.desktop"),
            OsString::from("/tmp/a.txt"),
            OsString::from("/tmp/b.png"),
        ]
    );
}
```

The exact systemd wrapping depends on `probe_scope`; use the no-manager environment used by existing tests so the launch is a direct `gio` call, and assert the full `["launch", desktop, a, b]` vector.

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p clarion-launcher launch_with_paths -- --nocapture
```

Expected: compile error for missing function.

- [ ] **Step 3: Implement launch and backend plumbing**

In `launch.rs`:

```rust
fn launch_with_runner(
    path: &Path,
    files: &[PathBuf],
    env: &HashMap<String, String>,
    runner: &mut impl CommandRunner,
) -> Result<()> {
    let scope = probe_scope(env, runner)?;
    let mut arguments = vec![
        OsString::from("launch"),
        path.as_os_str().to_owned(),
    ];
    arguments.extend(files.iter().map(|file| file.as_os_str().to_owned()));
    match scope {
        Some(scope) => runner.launch("systemd-run", &scope.arguments("gio", &arguments), env),
        None => runner.launch("gio", &arguments, env),
    }
}

pub fn launch_desktop_file(path: &Path, env: &HashMap<String, String>) -> Result<()> {
    launch_with_runner(path, &[], env, &mut SystemRunner)
}

pub fn launch_desktop_file_with_paths(
    path: &Path,
    files: &[PathBuf],
    env: &HashMap<String, String>,
) -> Result<()> {
    anyhow::ensure!(
        files.len() <= 16
            && files.iter().all(|file| file.is_absolute()
                && !file.as_os_str().as_bytes().contains(&0)),
        "Invalid file arguments"
    );
    launch_with_runner(path, files, env, &mut SystemRunner)
}
```

Import `PathBuf` in `launch.rs`. Update existing tests to pass `&[]` to `launch_with_runner`.

In `backend.rs`, add the command and handler:

```rust
LaunchDesktopWithPaths(PathBuf, Vec<PathBuf>),
```

```rust
Command::LaunchDesktopWithPaths(path, files) => {
    anyhow::ensure!(
        path.is_absolute() && path.extension().is_some_and(|ext| ext == "desktop"),
        "Invalid desktop entry path"
    );
    clarion_launcher::launch::launch_desktop_file_with_paths(
        &path,
        &files,
        &options.environment,
    )?;
}
```

In `Host::handle`:

```rust
Action::OpenPathsWithApp { app_id, paths } => {
    let Some(app) = self.catalog.get(&app_id).cloned() else { return; };
    if !crate::mime::app_accepts_paths(&app, &paths) {
        return;
    }
    self.auto_hidden = false;
    let _ = self.backend.send(backend::Command::LaunchDesktopWithPaths(
        app.desktop_file.clone(),
        paths,
    ));
    self.prefs.recent_app_ids.retain(|other| other != &app_id);
    self.prefs.recent_app_ids.insert(0, app_id);
    self.prefs.recent_app_ids.truncate(24);
    self.save(cx);
    self.close(cx);
}
```

Add a Host test:

```rust
#[gpui::test]
fn open_paths_with_app_rejects_mixed_unsupported_paths(cx: &mut TestAppContext) {
    // Build a catalog entry with MimeType=text/plain, drop a .txt and a .png,
    // assert the captured backend receiver has no LaunchDesktopWithPaths.
}
```

Use the existing `testing::capture_backend` helper and a fixture catalog with a `MimeType` entry.

- [ ] **Step 4: Run the launch and action tests**

```bash
cargo test -p clarion-launcher launch_ -- --nocapture
cargo test -p clarion-shell open_paths_with_app -- --nocapture
cargo test -p clarion-shell backend -- --nocapture
```

Expected: all launch and action tests pass; existing one-argument launches still work.

- [ ] **Step 5: Commit**

```bash
git add clarion-launcher/src/launch.rs clarion-shell/src/backend.rs clarion-shell/src/app.rs clarion-shell/src/app/dock_folder_tests.rs
git commit -m "feat: launch a target app with dropped files"
```

---

### Task 15: Drag folder entries onto Dock app tiles

**Files:**
- Modify: `clarion-shell/src/view/dock_folder.rs`
- Modify: `clarion-shell/src/view.rs`
- Test: `clarion-shell/src/view/dock_folder_tests.rs`

**Interfaces:**
- Consumes: `Action::OpenPathsWithApp`, `app_accepts_paths`, `dock_tile_center`.
- Produces: `FolderEntryDrag { path: PathBuf }`; debug selectors `folder-drop-app-<index>`.

- [ ] **Step 1: Write failing drag/drop test**

```rust
#[gpui::test]
fn a_folder_entry_can_be_dropped_on_a_compatible_dock_app(cx: &mut TestAppContext) {
    // Create a .txt in a pinned folder, app alpha has MimeType=text/plain,
    // open the folder popup, start a drag on folder-entry-0, move over
    // folder-drop-app-0, release, assert the captured backend command is
    // LaunchDesktopWithPaths with the .txt path.
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p clarion-shell a_folder_entry_can_be_dropped -- --nocapture
```

Expected: no drop zone or no command.

- [ ] **Step 3: Implement drag and overlay drop zones**

Add:

```rust
#[derive(Clone, Debug)]
pub(super) struct FolderEntryDrag {
    pub path: PathBuf,
}
```

On each entry add:

```rust
.on_drag(
    FolderEntryDrag { path: entry.path.clone() },
    |drag, _offset, _window, cx| {
        cx.new(|_| FolderDragPreview { path: drag.path.clone() })
    },
)
```

Add the preview view:

```rust
struct FolderDragPreview {
    path: PathBuf,
}

impl Render for FolderDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned());
        div()
            .flex()
            .items_center()
            .gap_1()
            .px(px(8.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(rgba(0x2c2c31e6))
            .text_color(rgb(0xffffffff))
            .shadow(vec![BoxShadow {
                color: rgba(0x0000002e).into(),
                offset: point(px(0.), px(4.)),
                blur_radius: px(12.),
                spread_radius: px(0.),
            }])
            .child(crate::desktop_icons::icon(
                crate::desktop_icons::DesktopIconKind::Document,
                28.,
            ))
            .child(
                div()
                    .max_w(px(180.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(name),
            )
    }
}
```

In the folder popup render, before the popup body, add app drop zones:

```rust
let metrics = crate::app::panels::dock_metrics(host, &host.dock_apps(), width, height);
let launcher = usize::from(host.prefs.show_launcher);
let display_apps = crate::app::dock_display_apps(&host.dock_apps(), &metrics);
for (index, app) in display_apps.iter().enumerate() {
    let (cx_center, cy_center) = crate::app::panels::dock_tile_center(
        host,
        &metrics,
        index + launcher,
        width,
        height,
    );
    let size = metrics.icon_size;
    let host_handle = self.host.clone();
    let app = (*app).clone();
    let drop = div()
        .id(SharedString::from(format!("folder-drop-app-{index}")))
        .debug_selector(move || format!("folder-drop-app-{index}"))
        .absolute()
        .left(px(cx_center - size / 2.))
        .top(px(cy_center - size / 2.))
        .size(px(size))
        .rounded(px(size * 0.28))
        .can_drop(move |value, _, _| {
            value
                .downcast_ref::<FolderEntryDrag>()
                .is_some_and(|drag| crate::mime::app_accepts_paths(&app, std::slice::from_ref(&drag.path)))
        })
        .on_drop::<FolderEntryDrag>(move |drag, _, cx| {
            let paths = vec![drag.path.clone()];
            if crate::mime::app_accepts_paths(&app, &paths) {
                host_handle.update(cx, |host, cx| {
                    host.handle(Action::OpenPathsWithApp {
                        app_id: app.id.clone(),
                        paths,
                    }, cx)
                });
            }
        })
        .drag_over::<FolderEntryDrag>(|style, _, _, _| {
            style.border_2().border_color(rgba(0x0a84ffff))
        });
    overlay = overlay.child(drop);
}
```

Render the drop zones before the popup content so they are under the folder rows and do not intercept normal clicks outside their icon rectangles. When not dragging, a click in a dock icon area dismisses the popup as before.

- [ ] **Step 4: Run the drag test and all folder tests**

```bash
cargo test -p clarion-shell a_folder_entry_can_be_dropped -- --nocapture
cargo test -p clarion-shell view::dock_folder_tests -- --nocapture
```

Expected: compatible drop launches; incompatible drop does nothing.

- [ ] **Step 5: Commit**

```bash
git add clarion-shell/src/view/dock_folder.rs clarion-shell/src/view.rs clarion-shell/src/view/dock_folder_tests.rs
git commit -m "feat: drag folder entries onto dock apps"
```

---

### Task 16: External file drop onto app tiles

**Files:**
- Modify: `clarion-shell/src/view/macos_dock.rs`
- Modify: `clarion-shell/src/view/dock_tests.rs`
- Test: `clarion-shell/src/view/dock_tests.rs`

**Interfaces:**
- Consumes: `ExternalPaths::new` from Task 7, `app_accepts_paths`, `Action::OpenPathsWithApp`.
- Produces: app tile selectors already include `dock-<app-id>`; drop highlight uses the existing tile.

- [ ] **Step 1: Write failing external app drop tests**

```rust
#[gpui::test]
fn dropping_supported_files_on_an_app_tile_opens_that_app(cx: &mut TestAppContext) {
    // Fixture app alpha with MimeType=text/plain; create a .txt file;
    // FileDropEvent::Entered at alpha tile center with the file, then Submit;
    // assert captured backend command is LaunchDesktopWithPaths.
}

#[gpui::test]
fn dropping_unsupported_files_on_an_app_tile_does_nothing(cx: &mut TestAppContext) {
    // Same but .png against text/plain; assert no LaunchDesktopWithPaths and
    // dock_folders stays empty.
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p clarion-shell dropping_supported_files -- --nocapture
cargo test -p clarion-shell dropping_unsupported_files -- --nocapture
```

Expected: no backend command.

- [ ] **Step 3: Implement app tile drop handling**

Replace the no-op app tile drop listener from Task 7 with:

```rust
let drop_app = app.clone();
let drop_host = self.host.clone();
let highlight_app = app.clone();
tile = tile
    .can_drop(|_, _, _| true)
    .on_drop::<gpui::ExternalPaths>(move |paths, _, cx| {
        let paths = paths.paths().to_vec();
        if !crate::mime::app_accepts_paths(&drop_app, &paths) {
            // Consume the drop so it cannot become a folder pin.
            return;
        }
        drop_host.update(cx, |host, cx| {
            host.handle(Action::OpenPathsWithApp {
                app_id: drop_app.id.clone(),
                paths,
            }, cx)
        });
    })
    .drag_over::<gpui::ExternalPaths>(move |style, paths, _, _| {
        if crate::mime::app_accepts_paths(&highlight_app, paths.paths()) {
            style
                .border_2()
                .border_color(rgba(0x0a84ffff))
                .rounded(px(14.))
        } else {
            style
        }
    });
```

Keep the tray-level folder pin handler from Task 7. Because app tiles consume all `ExternalPaths` drops, unsupported drops cannot bubble to it.

- [ ] **Step 4: Run the drop tests and full Dock tests**

```bash
cargo test -p clarion-shell dropping_supported_files -- --nocapture
cargo test -p clarion-shell dropping_unsupported_files -- --nocapture
cargo test -p clarion-shell view::dock_tests -- --nocapture
```

Expected: supported drops launch; unsupported drops do nothing and do not pin.

- [ ] **Step 5: Commit**

```bash
git add clarion-shell/src/view/macos_dock.rs clarion-shell/src/view/dock_tests.rs
git commit -m "feat: open files by dropping them on dock apps"
```

---

### Task 17: Documentation, visual regression, and full suite

**Files:**
- Modify: `docs/native-shell.md`
- Modify: `docs/macos-downloads-fan.md`
- Test: entire workspace shell suite

- [ ] **Step 1: Document the feature**

Add a section to `docs/native-shell.md`:

```markdown
## Dock 文件夹与文件投放

Dock 文件夹使用独立的 `dock_folders` 顺序，位于 overflow 磁贴与垃圾桶之间。目录内容在后台扫描为最多 256 项的不可变快照；磁贴默认叠层预览，可切换标准文件夹。点击文件夹打开全屏透明面板，支持列表、网格、扇形和自动布局；列表/网格使用三角箭头，扇形从磁贴直接展开，不添加全屏遮罩、模糊或 Dock 挖洞。

文件夹条目可以拖到兼容的 Dock 应用磁贴，由应用打开；外部拖入的文件/文件夹只有目标应用 `MimeType` 全部支持时才通过 `gio launch` 打开。移除文件夹只修改偏好，不删除目录。
```

Update `docs/macos-downloads-fan.md` with a short note that the native implementation follows the design spec and does not change the dormant `show_downloads` behavior.

- [ ] **Step 2: Run all relevant tests**

```bash
cargo test -p clarion-launcher
cargo test -p clarion-shell
cargo build -p clarion-shell
```

Expected:

- `clarion-launcher`: all tests pass.
- `clarion-shell`: all existing + new tests pass.
- `clarion-shell` builds with no new warnings beyond the existing vendored GPUI warning.

- [ ] **Step 3: Manual visual checklist**

Run the shell in preview if available, or use the GPUI tests for geometry and add screenshots if the repository has a visual harness. Check:

- Bottom / left / right Dock folder tiles align with the tray and separator.
- Folder popup arrow aligns with the tile; no one-gap offset.
- Fan opens from the tile on every Dock edge and stays within output bounds.
- Dock is fully visible through the transparent popup; no dim or blur band.
- Long folder/file/window names stay one line.
- Drag highlight appears only on compatible app tiles.

- [ ] **Step 4: Commit**

```bash
git add docs/native-shell.md docs/macos-downloads-fan.md
git commit -m "docs: document dock folders and file drop"
```

---

## Self-Review Notes

- Spec coverage: preferences, scan/sort, cache, dock geometry, tile preview, reorder, folder popup list/grid/fan/auto, navigation, context menu, external folder drop, folder-entry drag, MIME parsing/matching, target-app launch, and visual continuity each have an owning task.
- No `show_downloads` implementation is included, matching the spec's non-goal.
- The plan intentionally keeps `DockMetrics::new` as a zero-folder wrapper so existing call sites and tests do not churn; `with_folders` is the new path.
- App tiles consume all external drops in Task 7/16 so unsupported files never pin folders.
- Long-name behavior is tested at both the window-title row (existing test) and folder entry/tile levels.
- `OpenPathsWithApp` validates MIME support again in `Host`, not only at the drop site, so a stale catalog cannot bypass the all-or-nothing rule.
- Fan geometry is tested for all three Dock edges and scroll windows; auto layout only chooses fan when the full set fits.
