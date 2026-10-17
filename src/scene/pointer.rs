//! Cursor and drag-icon imports from an immutable pointer projection.
use std::time::Duration;

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Scale};

use super::PointerRenderElements;
use crate::cursor::{CursorManager, CursorTextureCache, RenderCursor, XCursor};
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::surface::push_elements_from_surface_tree;

pub(crate) struct SceneDndIcon<'a> {
    pub surface: &'a WlSurface,
    pub offset: Point<i32, Logical>,
}

pub(crate) struct ScenePointer<'a> {
    pub visible: bool,
    pub position: Point<f64, Logical>,
    pub manager: &'a CursorManager,
    pub textures: &'a CursorTextureCache,
    pub elapsed: Duration,
    pub dnd_icon: Option<SceneDndIcon<'a>>,
}

impl ScenePointer<'_> {
    pub(crate) fn render<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        output: &Output,
        push: &mut dyn FnMut(PointerRenderElements<R>),
    ) {
        let _span = clarion_tracy_span!("Clarion::render_pointer");
        let output_scale = output.current_scale();
        let pointer_pos = self.position;
        let render_cursor = self.manager.get_render_cursor(output_scale.integer_scale());

        let output_scale = Scale::from(output.current_scale().fractional_scale());

        match render_cursor {
            RenderCursor::Hidden => (),
            RenderCursor::Surface { surface, hotspot } => {
                let pointer_pos =
                    (pointer_pos - hotspot.to_f64()).to_physical_precise_round(output_scale);

                push_elements_from_surface_tree(
                    renderer,
                    &surface,
                    pointer_pos,
                    output_scale,
                    1.,
                    Kind::Cursor,
                    &mut |elem| push(elem.into()),
                );
            }
            RenderCursor::Named {
                icon,
                scale,
                cursor,
            } => {
                let (idx, frame) = cursor.frame(self.elapsed.as_millis() as u32);
                let hotspot = XCursor::hotspot(frame).to_logical(scale);
                let pointer_pos =
                    (pointer_pos - hotspot.to_f64()).to_physical_precise_round(output_scale);

                let texture = self.textures.get(icon, scale, &cursor, idx);
                match MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    pointer_pos,
                    &texture,
                    None,
                    None,
                    None,
                    Kind::Cursor,
                ) {
                    Ok(element) => push(element.into()),
                    Err(err) => {
                        warn!("error importing a cursor texture: {err:?}");
                    }
                }
            }
        }

        if let Some(dnd_icon) = self.dnd_icon.as_ref() {
            let pointer_pos =
                (pointer_pos + dnd_icon.offset.to_f64()).to_physical_precise_round(output_scale);
            push_elements_from_surface_tree(
                renderer,
                dnd_icon.surface,
                pointer_pos,
                output_scale,
                1.,
                Kind::ScanoutCandidate,
                &mut |elem| push(elem.into()),
            );
        }
    }
}
