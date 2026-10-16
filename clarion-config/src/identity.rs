//! Runtime identity and configuration migration without rewriting user files.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::utils::RegexEq;

pub const CONFIG_PATH_ENV: &str = "CLARION_CONFIG";
pub const LEGACY_CONFIG_PATH_ENV: &str = "REALAND_CONFIG";

/// Explicit Clarion configuration wins; empty variables do not name a file.
pub fn config_path_from_environment() -> Option<PathBuf> {
    explicit_config_path(
        std::env::var_os(CONFIG_PATH_ENV),
        std::env::var_os(LEGACY_CONFIG_PATH_ENV),
    )
}

fn explicit_config_path(primary: Option<OsString>, legacy: Option<OsString>) -> Option<PathBuf> {
    primary
        .filter(|path| !path.is_empty())
        .or_else(|| legacy.filter(|path| !path.is_empty()))
        .map(PathBuf::from)
}

/// Use existing Realand configuration only while Clarion has no corresponding file.
///
/// A broken symlink or unreadable Clarion path is still authoritative: its load error must not
/// silently select another configuration. The selected file remains the watcher/save target, so
/// relative includes, permissions and hand-maintained configuration continue to work unchanged.
pub fn prefer_existing_path(canonical: PathBuf, legacy: PathBuf) -> PathBuf {
    if !path_is_present(&canonical) && path_is_present(&legacy) {
        legacy
    } else {
        canonical
    }
}

