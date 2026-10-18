//! Minimal GTK surface identity, associated with its actual owning wl_surface.
//!
//! Version 1 and capabilities 0 keep application menus local. Menu addresses
//! sent by GTK clients are ignored; only the application id used by Clarion's
//! desktop shortcut routing is retained. This protocol never grants focus.

use std::sync::Mutex;

use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::wayland::compositor::with_states;

use crate::compositor::State;
use crate::protocols::raw::gtk_shell::server::gtk_shell1::{self, GtkShell1};
use crate::protocols::raw::gtk_shell::server::gtk_surface1::{self, GtkSurface1};

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
struct SurfaceIdentity(Mutex<Option<String>>);

/// Check the identity without cloning strings on the input path.
pub fn surface_application_id_matches(surface: &WlSurface, application_id: &str) -> bool {
    if !surface.is_alive() {
        return false;
    }
    with_states(surface, |states| {
        states
            .data_map
            .get::<SurfaceIdentity>()
            .is_some_and(|cache| {
                cache
                    .0
                    .lock()
                    .is_ok_and(|identity| identity.as_deref() == Some(application_id))
            })
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
        _state: &mut Self,
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
        if let gtk_surface1::Request::SetDbusProperties { application_id, .. } = request {
            let identity = bounded(application_id);
            with_states(&data.surface, |states| {
                states.data_map.insert_if_missing(SurfaceIdentity::default);
                *states
                    .data_map
                    .get::<SurfaceIdentity>()
                    .unwrap()
                    .0
                    .lock()
                    .unwrap() = identity;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::bounded;
    #[test]
    fn identity_text_is_bounded_before_retention() {
        assert_eq!(bounded(None), None);
        assert_eq!(bounded(Some(String::new())), None);
        assert_eq!(bounded(Some("x".repeat(4096))).unwrap().len(), 4096);
        assert_eq!(bounded(Some("x".repeat(4097))), None);
        assert_eq!(bounded(Some("é".repeat(2049))), None);
    }
}
