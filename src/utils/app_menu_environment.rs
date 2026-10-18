//! Retire only the GTK exporter previously installed by Clarion. Native client
//! menus need no exporter, registrar or global-menu capability. This migration
//! runs for a real session only; it never loads a GTK module or removes files.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::{collections::HashMap, env};

const MODULES: &str = "GTK_MODULES";
const SCHEMAS: &str = "GSETTINGS_SCHEMA_DIR";
const MODULE_SUFFIX: &str = "lib/gtk-3.0/modules/libappmenu-gtk-module.so";
const SCHEMA_SUFFIX: &str = "share/glib-2.0/schemas";

type Change = (&'static str, Option<OsString>);

pub struct LegacyAppMenuEnvironment {
    home: Option<PathBuf>,
    changes: Vec<Change>,
}

/// Remove our own absolute exporter paths before starting any worker threads.
/// User-installed bare module names, other GTK modules and custom schemas stay
/// intact. There is deliberately no Drop restoration which could re-enable it.
pub fn disable_legacy_appmenu_environment() -> LegacyAppMenuEnvironment {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute());
    let changes = home
        .as_deref()
        .map(|home| plan(home, env::var_os(MODULES), env::var_os(SCHEMAS)))
        .unwrap_or_default();
    for (name, value) in &changes {
        if let Some(value) = value {
            env::set_var(name, value);
        } else {
            env::remove_var(name);
        }
    }
    LegacyAppMenuEnvironment { home, changes }
}

impl LegacyAppMenuEnvironment {
    /// A user manager may outlive the previous compositor. Read its own values
    /// before changing them, rather than replacing unrelated modules with the
    /// new process's environment. D-Bus activation has no read/unset operation;
    /// update only keys with a positively identified legacy Clarion value.
    pub fn migrate_activation_environment(&self) {
        let Some(home) = self.home.as_deref() else {
            return;
        };
        let mut activation = self.changes.clone();
        if cfg!(feature = "systemd") {
            if let Some(manager) = manager_environment() {
                let changes = plan(
                    home,
                    manager.get(MODULES).cloned(),
                    manager.get(SCHEMAS).cloned(),
                );
                for change in &changes {
                    update_manager(change);
                }
                activation = activation_after_manager(&manager, &changes, &self.changes);
            }
        }
        if cfg!(feature = "dinit") {
            for (name, value) in &self.changes {
                let mut command = Command::new("dinitctl");
                command.args([
                    "--user",
                    if value.is_some() {
                        "setenv"
                    } else {
                        "unsetenv"
                    },
                ]);
                command.arg(if let Some(value) = value {
                    assignment(name, value)
                } else {
                    OsString::from(*name)
                });
                run(&mut command, "dinit legacy appmenu cleanup");
            }
        }
        if !activation.is_empty() {
            let mut command = busctl();
            command.args([
                "call",
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "UpdateActivationEnvironment",
                "a{ss}",
            ]);
            command.arg(activation.len().to_string());
            for (name, value) in &activation {
                command.arg(name);
                command.arg(value.as_deref().unwrap_or(OsStr::new("")));
            }
            run(&mut command, "D-Bus legacy appmenu cleanup");
        }
    }
}

fn plan(home: &Path, modules: Option<OsString>, schemas: Option<OsString>) -> Vec<Change> {
    let mut changes = Vec::new();
    if let Some(modules) = modules {
        let mut removed = false;
        let retained: Vec<_> = modules
            .as_bytes()
            .split(|byte| *byte == b':')
            .filter(|entry| {
                let own = owned_path(home, Path::new(OsStr::from_bytes(entry)), MODULE_SUFFIX);
                removed |= own;
                !own
            })
            .collect();
        if removed {
            let value = retained.join(&b':');
            changes.push((
                MODULES,
                (!value.is_empty()).then(|| OsString::from_vec(value)),
            ));
        }
    }
    if schemas
        .as_deref()
        .is_some_and(|value| owned_path(home, Path::new(value), SCHEMA_SUFFIX))
    {
        changes.push((SCHEMAS, None));
    }
    changes
}

