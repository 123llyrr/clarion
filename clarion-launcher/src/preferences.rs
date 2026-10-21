// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared configuration namespace. Shell appearance and UI state live in the
//! native Shell; the application catalog contains no independent launcher UI.
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
pub type Environment = HashMap<String, String>;
/// Prefer the canonical directory; consult the legacy namespace only when absent.
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
}
