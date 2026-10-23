// SPDX-License-Identifier: GPL-3.0-or-later
//! GIO owns Desktop Entry expansion; only one application invocation is attempted.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{fs, thread};

use anyhow::{bail, Context, Result};

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CAPTURE: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ApplicationScope {
    expand_environment: bool,
    collect: bool,
}

impl ApplicationScope {
    fn from_help(help: &str, version: &str) -> Result<Self> {
        let release = version.lines().find_map(|line| {
            let suffix = line.strip_prefix("systemd")?;
            if !suffix.starts_with(char::is_whitespace) {
                return None;
            }
            suffix
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u32>()
                .ok()
        });
        let release = release.context("systemd-run did not report a valid release")?;
        if !["--scope", "--user", "--slice="]
            .iter()
            .all(|flag| help.contains(flag))
        {
            bail!("systemd-run does not support an independent user application scope");
        }
        let expand_environment = help.contains("--expand-environment=");
        // Scope ran literal argv before v254. Modern clients must advertise
        // the explicit opt-out; never guess their environment-expansion default.
        if release >= 254 && !expand_environment {
            bail!("systemd-run cannot guarantee literal application arguments");
        }
        Ok(Self {
            expand_environment,
            collect: help.contains("--collect"),
        })
    }

    fn arguments(self, program: &str, arguments: &[OsString]) -> Vec<OsString> {
        let mut result: Vec<OsString> = ["--user", "--scope", "--quiet"]
            .into_iter()
            .map(Into::into)
            .collect();
        if self.collect {
            result.push("--collect".into());
        }
        result.push("--slice=app.slice".into());
        if self.expand_environment {
            result.push("--expand-environment=no".into());
        }
        result.extend([OsString::from("--"), OsString::from(program)]);
        result.extend_from_slice(arguments);
        result
    }
}

trait CommandRunner {
    fn probe(
        &mut self,
        program: &str,
        arguments: &[OsString],
        env: &HashMap<String, String>,
    ) -> Result<String>;
    fn launch(
        &mut self,
        program: &str,
        arguments: &[OsString],
        env: &HashMap<String, String>,
    ) -> Result<()>;
}

struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn probe(
        &mut self,
        program: &str,
        arguments: &[OsString],
        env: &HashMap<String, String>,
    ) -> Result<String> {
        probe_with_timeout(program, arguments, env, PROBE_TIMEOUT)
    }

    fn launch(
        &mut self,
        program: &str,
        arguments: &[OsString],
        env: &HashMap<String, String>,
    ) -> Result<()> {
        let mut child = Command::new(program)
            .args(arguments)
            .env_clear()
            .envs(env)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("Start application launch helper {program}"))?;
        let deadline = Instant::now() + LAUNCH_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        bail!(
                            "Application launch failed: {program} ({status}); see launcher journal"
                        );
                    }
                    return Ok(());
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                result => {
                    // Only stop the helper; an app already placed in a scope
                    // remains owned by the user manager, independent of us.
                    let _ = child.kill();
                    let _ = child.wait();
                    match result {
                        Err(error) => {
                            return Err(error).context("Wait for application launch helper")
                        }
                        _ => {
                            bail!("Application launch helper {program} timed out after 10 seconds")
                        }
                    }
                }
            }
        }
    }
}

fn nonblocking(stream: &impl AsRawFd) -> Result<()> {
    let fd = stream.as_raw_fd();
    // SAFETY: fd is borrowed from the still-owned pipe; these operations only
    // read/change its descriptor flags and do not transfer ownership.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error()).context("Set nonblocking probe output");
    }
    Ok(())
}

