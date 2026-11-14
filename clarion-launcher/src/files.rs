// SPDX-License-Identifier: GPL-3.0-or-later
//! The palette's built-in file search.
//!
//! The reference implementation shells out to `fd`; Clarion keeps the walker
//! in-process so the palette has no runtime dependency. A search is bounded
//! by a depth, a visited-entry budget and a result cap, and it never follows
//! symlinks. Hidden entries and the usual build/cache directories are pruned
//! before they can flood the result set.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use walkdir::{DirEntry, WalkDir};

const MAX_DEPTH: usize = 12;
const MAX_VISITED: usize = 80_000;
const MAX_MATCHES: usize = 800;

/// Directory names that hold tool output, package caches or vendored code.
/// A launcher result is about the user's own files, so none of these are
/// walked. Hidden directories (`.git`, `.cargo`, `.cache`, `.venv`, …) are
/// pruned separately by the leading-dot check.
const PRUNED: &[&str] = &[
    "__pycache__",
    "bower_components",
    "build",
    "CMakeFiles",
    "coverage",
    "DerivedData",
    "dist",
    "node_modules",
    "obj",
    "out",
    "Pods",
    "site-packages",
    "target",
    "vendor",
    "venv",
    "zig-cache",
    "zig-out",
];

/// Generated trees below the search root that are too large to walk even
/// though their top-level name is not on the list above. `go/pkg` is the Go
/// module cache and `snap` holds mounted application payloads.
const PRUNED_PATHS: &[&str] = &[
    ".cargo/git",
    ".cargo/registry",
    "go/pkg",
    "go/bin",
    "snap",
];

/// One file or directory the walker matched. The shell renders this directly;
/// the walker stays independent of GPUI and of the file-icon machinery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHit {
    pub path: PathBuf,
    pub name: String,
    pub parent: PathBuf,
    pub is_dir: bool,
    /// Lower-case extension without the dot, empty for directories.
    pub extension: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl FileHit {
    /// The entry's own path with the home prefix shortened to `~`, for the
    /// row's metadata line. Paths outside home stay absolute.
    pub fn compact_path(&self, home: &Path) -> String {
        if home.as_os_str().is_empty() {
            return self.path.display().to_string();
        }
        self.path
            .strip_prefix(home)
            .map(|rest| {
                if rest.as_os_str().is_empty() {
                    "~".to_owned()
                } else {
                    format!("~/{}", rest.display())
                }
            })
            .unwrap_or_else(|_| self.path.display().to_string())
    }

    /// A home-relative parent path for callers that want the folder alone.
    pub fn compact_parent(&self, home: &Path) -> String {
        if home.as_os_str().is_empty() {
            return self.parent.display().to_string();
        }
        self.parent
            .strip_prefix(home)
            .map(|rest| {
                if rest.as_os_str().is_empty() {
                    "~".to_owned()
                } else {
                    format!("~/{}", rest.display())
                }
            })
            .unwrap_or_else(|_| self.parent.display().to_string())
    }
}

/// Search below `root` for entries whose name or path contains every token in
/// `query`. An empty query returns no rows: files mode begins idle instead of
/// walking the whole home directory.
pub fn search(root: &Path, query: &str, limit: usize) -> Vec<FileHit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }
    let tokens: Vec<&str> = query.split_whitespace().collect();
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut visited = 0usize;
    let mut matched: Vec<(u32, FileHit)> = Vec::new();
    let walker = WalkDir::new(root)
        .max_depth(MAX_DEPTH)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| keep_entry(entry, root));

    for entry in walker {
        visited += 1;
        if visited > MAX_VISITED || matched.len() >= MAX_MATCHES {
            break;
        }
        let Ok(entry) = entry else {
            continue;
        };
        if entry.depth() == 0 {
            continue;
        }
        let Some(name) = entry.file_name().to_str() else {
            continue;
        };
        let path = entry.path();
        let name_lower = name.to_lowercase();
        let path_lower = path.to_string_lossy().to_lowercase();
        if !tokens
            .iter()
            .all(|token| name_lower.contains(token) || path_lower.contains(token))
        {
            continue;
        }
        let is_dir = entry.file_type().is_dir();
        let score = score(&name_lower, &path_lower, &query, &tokens, entry.depth());
        let metadata = entry.metadata().ok();
        matched.push((
            score,
            FileHit {
                path: path.to_path_buf(),
                name: name.to_owned(),
                parent: path.parent().unwrap_or(root).to_path_buf(),
                is_dir,
                extension: if is_dir {
                    String::new()
                } else {
                    path.extension()
                        .and_then(|extension| extension.to_str())
                        .map(str::to_lowercase)
                        .unwrap_or_default()
                },
                // A directory's own inode size is not the tree size and only
                // confuses the metadata line.
                size: if is_dir {
                    0
                } else {
                    metadata.as_ref().map_or(0, |metadata| metadata.len())
                },
                modified: metadata.and_then(|metadata| metadata.modified().ok()),
            },
        ));
    }

    matched.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.path.cmp(&right.path))
    });
    matched
        .into_iter()
        .take(limit)
        .map(|(_, hit)| hit)
        .collect()
}

