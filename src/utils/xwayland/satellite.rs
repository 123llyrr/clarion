use std::os::fd::{AsRawFd as _, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use calloop::channel::Sender;
use calloop::generic::Generic;
use calloop::{Interest, Mode, PostAction, RegistrationToken};
use smithay::reexports::rustix::io::{fcntl_setfd, FdFlags};

use crate::compositor::State;
use crate::utils::expand_home;
use crate::utils::xwayland::X11Connection;

pub struct Satellite {
    x11: X11Connection,
    abstract_token: Option<RegistrationToken>,
    unix_token: Option<RegistrationToken>,
    to_main: Sender<ToMain>,
}

enum ToMain {
    SetupWatch,
}

impl Satellite {
    pub fn display_name(&self) -> &str {
        &self.x11.display_name
    }
}

/// Startup performs its bounded capability check before exporting DISPLAY or launching clients.
/// Reload uses the same probe on owned-data workers, keeping the running event loop responsive.
pub fn setup_startup(state: &mut State) {
    let config = state.realand.config.borrow();
    let setting = &config.xwayland_satellite;
    if setting.off || !test_ondemand(&setting.path) {
        return;
    }
    drop(config);
    finish_setup(state);
}

pub fn setup(state: &mut State) {
    state.realand.satellite_probe_request = state.realand.satellite_probe_request.wrapping_add(1);
    let request = state.realand.satellite_probe_request;
    if state.realand.satellite.is_some() {
        return;
    }
    let setting = state.realand.config.borrow().xwayland_satellite.clone();
    if setting.off {
        return;
    }
    let path = setting.path;
    let probe_path = path.clone();
    let task = state
        .realand
        .owned_tasks
        .submit(move || Ok(test_ondemand(&probe_path)));
    let event_loop = state.realand.event_loop.clone();
    let future = async move {
        let result = match task {
            Ok(task) => task.finish().await,
            Err(err) => Err(err),
        };
        event_loop.insert_idle(move |state| {
            if state.realand.satellite_probe_request != request {
                return;
            }
            let current = state.realand.config.borrow().xwayland_satellite.clone();
            if current.off || current.path != path || state.realand.satellite.is_some() {
                return;
            }
            match result {
                Ok(true) => {
                    finish_setup(state);
                    if let Some(satellite) = &state.realand.satellite {
                        let name = satellite.display_name().to_owned();
                        info!("listening on X11 socket: {name}");
                        *crate::utils::spawning::CHILD_DISPLAY.write().unwrap() = Some(name);
                    }
                }
                Ok(false) => (),
                Err(err) => warn!("error preparing xwayland-satellite: {err}"),
            }
        });
    };
    if let Err(err) = state.realand.scheduler.schedule(future) {
        warn!("error scheduling xwayland-satellite probe: {err:?}");
    }
}

fn finish_setup(state: &mut State) {
    if state.realand.satellite.is_some() {
        return;
    }
    let x11 = match super::setup_connection() {
        Ok(x11) => x11,
        Err(err) => {
            warn!("error opening X11 sockets, disabling xwayland-satellite integration: {err:?}");
            return;
        }
    };

    let event_loop = &state.realand.event_loop;
    let (to_main, rx) = calloop::channel::channel();
    event_loop
        .insert_source(rx, move |event, _, state| match event {
            calloop::channel::Event::Msg(msg) => match msg {
                ToMain::SetupWatch => setup_watch(state),
            },
            calloop::channel::Event::Closed => (),
        })
        .unwrap();

    state.realand.satellite = Some(Satellite {
        x11,
        abstract_token: None,
        unix_token: None,
        to_main,
    });

    setup_watch(state);
}

fn test_ondemand(path: &str) -> bool {
    let _span = realand_tracy_span!("satellite::test_ondemand");

    // Expand `~` at the start.
    let mut path = Path::new(path);
    let expanded = expand_home(path);
    match &expanded {
        Ok(Some(expanded)) => path = expanded.as_ref(),
        Ok(None) => (),
        Err(err) => {
            warn!("error expanding ~: {err:?}");
        }
    }

    let mut process = Command::new(path);
    process.args([":0", "--test-listenfd-support"]);
    configure_satellite_command(&mut process);

    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(err) => {
            warn!("error spawning xwayland-satellite at {path:?}, disabling integration: {err}");
            return false;
        }
    };

    let status = match wait_for_probe(&mut child, std::time::Duration::from_secs(2)) {
        Ok(Some(status)) => status,
        Ok(None) => {
            warn!("xwayland-satellite capability probe timed out, disabling integration");
            return false;
        }
        Err(err) => {
            warn!("error waiting for xwayland-satellite, disabling integration: {err}");
            return false;
        }
    };

    if !status.success() {
        warn!("xwayland-satellite doesn't support on-demand activation, disabling integration");
        return false;
    }

    true
}

