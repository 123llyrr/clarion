// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded, toolkit-independent snapshots of pinned Dock folder contents.

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
    /// Finder-style tags (`user.xdg.tags`), drawn as dots on the artwork.
    pub tags: Vec<String>,
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
    let directory =
        fs::read_dir(path).with_context(|| format!("读取文件夹 {}", path.display()))?;
    let mut entries = Vec::new();
    let mut visited = 0usize;
    let mut truncated = false;
    for item in directory {
        if visited >= MAX_VISITED || entries.len() >= MAX_FOLDER_ENTRIES {
            truncated = true;
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
        } else if entry_path
            .extension()
            .is_some_and(|extension| extension == "desktop")
        {
            FolderEntryKind::Application
        } else if mime_guess::from_path(&entry_path)
            .first_raw()
            .is_some_and(|mime| mime.starts_with("image/"))
        {
            FolderEntryKind::Image
        } else {
            FolderEntryKind::Document
        };
        let name = file_name.to_string_lossy().into_owned();
        let tags = crate::file_icons::tags(&entry_path);
        entries.push(FolderEntry {
            path: entry_path,
            name,
            kind,
            tags,
            modified: metadata.modified().ok(),
            size: if metadata.is_file() {
                metadata.len()
            } else {
                0
            },
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
        let primary = if descending {
            primary.reverse()
        } else {
            primary
        };
        let names = natural_name_cmp(&a.name, &b.name);
        let names = if descending { names.reverse() } else { names };
        primary
            .then(names)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preferences::FolderSort;
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
        let names = snapshot
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["z-dir", "a.txt", "b.txt"]);
        assert!(snapshot
            .entries
            .iter()
            .all(|entry| !entry.name.starts_with('.')));
    }

    #[test]
    fn natural_name_sort_is_case_insensitive_and_numeric() {
        let root = tempfile::tempdir().unwrap();
        for name in ["item10.txt", "Item2.txt", "item1.txt"] {
            fs::write(root.path().join(name), b"x").unwrap();
        }
        let snapshot = scan_folder(root.path(), FolderSort::Name, false).unwrap();
        let names = snapshot
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["item1.txt", "Item2.txt", "item10.txt"]);
        let snapshot = scan_folder(root.path(), FolderSort::Name, true).unwrap();
        let names = snapshot
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["item10.txt", "Item2.txt", "item1.txt"]);
    }

    #[test]
    fn modified_kind_and_size_sorts_are_stable_and_folders_first() {
        let root = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        fs::create_dir(root.path().join("dir")).unwrap();
        touch(
            &root.path().join("small.txt"),
            1,
            now - Duration::from_secs(20),
        );
        touch(&root.path().join("large.bin"), 30, now);
        touch(
            &root.path().join("icon.png"),
            3,
            now - Duration::from_secs(40),
        );
        touch(
            &root.path().join("app.desktop"),
            18,
            now - Duration::from_secs(50),
        );

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
    fn missing_paths_return_errors() {
        let root = tempfile::tempdir().unwrap();
        assert!(scan_folder(&root.path().join("missing"), FolderSort::Name, false).is_err());
    }
}
