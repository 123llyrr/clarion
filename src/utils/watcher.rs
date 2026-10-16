//! File modification watcher.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};
use std::{io, thread};

use clarion_config::{Config, ConfigParseResult, ConfigPath};
use smithay::reexports::calloop::channel::SyncSender;

use crate::compositor::State;

const POLLING_INTERVAL: Duration = Duration::from_millis(500);

pub struct Watcher {
    load_config: mpsc::Sender<Option<String>>,
}

struct WatcherInner {
    /// The paths we're watching.
    path: ConfigPath,

    /// Last observed props of the watched file.
    last_props: Option<Props>,

    /// Last observed props for included files.
    includes: HashMap<PathBuf, Option<Props>>,
}

/// Properties of the watched file.
///
/// Equality on this means the file did not change.
#[derive(Debug, PartialEq, Eq)]
struct Props {
    /// Modification time of the watched file.
    mtime: SystemTime,

    /// Detect in-place writes that restore mtime and replacements with preserved timestamps.
    ctime: (i64, i64),
    device: u64,
    inode: u64,
    size: u64,

    /// Canonical form of the watched path.
    ///
    /// We store the absolute path in addition to mtime to account for symlinked configs where the
    /// symlink target may change without mtime. This is common on nix where everything is a
    /// symlink to /nix/store, which keeps no mtime (= 1970-01-01).
    canonical: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
enum CheckResult {
    Missing,
    Unchanged,
    Changed,
}

impl Watcher {
    pub fn new(
        path: ConfigPath,
        includes: Vec<PathBuf>,
        mut process: impl FnMut(&ConfigPath) -> ConfigParseResult<Config, ()> + Send + 'static,
        changed: SyncSender<Result<Config, ()>>,
    ) -> Self {
        let (load_config, load_config_rx) = mpsc::channel();

        thread::Builder::new()
            .name(format!("Filesystem Watcher for {path:?}"))
            .spawn(move || {
                let mut inner = WatcherInner::new(path, includes);

                loop {
                    let mut should_load = match load_config_rx.recv_timeout(POLLING_INTERVAL) {
                        Ok(path) => {
                            if let Some(path) = path {
                                inner = WatcherInner::new(
                                    ConfigPath::Explicit(PathBuf::from(path)),
                                    Vec::new(),
                                );
                            }
                            true
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => false,
                    };

                    match inner.check() {
                        CheckResult::Missing if !should_load => continue,
                        // Explicit requests must report load failures to the caller even
                        // if the file disappeared since IPC validation.
                        CheckResult::Missing | CheckResult::Unchanged => (),
                        CheckResult::Changed => {
                            trace!("config file changed");
                            should_load = true;
                        }
                    }

                    if should_load {
                        let res = process(&inner.path);

                        if let Err(err) = changed.send(res.config) {
                            warn!("error sending change notification: {err:?}");
                            break;
                        }

                        // Keep the pre-load observations for known includes so edits during
                        // parsing are detected on the next poll. Newly discovered includes
                        // can only be observed once their paths have been parsed.
                        inner.set_includes(res.includes);
                    }
                }

                debug!("exiting watcher thread for {:?}", inner.path);
            })
            .unwrap();

        Self { load_config }
    }

    pub fn load_config(&self, path: Option<String>) {
        let _ = self.load_config.send(path);
    }
}

impl Props {
    fn from_path(path: &Path) -> io::Result<Self> {
        let canonical = path.canonicalize()?;
        let metadata = canonical.metadata()?;
        Ok(Self {
            mtime: metadata.modified()?,
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            canonical,
        })
    }

    fn from_config_path(config_path: &ConfigPath) -> io::Result<Self> {
        match config_path {
            ConfigPath::Explicit(path) => Self::from_path(path),
            ConfigPath::Regular {
                user_path,
                system_path,
            } => match std::fs::symlink_metadata(user_path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    Self::from_path(system_path)
                }
                _ => Self::from_path(user_path),
            },
        }
    }
}

impl WatcherInner {
    pub fn new(path: ConfigPath, includes: Vec<PathBuf>) -> Self {
        let last_props = Props::from_config_path(&path).ok();

        let mut rv = Self {
            path,
            last_props,
            includes: HashMap::new(),
        };
        rv.set_includes(includes);
        rv
    }