fn owned_path(home: &Path, path: &Path, suffix: &str) -> bool {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return false;
    }
    let standalone = home.join(".local/lib/clarion-appmenu");
    if path == standalone.join("current").join(suffix) {
        return true;
    }
    // The stable current path was the only injected path. Accept its canonical
    // managed release form too, without following arbitrary user symlinks.
    for (releases, nested) in [
        (standalone.join("releases"), false),
        (home.join(".local/lib/clarion/releases"), true),
    ] {
        let Ok(relative) = path.strip_prefix(releases) else {
            continue;
        };
        let Some(Component::Normal(release)) = relative.components().next() else {
            continue;
        };
        let mut expected = PathBuf::from(release);
        if nested {
            expected.push("gtk-appmenu");
        }
        expected.push(suffix);
        if relative == expected {
            return true;
        }
    }
    false
}

fn activation_after_manager(
    manager: &HashMap<String, OsString>,
    manager_changes: &[Change],
    process_changes: &[Change],
) -> Vec<Change> {
    // Existing manager values take precedence, including a custom schema or
    // module list which this migration must not replace using process values.
    let mut changes: Vec<_> = process_changes
        .iter()
        .filter(|(name, _)| !manager.contains_key(*name))
        .cloned()
        .collect();
    changes.extend_from_slice(manager_changes);
    changes
}

fn assignment(name: &str, value: &OsStr) -> OsString {
    let mut result = OsString::from(name);
    result.push("=");
    result.push(value);
    result
}

fn busctl() -> Command {
    let mut command = Command::new("busctl");
    command.args(["--user", "--timeout=1s"]);
    command
}

