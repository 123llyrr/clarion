// SPDX-License-Identifier: GPL-3.0-or-later
//! XDG desktop application catalog, compatible with the Flutter Shell scanner.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::icons::IconResolver;

pub type CatalogEnvironment = HashMap<String, String>;
const MAX_DESKTOP_BYTES: u64 = 1024 * 1024;
const CATALOG_ICON_PIXELS: u32 = 128;

/// Tahoe's seven stable primary groups, in their presentation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AppCategory {
    Utilities,
    ProductivityFinance,
    Social,
    Creativity,
    InformationReading,
    Entertainment,
    Other,
}

impl AppCategory {
    pub const ALL: [Self; 7] = [
        Self::Utilities,
        Self::ProductivityFinance,
        Self::Social,
        Self::Creativity,
        Self::InformationReading,
        Self::Entertainment,
        Self::Other,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Utilities => "utilities",
            Self::ProductivityFinance => "productivity_finance",
            Self::Social => "social",
            Self::Creativity => "creativity",
            Self::InformationReading => "information_reading",
            Self::Entertainment => "entertainment",
            Self::Other => "other",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Utilities => "工具",
            Self::ProductivityFinance => "生产力与财务",
            Self::Social => "社交",
            Self::Creativity => "创意",
            Self::InformationReading => "信息与阅读",
            Self::Entertainment => "娱乐",
            Self::Other => "其他",
        }
    }

    /// Specific secondary categories take precedence over broad XDG groups.
    pub fn from_xdg(categories: &[String]) -> Self {
        let has = |names: &[&str]| {
            categories
                .iter()
                .any(|category| names.contains(&category.as_str()))
        };
        if has(&["Game", "Player", "TV", "Amusement"]) {
            Self::Entertainment
        } else if has(&[
            "AudioVideoEditing",
            "AudioEditing",
            "VideoEditing",
            "Recorder",
            "Sequencer",
            "Midi",
            "Mixer",
            "Music",
            "Graphics",
            "Photography",
            "Publishing",
            "2DGraphics",
            "3DGraphics",
            "RasterGraphics",
            "VectorGraphics",
        ]) {
            Self::Creativity
        } else if has(&[
            "WebBrowser",
            "News",
            "Feed",
            "Education",
            "Science",
            "Dictionary",
            "Documentation",
            "Viewer",
        ]) {
            Self::InformationReading
        } else if has(&[
            "InstantMessaging",
            "Chat",
            "Email",
            "IRCClient",
            "Telephony",
            "VideoConference",
        ]) {
            Self::Social
        } else if has(&[
            "Office",
            "Finance",
            "Calendar",
            "ContactManagement",
            "ProjectManagement",
            "Spreadsheet",
            "WordProcessor",
            "Presentation",
            "Database",
        ]) {
            Self::ProductivityFinance
        } else if has(&["AudioVideo", "Audio", "Video"]) {
            Self::Creativity
        } else if has(&["Network"]) {
            Self::Social
        } else if has(&["Utility", "System", "Settings", "Development"]) {
            Self::Utilities
        } else {
            Self::Other
        }
    }
}

/// A launchable application. `desktop_file` is passed literally to GIO by launch.rs.
#[derive(Clone, Debug)]
pub struct DesktopApp {
    pub id: String,
    pub name: String,
    pub description: String,
    pub generic_name: String,
    pub desktop_file: PathBuf,
    pub keywords: Vec<String>,
    pub categories: Vec<String>,
    pub category: AppCategory,
    pub icon_path: Option<PathBuf>,
    pub icon_name: Option<String>,
}

/// An immutable snapshot; reload off the UI thread and replace the whole catalog.
#[derive(Clone, Debug, Default)]
pub struct AppCatalog {
    apps: Vec<DesktopApp>,
    by_id: HashMap<String, usize>,
    search_texts: Vec<String>,
}

