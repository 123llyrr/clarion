#[macro_use]
extern crate tracing;

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::FromRawFd;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::{env, mem};

use calloop::EventLoop;
use clap::{CommandFactory, Parser};
use clap_complete::Shell;
use clap_complete_nushell::Nushell;
use clarion::cli::{Cli, CompletionShell, Sub};
use clarion::compositor::State;
#[cfg(feature = "dbus")]
use clarion::dbus;
use clarion::ipc::client::handle_msg;
use clarion::render_helpers::vulkan::lifecycle::{self, DEFAULT_SHUTDOWN_TIMEOUT};
use clarion::utils::spawning::{
    spawn, spawn_sh, store_and_increase_nofile_rlimit, CHILD_DISPLAY, CHILD_ENV,
    REMOVE_ENV_RUST_BACKTRACE, REMOVE_ENV_RUST_LIB_BACKTRACE,
};
use clarion::utils::{cause_panic, version, watcher, xwayland, IS_SYSTEMD_SERVICE};
use clarion_config::{Config, ConfigPath};
use clarion_ipc::socket::{LEGACY_SOCKET_PATH_ENV, SOCKET_PATH_ENV};
use directories::ProjectDirs;
use sd_notify::NotifyState;
use smithay::reexports::wayland_server::Display;
use tracing_subscriber::EnvFilter;

const DEFAULT_LOG_FILTER: &str = "clarion=debug,smithay::backend::renderer::gles=error";