fn manager_environment() -> Option<HashMap<String, OsString>> {
    let output = busctl()
        .args([
            "--json=short",
            "get-property",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "Environment",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_manager_environment(&output.stdout)
}

fn parse_manager_environment(bytes: &[u8]) -> Option<HashMap<String, OsString>> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if value.get("type")?.as_str()? != "as" {
        return None;
    }
    let entries = value.get("data")?.as_array()?;
    let mut selected = HashMap::new();
    for entry in entries {
        let Some((name, value)) = entry.as_str()?.split_once('=') else {
            continue;
        };
        if matches!(name, MODULES | SCHEMAS) {
            selected.insert(name.to_owned(), OsString::from(value));
        }
    }
    Some(selected)
}

fn update_manager((name, value): &Change) {
    let mut command = busctl();
    command.args([
        "call",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        if value.is_some() {
            "SetEnvironment"
        } else {
            "UnsetEnvironment"
        },
        "as",
        "1",
    ]);
    command.arg(if let Some(value) = value {
        assignment(name, value)
    } else {
        OsString::from(*name)
    });
    run(&mut command, "systemd legacy appmenu cleanup");
}

fn run(command: &mut Command, operation: &str) {
    let result = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !result.is_ok_and(|status| status.success()) {
        tracing::warn!(
            "{operation} did not complete; existing applications may need a fresh session"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_our_absolute_module_and_schema_paths() {
        let home = Path::new("/home/test");
        let module = home
            .join(".local/lib/clarion-appmenu/current")
            .join(MODULE_SUFFIX);
        let schema = home
            .join(".local/lib/clarion-appmenu/current")
            .join(SCHEMA_SUFFIX);
        let modules = format!("colorreload:{}:decorations", module.display());
        assert_eq!(
            plan(home, Some(modules.into()), Some(schema.into_os_string())),
            vec![
                (MODULES, Some("colorreload:decorations".into())),
                (SCHEMAS, None)
            ]
        );
        assert_eq!(
            plan(home, Some(module.into_os_string()), None),
            vec![(MODULES, None)]
        );
    }

    #[test]
    fn preserves_other_exporters_custom_schemas_and_non_utf8_module_bytes() {
        let home = Path::new("/home/test");
        for modules in [
            "appmenu-gtk-module:other",
            "/another/libappmenu-gtk-module.so",
            "/home/other/.local/lib/clarion-appmenu/current/lib/gtk-3.0/modules/libappmenu-gtk-module.so",
            ".local/lib/clarion-appmenu/current/lib/gtk-3.0/modules/libappmenu-gtk-module.so",
            "/home/test/.local/lib/clarion-appmenu/current/../foreign/libappmenu-gtk-module.so",
        ] {
            assert!(plan(home, Some(modules.into()), Some("/custom/schemas".into())).is_empty());
        }
        let mut value = b"other-\xff:".to_vec();
        value.extend_from_slice(
            home.join(".local/lib/clarion-appmenu/current")
                .join(MODULE_SUFFIX)
                .as_os_str()
                .as_bytes(),
        );
        assert_eq!(
            plan(home, Some(OsString::from_vec(value)), None),
            vec![(MODULES, Some(OsString::from_vec(b"other-\xff".to_vec())))]
        );
        assert!(plan(home, None, None).is_empty());
    }

    #[test]
    fn recognizes_only_exact_managed_release_shapes() {
        let home = Path::new("/home/test");
        for prefix in [
            "/home/test/.local/lib/clarion-appmenu/releases/old",
            "/home/test/.local/lib/clarion/releases/old/gtk-appmenu",
        ] {
            assert!(owned_path(
                home,
                &Path::new(prefix).join(MODULE_SUFFIX),
                MODULE_SUFFIX
            ));
            assert!(owned_path(
                home,
                &Path::new(prefix).join(SCHEMA_SUFFIX),
                SCHEMA_SUFFIX
            ));
        }
        for prefix in [
            "/home/test/.local/lib/clarion/releases/old",
            "/home/test/.local/lib/clarion/releases/old/other/gtk-appmenu",
            "/home/test/.local/lib/clarion-appmenu/releases/old/other",
        ] {
            assert!(!owned_path(
                home,
                &Path::new(prefix).join(MODULE_SUFFIX),
                MODULE_SUFFIX
            ));
        }
    }

    #[test]
    fn activation_migration_preserves_existing_manager_choices() {
        let home = Path::new("/home/test");
        let owned = home
            .join(".local/lib/clarion-appmenu/current")
            .join(MODULE_SUFFIX);
        let manager = HashMap::from([
            (
                MODULES.into(),
                OsString::from(format!("manager-only:{}", owned.display())),
            ),
            (SCHEMAS.into(), OsString::from("/manager/custom/schemas")),
        ]);
        let manager_changes = plan(
            home,
            manager.get(MODULES).cloned(),
            manager.get(SCHEMAS).cloned(),
        );
        let process_changes = vec![(MODULES, Some("process-only".into())), (SCHEMAS, None)];
        assert_eq!(
            activation_after_manager(&manager, &manager_changes, &process_changes),
            vec![(MODULES, Some("manager-only".into()))]
        );
        let empty = HashMap::new();
        assert_eq!(
            activation_after_manager(&empty, &[], &process_changes),
            process_changes
        );
        assert!(activation_after_manager(&manager, &[], &process_changes).is_empty());
    }

    #[test]
    fn manager_json_preserves_quoted_values_without_shell_parsing() {
        let value = serde_json::json!({"type":"as","data":[
            "GTK_MODULES=custom:module with 'quotes'",
            "GSETTINGS_SCHEMA_DIR=/custom/schema directory",
            "OTHER=value"
        ]});
        let selected = parse_manager_environment(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(
            selected[MODULES],
            OsString::from("custom:module with 'quotes'")
        );
        assert_eq!(
            selected[SCHEMAS],
            OsString::from("/custom/schema directory")
        );
        assert!(parse_manager_environment(b"not json").is_none());
    }
}