impl AppCatalog {
    pub fn load(environment: &CatalogEnvironment) -> anyhow::Result<Self> {
        let desktops = environment
            .get("XDG_CURRENT_DESKTOP")
            .map(String::as_str)
            .unwrap_or("clarion")
            .split(':')
            .collect::<HashSet<_>>();
        let locales = locale_candidates(environment);
        let mut seen = HashSet::new();
        let mut executables = ExecutableCache::new(environment);
        let mut icons = IconResolver::new(environment);
        let mut apps = Vec::new();
        for root in data_roots(environment) {
            let directory = root.join("applications");
            let mut files = WalkDir::new(&directory)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| !entry.file_type().is_dir())
                .map(|entry| entry.into_path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "desktop")
                })
                .collect::<Vec<_>>();
            files.sort();
            for path in files {
                let Some(relative) = path.strip_prefix(&directory).ok().and_then(Path::to_str)
                else {
                    continue;
                };
                let id = relative.replace('/', "-");
                // User overrides mask lower-priority entries even if reading or
                // validation subsequently fails (Hidden/NoDisplay/TryExec included).
                if !seen.insert(id.clone()) {
                    continue;
                }
                let Some(source) = read_desktop(&path) else {
                    continue;
                };
                let values = parse_desktop_entry(&source);
                if values.get("Type").map(String::as_str) != Some("Application")
                    || values.get("Hidden").is_some_and(|value| value == "true")
                    || values.get("NoDisplay").is_some_and(|value| value == "true")
                {
                    continue;
                }
                let only = string_list(
                    values
                        .get("OnlyShowIn")
                        .map(String::as_str)
                        .unwrap_or_default(),
                );
                let not = string_list(
                    values
                        .get("NotShowIn")
                        .map(String::as_str)
                        .unwrap_or_default(),
                );
                if (!only.is_empty()
                    && !only
                        .iter()
                        .any(|desktop| desktops.contains(desktop.as_str())))
                    || not
                        .iter()
                        .any(|desktop| desktops.contains(desktop.as_str()))
                {
                    continue;
                }
                let dbus = values
                    .get("DBusActivatable")
                    .is_some_and(|value| value == "true");
                let exec = values.get("Exec").map(String::as_str).unwrap_or_default();
                if !dbus && (exec.trim().is_empty() || !executables.exec_available(exec)) {
                    continue;
                }
                if values
                    .get("TryExec")
                    .is_some_and(|exec| !executables.exists(exec, None))
                {
                    continue;
                }
                let localized = |key| localized_value(&values, key, &locales).to_owned();
                let name = localized("Name");
                if name.is_empty() {
                    continue;
                }
                let categories = string_list(
                    values
                        .get("Categories")
                        .map(String::as_str)
                        .unwrap_or_default(),
                );
                let icon_name = values.get("Icon").filter(|icon| !icon.is_empty()).cloned();
                let icon_path = icon_name
                    .as_deref()
                    .and_then(|name| icons.resolve(name, CATALOG_ICON_PIXELS));
                apps.push(DesktopApp {
                    id,
                    name,
                    description: localized("Comment"),
                    generic_name: localized("GenericName"),
                    desktop_file: path,
                    keywords: string_list(&localized("Keywords")),
                    category: AppCategory::from_xdg(&categories),
                    categories,
                    icon_path,
                    icon_name,
                });
            }
        }
        apps.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        let by_id = apps
            .iter()
            .enumerate()
            .map(|(index, app)| (app.id.clone(), index))
            .collect();
        let search_texts = apps
            .iter()
            .map(|app| {
                format!(
                    "{} {} {} {}",
                    app.name,
                    app.generic_name,
                    app.description,
                    app.keywords.join(" ")
                )
                .to_lowercase()
            })
            .collect();
        Ok(Self {
            apps,
            by_id,
            search_texts,
        })
    }

    pub fn apps(&self) -> &[DesktopApp] {
        &self.apps
    }

    pub fn get(&self, id: &str) -> Option<&DesktopApp> {
        self.by_id.get(id).map(|&index| &self.apps[index])
    }

    /// Every whitespace-separated term must occur in localized app metadata.
    pub fn search(&self, query: &str) -> Vec<&DesktopApp> {
        let lowered = query.to_lowercase();
        let terms = lowered.split_whitespace().collect::<Vec<_>>();
        self.apps
            .iter()
            .zip(&self.search_texts)
            .filter(|(_, text)| terms.iter().all(|term| text.contains(term)))
            .map(|(app, _)| app)
            .collect()
    }

    pub fn suggested(&self, recent_ids: &[String], limit: usize) -> Vec<&DesktopApp> {
        let mut seen = HashSet::new();
        recent_ids
            .iter()
            .map(|id| id.trim())
            .filter(|id| seen.insert(*id))
            .filter_map(|id| self.get(id))
            .take(limit)
            .collect()
    }
}