fn drain(stream: &mut impl Read, retained: &mut Vec<u8>, eof: &mut bool) -> Result<()> {
    if *eof {
        return Ok(());
    }
    let mut buffer = [0; 8192];
    // Bound each turn even if a broken probe continuously writes output, so
    // the timeout remains enforceable while excess bytes are discarded.
    for _ in 0..32 {
        match stream.read(&mut buffer) {
            Ok(0) => {
                *eof = true;
                break;
            }
            Ok(count) => {
                let keep = count.min(MAX_CAPTURE - retained.len());
                retained.extend_from_slice(&buffer[..keep]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).context("Read probe output"),
        }
    }
    Ok(())
}

fn probe_with_timeout(
    program: &str,
    arguments: &[OsString],
    env: &HashMap<String, String>,
    timeout: Duration,
) -> Result<String> {
    let mut child = Command::new(program)
        .args(arguments)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Start capability probe {program}"))?;
    let result = (|| {
        let mut stdout = child.stdout.take().context("Missing probe stdout")?;
        let mut stderr = child.stderr.take().context("Missing probe stderr")?;
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        let (mut output, mut errors) = (Vec::new(), Vec::new());
        let (mut stdout_eof, mut stderr_eof) = (false, false);
        let deadline = Instant::now() + timeout;
        let mut status = None;
        loop {
            drain(&mut stdout, &mut output, &mut stdout_eof)?;
            drain(&mut stderr, &mut errors, &mut stderr_eof)?;
            if status.is_none() {
                status = child.try_wait().context("Wait for capability probe")?;
            }
            if let Some(status) = status {
                if stdout_eof && stderr_eof {
                    if !status.success() {
                        let error = String::from_utf8_lossy(&errors);
                        if error.trim().is_empty() {
                            bail!("{program} failed ({status})");
                        }
                        bail!("{program} failed ({status}): {}", error.trim());
                    }
                    return Ok(String::from_utf8_lossy(&output).into_owned());
                }
            }
            if Instant::now() >= deadline {
                bail!("Capability probe {program} timed out");
            }
            thread::sleep(Duration::from_millis(5));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn probe_scope(
    env: &HashMap<String, String>,
    runner: &mut impl CommandRunner,
) -> Result<Option<ApplicationScope>> {
    let Some(runtime) = env
        .get("XDG_RUNTIME_DIR")
        .filter(|runtime| runtime.starts_with('/'))
    else {
        return Ok(None);
    };
    match fs::metadata(Path::new(runtime).join("systemd/private")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Inspect user-manager socket"),
        Ok(_) => {}
    }
    runner.probe(
        "systemctl",
        &[
            "--user".into(),
            "show".into(),
            "--property=Version".into(),
            "--value".into(),
        ],
        env,
    )?;
    let help = runner.probe("systemd-run", &["--help".into()], env)?;
    let version = runner.probe("systemd-run", &["--version".into()], env)?;
    Ok(Some(ApplicationScope::from_help(&help, &version)?))
}

fn launch_with_runner(
    path: &Path,
    files: &[PathBuf],
    env: &HashMap<String, String>,
    runner: &mut impl CommandRunner,
) -> Result<()> {
    // Finish every probe before invoking GIO. Failure after this point must
    // reach the caller unchanged: retrying directly could open the app twice.
    let scope = probe_scope(env, runner)?;
    let mut arguments = vec![
        OsString::from("launch"),
        path.as_os_str().to_owned(),
    ];
    arguments.extend(files.iter().map(|file| file.as_os_str().to_owned()));
    match scope {
        Some(scope) => runner.launch("systemd-run", &scope.arguments("gio", &arguments), env),
        None => runner.launch("gio", &arguments, env),
    }
}

pub fn launch_desktop_file(path: &Path, env: &HashMap<String, String>) -> Result<()> {
    launch_with_runner(path, &[], env, &mut SystemRunner)
}

/// Launch one application from its desktop entry and pass files as arguments.
/// Every path must be absolute; the caller has already matched MIME support.
pub fn launch_desktop_file_with_paths(
    path: &Path,
    files: &[PathBuf],
    env: &HashMap<String, String>,
) -> Result<()> {
    anyhow::ensure!(
        files.len() <= 16
            && files.iter().all(|file| file.is_absolute()
                && !file.as_os_str().as_bytes().contains(&0)),
        "Invalid file arguments"
    );
    launch_with_runner(path, files, env, &mut SystemRunner)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELP: &str = "--scope --user --slice=SLICE --collect --expand-environment=BOOL";

    struct FakeRunner {
        probes: Vec<(String, Vec<OsString>)>,
        launches: Vec<(String, Vec<OsString>)>,
        help: &'static str,
        version: &'static str,
        fail_probe: bool,
        fail_launch: bool,
    }

    impl Default for FakeRunner {
        fn default() -> Self {
            Self {
                probes: Vec::new(),
                launches: Vec::new(),
                help: HELP,
                version: "systemd 262 (262-1-arch)",
                fail_probe: false,
                fail_launch: false,
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn probe(
            &mut self,
            program: &str,
            args: &[OsString],
            _: &HashMap<String, String>,
        ) -> Result<String> {
            self.probes.push((program.into(), args.to_vec()));
            if self.fail_probe {
                bail!("probe refused");
            }
            Ok(if args == [OsString::from("--help")] {
                self.help
            } else {
                self.version
            }
            .into())
        }
        fn launch(
            &mut self,
            program: &str,
            args: &[OsString],
            _: &HashMap<String, String>,
        ) -> Result<()> {
            self.launches.push((program.into(), args.to_vec()));
            if self.fail_launch {
                bail!("attempted invocation failed");
            }
            Ok(())
        }
    }

    fn manager_environment() -> (tempfile::TempDir, HashMap<String, String>) {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("systemd")).unwrap();
        fs::write(
            root.path().join("systemd/private"),
            "present manager fixture",
        )
        .unwrap();
        let env = [(
            "XDG_RUNTIME_DIR".into(),
            root.path().to_string_lossy().into_owned(),
        )]
        .into();
        (root, env)
    }

    #[test]
    fn literal_desktop_path_is_one_argv_element_and_never_shell_code() {
        let (_root, env) = manager_environment();
        let mut runner = FakeRunner::default();
        let path = Path::new(r"/home/应用 名/${HOME} $(touch nope).desktop");
        launch_with_runner(path, &[], &env, &mut runner).unwrap();
        assert_eq!(runner.probes.len(), 3);
        assert_eq!(
            runner.launches,
            vec![(
                "systemd-run".into(),
                [
                    "--user",
                    "--scope",
                    "--quiet",
                    "--collect",
                    "--slice=app.slice",
                    "--expand-environment=no",
                    "--",
                    "gio",
                    "launch",
                    path.to_str().unwrap(),
                ]
                .into_iter()
                .map(OsString::from)
                .collect()
            )]
        );
    }

    #[test]
    fn only_an_absent_manager_permits_one_direct_launch() {
        let root = tempfile::tempdir().unwrap();
        let env = [(
            "XDG_RUNTIME_DIR".into(),
            root.path().to_string_lossy().into_owned(),
        )]
        .into();
        let mut runner = FakeRunner::default();
        launch_with_runner(Path::new("literal.desktop"), &[], &env, &mut runner).unwrap();
        assert!(runner.probes.is_empty());
        assert_eq!(
            runner.launches,
            vec![(
                "gio".into(),
                vec!["launch".into(), "literal.desktop".into()]
            )]
        );
    }

    #[test]
    fn launch_with_paths_appends_each_file_after_the_desktop_file() {
        let root = tempfile::tempdir().unwrap();
        let env = [(
            "XDG_RUNTIME_DIR".into(),
            root.path().to_string_lossy().into_owned(),
        )]
        .into();
        let mut runner = FakeRunner::default();
        launch_with_runner(
            Path::new("/apps/viewer.desktop"),
            &[PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.png")],
            &env,
            &mut runner,
        )
        .unwrap();
        assert_eq!(
            runner.launches,
            vec![(
                "gio".into(),
                vec![
                    OsString::from("launch"),
                    OsString::from("/apps/viewer.desktop"),
                    OsString::from("/tmp/a.txt"),
                    OsString::from("/tmp/b.png"),
                ]
            )]
        );
    }

    #[test]
    fn probe_failure_invokes_nothing_and_launch_failure_never_retries() {
        let (_root, env) = manager_environment();
        for fail_before in [true, false] {
            let mut runner = FakeRunner {
                fail_probe: fail_before,
                fail_launch: true,
                ..Default::default()
            };
            assert!(
                launch_with_runner(Path::new("bad.desktop"), &[], &env, &mut runner).is_err()
            );
            assert_eq!(runner.launches.len(), usize::from(!fail_before));
            assert!(runner.launches.iter().all(|call| call.0 == "systemd-run"));
        }
    }

    #[test]
    fn old_scope_preserves_literal_argv_and_modern_scope_requires_opt_out() {
        let old =
            ApplicationScope::from_help("--scope --user --slice=SLICE", "systemd 253 (253.9)")
                .unwrap();
        assert_eq!(
            old,
            ApplicationScope {
                expand_environment: false,
                collect: false
            }
        );
        assert!(!old
            .arguments("gio", &[])
            .contains(&"--expand-environment=no".into()));
        assert!(
            ApplicationScope::from_help("--scope --user --slice=SLICE", "systemd 254").is_err()
        );
        assert!(ApplicationScope::from_help(HELP, "not systemd").is_err());
        assert!(ApplicationScope::from_help("--user --slice=SLICE", "systemd 262").is_err());
    }

    #[test]
    fn unsupported_modern_client_fails_before_any_invocation() {
        let (_root, env) = manager_environment();
        let mut runner = FakeRunner {
            help: "--scope --user --slice=SLICE --collect",
            ..Default::default()
        };
        assert!(
            launch_with_runner(Path::new("fixture.desktop"), &[], &env, &mut runner).is_err()
        );
        assert!(runner.launches.is_empty());
    }

    #[test]
    fn probe_output_is_drained_but_retained_bytes_are_bounded() {
        let env = std::env::vars().collect();
        let output = probe_with_timeout(
            "head",
            &["-c".into(), "200000".into(), "/dev/zero".into()],
            &env,
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(output.len(), MAX_CAPTURE);
    }

    #[test]
    fn probe_timeout_kills_and_reaps_only_the_fixture_helper() {
        let env = std::env::vars().collect();
        let start = Instant::now();
        let error = probe_with_timeout(
            "/bin/sh",
            &["-c".into(), "while :; do :; done".into()],
            &env,
            Duration::from_millis(40),
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