#[cfg(feature = "profile-with-tracy-allocations")]
#[global_allocator]
static GLOBAL: tracy_client::ProfiledAllocator<std::alloc::System> =
    tracy_client::ProfiledAllocator::new(std::alloc::System, 100);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    clarion::utils::identity::inherit_legacy_environment();
    // Set backtrace defaults if not set.
    if env::var_os("RUST_BACKTRACE").is_none() {
        env::set_var("RUST_BACKTRACE", "1");
        REMOVE_ENV_RUST_BACKTRACE.store(true, Ordering::Relaxed);
    }
    if env::var_os("RUST_LIB_BACKTRACE").is_none() {
        env::set_var("RUST_LIB_BACKTRACE", "0");
        REMOVE_ENV_RUST_LIB_BACKTRACE.store(true, Ordering::Relaxed);
    }

    let directives = env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_LOG_FILTER.to_owned());
    let env_filter = EnvFilter::builder().parse_lossy(directives);
    tracing_subscriber::fmt()
        .compact()
        .with_writer(io::stderr)
        .with_env_filter(env_filter)
        .with_ansi_sanitization(false)
        .init();

    if env::var_os("NOTIFY_SOCKET").is_some() {
        IS_SYSTEMD_SERVICE.store(true, Ordering::Relaxed);

        #[cfg(not(feature = "systemd"))]
        warn!(
            "running as a systemd service, but systemd support is compiled out. \
             Are you sure you did not forget to set `--features systemd`?"
        );
    }

    let cli = Cli::parse();

    if cli.session {
        // If we're starting as a session, assume that the intention is to start on a TTY unless
        // this is a WSL environment. Remove DISPLAY, WAYLAND_DISPLAY or WAYLAND_SOCKET from our
        // environment if they are set, since they will cause the winit backend to be selected
        // instead.
        if env::var_os("WSL_DISTRO_NAME").is_none() {
            if env::var_os("DISPLAY").is_some() {
                warn!("running as a session but DISPLAY is set, removing it");
                env::remove_var("DISPLAY");
            }
            if env::var_os("WAYLAND_DISPLAY").is_some() {
                warn!("running as a session but WAYLAND_DISPLAY is set, removing it");
                env::remove_var("WAYLAND_DISPLAY");
            }
            if env::var_os("WAYLAND_SOCKET").is_some() {
                warn!("running as a session but WAYLAND_SOCKET is set, removing it");
                env::remove_var("WAYLAND_SOCKET");
            }
        }

        // Set the current desktop for xdg-desktop-portal.
        env::set_var("XDG_CURRENT_DESKTOP", "clarion");
        // Ensure the session type is set to Wayland for xdg-autostart and Qt apps.
        env::set_var("XDG_SESSION_TYPE", "wayland");
    }

    // Handle subcommands.
    if let Some(subcommand) = cli.subcommand {
        match subcommand {
            Sub::Validate { config } => {
                tracy_client::Client::start();

                config_path(config).load().config?;
                info!("config is valid");
                return Ok(());
            }
            Sub::Msg { msg, json } => {
                handle_msg(msg, json)?;
                return Ok(());
            }
            Sub::Panic => cause_panic(),
            Sub::Completions { shell } => {
                match shell {
                    CompletionShell::Nushell => {
                        clap_complete::generate(
                            Nushell,
                            &mut Cli::command(),
                            "clarion",
                            &mut io::stdout(),
                        );
                    }
                    other => {
                        let generator = Shell::try_from(other).unwrap();
                        clap_complete::generate(
                            generator,
                            &mut Cli::command(),
                            "clarion",
                            &mut io::stdout(),
                        );
                    }
                }
                return Ok(());
            }
        }
    }

    // Needs to be done before starting Tracy, so that it applies to Tracy's threads.
    let legacy_app_menu_environment = cli
        .session
        .then(clarion::utils::app_menu_environment::disable_legacy_appmenu_environment);
    clarion::utils::signals::block_early().unwrap();

    // Avoid starting Tracy for the `clarion msg` code path since starting/stopping Tracy is a bit
    // slow.
    tracy_client::Client::start();

    // In on-demand mode, we must shut down Tracy manually to terminate the connection cleanly.
    // Do it from a Drop impl here, so that it runs after the Drop code for all of the state created
    // below, because some of those Drop impls themselves create Tracy spans.
    let _shutdown_tracy = ShutdownTracy;

    // Declared before State and the loop: their owner-only resources drop first,
    // then this guard waits for all GPU worker/TLS destructors to finish. A main
    // thread panic arms the same deadline before unwinding those resources.
    install_gpu_exit_panic_hook();
    let _shutdown_gpu = ShutdownGpuWorkers;

    info!("starting version {}", &version());

    // Load the config.
    let config_path = config_path(cli.config);
    env::remove_var("CLARION_CONFIG");
    env::remove_var("REALAND_CONFIG");
    let (config_created_at, config_load_result) = config_path.load_or_create();
    let config_errored = config_load_result.config.is_err();
    let mut config = config_load_result.config.unwrap_or_else(|err| {
        warn!("{err:?}");
        Config::load_default()
    });
    let config_includes = config_load_result.includes;

    let renderer_backend = clarion::backend::RendererBackendType::resolve(cli.renderer, &config);
    if let Some(r) = cli.renderer {
        env::set_var("CLARION_RENDERER", r.as_str());
    }
    let backend_source = if cli.renderer.is_some() {
        "command line flag (--renderer)"
    } else if config.renderer_backend().is_some() {
        "configuration file"
    } else if env::var_os("CLARION_RENDERER").is_some() {
        "environment variable (CLARION_RENDERER)"
    } else {
        "default"
    };
    info!("configured graphics renderer backend: {renderer_backend:?} (from {backend_source})");
    if renderer_backend.is_vulkan() {
        info!(
            "Vulkan renderer selected. \
             The active backend will probe Vulkan device, DMA-BUF and presentation support before enabling native rendering."
        );
    }

    let spawn_at_startup = mem::take(&mut config.spawn_at_startup);
    let spawn_sh_at_startup = mem::take(&mut config.spawn_sh_at_startup);
    *CHILD_ENV.write().unwrap() = mem::take(&mut config.environment);

    store_and_increase_nofile_rlimit();

    // Create the main event loop.
    let mut event_loop = EventLoop::<State>::try_new().unwrap();

    // Handle Ctrl+C and other signals.
    clarion::utils::signals::listen(&event_loop.handle());

    // Create the compositor.
    let display = Display::new().unwrap();

    // Increase the buffer size so that it's harder to crash a frozen client with a 1000 Hz mouse.
    set_default_max_buffer_size(&display, 1024 * 1024);

    let mut state = State::new(
        config,
        event_loop.handle(),
        event_loop.get_signal(),
        display,
        false,
        true,
        cli.session,
        renderer_backend,
    )
    .unwrap();

    // Set WAYLAND_DISPLAY for children.
    let socket_name = state.clarion.socket_name.as_deref().unwrap();
    env::set_var("WAYLAND_DISPLAY", socket_name);
    info!(
        "listening on Wayland socket: {}",
        socket_name.to_string_lossy()
    );

    // Set CLARION_SOCKET for children.
    if let Some(ipc) = &state.clarion.ipc_server {
        let socket_path = ipc.socket_path.as_deref().unwrap();
        env::set_var(SOCKET_PATH_ENV, socket_path);
        env::set_var(LEGACY_SOCKET_PATH_ENV, socket_path);
        info!("IPC listening on: {}", socket_path.to_string_lossy());
    }

    // Setup xwayland-satellite integration.
    xwayland::satellite::setup_startup(&mut state);
    if let Some(satellite) = &state.clarion.satellite {
        let name = satellite.display_name();
        *CHILD_DISPLAY.write().unwrap() = Some(name.to_owned());
        env::set_var("DISPLAY", name);
        info!("listening on X11 socket: {name}");
    } else {
        // Avoid spawning children in the host X11.
        env::remove_var("DISPLAY");
    }

    if cli.session {
        // We're starting as a session. Import our variables.
        if let Some(legacy) = &legacy_app_menu_environment {
            legacy.migrate_activation_environment();
        }
        import_environment();

        // Inhibit power key handling so we can suspend on it.
        #[cfg(feature = "dbus")]
        if !state
            .clarion
            .config
            .borrow()
            .input
            .disable_power_key_handling
        {
            if let Err(err) = state.clarion.inhibit_power_key() {
                warn!("error inhibiting power key: {err:?}");
            }
        }
    }

    #[cfg(feature = "dbus")]
    dbus::DBusServers::start(&mut state, cli.session);

    #[cfg(feature = "dbus")]
    if cli.session {
        state.clarion.a11y.start();
    }

    if env::var_os("CLARION_DISABLE_SYSTEM_MANAGER_NOTIFY").is_none_or(|x| x != "1") {
        // Notify systemd we're ready.
        if let Err(err) = sd_notify::notify(&[NotifyState::Ready]) {
            warn!("error notifying systemd: {err:?}");
        };

        // Send ready notification to the NOTIFY_FD file descriptor.
        if let Err(err) = notify_fd() {
            warn!("error notifying fd: {err:?}");
        }
    }

    watcher::setup(&mut state, &config_path, config_includes);

    // Spawn commands from cli and auto-start.
    spawn(cli.command, None);

    for elem in spawn_at_startup {
        spawn(elem.command, None);
    }
    for elem in spawn_sh_at_startup {
        spawn_sh(elem.command, None);
    }

    // Show the config error notification right away if needed.
    if config_errored {
        state.clarion.config_error_notification.show();
        state.ipc_config_loaded(true);
    } else if let Some(path) = config_created_at {
        state.clarion.config_error_notification.show_created(path);
    }

    // Run the compositor.
    let result = event_loop.run(None, &mut state, |state| state.refresh_and_flush_clients());

    // Begin the deadline before owner teardown: a lost driver can also stall an
    // EGL/KMS/device destructor, before the final worker barrier gets control.
    lifecycle::arm_exit_deadline(DEFAULT_SHUTDOWN_TIMEOUT);
    let stopped_loop = event_loop.handle().downgrade();
    lifecycle::log_shutdown_state("before-owner-teardown");
    state.prepare_shutdown();
    drop(state);
    lifecycle::log_shutdown_state("after-owner-teardown");
    drop(event_loop);
    info!(
        expired = stopped_loop.expired(),
        "compositor event loop released"
    );
    lifecycle::log_shutdown_state("after-event-loop-teardown");
    result?;

    Ok(())
}

