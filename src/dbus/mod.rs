use zbus::blocking::Connection;
use zbus::object_server::Interface;

use crate::compositor::State;

pub mod freedesktop_a11y;
pub mod freedesktop_locale1;
pub mod freedesktop_login1;
pub mod freedesktop_screensaver;
pub mod gnome_shell_introspect;
pub mod gnome_shell_screenshot;
pub mod mutter_display_config;
pub mod mutter_service_channel;

#[cfg(feature = "xdp-gnome-screencast")]
pub mod mutter_screen_cast;
#[cfg(feature = "xdp-gnome-screencast")]
use mutter_screen_cast::ScreenCast;

use self::freedesktop_screensaver::ScreenSaver;
use self::gnome_shell_introspect::Introspect;
use self::mutter_display_config::DisplayConfig;
use self::mutter_service_channel::ServiceChannel;

trait Start: Interface {
    fn start(self) -> anyhow::Result<zbus::blocking::Connection>;
}

#[derive(Default)]
pub struct DBusServers {
    pub conn_service_channel: Option<Connection>,
    pub conn_display_config: Option<Connection>,
    pub conn_screen_saver: Option<Connection>,
    pub conn_screen_shot: Option<Connection>,
    pub conn_introspect: Option<Connection>,
    #[cfg(feature = "xdp-gnome-screencast")]
    pub conn_screen_cast: Option<Connection>,
    pub conn_login1: Option<Connection>,
    pub conn_locale1: Option<Connection>,
    pub conn_a11y_manager: Option<Connection>,
}

impl DBusServers {
    /// Asks logind to suspend the system. The idle policy calls this after it
    /// has locked, if locking before sleep is configured.
    pub fn suspend(&self) -> anyhow::Result<()> {
        let conn = self
            .conn_login1
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("login1 connection unavailable"))?;
        conn.call_method(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            Some("org.freedesktop.login1.Manager"),
            "Suspend",
            &(false,),
        )?;
        Ok(())
    }

    pub fn start(state: &mut State, is_session_instance: bool) {
        let _span = clarion_tracy_span!("DBusServers::start");

        let backend = &state.backend;
        let clarion = &mut state.clarion;
        let config = clarion.config.borrow();

        let mut dbus = Self::default();

        if is_session_instance {
            let (to_clarion, from_service_channel) = calloop::channel::channel();
            let service_channel = ServiceChannel::new(to_clarion);
            clarion
                .event_loop
                .insert_source(from_service_channel, move |event, _, state| match event {
                    calloop::channel::Event::Msg(new_client) => {
                        state.clarion.insert_client(new_client);
                    }
                    calloop::channel::Event::Closed => (),
                })
                .unwrap();
            dbus.conn_service_channel = try_start(service_channel);
        }

        if is_session_instance || config.debug.dbus_interfaces_in_non_session_instances {
            let (to_clarion, from_display_config) = calloop::channel::channel();
            let display_config = DisplayConfig::new(to_clarion, backend.ipc_outputs());
            clarion
                .event_loop
                .insert_source(from_display_config, move |event, _, state| match event {
                    calloop::channel::Event::Msg(new_conf) => {
                        for (name, conf) in new_conf {
                            state.modify_output_config(&name, move |output| {
                                if let Some(new_output) = conf {
                                    *output = new_output;
                                } else {
                                    output.off = true;
                                }
                            });
                        }
                        state.reload_output_config();
                    }
                    calloop::channel::Event::Closed => (),
                })
                .unwrap();
            dbus.conn_display_config = try_start(display_config);

            let screen_saver = ScreenSaver::new(clarion.is_fdo_idle_inhibited.clone());
            dbus.conn_screen_saver = try_start(screen_saver);

            let (to_clarion, from_screenshot) = calloop::channel::channel();
            clarion
                .event_loop
                .insert_source(from_screenshot, move |event, _, state| match event {
                    calloop::channel::Event::Msg(msg) => state.on_screen_shot_msg(msg),
                    calloop::channel::Event::Closed => (),
                })
                .unwrap();
            let screenshot = gnome_shell_screenshot::Screenshot::new(to_clarion);
            dbus.conn_screen_shot = try_start(screenshot);

            let (to_clarion, from_introspect) = calloop::channel::channel();
            let (to_introspect, from_clarion) = async_channel::unbounded();
            clarion
                .event_loop
                .insert_source(from_introspect, move |event, _, state| match event {
                    calloop::channel::Event::Msg(msg) => {
                        state.on_introspect_msg(&to_introspect, msg)
                    }
                    calloop::channel::Event::Closed => (),
                })
                .unwrap();
            let introspect = Introspect::new(to_clarion, from_clarion);
            dbus.conn_introspect = try_start(introspect);

            #[cfg(feature = "xdp-gnome-screencast")]
            {
                let (to_clarion, from_screen_cast) = calloop::channel::channel();
                clarion
                    .event_loop
                    .insert_source(from_screen_cast, {
                        move |event, _, state| match event {
                            calloop::channel::Event::Msg(msg) => state.on_screen_cast_msg(msg),
                            calloop::channel::Event::Closed => (),
                        }
                    })
                    .unwrap();
                let screen_cast = ScreenCast::new(backend.ipc_outputs(), to_clarion);
                dbus.conn_screen_cast = try_start(screen_cast);
            }

            let (to_clarion, from_a11y) = calloop::channel::channel();
            let (to_a11y, from_clarion) = async_channel::unbounded();
            clarion
                .event_loop
                .insert_source(from_a11y, move |event, _, state| match event {
                    calloop::channel::Event::Msg(msg) => state.on_a11y_manager_msg(&to_a11y, msg),
                    calloop::channel::Event::Closed => (),
                })
                .unwrap();
            let a11y_manager = freedesktop_a11y::Manager::new(to_clarion, from_clarion);
            match a11y_manager.start() {
                Ok(conn) => {
                    dbus.conn_a11y_manager = Some(conn);
                    clarion.a11y_manager = Some(a11y_manager);
                }
                Err(err) => {
                    warn!("error starting a11y manager: {err:?}");
                }
            }
        }

        let (to_clarion, from_login1) = calloop::channel::channel();
        clarion
            .event_loop
            .insert_source(from_login1, move |event, _, state| match event {
                calloop::channel::Event::Msg(msg) => state.on_login1_msg(msg),
                calloop::channel::Event::Closed => (),
            })
            .unwrap();
        match freedesktop_login1::start(to_clarion) {
            Ok(conn) => {
                dbus.conn_login1 = Some(conn);
            }
            Err(err) => {
                warn!("error starting login1 watcher: {err:?}");
            }
        }

        let (to_clarion, from_locale1) = calloop::channel::channel();
        clarion
            .event_loop
            .insert_source(from_locale1, move |event, _, state| match event {
                calloop::channel::Event::Msg(msg) => state.on_locale1_msg(msg),
                calloop::channel::Event::Closed => (),
            })
            .unwrap();
        match freedesktop_locale1::start(to_clarion) {
            Ok(conn) => {
                dbus.conn_locale1 = Some(conn);
            }
            Err(err) => {
                warn!("error starting locale1 watcher: {err:?}");
            }
        }

        clarion.dbus = Some(dbus);
    }
}

fn try_start<I: Start>(iface: I) -> Option<Connection> {
    match iface.start() {
        Ok(conn) => Some(conn),
        Err(err) => {
            warn!("error starting {}: {err:?}", I::name());
            None
        }
    }
}
