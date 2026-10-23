// SPDX-License-Identifier: GPL-3.0-or-later
//! Match dropped paths against the MIME types an application declares.

use std::fs;
use std::path::{Path, PathBuf};

use clarion_launcher::catalog::DesktopApp;

pub fn path_mime(path: &Path) -> Option<String> {
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return None;
    }
    if fs::metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.is_dir())
    {
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
                && supported
                    .strip_suffix("/*")
                    .is_some_and(|prefix| prefix == kind))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app(mime_types: &[&str]) -> DesktopApp {
        DesktopApp {
            id: "test.desktop".into(),
            name: "Test".into(),
            description: String::new(),
            generic_name: String::new(),
            desktop_file: PathBuf::from("/tmp/test.desktop"),
            keywords: Vec::new(),
            categories: Vec::new(),
            category: clarion_launcher::catalog::AppCategory::Other,
            icon_path: None,
            icon_name: None,
            mime_types: mime_types.iter().map(|mime| (*mime).into()).collect(),
        }
    }

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
}
