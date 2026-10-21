use std::sync::Mutex;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::{with_states, SurfaceData};
use crate::color::SurfaceColor;

#[derive(Default)]
pub struct Publication {
    pub owner: Option<u64>,
    pub visible: SurfaceColor,
}

#[derive(Default)]
pub struct ColorAttachment(pub Mutex<Publication>);

pub fn surface_color_description(states: &SurfaceData) -> SurfaceColor {
    states
        .data_map
        .get::<ColorAttachment>()
        .map(|state| state.0.lock().unwrap().visible)
        .unwrap_or_default()
}

pub fn surface_color_description_for_surface(surface: &WlSurface) -> SurfaceColor {
    with_states(surface, surface_color_description)
}
