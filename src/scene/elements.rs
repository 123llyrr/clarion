//! Render-element ownership and coordinate transforms for output snapshots.
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::utils::{
    CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::Element;
use smithay::utils::{Logical, Point, Rectangle, Scale};

use crate::clarion_render_elements;
use crate::layer::mapped::LayerSurfaceRenderElement;
use crate::layout::tile::TileRenderElement;
use crate::layout::{LayoutElementRenderElement, MonitorRenderElement};
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::solid_color::SolidColorRenderElement;
use crate::ui::exit_confirm_dialog::ExitConfirmDialogRenderElement;
use crate::ui::mru::WindowMruUiRenderElement;
use crate::ui::screenshot_ui::ScreenshotUiRenderElement;

pub(super) fn scale_relocate_crop<E: Element>(
    elem: E,
    output_scale: Scale<f64>,
    zoom: f64,
    ws_geo: Rectangle<f64, Logical>,
) -> Option<CropRenderElement<RelocateRenderElement<RescaleRenderElement<E>>>> {
    let ws_geo = ws_geo.to_physical_precise_round(output_scale);
    let elem = RescaleRenderElement::from_element(elem, Point::from((0, 0)), zoom);
    let elem = RelocateRenderElement::from_element(elem, ws_geo.loc, Relocate::Relative);
    CropRenderElement::from_element(elem, output_scale, ws_geo)
}

clarion_render_elements! {
    PointerRenderElements<R> => {
        Wayland = WaylandSurfaceRenderElement<R>,
        NamedPointer = MemoryRenderBufferRenderElement<R>,
    }
}

clarion_render_elements! {
    WindowScreenshotRenderElement<R> => {
        Layout = LayoutElementRenderElement<R>,
        Pointer = RelocateRenderElement<PointerRenderElements<R>>,
    }
}

clarion_render_elements! {
    OutputRenderElements<R> => {
        Monitor = MonitorRenderElement<R>,
        RescaledTile = RescaleRenderElement<TileRenderElement<R>>,
        LayerSurface = LayerSurfaceRenderElement<R>,
        RelocatedLayerSurface = CropRenderElement<RelocateRenderElement<RescaleRenderElement<
            LayerSurfaceRenderElement<R>
        >>>,
        RelocatedColor = CropRenderElement<RelocateRenderElement<RescaleRenderElement<
            SolidColorRenderElement
        >>>,
        Pointer = PointerRenderElements<R>,
        Wayland = WaylandSurfaceRenderElement<R>,
        SolidColor = SolidColorRenderElement,
        ScreenshotUi = ScreenshotUiRenderElement,
        WindowMruUi = WindowMruUiRenderElement<R>,
        ExitConfirmDialog = ExitConfirmDialogRenderElement,
        Texture = PrimaryGpuTextureRenderElement,
        // Used for the CPU-rendered panels.
        RelocatedMemoryBuffer = RelocateRenderElement<MemoryRenderBufferRenderElement<R>>,
    }
}