fn import_environment() {
    let variables = [
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_TYPE",
        SOCKET_PATH_ENV,
        LEGACY_SOCKET_PATH_ENV,
    ];
    let variables = variables.join(" ");

    let mut init_system_import = String::new();
    if cfg!(feature = "systemd") {
        write!(
            init_system_import,
            "systemctl --user import-environment {variables};"
        )
        .unwrap();
    }
    if cfg!(feature = "dinit") {
        write!(init_system_import, "dinitctl setenv {variables};").unwrap();
    }

    let rv = Command::new("/bin/sh")
        .args([
            "-c",
            &format!(
                "{init_system_import}\
                 hash dbus-update-activation-environment 2>/dev/null && \
                 dbus-update-activation-environment {variables}"
            ),
        ])
        .spawn();
    // Wait for the import process to complete, otherwise services will start too fast without
    // environment variables available.
    match rv {
        Ok(mut child) => match child.wait() {
            Ok(status) => {
                if !status.success() {
                    warn!("import environment shell exited with {status}");
                }
            }
            Err(err) => {
                warn!("error waiting for import environment shell: {err:?}");
            }
        },
        Err(err) => {
            warn!("error spawning shell to import environment: {err:?}");
        }
    }
}

fn env_config_path() -> Option<PathBuf> {
    clarion_config::identity::config_path_from_environment()
}