fn wait_for_probe(
    child: &mut std::process::Child,
    timeout: std::time::Duration,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status)),
            Ok(None) if std::time::Instant::now() < deadline => {
                thread::sleep(std::time::Duration::from_millis(10))
            }
            result => {
                // Kill/reap only our own capability probe, never an active Satellite.
                let _ = child.kill();
                let _ = child.wait();
                return result.map(|_| None);
            }
        }
    }
}

fn configure_satellite_command(process: &mut Command) {
    // This child is not the compositor's systemd service. In particular, it must
    // not notify readiness on our NOTIFY_SOCKET or inherit our logging settings.
    // Leave the Wayland connection and session environment available.
    process
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove("DISPLAY")
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        .env_remove("RUST_LOG")
        .env_remove("JOURNAL_STREAM")
        .env_remove("SYSTEMD_EXEC_PID")
        .env_remove("INVOCATION_ID")
        .env_remove("MANAGERPID")
        .env_remove("MANAGERPIDFD")
        .env_remove("NOTIFY_SOCKET")
        .env_remove("WATCHDOG_PID")
        .env_remove("WATCHDOG_USEC")
        .env_remove("LISTEN_PID")
        .env_remove("LISTEN_FDS")
        .env_remove("LISTEN_FDNAMES");
    // xwayland-satellite 0.8.3 still selects its own Debug syslog logger for
    // -listenfd, independently of this environment and stdio. Filtering that
    // logger requires upstream support; this only isolates our service state.
}

// When xwayland-satellite fails to start and accept a connection on the socket, the socket will
// keep triggering our event source, even after the X11 client quits, resulting in a busyloop of
// trying to start xwayland-satellite. This function will clear out (accept and drop) all pending
// connections on the socket before registering a new event source, working around this problem.
// When the problem happens, it's very likely that xwayland-satellite won't be able to accept the
// pending client (since it had just failed to do so), so it's fine to drop the connections.
fn clear_out_pending_connections(fd: OwnedFd) -> OwnedFd {
    let listener = UnixListener::from(fd);

    if let Err(err) = listener.set_nonblocking(true) {
        warn!("error setting X11 socket to nonblocking: {err:?}");
        return OwnedFd::from(listener);
    }

    while listener.accept().is_ok() {}

    if let Err(err) = listener.set_nonblocking(false) {
        warn!("error setting X11 socket to blocking: {err:?}");
    }

    OwnedFd::from(listener)
}

