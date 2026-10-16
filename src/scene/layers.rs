//! Layer-shell scene imports; stacking, backdrop routing and cache namespaces.
use crate::layer::mapped::LayerSurfaceRenderElement;
use crate::layer::MappedLayer;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use smithay::desktop::{layer_map_for_output, LayerMap, LayerSurface};
use smithay::output::Output;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::shell::wlr_layer::Layer;
use std::collections::HashMap;

pub(crate) struct SceneLayers<'a> {
    pub mapped: &'a HashMap<LayerSurface, MappedLayer>,
}

impl SceneLayers<'_> {
    pub(crate) fn has_blocked_background(&self, output: &Output) -> bool {
        let layer_map = layer_map_for_output(output);
        [false, true].into_iter().any(|backdrop| {
            self.in_render_order(&layer_map, Layer::Background, backdrop)
                .any(|(mapped, _)| mapped.rules().block_out_from.is_some())
        })
    }

    pub(crate) fn in_render_order<'a>(
        &'a self,
        layer_map: &'a LayerMap,
        layer: Layer,
        for_backdrop: bool,
    ) -> impl Iterator<Item = (&'a MappedLayer, Rectangle<i32, Logical>)> {
        // LayerMap returns layers in reverse stacking order.
        layer_map.layers_on(layer).rev().filter_map(move |surface| {
            let mapped = self.mapped.get(surface)?;

            if for_backdrop != mapped.place_within_backdrop() {
                return None;
            }

            let geo = layer_map.layer_geometry(surface)?;
            Some((mapped, geo))
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_normal<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        ns: Option<usize>,
        layer_map: &LayerMap,
        layer: Layer,
        xray_pos: XrayPos,
        for_backdrop: bool,
        push: &mut dyn FnMut(LayerSurfaceRenderElement<R>),
    ) {
        for (mapped, geo) in self.in_render_order(layer_map, layer, for_backdrop) {
            let loc = geo.loc.to_f64();
            let xray_pos = xray_pos.offset(loc);
            mapped.render_normal(ctx.r(), ns, loc, xray_pos, push);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_popups<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        ns: Option<usize>,
        layer_map: &LayerMap,
        layer: Layer,
        xray_pos: XrayPos,
        for_backdrop: bool,
        push: &mut dyn FnMut(LayerSurfaceRenderElement<R>),
    ) {
        for (mapped, geo) in self.in_render_order(layer_map, layer, for_backdrop) {
            let loc = geo.loc.to_f64();
            let xray_pos = xray_pos.offset(loc);
            mapped.render_popups(ctx.r(), ns, loc, xray_pos, push);
        }
    }
    pub(crate) fn fill_xray_elements<R: ClarionRenderer>(
        &self,
        output: &Output,
        xray: &crate::render_helpers::xray::Xray,
        mut ctx: RenderCtx<R>,
    ) {
        // Sources of the effect buffer cannot recursively consume that buffer.
        ctx.xray = None;
        let layer_map = layer_map_for_output(output);
        for (buffers, for_backdrop) in [(&xray.background, false), (&xray.backdrop, true)] {
            let mut buffer = buffers[ctx.target as usize].borrow_mut();
            if let Some(mut gles) = ctx.as_gles() {
                let elements = buffer.elements();
                elements.clear();
                self.render_normal(
                    gles.r(),
                    None,
                    &layer_map,
                    Layer::Background,
                    XrayPos::default(),
                    for_backdrop,
                    &mut |elem| elements.push(elem.into()),
                );
            } else if let Some(mut native) = ctx.as_native() {
                let elements = buffer.elements_native();
                elements.clear();
                self.render_normal(
                    native.r(),
                    None,
                    &layer_map,
                    Layer::Background,
                    XrayPos::default(),
                    for_backdrop,
                    &mut |elem| elements.push(elem.into()),
                );
            }
        }
    }
}
