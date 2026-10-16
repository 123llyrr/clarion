//! Opt-in integration for the separately installed, user-local GTK exporter.
//! Only the real desktop session enables it; nested compositors and CLI tools
//! never change their parent's application environment.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;
use std::{env, fs};

const VARIABLES: [&str; 2] = ["GTK_MODULES", "GSETTINGS_SCHEMA_DIR"];

pub struct AppMenuEnvironment {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl AppMenuEnvironment {
    pub fn enable() -> Self {
        let mut previous = Vec::new();
        if let Some(home) = env::var_os("HOME") {
            let root = Path::new(&home).join(".local/lib/clarion-appmenu/current");
            let module = root.join("lib/gtk-3.0/modules/libappmenu-gtk-module.so");
            let schemas = root.join("share/glib-2.0/schemas");
            let assets_ready = module.is_file()
                && schemas.join("gschemas.compiled").is_file()
                && schemas.join("org.appmenu.gtk-module.gschema.xml").is_file();
            let modules = env::var_os(VARIABLES[0]);
            let schema_dir = env::var_os(VARIABLES[1]);
            for (name, value) in plan(&module, &schemas, assets_ready, modules, schema_dir) {
                previous.push((name, env::var_os(name)));
                env::set_var(name, value);
            }
        }
        Self { previous }
    }

    pub fn variables(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.previous.iter().map(|(name, _)| *name)
    }
}

impl Drop for AppMenuEnvironment {
    fn drop(&mut self) {
        // Restore only variables that we supplied. In particular, never erase
        // another GTK module or a user-selected GSettings schema directory.
        for (name, value) in &self.previous {
            if let Some(value) = value {
                env::set_var(name, value);
            } else {
                env::remove_var(name);
            }
            if cfg!(feature = "systemd") {
                let mut command = Command::new("systemctl");
                command.args(["--user"]);
                if value.is_some() {
                    command.args(["import-environment", name]);
                } else {
                    command.args(["unset-environment", name]);
                }
                let _ = command.status();
            }
            if cfg!(feature = "dinit") {
                let _ = Command::new("dinitctl")
                    .args([
                        "--user",
                        if value.is_some() {
                            "setenv"
                        } else {
                            "unsetenv"
                        },
                        name,
                    ])
                    .status();
            }
            // D-Bus activation has no unset operation. An empty module list /
            // schema directory restores GLib's standard lookup behavior.
            let mut assignment = OsString::from(name);
            assignment.push("=");
            assignment.push(value.as_deref().unwrap_or(OsStr::new("")));
            let _ = Command::new("dbus-update-activation-environment")
                .arg(assignment)
                .status();
        }
    }
}

fn plan(
    module: &Path,
    schemas: &Path,
    assets_ready: bool,
    modules: Option<OsString>,
    schema_dir: Option<OsString>,
) -> Vec<(&'static str, OsString)> {
    if !assets_ready {
        return Vec::new();
    }
    // A custom schema source must contain this exporter's schema. Loading a
    // module whose GSettings schema cannot be found can abort GTK clients.
    if let Some(directory) = schema_dir.as_ref().filter(|value| !value.is_empty()) {
        if directory != schemas.as_os_str()
            && !fs::metadata(Path::new(directory).join("org.appmenu.gtk-module.gschema.xml"))
                .is_ok_and(|metadata| metadata.is_file())
        {
            return Vec::new();
        }
    }
    let mut value = modules.unwrap_or_default();
    let text = value.to_string_lossy();
    if text.split(':').any(|entry| {
        entry == "appmenu-gtk-module"
            || Path::new(entry).file_name() == Some(OsStr::new("libappmenu-gtk-module.so"))
    }) {
        return Vec::new();
    }
    if !value.is_empty() {
        value.push(":");
    }
    value.push(module);
    let mut result = vec![(VARIABLES[0], value)];
    if schema_dir.as_ref().is_none_or(|value| value.is_empty()) {
        result.push((VARIABLES[1], schemas.as_os_str().to_owned()));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_existing_modules_and_uses_absolute_installed_paths() {
        let module = Path::new("/home/test/.local/lib/clarion-appmenu/current/lib/gtk-3.0/modules/libappmenu-gtk-module.so");
        let schemas =
            Path::new("/home/test/.local/lib/clarion-appmenu/current/share/glib-2.0/schemas");
        let result = plan(
            module,
            schemas,
            true,
            Some("colorreload:decorations".into()),
            None,
        );
        assert_eq!(result[0].0, "GTK_MODULES");
        assert_eq!(
            result[0].1,
            OsString::from(format!("colorreload:decorations:{}", module.display()))
        );
        assert_eq!(result[1], ("GSETTINGS_SCHEMA_DIR", schemas.into()));
    }

    #[test]
    fn missing_install_and_custom_schema_directory_are_not_overridden() {
        let module = Path::new("/module.so");
        let schemas = Path::new("/schemas");
        assert!(plan(module, schemas, false, None, None).is_empty());
        assert!(plan(
            module,
            schemas,
            true,
            None,
            Some("/custom-missing-schemas".into())
        )
        .is_empty());
    }

    #[test]
    fn does_not_load_an_already_configured_exporter_twice() {
        for modules in [
            "appmenu-gtk-module:other",
            "/another/libappmenu-gtk-module.so",
        ] {
            assert!(plan(
                Path::new("/module.so"),
                Path::new("/schemas"),
                true,
                Some(modules.into()),
                None
            )
            .is_empty());
        }
    }
}