pub(crate) fn data_roots(environment: &CatalogEnvironment) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = environment
        .get("XDG_DATA_HOME")
        .filter(|home| Path::new(home).is_absolute())
    {
        roots.push(PathBuf::from(home));
    } else if let Some(home) = environment
        .get("HOME")
        .filter(|home| Path::new(home).is_absolute())
    {
        roots.push(PathBuf::from(home).join(".local/share"));
    }
    let system = environment
        .get("XDG_DATA_DIRS")
        .map(String::as_str)
        .unwrap_or_default()
        .split(':')
        .filter(|root| Path::new(root).is_absolute())
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if system.is_empty() {
        roots.extend([
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]);
    } else {
        roots.extend(system);
    }
    let mut seen = HashSet::new();
    roots.retain(|root| seen.insert(root.clone()));
    roots
}

fn read_desktop(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_DESKTOP_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_DESKTOP_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_DESKTOP_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn parse_desktop_entry(source: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    let mut entry = false;
    for raw in source.trim_start_matches('\u{feff}').lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            entry = line == "[Desktop Entry]";
            continue;
        }
        if !entry || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if !key.is_empty() {
                values.insert(key.to_owned(), unescape(value));
            }
        }
    }
    values
}

fn unescape(value: &str) -> String {
    let mut chars = value.chars().peekable();
    let mut output = String::with_capacity(value.len());
    while let Some(character) = chars.next() {
        if character == '\\' {
            let escaped = match chars.peek() {
                Some('s') => Some(' '),
                Some('n') => Some('\n'),
                Some('t') => Some('\t'),
                Some('r') => Some('\r'),
                Some('\\') => Some('\\'),
                _ => None,
            };
            if let Some(escaped) = escaped {
                chars.next();
                output.push(escaped);
                continue;
            }
        }
        output.push(character);
    }
    output
}

fn string_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn locale_candidates(environment: &CatalogEnvironment) -> Vec<String> {
    let raw = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|key| environment.get(*key).filter(|value| !value.is_empty()))
        .map(String::as_str)
        .unwrap_or_default();
    let (base, modifier) = raw
        .split_once('@')
        .map_or((raw, None), |(base, modifier)| (base, Some(modifier)));
    let base = base.split('.').next().unwrap_or_default();
    let language = base.split('_').next().unwrap_or_default();
    let mut locales = Vec::new();
    if let Some(modifier) = modifier.filter(|modifier| !modifier.is_empty()) {
        locales.push(format!("{base}@{modifier}"));
    }
    locales.push(base.to_owned());
    if let Some(modifier) = modifier.filter(|modifier| !modifier.is_empty()) {
        locales.push(format!("{language}@{modifier}"));
    }
    locales.push(language.to_owned());
    let mut seen = HashSet::new();
    locales.retain(|locale| !locale.is_empty() && seen.insert(locale.clone()));
    locales
}

fn localized_value<'a>(
    values: &'a HashMap<String, String>,
    key: &str,
    locales: &[String],
) -> &'a str {
    locales
        .iter()
        .find_map(|locale| values.get(&format!("{key}[{locale}]")))
        .or_else(|| values.get(key))
        .map(String::as_str)
        .unwrap_or_default()
}

struct ExecutableCache<'a> {
    environment: &'a CatalogEnvironment,
    exists: HashMap<(String, String), bool>,
}

impl<'a> ExecutableCache<'a> {
    fn new(environment: &'a CatalogEnvironment) -> Self {
        Self {
            environment,
            exists: HashMap::new(),
        }
    }