    pub fn check(&mut self) -> CheckResult {
        if let Ok(new_props) = Props::from_config_path(&self.path) {
            let mut changed = self.last_props.as_ref() != Some(&new_props);
            self.last_props = Some(new_props);
            // Snapshot every known include before loading, even when the main file changed.
            for (path, last_props) in &mut self.includes {
                let new_props = Props::from_path(path).ok();

                // A disappearing include also requires a reload.
                changed |= *last_props != new_props;
                *last_props = new_props;
            }

            if changed {
                CheckResult::Changed
            } else {
                CheckResult::Unchanged
            }
        } else {
            CheckResult::Missing
        }
    }

    fn set_includes(&mut self, includes: Vec<PathBuf>) {
        let mut previous = std::mem::take(&mut self.includes);
        for path in includes {
            self.includes.entry(path).or_insert_with_key(|path| {
                previous
                    .remove(path)
                    .unwrap_or_else(|| Props::from_path(path).ok())
            });
        }
    }
}

pub fn setup(state: &mut State, config_path: &ConfigPath, includes: Vec<PathBuf>) {
    // Parsing the config actually takes > 20 ms on my beefy machine, so let's do it on the
    // watcher thread.
    let process = |path: &ConfigPath| {
        path.load().map_config_res(|res| {
            res.map_err(|err| {
                warn!("{err:?}");
            })
        })
    };

    let (tx, rx) = calloop::channel::sync_channel(1);
    state
        .clarion
        .event_loop
        .insert_source(
            rx,
            |event: calloop::channel::Event<Result<Config, ()>>, _, state| match event {
                calloop::channel::Event::Msg(config) => {
                    let failed = config.is_err();
                    state.reload_config(config);
                    state.ipc_config_loaded(failed);
                }
                calloop::channel::Event::Closed => (),
            },
        )
        .unwrap();

    let watcher = Watcher::new(config_path.clone(), includes, process, tx);
    state.clarion.config_file_watcher = Some(watcher);
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs::{self, File, FileTimes};
    use std::io::Write;

    use xshell::{cmd, Shell, TempDir};

    use super::*;

    type Result<T = (), E = Box<dyn Error>> = std::result::Result<T, E>;

    #[test]
    fn broken_user_config_does_not_watch_a_different_system_config() {
        let sh = Shell::new().unwrap();
        let dir = sh.create_temp_dir().unwrap();
        let user_path = dir.path().join("clarion.kdl");
        let system_path = dir.path().join("system.kdl");
        fs::write(&system_path, "").unwrap();
        std::os::unix::fs::symlink(dir.path().join("missing.kdl"), &user_path).unwrap();
        let config = ConfigPath::Regular {
            user_path: user_path.clone(),
            system_path: system_path.clone(),
        };
        assert!(Props::from_config_path(&config).is_err());
        fs::remove_file(user_path).unwrap();
        assert_eq!(
            Props::from_config_path(&config).unwrap().canonical,
            system_path.canonicalize().unwrap()
        );
    }

    fn canon(config_path: &ConfigPath) -> &PathBuf {
        match config_path {
            ConfigPath::Explicit(path) => path,
            ConfigPath::Regular {
                user_path,
                system_path,
            } => {
                if user_path.exists() {
                    user_path
                } else {
                    system_path
                }
            }
        }
    }

    enum TestPath<P> {
        Explicit(P),
        Regular { user_path: P, system_path: P },
    }

    impl<P: AsRef<Path>> TestPath<P> {
        fn setup<Discard>(
            self,
            setup: impl FnOnce(&Shell) -> xshell::Result<Discard>,
        ) -> TestSetup {
            self.setup_any(|sh| {
                _ = setup(sh)?;
                Ok(())
            })
        }

        fn without_setup(self) -> TestSetup {
            self.setup_any(|_| Ok(())).assert_initial_not_exists()
        }

        fn setup_any(self, setup: impl FnOnce(&Shell) -> Result) -> TestSetup {
            let sh = Shell::new().unwrap();
            let temp_dir = sh.create_temp_dir().unwrap();
            sh.change_dir(temp_dir.path());

            let dir = sh.current_dir();
            let config_path = match self {
                TestPath::Explicit(path) => ConfigPath::Explicit(dir.join(path)),
                TestPath::Regular {
                    user_path,
                    system_path,
                } => ConfigPath::Regular {
                    user_path: dir.join(user_path),
                    system_path: dir.join(system_path),
                },
            };

            setup(&sh).unwrap();

            TestSetup {
                sh,
                config_path,
                _temp_dir: temp_dir,
            }
        }
    }

    struct TestSetup {
        sh: Shell,
        config_path: ConfigPath,
        _temp_dir: TempDir,
    }

    impl TestSetup {
        fn assert_initial_not_exists(self) -> Self {
            let canon = canon(&self.config_path);
            assert!(!canon.exists(), "initial should not exist");
            self
        }

        fn assert_initial(self, expected: &str) -> Self {
            let canon = canon(&self.config_path);
            assert!(canon.exists(), "initial should exist at {canon:?}");
            let actual = fs::read_to_string(canon).unwrap();
            assert_eq!(actual, expected, "initial file contents do not match");
            self
        }

        fn run(self, body: impl FnOnce(&Shell, &mut TestUtil) -> Result) -> Result {
            let TestSetup {
                sh, config_path, ..
            } = self;

            let includes = config_path.load().includes;
            let mut test = TestUtil {
                watcher: WatcherInner::new(config_path, includes),
            };

            // don't trigger before we start
            test.assert_unchanged();
            // pass_time() inside assert_unchanged() ensures that mtime
            // isn't the same as the initial time

            body(&sh, &mut test)?;

            // nothing should trigger after the test runs
            test.assert_unchanged();

            Ok(())
        }
    }

    struct TestUtil {
        watcher: WatcherInner,
    }

    impl TestUtil {
        // Ensures that mtime is different between writes in the tests.
        fn pass_time(&self) {
            thread::sleep(Duration::from_millis(50));
        }

        fn assert_unchanged(&mut self) {
            let res = self.watcher.check();

            // This may be Missing or Unchanged, both are fine.
            assert_ne!(
                res,
                CheckResult::Changed,
                "watcher should not have noticed any changes"
            );

            self.pass_time();
        }

        fn assert_changed_to(&mut self, expected: &str) {
            let res = self.watcher.check();
            assert_eq!(
                res,
                CheckResult::Changed,
                "watcher should have noticed a change, but it didn't"
            );

            let new_path = canon(&self.watcher.path);
            let actual = fs::read_to_string(new_path).unwrap();
            assert_eq!(actual, expected, "wrong file contents");

            self.watcher.set_includes(Config::load(new_path).includes);

            self.pass_time();
        }
    }

    #[test]
    fn explicit_reload_of_missing_config_reports_failure() {
        let sh = Shell::new().unwrap();
        let dir = sh.create_temp_dir().unwrap();
        let missing = dir.path().join("missing.kdl");
        for requested_path in [None, Some(missing.to_string_lossy().into_owned())] {
            let (observed_tx, observed_rx) = mpsc::channel();
            let (changed_tx, _changed_rx) = calloop::channel::sync_channel(1);
            let watcher = Watcher::new(
                ConfigPath::Explicit(missing.clone()),
                Vec::new(),
                move |path| {
                    let result = path.load().map_config_res(|result| result.map_err(|_| ()));
                    observed_tx.send(result.config.is_err()).unwrap();
                    result
                },
                changed_tx,
            );
            // Missing files remain quiet during ordinary polling.
            assert!(matches!(
                observed_rx.recv_timeout(POLLING_INTERVAL * 2),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            watcher.load_config(requested_path);
            assert!(observed_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("explicit reload must be processed even when its file is missing"));
        }
    }

    #[test]
    fn change_file() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| sh.write_file("clarion/config.kdl", "a"))
            .assert_initial("a")
            .run(|sh, test| {
                sh.write_file("clarion/config.kdl", "b")?;
                test.assert_changed_to("b");

                Ok(())
            })
    }

    #[test]
    fn overwrite_but_dont_change_file() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| sh.write_file("clarion/config.kdl", "a"))
            .assert_initial("a")
            .run(|sh, test| {
                sh.write_file("clarion/config.kdl", "a")?;
                test.assert_changed_to("a");

                Ok(())
            })
    }

    #[test]
    fn touch_file() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| sh.write_file("clarion/config.kdl", "a"))
            .assert_initial("a")
            .run(|sh, test| {
                cmd!(sh, "touch clarion/config.kdl").run()?;
                test.assert_changed_to("a");

                Ok(())
            })
    }

    #[test]
    fn create_file() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| sh.create_dir("clarion"))
            .assert_initial_not_exists()
            .run(|sh, test| {
                sh.write_file("clarion/config.kdl", "a")?;
                test.assert_changed_to("a");

                Ok(())
            })
    }

    #[test]
    fn create_dir_and_file() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .without_setup()
            .run(|sh, test| {
                sh.write_file("clarion/config.kdl", "a")?;
                test.assert_changed_to("a");

                Ok(())
            })
    }

    #[test]
    fn change_linked_file() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| {
                sh.write_file("clarion/config2.kdl", "a")?;
                cmd!(sh, "ln -sf config2.kdl clarion/config.kdl").run()
            })
            .assert_initial("a")
            .run(|sh, test| {
                sh.write_file("clarion/config2.kdl", "b")?;
                test.assert_changed_to("b");

                Ok(())
            })
    }

    #[test]
    fn change_file_in_linked_dir() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| {
                sh.write_file("cfg2/config.kdl", "a")?;
                cmd!(sh, "ln -s cfg2 clarion").run()
            })
            .assert_initial("a")
            .run(|sh, test| {
                sh.write_file("cfg2/config.kdl", "b")?;
                test.assert_changed_to("b");

                Ok(())
            })
    }

    #[test]
    fn remove_file() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| sh.write_file("clarion/config.kdl", "a"))
            .assert_initial("a")
            .run(|sh, test| {
                sh.remove_path("clarion/config.kdl")?;
                test.assert_unchanged();

                Ok(())
            })
    }

    #[test]
    fn remove_dir() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| sh.write_file("clarion/config.kdl", "a"))
            .assert_initial("a")
            .run(|sh, test| {
                sh.remove_path("clarion")?;
                test.assert_unchanged();

                Ok(())
            })
    }

    #[test]
    fn recreate_file() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| sh.write_file("clarion/config.kdl", "a"))
            .assert_initial("a")
            .run(|sh, test| {
                sh.remove_path("clarion/config.kdl")?;
                sh.write_file("clarion/config.kdl", "b")?;
                test.assert_changed_to("b");

                Ok(())
            })
    }

    #[test]
    fn recreate_dir() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| {
                sh.write_file("clarion/config.kdl", "a")?;
                Ok(())
            })
            .assert_initial("a")
            .run(|sh, test| {
                sh.remove_path("clarion")?;
                sh.write_file("clarion/config.kdl", "b")?;
                test.assert_changed_to("b");

                Ok(())
            })
    }

    #[test]
    fn swap_dir() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| sh.write_file("clarion/config.kdl", "a"))
            .assert_initial("a")
            .run(|sh, test| {
                sh.write_file("cfg2/config.kdl", "b")?;
                sh.remove_path("clarion")?;
                cmd!(sh, "mv cfg2 clarion").run()?;
                test.assert_changed_to("b");

                Ok(())
            })
    }

    #[test]
    fn swap_dir_link() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| {
                sh.write_file("cfg2/config.kdl", "a")?;
                cmd!(sh, "ln -s cfg2 clarion").run()
            })
            .assert_initial("a")
            .run(|sh, test| {
                sh.write_file("cfg3/config.kdl", "b")?;
                sh.remove_path("clarion")?;
                cmd!(sh, "ln -s cfg3 clarion").run()?;
                test.assert_changed_to("b");

                Ok(())
            })
    }

    #[test]
    fn change_included_file() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| {
                sh.write_file("clarion/config.kdl", "include \"colors.kdl\"")?;
                sh.write_file("clarion/colors.kdl", "// Colors")
            })
            .assert_initial("include \"colors.kdl\"")
            .run(|sh, test| {
                sh.write_file("clarion/colors.kdl", "// Updated colors")?;
                test.assert_changed_to("include \"colors.kdl\"");

                Ok(())
            })
    }

    #[test]
    fn remove_included_file() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| {
                sh.write_file("clarion/config.kdl", "include \"colors.kdl\"")?;
                sh.write_file("clarion/colors.kdl", "// Colors")
            })
            .assert_initial("include \"colors.kdl\"")
            .run(|sh, test| {
                sh.remove_path("clarion/colors.kdl")?;
                test.assert_changed_to("include \"colors.kdl\"");

                Ok(())
            })
    }

    #[test]
    fn nested_includes() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| {
                sh.write_file("clarion/config.kdl", "include \"a.kdl\"")?;
                sh.write_file("clarion/a.kdl", "include \"b.kdl\"")?;
                sh.write_file("clarion/b.kdl", "// b content")
            })
            .assert_initial("include \"a.kdl\"")
            .run(|sh, test| {
                sh.write_file("clarion/b.kdl", "// updated b")?;
                test.assert_changed_to("include \"a.kdl\"");

                Ok(())
            })
    }

    #[test]
    fn broken_include_still_gets_watched() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup(|sh| {
                sh.write_file("clarion/config.kdl", "include \"colors.kdl\"")?;
                sh.write_file("clarion/colors.kdl", "broken")
            })
            .assert_initial("include \"colors.kdl\"")
            .run(|sh, test| {
                sh.write_file("clarion/colors.kdl", "// Fixed")?;
                test.assert_changed_to("include \"colors.kdl\"");

                Ok(())
            })
    }

    // Important: On systems like NixOS, mtime is not kept for config files.
    // So, this is testing that the watcher handles that correctly.
    fn create_epoch(path: impl AsRef<Path>, content: &str) -> Result {
        let mut file = File::create(path)?;
        file.write_all(content.as_bytes())?;
        file.set_times(
            FileTimes::new()
                .set_accessed(SystemTime::UNIX_EPOCH)
                .set_modified(SystemTime::UNIX_EPOCH),
        )?;
        file.sync_all()?;
        Ok(())
    }

    #[test]
    fn replace_file_preserving_mtime() -> Result {
        TestPath::Explicit("config.kdl")
            .setup_any(|sh| create_epoch(sh.current_dir().join("config.kdl"), "a"))
            .assert_initial("a")
            .run(|sh, test| {
                create_epoch(sh.current_dir().join("replacement.kdl"), "b")?;
                fs::rename(
                    sh.current_dir().join("replacement.kdl"),
                    sh.current_dir().join("config.kdl"),
                )?;
                test.assert_changed_to("b");
                Ok(())
            })
    }

    #[test]
    fn overwrite_file_preserving_mtime() -> Result {
        TestPath::Explicit("config.kdl")
            .setup_any(|sh| create_epoch(sh.current_dir().join("config.kdl"), "a"))
            .assert_initial("a")
            .run(|sh, test| {
                create_epoch(sh.current_dir().join("config.kdl"), "b")?;
                test.assert_changed_to("b");
                Ok(())
            })
    }

    #[test]
    fn overwrite_include_preserving_mtime() -> Result {
        TestPath::Explicit("config.kdl")
            .setup_any(|sh| {
                sh.write_file("config.kdl", "include \"colors.kdl\"")?;
                create_epoch(sh.current_dir().join("colors.kdl"), "// a")
            })
            .assert_initial("include \"colors.kdl\"")
            .run(|sh, test| {
                create_epoch(sh.current_dir().join("colors.kdl"), "// b")?;
                test.assert_changed_to("include \"colors.kdl\"");
                Ok(())
            })
    }

    #[test]
    fn include_change_during_reload_is_not_swallowed() -> Result {
        TestPath::Explicit("config.kdl")
            .setup(|sh| {
                sh.write_file("config.kdl", "include \"colors.kdl\"")?;
                sh.write_file("colors.kdl", "// a")
            })
            .assert_initial("include \"colors.kdl\"")
            .run(|sh, test| {
                // Begin a reload, then change an include after it has been read.
                sh.write_file("config.kdl", "include \"colors.kdl\"\n")?;
                assert_eq!(test.watcher.check(), CheckResult::Changed);
                let loaded = test.watcher.path.load();
                sh.write_file("colors.kdl", "// b")?;
                test.watcher.set_includes(loaded.includes);

                // The next poll must load the include's new contents.
                test.assert_changed_to("include \"colors.kdl\"\n");
                Ok(())
            })
    }

    #[test]
    fn swap_just_link() -> Result {
        TestPath::Explicit("clarion/config.kdl")
            .setup_any(|sh| {
                let dir = sh.current_dir().join("clarion");

                sh.create_dir(&dir)?;

                create_epoch(dir.join("config2.kdl"), "a")?;
                create_epoch(dir.join("config3.kdl"), "b")?;

                cmd!(sh, "ln -s config2.kdl clarion/config.kdl").run()?;

                Ok(())
            })
            .assert_initial("a")
            .run(|sh, test| {
                cmd!(sh, "ln -sf config3.kdl clarion/config.kdl").run()?;
                test.assert_changed_to("b");

                Ok(())
            })
    }

    #[test]
    fn swap_many_regular() -> Result {
        TestPath::Regular {
            user_path: "user-clarion/config.kdl",
            system_path: "system-clarion/config.kdl",
        }
        .setup(|sh| sh.write_file("system-clarion/config.kdl", "system config"))
        .assert_initial("system config")
        .run(|sh, test| {
            sh.write_file("user-clarion/config.kdl", "user config")?;
            test.assert_changed_to("user config");

            cmd!(sh, "touch system-clarion/config.kdl").run()?;
            test.assert_unchanged();

            sh.remove_path("system-clarion")?;
            test.assert_unchanged();

            sh.write_file("system-clarion/config.kdl", "new system config")?;
            test.assert_unchanged();

            sh.remove_path("user-clarion")?;
            test.assert_changed_to("new system config");

            sh.write_file("system-clarion/config.kdl", "updated system config")?;
            test.assert_changed_to("updated system config");

            sh.write_file("user-clarion/config.kdl", "new user config")?;
            test.assert_changed_to("new user config");

            Ok(())
        })
    }

    #[test]
    fn swap_many_links_regular_like_nix() -> Result {
        TestPath::Regular {
            user_path: "user-clarion/config.kdl",
            system_path: "system-clarion/config.kdl",
        }
        .setup_any(|sh| {
            let store = sh.current_dir().join("store");

            sh.create_dir(&store)?;

            create_epoch(store.join("gen1"), "gen 1")?;
            create_epoch(store.join("gen2"), "gen 2")?;
            create_epoch(store.join("gen3"), "gen 3")?;

            sh.create_dir("user-clarion")?;
            sh.create_dir("system-clarion")?;

            Ok(())
        })
        .assert_initial_not_exists()
        .run(|sh, test| {
            let store = sh.current_dir().join("store");
            test.assert_unchanged();

            cmd!(sh, "ln -s {store}/gen1 user-clarion/config.kdl").run()?;
            test.assert_changed_to("gen 1");

            cmd!(sh, "ln -s {store}/gen2 system-clarion/config.kdl").run()?;
            test.assert_unchanged();

            cmd!(sh, "unlink user-clarion/config.kdl").run()?;
            test.assert_changed_to("gen 2");

            cmd!(sh, "ln -s {store}/gen3 user-clarion/config.kdl").run()?;
            test.assert_changed_to("gen 3");

            cmd!(sh, "ln -sf {store}/gen1 system-clarion/config.kdl").run()?;
            test.assert_unchanged();

            cmd!(sh, "unlink system-clarion/config.kdl").run()?;
            test.assert_unchanged();

            cmd!(sh, "ln -s {store}/gen1 system-clarion/config.kdl").run()?;
            test.assert_unchanged();

            cmd!(sh, "unlink user-clarion/config.kdl").run()?;
            test.assert_changed_to("gen 1");

            Ok(())
        })
    }
}