fn setup_watch(state: &mut State) {
    let Some(satellite) = state.realand.satellite.as_mut() else {
        return;
    };

    let event_loop = &state.realand.event_loop;

    if let Some(token) = satellite.abstract_token.take() {
        error!("abstract_token must be None in setup_watch()");
        event_loop.remove(token);
    }
    if let Some(token) = satellite.unix_token.take() {
        error!("unix_token must be None in setup_watch()");
        event_loop.remove(token);
    }

    if let Some(fd) = &satellite.x11.abstract_fd {
        let fd = fd.try_clone().unwrap();
        let fd = clear_out_pending_connections(fd);
        let source = Generic::new(fd, Interest::READ, Mode::Level);
        let token = event_loop
            .insert_source(source, move |_, _, state| {
                if let Some(satellite) = &mut state.realand.satellite {
                    // Remove the other source.
                    if let Some(token) = satellite.unix_token.take() {
                        state.realand.event_loop.remove(token);
                    }
                    // Clear this source.
                    satellite.abstract_token = None;

                    debug!("connection to X11 abstract socket; spawning xwayland-satellite");
                    let path = state
                        .realand
                        .config
                        .borrow()
                        .xwayland_satellite
                        .path
                        .clone();
                    spawn(path, satellite);
                }
                Ok(PostAction::Remove)
            })
            .unwrap();
        satellite.abstract_token = Some(token);
    }

    let fd = satellite.x11.unix_fd.try_clone().unwrap();
    let fd = clear_out_pending_connections(fd);
    let source = Generic::new(fd, Interest::READ, Mode::Level);
    let token = event_loop
        .insert_source(source, move |_, _, state| {
            if let Some(satellite) = &mut state.realand.satellite {
                // Remove the other source.
                if let Some(token) = satellite.abstract_token.take() {
                    state.realand.event_loop.remove(token);
                }
                // Clear this source.
                satellite.unix_token = None;

                debug!("connection to X11 unix socket; spawning xwayland-satellite");
                let path = state
                    .realand
                    .config
                    .borrow()
                    .xwayland_satellite
                    .path
                    .clone();
                spawn(path, satellite);
            }
            Ok(PostAction::Remove)
        })
        .unwrap();
    satellite.unix_token = Some(token);
}

fn spawn(path: String, xwl: &Satellite) {
    let _span = realand_tracy_span!("satellite::spawn");

    let abstract_fd = xwl
        .x11
        .abstract_fd
        .as_ref()
        .map(|fd| fd.try_clone().unwrap());
    let unix_fd = xwl.x11.unix_fd.try_clone().unwrap();
    let to_main = xwl.to_main.clone();

    // Expand `~` at the start.
    let mut path = PathBuf::from(path);
    let expanded = expand_home(&path);
    match expanded {
        Ok(Some(expanded)) => path = expanded,
        Ok(None) => (),
        Err(err) => {
            warn!("error expanding ~: {err:?}");
        }
    }

    let mut process = Command::new(&path);
    process.arg(&xwl.x11.display_name);
    configure_satellite_command(&mut process);

    unsafe { process.pre_exec(crate::utils::signals::unblock_all) };

    // Spawning and waiting takes some milliseconds, so do it in a thread.
    let res = thread::Builder::new()
        .name("Xwl-s Spawner".to_owned())
        .spawn(move || {
            spawn_and_wait(&path, process, abstract_fd, unix_fd);

            // Once xwayland-satellite crashes or fails to spawn, re-establish our X11 socket watch
            // to try again next time.
            let _ = to_main.send(ToMain::SetupWatch);
        });

    if let Err(err) = res {
        warn!("error spawning a thread to spawn xwayland-satellite: {err:?}");
        let _ = xwl.to_main.send(ToMain::SetupWatch);
    }
}

