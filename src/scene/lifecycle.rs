//! Scene-resource updates, deliberately independent of device scheduling and IPC.
use std::collections::HashMap;

use smithay::backend::renderer::Color32F;
use smithay::desktop::{layer_map_for_output, LayerSurface};
use smithay::output::Output;
use smithay::utils::{Logical, Rectangle, Scale};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::layer::MappedLayer;
use crate::render_helpers::blur::BlurOptions;
use crate::render_helpers::xray::Xray;
use crate::ui::screen_transition::ScreenTransition;

pub(crate) fn advance_screen_transition(
    transition: &mut Option<ScreenTransition>,
    locking: bool,
    overview_disabled: bool,
) {
    if transition.as_ref().is_some_and(|transition| {
        locking || (overview_disabled && transition.is_overview()) || transition.is_done()
    }) {
        *transition = None;
    }
}

/// Update only the resources whose contents are shared by glass/backdrop effects.
/// The caller retains single-threaded desktop ownership and chooses the output.
pub(crate) fn update_xray(
    output: &Output,
    backgrounds: impl IntoIterator<Item = (Rectangle<f64, Logical>, Color32F)>,
    xray: &mut Xray,
    backdrop_color: Color32F,
    blur: BlurOptions,
) {
    let scale = Scale::from(output.current_scale().fractional_scale());
    let size = output
        .current_transform()
        .transform_size(output.current_mode().unwrap().size);
    xray.workspaces.clear();
    xray.workspaces.extend(backgrounds);
    xray.backdrop_color = backdrop_color;
    for buffer in xray.background.iter().chain(&xray.backdrop) {
        let mut buffer = buffer.borrow_mut();
        buffer.update_size(size, scale);
        buffer.update_blur_options(blur);
    }
}

pub(crate) fn update_transition(output: &Output, transition: Option<&mut ScreenTransition>) {
    if let Some(transition) = transition {
        transition.update_render_elements(
            Scale::from(output.current_scale().fractional_scale()),
            output.current_transform(),
        );
    }
}

// Smithay hashes LayerSurface by its stable protocol identity. Its internal
// resource liveness atomics never change that identity or the map's key hash.
#[allow(clippy::mutable_key_type)]
pub(crate) fn update_layers(
    output: &Output,
    layers: &mut HashMap<LayerSurface, MappedLayer>,
    background_only: bool,
) {
    let map = layer_map_for_output(output);
    for surface in map.layers() {
        if background_only && surface.layer() != Layer::Background {
            continue;
        }
        let Some(mapped) = layers.get_mut(surface) else {
            continue;
        };
        if let Some(geometry) = map.layer_geometry(surface) {
            mapped.update_render_elements(geometry.size.to_f64());
        }
    }
}