fn keep_entry(entry: &DirEntry, root: &Path) -> bool {
    if entry.depth() == 0 || entry.path() == root {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    if name.starts_with('.') {
        return false;
    }
    if entry.file_type().is_dir() {
        if PRUNED.iter().any(|pruned| name.eq_ignore_ascii_case(pruned)) {
            return false;
        }
        // A few generated trees are only recognisable by where they sit under
        // the home directory, not by their own name.
        let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
        if PRUNED_PATHS
            .iter()
            .any(|pruned| relative.starts_with(pruned))
        {
            return false;
        }
    }
    true
}

fn score(name: &str, path: &str, query: &str, tokens: &[&str], depth: usize) -> u32 {
    let mut score: u32 = if name == query {
        1_000
    } else if name.starts_with(query) {
        800
    } else if name
        .split(|character: char| !character.is_alphanumeric())
        .any(|word| !word.is_empty() && word.starts_with(query))
    {
        650
    } else if name.contains(query) {
        500
    } else if path.contains(query) {
        300
    } else {
        200
    };
    for token in tokens {
        if name.starts_with(token) {
            score += 70;
        } else if name.contains(token) {
            score += 45;
        } else if path.contains(token) {
            score += 10;
        }
    }
    // Shorter names are easier to scan when two rows match equally, and a
    // shallow match beats the same name buried under many folders.
    score = score.saturating_sub((name.chars().count() as u32).min(120));
    score.saturating_sub((depth as u32).saturating_mul(8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(root: &Path, path: &str) -> PathBuf {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, b"x").unwrap();
        path
    }

    #[test]
    fn an_empty_query_does_not_walk_or_return_the_home_directory() {
        let root = tempfile::tempdir().unwrap();
        touch(root.path(), "notes.txt");
        assert!(search(root.path(), "", 50).is_empty());
        assert!(search(root.path(), "   ", 50).is_empty());
    }

    #[test]
    fn names_rank_above_deeper_path_matches_and_results_keep_the_limit() {
        let root = tempfile::tempdir().unwrap();
        let exact = touch(root.path(), "report.txt");
        let prefix = touch(root.path(), "report-final.txt");
        let path_hit = touch(root.path(), "archive/2024/report.txt");
        touch(root.path(), "archive/2024/notes.txt");
        let hits = search(root.path(), "report", 10);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].path, exact);
        assert_eq!(hits[1].path, prefix);
        assert!(hits.iter().any(|hit| hit.path == path_hit));

        let hits = search(root.path(), "report", 2);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn hidden_and_build_directories_are_pruned() {
        let root = tempfile::tempdir().unwrap();
        touch(root.path(), "visible.txt");
        touch(root.path(), ".hidden.txt");
        touch(root.path(), ".git/config");
        touch(root.path(), "target/debug/visible.txt");
        touch(root.path(), "node_modules/pkg/visible.txt");
        touch(root.path(), "go/pkg/mod/visible.txt");
        touch(root.path(), "snap/app/visible.txt");
        touch(root.path(), "out/visible.txt");
        touch(root.path(), "DerivedData/Build/visible.txt");
        touch(root.path(), "Pods/visible.txt");
        let hits = search(root.path(), "visible", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "visible.txt");
    }

    #[test]
    fn every_token_must_match_and_directories_carry_no_extension() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("Project Alpha");
        std::fs::create_dir_all(directory.join("docs")).unwrap();
        let file = touch(&directory, "docs/readme.md");
        let hits = search(root.path(), "project readme", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, file);
        assert_eq!(hits[0].extension, "md");
        assert!(!hits[0].is_dir);

        let hits = search(root.path(), "project alpha", 10);
        assert!(hits.iter().any(|hit| hit.path == directory && hit.is_dir));
        let directory = hits.iter().find(|hit| hit.is_dir).unwrap();
        assert!(directory.extension.is_empty());
        assert_eq!(directory.size, 0);
    }

    #[test]
    fn a_folder_keyword_returns_the_folder_and_the_files_inside_it() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("Invoices 2026");
        std::fs::create_dir_all(&folder).unwrap();
        let inside = touch(root.path(), "Invoices 2026/march.pdf");
        touch(root.path(), "elsewhere/march.pdf");

        let hits = search(root.path(), "invoices", 10);
        assert!(
            hits.iter().any(|hit| hit.path == folder && hit.is_dir),
            "the folder itself is a result"
        );
        assert!(
            hits.iter().any(|hit| hit.path == inside),
            "its contents match through the folder name"
        );
        assert!(
            !hits
                .iter()
                .any(|hit| hit.path.ends_with("elsewhere/march.pdf")),
            "a sibling folder without the keyword stays out"
        );
    }

    #[test]
    fn compact_parents_speak_home_relative_paths() {
        let root = tempfile::tempdir().unwrap();
        let file = touch(root.path(), "Pictures/one.png");
        let hit = search(root.path(), "one.png", 10).remove(0);
        assert_eq!(hit.path, file);
        assert_eq!(hit.compact_parent(root.path()), "~/Pictures");
        assert_eq!(hit.compact_path(root.path()), "~/Pictures/one.png");
    }
}