fn spawn_and_wait(
    path: &Path,
    mut process: Command,
    abstract_fd: Option<OwnedFd>,
    unix_fd: OwnedFd,
) {
    let abstract_raw = abstract_fd.as_ref().map(|fd| fd.as_raw_fd());
    let unix_raw = unix_fd.as_raw_fd();

    process.arg("-listenfd").arg(unix_raw.to_string());

    if let Some(abstract_raw) = abstract_raw {
        process.arg("-listenfd").arg(abstract_raw.to_string());
    }

    unsafe {
        process.pre_exec(move || {
            // We're about to exec xwl-s; perfect time to clear CLOEXEC on the file descriptors
            // that we want to pass it.

            // We're not dropping these until after spawn().
            let unix_fd = BorrowedFd::borrow_raw(unix_raw);
            fcntl_setfd(unix_fd, FdFlags::empty())?;

            if let Some(abstract_raw) = abstract_raw {
                let abstract_fd = BorrowedFd::borrow_raw(abstract_raw);
                fcntl_setfd(abstract_fd, FdFlags::empty())?;
            }

            Ok(())
        })
    };

    let mut child = {
        let _span = realand_tracy_span!();
        match process.spawn() {
            Ok(child) => child,
            Err(err) => {
                warn!("error spawning {path:?}: {err:?}");
                return;
            }
        }
    };

    // The process spawned, we can drop our fds.
    drop(abstract_fd);
    drop(unix_fd);

    let status = match child.wait() {
        Ok(status) => status,
        Err(err) => {
            warn!("error waiting for xwayland-satellite: {err:?}");
            return;
        }
    };

    // This is most likely a crash, hence warn!().
    warn!("xwayland-satellite exited with: {status}");
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    const PARENT_ENV: &[&str] = &[
        "DISPLAY",
        "RUST_BACKTRACE",
        "RUST_LIB_BACKTRACE",
        "RUST_LOG",
        "JOURNAL_STREAM",
        "SYSTEMD_EXEC_PID",
        "INVOCATION_ID",
        "MANAGERPID",
        "MANAGERPIDFD",
        "NOTIFY_SOCKET",
        "WATCHDOG_PID",
        "WATCHDOG_USEC",
        "LISTEN_PID",
        "LISTEN_FDS",
        "LISTEN_FDNAMES",
    ];

    #[test]
    fn stuck_capability_probe_is_killed_and_reaped() {
        let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let result = wait_for_probe(&mut child, std::time::Duration::from_millis(20)).unwrap();
        assert!(result.is_none());
        assert!(child.try_wait().unwrap().is_some());
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }

    #[test]
    fn probe_and_activation_commands_explicitly_remove_parent_environment() {
        for args in [
            vec![":0", "--test-listenfd-support"],
            vec![":1", "-listenfd", "7", "-listenfd", "8"],
        ] {
            let mut process = Command::new("xwayland-satellite");
            process.args(&args);
            configure_satellite_command(&mut process);
            let environment = process.get_envs().collect::<Vec<_>>();
            for name in PARENT_ENV {
                assert!(
                    environment.contains(&(OsStr::new(name), None)),
                    "{name} must explicitly override inherited values"
                );
            }
            for name in ["WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "PATH"] {
                assert!(
                    environment.iter().all(|(key, _)| *key != OsStr::new(name)),
                    "{name} must retain its inherited value"
                );
            }
            assert_eq!(
                process.get_args().collect::<Vec<_>>(),
                args.iter().map(OsStr::new).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn configured_child_gets_null_stdio_and_wayland_environment_without_service_state() {
        // Seed only this Command, avoiding process-wide env changes that race
        // other tests. Validate the actual exec environment and fd redirections.
        let mut process = Command::new("/bin/sh");
        process.env_clear();
        for name in PARENT_ENV {
            process.env(name, "parent-value");
        }
        process
            .env("WAYLAND_DISPLAY", "wayland-test")
            .env("XDG_RUNTIME_DIR", "/run/user/test")
            .args([
                "-c",
                r#"
                for name in DISPLAY RUST_BACKTRACE RUST_LIB_BACKTRACE RUST_LOG \
                    JOURNAL_STREAM SYSTEMD_EXEC_PID INVOCATION_ID MANAGERPID \
                    MANAGERPIDFD NOTIFY_SOCKET WATCHDOG_PID WATCHDOG_USEC \
                    LISTEN_PID LISTEN_FDS LISTEN_FDNAMES; do
                    eval 'present=${'"$name"'+set}'
                    [ -z "$present" ] || exit 10
                done
                [ "$WAYLAND_DISPLAY" = wayland-test ] || exit 11
                [ "$XDG_RUNTIME_DIR" = /run/user/test ] || exit 12
                for fd in 0 1 2; do
                    [ "$(/usr/bin/readlink /proc/$$/fd/$fd)" = /dev/null ] || exit 13
                done
            "#,
            ]);
        configure_satellite_command(&mut process);
        let status = process.status().unwrap();
        assert!(
            status.success(),
            "child environment/stdio check failed: {status}"
        );
    }
}
