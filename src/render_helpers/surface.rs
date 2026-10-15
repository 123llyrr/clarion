use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::utils::{import_surface, RendererSurfaceStateUserData};
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Scale};
use smithay::wayland::compositor::{with_surface_tree_downward, TraversalAction};
use smithay::wayland::single_pixel_buffer::get_single_pixel_buffer;

use super::texture::TextureBuffer;
use super::BakedBuffer;

/// Renders elements from a surface tree as textures into `storage`.
pub fn render_snapshot_from_surface_tree<R>(
    renderer: &mut R,
    surface: &WlSurface,
    location: Point<f64, Logical>,
    storage: &mut Vec<BakedBuffer<TextureBuffer<R::TextureId>>>,
) where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + 'static,
{
    let _span = realand_tracy_span!("render_snapshot_from_surface_tree");

    with_surface_tree_downward(
        surface,
        location,
        |_, states, location| {
            let mut location = *location;
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            if let Some(data) = data {
                let data = &*data.lock().unwrap();

                if let Some(view) = data.view() {
                    location += view.offset.to_f64();
                    TraversalAction::DoChildren(location)
                } else {
                    TraversalAction::SkipChildren
                }
            } else {
                TraversalAction::SkipChildren
            }
        },
        |_, states, location| {
            let mut location = *location;
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            if let Some(data) = data {
                let Some(view) = data.lock().unwrap().view() else {
                    return;
                };
                location += view.offset.to_f64();

                if let Err(err) = import_surface(renderer, states) {
                    warn!("failed to import surface: {err:?}");
                    return;
                }

                let data = data.lock().unwrap();
                // Smithay draws single-pixel buffers as solid colors and deliberately
                // skips importing them. A texture snapshot must materialize that color,
                // including on subsurfaces, rather than silently losing its contents.
                let pixel = data
                    .buffer()
                    .and_then(|buffer| get_single_pixel_buffer(buffer).ok());
                let texture = if let Some(pixel) = pixel {
                    match renderer.import_memory(
                        &pixel.rgba8888(),
                        Fourcc::Abgr8888,
                        (1, 1).into(),
                        false,
                    ) {
                        Ok(texture) => texture,
                        Err(err) => {
                            warn!("failed to snapshot single-pixel surface: {err:?}");
                            return;
                        }
                    }
                } else if let Some(texture) = data.texture(renderer.context_id()) {
                    texture.clone()
                } else {
                    return;
                };

                let buffer = TextureBuffer::from_texture(
                    renderer,
                    texture,
                    f64::from(data.buffer_scale()),
                    data.buffer_transform(),
                    Vec::new(),
                );

                let baked = BakedBuffer {
                    buffer,
                    location,
                    src: Some(view.src),
                    dst: Some(view.dst),
                };

                storage.push(baked);
            }
        },
        |_, _, _| true,
    );
}

pub fn push_elements_from_surface_tree<R>(
    renderer: &mut R,
    surface: &WlSurface,
    // Fractional scale expects surface buffers to be aligned to physical pixels.
    location: Point<i32, Physical>,
    scale: Scale<f64>,
    alpha: f32,
    kind: Kind,
    push: &mut dyn FnMut(WaylandSurfaceRenderElement<R>),
) where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    let _span = realand_tracy_span!("push_elements_from_surface_tree");

    let location = location.to_f64();

    with_surface_tree_downward(
        surface,
        location,
        |_, states, location| {
            let mut location = *location;
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            if let Some(data) = data {
                if let Some(view) = data.lock().unwrap().view() {
                    location += view.offset.to_f64().to_physical(scale);
                    TraversalAction::DoChildren(location)
                } else {
                    TraversalAction::SkipChildren
                }
            } else {
                TraversalAction::SkipChildren
            }
        },
        |surface, states, location| {
            let mut location = *location;
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            if let Some(data) = data {
                let has_view = if let Some(view) = data.lock().unwrap().view() {
                    location += view.offset.to_f64().to_physical(scale);
                    true
                } else {
                    false
                };

                if has_view {
                    match WaylandSurfaceRenderElement::from_surface(
                        renderer, surface, states, location, alpha, kind,
                    ) {
                        Ok(Some(surface)) => push(surface),
                        Ok(None) => {} // surface is not mapped
                        Err(err) => {
                            warn!("failed to import surface: {}", err);
                        }
                    };
                }
            }
        },
        |_, _, _| true,
    );
}
