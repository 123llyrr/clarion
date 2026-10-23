// SPDX-License-Identifier: GPL-3.0-or-later
//! Pinned Dock folder state. Directory scans run on the background executor;
//! rendering only sees immutable snapshots.

use super::*;
use crate::dock_folder::{scan_folder, sort_folder_entries, FolderSnapshot};
use crate::preferences::MAX_DOCK_FOLDERS;
use std::path::Path;
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
                }
            })
            .collect()
    }

    fn folder_preference(&self, path: &Path) -> Option<&DockFolderPreference> {
        self.prefs
            .dock_folders
            .iter()
            .find(|folder| Path::new(&folder.path) == path)
    }

    fn folder_preference_mut(&mut self, path: &Path) -> Option<&mut DockFolderPreference> {
        self.prefs
            .dock_folders
            .iter_mut()
            .find(|folder| Path::new(&folder.path) == path)
    }

    pub(super) fn pin_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // The drop is over: the reserved preview slot becomes the real slot, or
        // disappears if the folder cannot be pinned.
        self.folder_drop_preview = false;
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

    pub(super) fn unpin_folder(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.prefs
            .dock_folders
            .retain(|folder| Path::new(&folder.path) != path);
        self.dock_folder_cache.remove(path);
        self.dock_folder_errors.remove(path);
        self.folder_scan_generation.remove(path);
        if self
            .folder_navigation
            .as_ref()
            .is_some_and(|navigation| navigation.root == path)
        {
            self.folder_navigation = None;
        }
        let closes_menu = matches!(
            self.panel.as_ref(),
            Some(Panel::DockFolderMenu { path: open }) if open == path
        );
        let closes_popup = matches!(
            self.panel.as_ref(),
            Some(Panel::DockFolder { root }) if root == path
        );
        self.save(cx);
        self.changed(cx);
        if closes_menu || closes_popup {
            self.close(cx);
        }
    }

    pub(super) fn reorder_folders(&mut self, order: Vec<String>, cx: &mut Context<Self>) {
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

    pub(super) fn navigate_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(navigation) = &mut self.folder_navigation else {
            return;
        };
        let Ok(metadata) = std::fs::metadata(&path) else {
            return;
        };
        if !metadata.is_dir() {
            return;
        }
        navigation.history.push(navigation.current.clone());
        navigation.current = path.clone();
        self.request_folder_scan(path, cx);
        self.changed(cx);
    }

    pub(super) fn folder_back(&mut self, cx: &mut Context<Self>) {
        let current = {
            let Some(navigation) = &mut self.folder_navigation else {
                return;
            };
            let Some(previous) = navigation.history.pop() else {
                return;
            };
            navigation.current = previous;
            navigation.current.clone()
        };
        self.request_folder_scan(current, cx);
        self.changed(cx);
    }

    pub(super) fn configure_folder(
        &mut self,
        path: &Path,
        update: impl FnOnce(&mut DockFolderPreference),
        cx: &mut Context<Self>,
    ) {
        let Some(folder) = self.folder_preference_mut(path) else {
            return;
        };
        update(folder);
        let sort = folder.sort;
        let descending = folder.descending;
        self.resort_folder_cache(path, sort, descending);
        self.save(cx);
        self.changed(cx);
    }

    fn resort_folder_cache(&mut self, root: &Path, sort: FolderSort, descending: bool) {
        let keys = self
            .dock_folder_cache
            .keys()
            .filter(|key| key.starts_with(root))
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            let Some(snapshot) = self.dock_folder_cache.get(&key).cloned() else {
                continue;
            };
            let mut entries = snapshot.entries.clone();
            sort_folder_entries(&mut entries, sort, descending);
            self.dock_folder_cache.insert(
                key,
                Arc::new(FolderSnapshot {
                    entries,
                    truncated: snapshot.truncated,
                }),
            );
        }
    }

    pub(super) fn request_folder_scan(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // Subfolders are not entries in `dock_folders`; they inherit the sort
        // settings of the nearest pinned ancestor. Without this fallback a
        // subfolder navigation would request no scan at all and render empty.
        let preference = self.folder_preference(&path).or_else(|| {
            self.prefs
                .dock_folders
                .iter()
                .filter(|folder| path.starts_with(Path::new(&folder.path)))
                .max_by_key(|folder| folder.path.len())
        });
        let Some(preference) = preference else {
            return;
        };
        let sort = preference.sort;
        let descending = preference.descending;
        let generation = {
            let generation = self
                .folder_scan_generation
                .entry(path.clone())
                .or_insert(0);
            *generation = generation.wrapping_add(1);
            *generation
        };
        let host = cx.entity().downgrade();
        let scan_path = path.clone();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { scan_folder(&scan_path, sort, descending) })
                .await;
            let _ = host.update(cx, |host, cx| {
                if host.folder_scan_generation.get(&path).copied() != Some(generation) {
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
