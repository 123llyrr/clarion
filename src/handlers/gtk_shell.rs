//! GTK Wayland menu metadata, associated with the actual owning wl_surface.
//!
//! Advertise version 1 and capabilities 0: exporting GMenu does not require
//! hiding client menus. Material mode and applications without menu support
//! consequently keep their own menu bars. This protocol never grants focus.

use std::sync::Mutex;

use clarion_ipc::GtkAppMenu;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::wayland::compositor::with_states;

use crate::compositor::State;
use crate::protocols::raw::gtk_shell::server::{
    gtk_shell1::{self, GtkShell1},
    gtk_surface1::{self, GtkSurface1},
};

pub struct GtkShellState {
    _global: GlobalId,
}

impl GtkShellState {
    pub fn new(display: &DisplayHandle) -> Self {
        Self {
            _global: display.create_global::<State, GtkShell1, _>(1, ()),
        }
    }
}

pub struct GtkSurfaceData {
    surface: WlSurface,
}

#[derive(Default)]
struct MenuMetadata(Mutex<Option<GtkAppMenu>>);

/// Check surface identity without cloning menu strings on the input path.
pub fn surface_application_id_matches(surface: &WlSurface, application_id: &str) -> bool {
    if !surface.is_alive() {
        return false;
    }
    with_states(surface, |states| {
        states.data_map.get::<MenuMetadata>().is_some_and(|cache| {
            cache.0.lock().is_ok_and(|menu| {
                menu.as_ref()
                    .and_then(|menu| menu.application_id.as_deref())
                    == Some(application_id)
            })
        })
    })
}

pub fn surface_menu(surface: &WlSurface) -> Option<GtkAppMenu> {
    if !surface.is_alive() {
        return None;
    }
    with_states(surface, |states| {
        states
            .data_map
            .get::<MenuMetadata>()?
            .0
            .lock()
            .ok()?
            .clone()
    })
}

/// Compare a cached IPC value without allocating six menu strings per window
/// on every IPC refresh. The caller must be outside any with_states role lock.
pub fn surface_menu_matches(surface: &WlSurface, current: Option<&GtkAppMenu>) -> bool {
    if !surface.is_alive() {
        return current.is_none();
    }
    with_states(surface, |states| {
        let Some(cache) = states.data_map.get::<MenuMetadata>() else {
            return current.is_none();
        };
        cache.0.lock().is_ok_and(|menu| menu.as_ref() == current)
    })
}

fn bounded(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty() && value.len() <= 4096)
}

impl GlobalDispatch<GtkShell1, ()> for State {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<GtkShell1>,
        _global: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let shell = data_init.init(resource, ());
        shell.capabilities(0);
    }
}

impl Dispatch<GtkShell1, ()> for State {
    fn request(
        _state: &mut Self,
        client: &Client,
        resource: &GtkShell1,
        request: gtk_shell1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        // Metadata-only v1. A startup id or timestamp is not authority to
        // steal focus, ring the system bell, or bypass xdg activation.
        if let gtk_shell1::Request::GetGtkSurface {
            gtk_surface,
            surface,
        } = request
        {
            if surface.client().as_ref().map(Client::id) != Some(client.id()) {
                resource.post_error(0u32, "GTK surface must belong to this client");
                return;
            }
            data_init.init(gtk_surface, GtkSurfaceData { surface });
        }
    }
}

impl Dispatch<GtkSurface1, GtkSurfaceData> for State {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &GtkSurface1,
        request: gtk_surface1::Request,
        data: &GtkSurfaceData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if !data.surface.is_alive() {
            return;
        }
        if let gtk_surface1::Request::SetDbusProperties {
            application_id,
            app_menu_path,
            menubar_path,
            window_object_path,
            application_object_path,
            unique_bus_name,
        } = request
        {
            let menu = GtkAppMenu {
                application_id: bounded(application_id),
                app_menu_path: bounded(app_menu_path),
                menubar_path: bounded(menubar_path),
                window_object_path: bounded(window_object_path),
                application_object_path: bounded(application_object_path),
                unique_bus_name: bounded(unique_bus_name),
            };
            with_states(&data.surface, |states| {
                states.data_map.insert_if_missing(MenuMetadata::default);
                *states
                    .data_map
                    .get::<MenuMetadata>()
                    .unwrap()
                    .0
                    .lock()
                    .unwrap() = Some(menu);
            });
            state.ipc_refresh_windows();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::bounded;
    #[test]
    fn metadata_text_is_bounded_even_before_ipc_serialization() {
        assert_eq!(bounded(None), None);
        assert_eq!(bounded(Some(String::new())), None);
        assert_eq!(bounded(Some("x".repeat(4096))).unwrap().len(), 4096);
        assert_eq!(bounded(Some("x".repeat(4097))), None);
        assert_eq!(bounded(Some("é".repeat(2049))), None);
    }
}
