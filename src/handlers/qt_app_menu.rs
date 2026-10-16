//! KDE/Qt Wayland appmenu addresses belong to the actual owning wl_surface.
//! The bridge exports canonical menu metadata only; it never grants focus or
//! interprets compositor window identifiers as X11 identifiers.

use std::sync::Mutex;

use clarion_ipc::AppMenu;
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId, ObjectId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::wayland::compositor::with_states;
use wayland_protocols_plasma::appmenu::server::{
    org_kde_kwin_appmenu::{self, OrgKdeKwinAppmenu},
    org_kde_kwin_appmenu_manager::{self, OrgKdeKwinAppmenuManager},
};

use crate::compositor::State;

pub struct QtAppMenuState {
    _global: GlobalId,
}

impl QtAppMenuState {
    pub fn new(display: &DisplayHandle) -> Self {
        Self {
            _global: display.create_global::<State, OrgKdeKwinAppmenuManager, _>(2, ()),
        }
    }
}

pub struct AppMenuSurfaceData {
    surface: WlSurface,
}

#[derive(Default)]
struct MenuCache {
    owner: Option<ObjectId>,
    address: Option<AppMenu>,
}

#[derive(Default)]
struct MenuMetadata(Mutex<MenuCache>);

pub fn surface_menu(surface: &WlSurface) -> Option<AppMenu> {
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
            .address
            .clone()
    })
}

/// Called outside any role-state lock. Unchanged IPC refreshes do not clone
/// menu strings or recursively lock the same Smithay surface state.
pub fn surface_menu_matches(surface: &WlSurface, current: Option<&AppMenu>) -> bool {
    if !surface.is_alive() {
        return current.is_none();
    }
    with_states(surface, |states| {
        let Some(cache) = states.data_map.get::<MenuMetadata>() else {
            return current.is_none();
        };
        cache
            .0
            .lock()
            .is_ok_and(|cache| cache.address.as_ref() == current)
    })
}

fn address(bus_name: String, menu_path: String) -> Option<AppMenu> {
    // D-Bus bus names are ASCII and limited to 255 bytes. A unique name may
    // start a component with a digit; a well-known name may not.
    let unique = bus_name.starts_with(':');
    let body = if unique { &bus_name[1..] } else { &bus_name };
    let valid_bus = !bus_name.is_empty()
        && bus_name.len() <= 255
        && body.contains('.')
        && body.split('.').all(|part| {
            !part.is_empty()
                && (unique || !part.as_bytes()[0].is_ascii_digit())
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        });
    let valid_path = !menu_path.is_empty()
        && menu_path.len() <= 4096
        && menu_path.starts_with('/')
        && (menu_path == "/"
            || menu_path[1..].split('/').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            }));
    (valid_bus && valid_path).then_some(AppMenu {
        bus_name,
        menu_path,
    })
}

impl GlobalDispatch<OrgKdeKwinAppmenuManager, ()> for State {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<OrgKdeKwinAppmenuManager>,
        _global: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<OrgKdeKwinAppmenuManager, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &OrgKdeKwinAppmenuManager,
        request: org_kde_kwin_appmenu_manager::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let org_kde_kwin_appmenu_manager::Request::Create { id, surface } = request {
            if surface.client().as_ref().map(Client::id) != Some(client.id()) {
                resource.post_error(0u32, "Appmenu surface must belong to this client");
                return;
            }
            let appmenu = data_init.init(
                id,
                AppMenuSurfaceData {
                    surface: surface.clone(),
                },
            );
            with_states(&surface, |states| {
                states.data_map.insert_if_missing(MenuMetadata::default);
                let mut cache = states
                    .data_map
                    .get::<MenuMetadata>()
                    .unwrap()
                    .0
                    .lock()
                    .unwrap();
                // Recreated Qt objects may overlap their predecessor's lifetime.
                // Only the newest object can publish or clear this surface.
                cache.owner = Some(appmenu.id());
                cache.address = None;
            });
            state.ipc_refresh_windows();
        }
    }
}

impl Dispatch<OrgKdeKwinAppmenu, AppMenuSurfaceData> for State {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &OrgKdeKwinAppmenu,
        request: org_kde_kwin_appmenu::Request,
        data: &AppMenuSurfaceData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if !data.surface.is_alive() {
            return;
        }
        if let org_kde_kwin_appmenu::Request::SetAddress {
            service_name,
            object_path,
        } = request
        {
            let address = address(service_name, object_path);
            let changed = with_states(&data.surface, |states| {
                let Some(metadata) = states.data_map.get::<MenuMetadata>() else {
                    return false;
                };
                let mut cache = metadata.0.lock().unwrap();
                if cache.owner.as_ref() != Some(&resource.id()) || cache.address == address {
                    return false;
                }
                cache.address = address;
                true
            });
            if changed {
                state.ipc_refresh_windows();
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &OrgKdeKwinAppmenu,
        data: &AppMenuSurfaceData,
    ) {
        if !data.surface.is_alive() {
            return;
        }
        let cleared = with_states(&data.surface, |states| {
            let Some(metadata) = states.data_map.get::<MenuMetadata>() else {
                return false;
            };
            let mut cache = metadata.0.lock().unwrap();
            if cache.owner.as_ref() != Some(&resource.id()) {
                return false;
            }
            cache.owner = None;
            cache.address = None;
            true
        });
        if cleared {
            state.ipc_refresh_windows();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::address;
    #[test]
    fn menu_addresses_are_bounded_and_follow_dbus_grammar() {
        for (bus, path) in [
            (":1.42", "/MenuBar/1"),
            ("org.example.App", "/"),
            ("org.example.App", "/Menu_Bar/1"),
        ] {
            assert!(address(bus.into(), path.into()).is_some());
        }
        for (bus, path) in [
            ("", "/MenuBar"),
            ("single", "/MenuBar"),
            ("1org.example", "/MenuBar"),
            (":.42", "/MenuBar"),
            ("org.é", "/MenuBar"),
            (":1.42", "/Menu-Bar"),
            (":1.42", "/MenuBar/"),
            (":1.42", "MenuBar"),
            (":1.42", "/Menu//Bar"),
        ] {
            assert!(address(bus.into(), path.into()).is_none(), "{bus}: {path}");
        }
        assert!(address(format!("org.{}", "x".repeat(252)), "/MenuBar".into()).is_none());
        assert!(address(":1.42".into(), format!("/{}", "x".repeat(4096))).is_none());
    }
}