fn default_config_path() -> Option<PathBuf> {
    let Some(dirs) = ProjectDirs::from("", "", "clarion") else {
        warn!("error retrieving home directory");
        return None;
    };

    let mut path = dirs.config_dir().to_owned();
    path.push("config.kdl");
    let legacy_path = dirs.config_dir().parent()?.join("realand/config.kdl");
    Some(clarion_config::identity::prefer_existing_path(
        path,
        legacy_path,
    ))
}

fn system_config_path() -> PathBuf {
    clarion_config::identity::prefer_existing_path(
        PathBuf::from("/etc/clarion/config.kdl"),
        PathBuf::from("/etc/realand/config.kdl"),
    )
}

fn config_path(cli_path: Option<PathBuf>) -> ConfigPath {
    if let Some(explicit) = cli_path.or_else(env_config_path) {
        return ConfigPath::Explicit(explicit);
    }

    let system_path = system_config_path();

    if let Some(user_path) = default_config_path() {
        ConfigPath::Regular {
            user_path,
            system_path,
        }
    } else {
        // Couldn't find the home directory, or whatever.
        ConfigPath::Explicit(system_path)
    }
}

fn notify_fd() -> anyhow::Result<()> {
    let fd = match env::var("NOTIFY_FD") {
        Ok(notify_fd) => notify_fd.parse()?,
        Err(env::VarError::NotPresent) => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    env::remove_var("NOTIFY_FD");
    let mut notif = unsafe { File::from_raw_fd(fd) };
    notif.write_all(b"READY=1\n")?;
    Ok(())
}

// The wayland-server crate has set_default_max_buffer_size() under a libwayland_1_23 feature, but
// this hard-requires libwayland-server >= 1.23 which is not present on e.g. Ubuntu 24.04. Since
// calling this is an optional enhancement, do it optionally at runtime.
fn set_default_max_buffer_size(display: &Display<State>, size: usize) {
    use std::ffi::c_void;

    unsafe {
        // RTLD_NOLOAD ensures we only get a handle to the libwayland-server that wayland-rs has
        // already loaded into this process, rather than potentially pulling in a different copy.
        let lib = libc::dlopen(
            c"libwayland-server.so.0".as_ptr(),
            libc::RTLD_LAZY | libc::RTLD_NOLOAD,
        );
        if lib.is_null() {
            // It's not really expected that this can happen, maybe if some distro changes the
            // library name?
            warn!("cannot set default max buffer size: libwayland-server.so.0 is not loaded");
            return;
        }

        let sym = libc::dlsym(lib, c"wl_display_set_default_max_buffer_size".as_ptr());
        if sym.is_null() {
            // Expected on libwayland-server < 1.23.
            trace!("wl_display_set_default_max_buffer_size is missing; skipping");
        } else {
            let func: unsafe extern "C" fn(*mut c_void, libc::size_t) = std::mem::transmute(sym);
            let display_ptr = display.handle().backend_handle().display_ptr();
            func(display_ptr.cast(), size);
        }

        libc::dlclose(lib);
    }
}

struct ShutdownTracy;
impl Drop for ShutdownTracy {
    fn drop(&mut self) {
        #[cfg(feature = "profile-with-tracy-ondemand")]
        unsafe {
            tracy_client::sys::___tracy_shutdown_profiler();
        }
    }
}

fn install_gpu_exit_panic_hook() {
    let owner = std::thread::current().id();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Preparation jobs recover caught panics on their own worker. Those
        // panics must not close a healthy compositor's global GPU services.
        if std::thread::current().id() == owner {
            lifecycle::arm_exit_deadline(DEFAULT_SHUTDOWN_TIMEOUT);
        }
        previous(info);
    }));
}

struct ShutdownGpuWorkers;
impl Drop for ShutdownGpuWorkers {
    fn drop(&mut self) {
        lifecycle::shutdown_or_terminate(DEFAULT_SHUTDOWN_TIMEOUT);
    }
}