pub(crate) fn path_is_present(path: &Path) -> bool {
    !std::fs::symlink_metadata(path)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

pub fn user_config_path(config_root: &Path) -> PathBuf {
    prefer_existing_path(
        config_root.join("clarion/config.kdl"),
        config_root.join("realand/config.kdl"),
    )
}

/// Recognize only renamed first-party application and layer identities in existing rules.
///
/// This helper is shared by match and exclude evaluation; it does not translate arbitrary
/// third-party IDs, titles, or regular expressions. Protocol-visible identities remain Clarion.
pub fn matches_shell_identity(pattern: &RegexEq, actual: &str) -> bool {
    if pattern.0.is_match(actual) {
        return true;
    }
    let legacy = match actual {
        "clarion-shell" => "realand-shell",
        "clarion-shell-dock" => "realand-shell-dock",
        "clarion-shell-menu-reservation" => "realand-shell-menu-reservation",
        "clarion-shell-dock-reservation" => "realand-shell-dock-reservation",
        "clarion-desktop" => "realand-desktop",
        "clarion-desktop-interaction" => "realand-desktop-interaction",
        "dev.clarion.Shell" => "dev.realand.Shell",
        "clarion-settings" => "realand-settings",
        _ => return false,
    };
    pattern.0.is_match(legacy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_config_precedence_and_empty_variables() {
        let primary = OsString::from("/new config/config.kdl");
        let legacy = OsString::from("/old config/config.kdl");
        assert_eq!(
            explicit_config_path(Some(primary.clone()), Some(legacy.clone())),
            Some(PathBuf::from(primary))
        );
        assert_eq!(
            explicit_config_path(Some(OsString::new()), Some(legacy.clone())),
            Some(PathBuf::from(legacy))
        );
        assert_eq!(explicit_config_path(None, Some(OsString::new())), None);
    }

    #[test]
    fn existing_legacy_config_is_used_without_copying_or_overwriting_it() {
        let root = tempfile::tempdir().unwrap();
        let canonical = root.path().join("clarion/config.kdl");
        let legacy = root.path().join("realand/config.kdl");
        assert_eq!(user_config_path(root.path()), canonical);
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, "include \"my-colors.kdl\"").unwrap();
        assert_eq!(user_config_path(root.path()), legacy);
        assert!(!canonical.exists());
        std::fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        std::fs::write(&canonical, "// Clarion").unwrap();
        assert_eq!(user_config_path(root.path()), canonical);
        assert_eq!(
            std::fs::read_to_string(legacy).unwrap(),
            "include \"my-colors.kdl\""
        );
    }

    #[test]
    fn broken_canonical_symlink_does_not_resurrect_legacy_config() {
        let root = tempfile::tempdir().unwrap();
        let canonical = root.path().join("new");
        let legacy = root.path().join("old");
        std::fs::write(&legacy, "old").unwrap();
        std::os::unix::fs::symlink(root.path().join("missing"), &canonical).unwrap();
        assert_eq!(prefer_existing_path(canonical.clone(), legacy), canonical);
    }

    #[test]
    fn selected_legacy_config_keeps_relative_includes_and_reload_target() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("realand/config.kdl");
        let include = root.path().join("realand/colors.kdl");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, "include \"colors.kdl\"\n").unwrap();
        std::fs::write(&include, "layout { gaps 7; }\n").unwrap();
        let path = crate::ConfigPath::Regular {
            user_path: user_config_path(root.path()),
            system_path: root.path().join("system/clarion/config.kdl"),
        };
        let (created, loaded) = path.load_or_create();
        assert!(created.is_none());
        assert!(loaded.config.is_ok());
        assert!(loaded.includes.contains(&include));
        std::fs::write(include, "an-invalid-setting\n").unwrap();
        assert!(path.load().config.is_err());
        assert!(!root.path().join("clarion/config.kdl").exists());
        assert_eq!(
            std::fs::read_to_string(legacy).unwrap(),
            "include \"colors.kdl\"\n"
        );
    }

    #[test]
    fn regular_config_reports_broken_canonical_link_instead_of_loading_system_config() {
        let root = tempfile::tempdir().unwrap();
        let canonical = root.path().join("clarion.kdl");
        let system = root.path().join("system.kdl");
        std::os::unix::fs::symlink(root.path().join("missing"), &canonical).unwrap();
        std::fs::write(&system, "layout { gaps 7; }\n").unwrap();
        let path = crate::ConfigPath::Regular {
            user_path: canonical.clone(),
            system_path: system,
        };
        assert!(path.load().config.is_err());
        let (created, loaded) = path.load_or_create();
        assert!(created.is_none());
        assert!(loaded.config.is_err());
        assert!(canonical.is_symlink());
        assert!(!root.path().join("missing").exists());
    }

    #[test]
    fn renamed_shell_rules_accept_old_and_new_ids_but_no_other_applications() {
        let legacy: RegexEq = "^realand-shell(-dock)?$".parse().unwrap();
        let canonical: RegexEq = "^clarion-shell(-dock)?$".parse().unwrap();
        for id in ["clarion-shell", "clarion-shell-dock"] {
            assert!(matches_shell_identity(&legacy, id));
            assert!(matches_shell_identity(&canonical, id));
        }
        assert!(!matches_shell_identity(&legacy, "clarion-shell-extra"));
        assert!(!matches_shell_identity(
            &legacy,
            "third-party-clarion-shell"
        ));
        let settings: RegexEq = "^realand-settings$".parse().unwrap();
        assert!(matches_shell_identity(&settings, "clarion-settings"));
        assert!(!matches_shell_identity(&settings, "clarion-shell"));
        // No reverse mapping: an old running application does not impersonate the new app ID.
        assert!(!matches_shell_identity(&canonical, "realand-shell"));
    }

    #[test]
    fn desktop_and_reservation_rules_survive_identity_migration() {
        let desktop: RegexEq = "^realand-desktop(-interaction)?$".parse().unwrap();
        for id in ["clarion-desktop", "clarion-desktop-interaction"] {
            assert!(matches_shell_identity(&desktop, id));
        }
        let reservation: RegexEq = "^realand-shell-(menu|dock)-reservation$".parse().unwrap();
        for id in [
            "clarion-shell-menu-reservation",
            "clarion-shell-dock-reservation",
        ] {
            assert!(matches_shell_identity(&reservation, id));
        }
        assert!(!matches_shell_identity(
            &reservation,
            "clarion-shell-other-reservation"
        ));
        let preview: RegexEq = "^dev\\.realand\\.Shell$".parse().unwrap();
        assert!(matches_shell_identity(&preview, "dev.clarion.Shell"));
    }
}