    fn exists(&mut self, executable: &str, search_path: Option<&str>) -> bool {
        let path = search_path.unwrap_or_else(|| {
            self.environment
                .get("PATH")
                .map(String::as_str)
                .unwrap_or("/usr/bin:/bin")
        });
        let key = (path.to_owned(), executable.to_owned());
        *self.exists.entry(key).or_insert_with(|| {
            if executable.is_empty() {
                return false;
            }
            // Relative programs and field codes require GIO's working directory
            // and expansion context; they cannot prove an application is absent.
            if executable.contains('%')
                || (executable.contains('/') && !Path::new(executable).is_absolute())
            {
                return true;
            }
            if Path::new(executable).is_absolute() {
                return executable_file(Path::new(executable));
            }
            let mut uncertain = false;
            for directory in path.split(':') {
                if !Path::new(directory).is_absolute() {
                    uncertain = true;
                } else if executable_file(&Path::new(directory).join(executable)) {
                    return true;
                }
            }
            uncertain
        })
    }

    fn exec_available(&mut self, exec: &str) -> bool {
        let Some(words) = exec_words(exec) else {
            return true;
        };
        let Some(program) = words.first() else {
            return true;
        };
        if !self.exists(program, None) {
            return false;
        }
        if Path::new(program)
            .file_name()
            .is_none_or(|name| name != "env")
        {
            return true;
        }
        let mut path = self
            .environment
            .get("PATH")
            .map(String::as_str)
            .unwrap_or("/usr/bin:/bin")
            .to_owned();
        let mut index = 1;
        while let Some(word) = words.get(index) {
            if word == "--" {
                index += 1;
                break;
            }
            if environment_assignment(word) {
                if let Some(value) = word.strip_prefix("PATH=") {
                    path = value.to_owned();
                }
                index += 1;
                continue;
            }
            if word.starts_with('-') {
                return true;
            }
            break;
        }
        words
            .get(index)
            .is_none_or(|program| self.exists(program, Some(&path)))
    }
}

