use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

/// The rename committed the new contents, but syncing the directory failed.
/// Callers must not treat this as a failure before the file was replaced.
#[derive(Debug)]
pub(crate) struct CommittedWriteError(std::io::Error);

impl std::fmt::Display for CommittedWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "文件已替换，但无法同步目录：{}", self.0)
    }
}

impl std::error::Error for CommittedWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let resolved = if path.is_symlink() {
        fs::canonicalize(path)?
    } else {
        path.to_owned()
    };
    let permissions = fs::metadata(&resolved)
        .ok()
        .map(|metadata| metadata.permissions());
    replace_file(&resolved, bytes, permissions)
}

/// A generated backup belongs to this save operation. Replace a pre-existing
/// link instead of following it, and use the source file's access permissions
/// so an older, more permissive backup cannot expose a private configuration.
pub fn atomic_write_backup(path: &Path, bytes: &[u8], permissions: fs::Permissions) -> Result<()> {
    replace_file(path, bytes, Some(permissions))
}

fn replace_file(path: &Path, bytes: &[u8], permissions: Option<fs::Permissions>) -> Result<()> {
    replace_file_with_directory_sync(path, bytes, permissions, |parent| {
        fs::File::open(parent)?.sync_all()
    })
}

fn replace_file_with_directory_sync(
    path: &Path,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
    sync_directory: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    if let Some(permissions) = permissions {
        temp.as_file().set_permissions(permissions)?;
    }
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .with_context(|| format!("Cannot replace {}", path.display()))?;
    sync_directory(parent).map_err(CommittedWriteError)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_sync_failure_reports_that_replacement_committed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        fs::write(&path, b"old").unwrap();
        let error = replace_file_with_directory_sync(&path, b"new", None, |_| {
            Err(std::io::Error::other("injected directory sync failure"))
        })
        .unwrap_err();
        assert!(error.downcast_ref::<CommittedWriteError>().is_some());
        assert_eq!(fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn atomic_save_preserves_symlink_and_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actual");
        let link = dir.path().join("config");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&path, &link).unwrap();
        atomic_write(&link, b"new").unwrap();
        assert!(link.is_symlink());
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn backup_replaces_symlink_without_modifying_its_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("unrelated");
        let backup = dir.path().join("config.kdl.bak");
        fs::write(&target, b"keep this file").unwrap();
        symlink(&target, &backup).unwrap();
        atomic_write_backup(&backup, b"backup", fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!backup.is_symlink());
        assert_eq!(fs::read(backup).unwrap(), b"backup");
        assert_eq!(fs::read(target).unwrap(), b"keep this file");
    }

    #[test]
    fn backup_uses_source_permissions_instead_of_older_public_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("config.kdl.bak");
        fs::write(&backup, b"old").unwrap();
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o644)).unwrap();
        atomic_write_backup(&backup, b"private", fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            fs::metadata(backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