fn executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn environment_assignment(value: &str) -> bool {
    let Some((name, _)) = value.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Desktop Entry argument boundaries, not shell syntax: single quotes are literal.
fn exec_words(value: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        match character {
            '\0' | '\n' | '\r' => return None,
            '\\' => {
                token.push(chars.next()?);
                started = true;
            }
            '"' => {
                quoted = !quoted;
                started = true;
            }
            ' ' | '\t' if !quoted => {
                if started {
                    words.push(std::mem::take(&mut token));
                }
                started = false;
            }
            _ => {
                token.push(character);
                started = true;
            }
        }
    }
    if quoted {
        return None;
    }
    if started {
        words.push(token);
    }
    Some(words)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    use super::*;

    struct Fixture {
        root: TempDir,
        env: CatalogEnvironment,
    }

    impl Fixture {
        fn new() -> Self {
            let root = TempDir::new().unwrap();
            let env = HashMap::from([
                ("HOME".into(), root.path().to_string_lossy().into_owned()),
                (
                    "XDG_DATA_HOME".into(),
                    root.path().join("user").to_string_lossy().into_owned(),
                ),
                (
                    "XDG_DATA_DIRS".into(),
                    root.path().join("system").to_string_lossy().into_owned(),
                ),
                (
                    "XDG_CONFIG_DIRS".into(),
                    root.path().join("config").to_string_lossy().into_owned(),
                ),
                ("XDG_CURRENT_DESKTOP".into(), "clarion".into()),
                ("GTK_ICON_THEME".into(), "Fixture".into()),
                (
                    "PATH".into(),
                    root.path().join("bin").to_string_lossy().into_owned(),
                ),
            ]);
            fs::create_dir_all(root.path().join("bin")).unwrap();
            Self { root, env }
        }

        fn executable(&self, name: &str) -> PathBuf {
            let path = self.root.path().join("bin").join(name);
            fs::write(&path, "fixture executable, never invoked").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        }

        fn entry(&self, root: &str, id: &str, fields: &str) -> PathBuf {
            let path = self.root.path().join(root).join("applications").join(id);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                &path,
                format!("[Desktop Entry]\nType=Application\nName={id}\n{fields}"),
            )
            .unwrap();
            path
        }

        fn load(&self) -> AppCatalog {
            AppCatalog::load(&self.env).unwrap()
        }
    }

    #[test]
    fn overrides_mask_system_entries_even_when_hidden_invalid_or_oversized() {
        let fixture = Fixture::new();
        fixture.executable("available");
        for id in [
            "hidden.desktop",
            "nodisplay.desktop",
            "missing.desktop",
            "large.desktop",
            "bad.desktop",
        ] {
            fixture.entry("system", id, "Exec=available\n");
        }
        fixture.entry("user", "hidden.desktop", "Hidden=true\n");
        fixture.entry("user", "nodisplay.desktop", "NoDisplay=true\n");
        fixture.entry("user", "missing.desktop", "Exec=removed\n");
        let large = fixture.entry("user", "large.desktop", "Exec=available\n");
        File::options()
            .write(true)
            .open(large)
            .unwrap()
            .set_len(MAX_DESKTOP_BYTES + 1)
            .unwrap();
        let invalid = fixture.entry("user", "bad.desktop", "");
        fs::write(invalid, [0xff, 0xfe]).unwrap();
        fixture.entry("system", "visible.desktop", "Exec=available\n");
        let catalog = fixture.load();
        assert_eq!(
            catalog
                .apps()
                .iter()
                .map(|app| app.id.as_str())
                .collect::<Vec<_>>(),
            ["visible.desktop"]
        );
    }

    #[test]
    fn localizes_metadata_and_matches_all_unicode_search_terms() {
        let mut fixture = Fixture::new();
        fixture.env.insert("LANG".into(), "zh_CN.UTF-8".into());
        fixture.entry("user", "browser.desktop", "DBusActivatable=true\nName=Browser\nName[zh]=浏览器\nName[zh_CN]=空间浏览器\nComment[zh_CN]=读取\\s网页\nGenericName[zh_CN]=互联网\nKeywords[zh_CN]=中文;搜索;WEB;\nOnlyShowIn=clarion;GNOME;\nCategories=Network;WebBrowser;\n[Desktop Action private]\nName=Ignored\n");
        fixture.entry(
            "user",
            "excluded.desktop",
            "DBusActivatable=true\nNotShowIn=clarion;\n",
        );
        fixture.entry(
            "user",
            "other.desktop",
            "DBusActivatable=true\nOnlyShowIn=GNOME;\n",
        );
        let catalog = fixture.load();
        let app = catalog.get("browser.desktop").unwrap();
        assert_eq!(catalog.apps().len(), 1);
        assert_eq!(app.name, "空间浏览器");
        assert_eq!(app.description, "读取 网页");
        assert_eq!(app.category, AppCategory::InformationReading);
        assert_eq!(catalog.search("中文 WEB 互联网").len(), 1);
        assert_eq!(catalog.search("中文 missing").len(), 0);
        assert_eq!(catalog.search(" \t\n").len(), 1);
        assert_eq!(catalog.search("ignored").len(), 0);
    }

    #[test]
    fn empty_locale_overrides_fall_back_without_losing_localized_search() {
        let mut fixture = Fixture::new();
        fixture.entry(
            "user",
            "browser.desktop",
            "DBusActivatable=true\nName=Browser\nName[zh_CN]=浏览器\nName[en_US]=English Browser\nKeywords[zh_CN]=网页;\n",
        );
        for (all, messages, lang, name, query, matches) in [
            ("", "", "zh_CN.UTF-8", "浏览器", "网页", 1),
            ("", "zh_CN.UTF-8", "en_US.UTF-8", "浏览器", "网页", 1),
            (
                "en_US.UTF-8",
                "zh_CN.UTF-8",
                "zh_CN.UTF-8",
                "English Browser",
                "网页",
                0,
            ),
        ] {
            fixture.env.insert("LC_ALL".into(), all.into());
            fixture.env.insert("LC_MESSAGES".into(), messages.into());
            fixture.env.insert("LANG".into(), lang.into());
            let catalog = fixture.load();
            assert_eq!(
                catalog.get("browser.desktop").unwrap().name,
                name,
                "LC_ALL={all:?}, LC_MESSAGES={messages:?}, LANG={lang:?}"
            );
            assert_eq!(catalog.search(query).len(), matches);
        }
    }

    #[test]
    fn preserves_dbus_activation_but_checks_tryexec_and_executable_bits() {
        let fixture = Fixture::new();
        fixture.executable("available");
        fixture.entry(
            "user",
            "dbus.desktop",
            "DBusActivatable=true\nExec=removed-optional-fallback\n",
        );
        fixture.entry(
            "user",
            "valid.desktop",
            "Exec=available\nTryExec=available\n",
        );
        fixture.entry(
            "user",
            "removed.desktop",
            "DBusActivatable=true\nTryExec=removed\n",
        );
        fixture.entry("user", "empty.desktop", "Exec=\n");
        fs::write(fixture.root.path().join("bin/plain"), "not executable").unwrap();
        fs::create_dir(fixture.root.path().join("bin/folder")).unwrap();
        fixture.entry("user", "plain.desktop", "Exec=plain\n");
        fixture.entry("user", "folder.desktop", "Exec=folder\n");
        let catalog = fixture.load();
        assert_eq!(
            catalog
                .apps()
                .iter()
                .map(|app| app.id.as_str())
                .collect::<Vec<_>>(),
            ["dbus.desktop", "valid.desktop"]
        );
    }

    #[test]
    fn checks_quoted_programs_and_env_path_without_evaluating_payloads() {
        let fixture = Fixture::new();
        fixture.executable("env");
        let quoted = fixture.executable("a program");
        let wine = fixture.executable("wine");
        fixture.entry(
            "user",
            "quoted.desktop",
            &format!("Exec=\"{}\" $(touch ignored) %U\n", quoted.display()),
        );
        fixture.entry(
            "user",
            "wine.desktop",
            "Exec=env WINEPREFIX=/preserved wine \"C:\\App.exe\" %f\n",
        );
        fixture.entry(
            "user",
            "env-path.desktop",
            &format!(
                "Exec=env \"PATH={}\" -- wine %f\n",
                wine.parent().unwrap().display()
            ),
        );
        fixture.entry(
            "user",
            "missing.desktop",
            "Exec=env WINEPREFIX=/preserved removed %f\n",
        );
        fixture.entry(
            "user",
            "options.desktop",
            "Exec=env -S \"unknown grammar\"\n",
        );
        fixture.entry(
            "user",
            "relative.desktop",
            "Exec=./application --directory-dependent\n",
        );
        let catalog = fixture.load();
        assert_eq!(catalog.apps().len(), 5);
        assert!(catalog.get("missing.desktop").is_none());
        assert!(!fixture.root.path().join("ignored").exists());
    }

    #[test]
    fn handles_nested_ids_and_symlink_files_without_following_directory_links() {
        let fixture = Fixture::new();
        let source = fixture.entry("user", "nested/app.desktop", "DBusActivatable=true\n");
        let exported = fixture
            .root
            .path()
            .join("user/applications/exported.desktop");
        symlink(&source, &exported).unwrap();
        let outside = fixture.root.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            outside.join("unexpected.desktop"),
            "[Desktop Entry]\nType=Application\nName=Unexpected\nDBusActivatable=true\n",
        )
        .unwrap();
        symlink(
            outside,
            fixture.root.path().join("user/applications/directory"),
        )
        .unwrap();
        let catalog = fixture.load();
        assert_eq!(catalog.apps().len(), 2);
        assert!(catalog.get("nested-app.desktop").is_some());
        assert!(catalog.get("exported.desktop").is_some());
        fs::remove_file(source).unwrap();
        assert!(fixture.load().apps().is_empty());
    }

    #[test]
    fn xdg_roots_are_absolute_ordered_deduplicated_and_relative_path_is_uncertain() {
        let mut fixture = Fixture::new();
        let system = fixture
            .root
            .path()
            .join("system")
            .to_string_lossy()
            .into_owned();
        fixture
            .env
            .insert("XDG_DATA_HOME".into(), "relative".into());
        fixture.env.insert(
            "XDG_DATA_DIRS".into(),
            format!("{system}:relative:{system}"),
        );
        assert_eq!(
            data_roots(&fixture.env),
            [
                fixture.root.path().join(".local/share"),
                PathBuf::from(system)
            ]
        );
        fixture.env.insert(
            "XDG_DATA_HOME".into(),
            fixture
                .root
                .path()
                .join("user")
                .to_string_lossy()
                .into_owned(),
        );
        fixture.entry("user", "uncertain.desktop", "Exec=working-directory-tool\n");
        assert!(fixture.load().apps().is_empty());
        fixture.env.insert(
            "PATH".into(),
            format!("{}:", fixture.root.path().join("bin").display()),
        );
        assert_eq!(fixture.load().apps().len(), 1);
    }

    #[test]
    fn recent_suggestions_preserve_order_skip_missing_and_deduplicate() {
        let fixture = Fixture::new();
        fixture.entry("user", "alpha.desktop", "DBusActivatable=true\n");
        fixture.entry("user", "beta.desktop", "DBusActivatable=true\n");
        let catalog = fixture.load();
        let recent =
            ["missing", " beta.desktop ", "beta.desktop", "alpha.desktop"].map(str::to_owned);
        assert_eq!(
            catalog
                .suggested(&recent, 5)
                .iter()
                .map(|app| app.id.as_str())
                .collect::<Vec<_>>(),
            ["beta.desktop", "alpha.desktop"]
        );
        assert_eq!(catalog.suggested(&recent, 1).len(), 1);
        assert!(catalog.suggested(&recent, 0).is_empty());
    }

    #[test]
    fn maps_specific_xdg_categories_to_one_fixed_tahoe_group() {
        for (raw, expected) in [
            ("Network;WebBrowser;", AppCategory::InformationReading),
            ("AudioVideo;Player;", AppCategory::Entertainment),
            ("AudioVideo;AudioVideoEditing;", AppCategory::Creativity),
            ("Network;InstantMessaging;", AppCategory::Social),
            ("Office;Finance;", AppCategory::ProductivityFinance),
            ("System;Utility;", AppCategory::Utilities),
            ("Unrecognized;", AppCategory::Other),
        ] {
            assert_eq!(AppCategory::from_xdg(&string_list(raw)), expected);
        }
        assert_eq!(AppCategory::ALL.len(), 7);
        assert_eq!(AppCategory::ProductivityFinance.title(), "生产力与财务");
    }

    #[test]
    fn resolves_catalog_icons_from_theme_to_immediately_renderable_paths() {
        let fixture = Fixture::new();
        let icon = fixture
            .root
            .path()
            .join("user/icons/Fixture/scalable/apps/catalog.svg");
        fs::create_dir_all(icon.parent().unwrap()).unwrap();
        fs::write(&icon, "<svg/>").unwrap();
        fixture.entry(
            "user",
            "app.desktop",
            "DBusActivatable=true\nIcon=catalog\n",
        );
        let catalog = fixture.load();
        let app = catalog.get("app.desktop").unwrap();
        assert_eq!(app.icon_path.as_ref(), Some(&icon));
        assert_eq!(app.icon_name.as_deref(), Some("catalog"));
    }

    #[test]
    fn locale_modifier_and_language_fallback_preserve_desktop_entry_escapes() {
        let mut env = CatalogEnvironment::new();
        env.insert("LC_ALL".into(), "sr_RS.UTF-8@latin".into());
        env.insert("LANG".into(), "zh_CN.UTF-8".into());
        assert_eq!(
            locale_candidates(&env),
            ["sr_RS@latin", "sr_RS", "sr@latin", "sr"]
        );
        let values = parse_desktop_entry("[Desktop Entry]\nName=Base\nName[sr]=Language\nName[sr@latin]=Latin\nComment=Space\\sand\\nnewline\\q\n");
        assert_eq!(
            localized_value(&values, "Name", &locale_candidates(&env)),
            "Latin"
        );
        assert_eq!(values["Comment"], "Space and\nnewline\\q");
    }
}
