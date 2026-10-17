//! Scene-owned decoration buffers. Layout supplies numeric geometry and style.

use std::time::Duration;

use clarion_config::CornerRadius;
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::focus_ring::FocusRing;
#[cfg(test)]
use super::focus_ring::FocusRingRenderElement;
use super::shadow::Shadow;
use super::tile::TileRenderElement;
use crate::render_helpers::clipped_surface::RoundedCornerDamage;
use crate::render_helpers::damage::ExtraDamage;
use crate::render_helpers::renderer::ClarionRenderer;
#[cfg(test)]
use crate::render_helpers::renderer::DecorationRenderer;
use crate::render_helpers::solid_color::SolidColorBuffer;

#[derive(Debug)]
pub(crate) struct WindowDecorations {
    pub(crate) border: FocusRing,
    pub(crate) focus_ring: FocusRing,
    pub(crate) shadow: Shadow,
    pub(crate) fullscreen_backdrop: SolidColorBuffer,
    pub(crate) rounded_corner_damage: RoundedCornerDamage,
    last_size: Option<Size<f64, Logical>>,
    last_view: Option<Rectangle<f64, Logical>>,
    last_active: bool,
}

impl Default for WindowDecorations {
    fn default() -> Self {
        Self::new(
            Default::default(),
            Default::default(),
            Default::default(),
            2.,
        )
    }
}

impl WindowDecorations {
    /// Apply initial style without allocating a second set of decoration IDs.
    pub(crate) fn configure(
        &mut self,
        border: clarion_config::FocusRing,
        focus_ring: clarion_config::FocusRing,
        shadow: clarion_config::Shadow,
        rounding_power: f32,
    ) {
        self.border.update_config(border);
        self.focus_ring.update_config(focus_ring);
        self.shadow.update_config(shadow);
        self.border.set_rounding_power(rounding_power);
        self.focus_ring.set_rounding_power(rounding_power);
        self.last_size = None;
        self.last_view = None;
    }

    pub(crate) fn new(
        border: clarion_config::FocusRing,
        focus_ring: clarion_config::FocusRing,
        shadow: clarion_config::Shadow,
        rounding_power: f32,
    ) -> Self {
        let mut border = FocusRing::new(border);
        let mut focus_ring = FocusRing::new(focus_ring);
        border.set_rounding_power(rounding_power);
        focus_ring.set_rounding_power(rounding_power);
        Self {
            border,
            focus_ring,
            shadow: Shadow::new(shadow),
            fullscreen_backdrop: SolidColorBuffer::new((0., 0.), [0., 0., 0., 1.]),
            rounded_corner_damage: Default::default(),
            last_size: None,
            last_view: None,
            last_active: false,
        }
    }

    pub(crate) fn update_shaders(&mut self) {
        self.border.update_shaders();
        self.focus_ring.update_shaders();
        self.shadow.update_shaders();
    }

    pub(crate) fn update(&mut self, update: DecorationUpdate) {
        let DecorationUpdate {
            tile_size,
            border_width,
            border_visible,
            draw_border_with_background,
            is_active,
            is_urgent,
            view,
            radius,
            window_radius,
            scale,
            alpha,
            time,
            geometry_changed,
        } = update;
        let geometry_changed = geometry_changed
            || self.last_active != is_active
            || self.last_size != Some(tile_size)
            || self.last_view != Some(view);
        self.last_active = is_active;
        self.last_size = Some(tile_size);
        self.last_view = Some(view);

        let mut border_window_size = tile_size;
        border_window_size.w -= border_width * 2.;
        border_window_size.h -= border_width * 2.;
        self.border.update_render_elements(
            border_window_size,
            is_active,
            !draw_border_with_background,
            is_urgent,
            Rectangle::new(
                view.loc - Point::from((border_width, border_width)),
                view.size,
            ),
            radius,
            scale,
            alpha,
            time,
        );
        let radius = if border_visible {
            radius
        } else {
            window_radius
        };
        self.shadow
            .update_render_elements(tile_size, is_active, radius, scale, alpha);
        let draw_focus_ring_with_background = self.border.is_off() && draw_border_with_background;
        self.focus_ring.update_render_elements(
            tile_size,
            is_active,
            !draw_focus_ring_with_background,
            is_urgent,
            view,
            radius.expanded_by(self.focus_ring.width() as f32),
            scale,
            alpha,
            time,
        );
        if geometry_changed {
            self.fullscreen_backdrop.resize(tile_size);
        }
    }

    pub(crate) fn render_border<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        location: Point<f64, Logical>,
        border_width: Option<f64>,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) {
        if let Some(width) = border_width {
            self.border.render(
                renderer,
                location + Point::from((width, width)),
                &mut |elem| push(elem.into()),
            );
        }
    }

    pub(crate) fn render_focus_ring<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        location: Point<f64, Logical>,
        visible: bool,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) {
        if visible {
            self.focus_ring
                .render(renderer, location, &mut |elem| push(elem.into()));
        }
    }

    pub(crate) fn render_shadow<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        location: Point<f64, Logical>,
        visible: bool,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) {
        if visible {
            self.shadow
                .render(renderer, location, &mut |elem| push(elem.into()));
        }
    }

    #[cfg(test)]
    pub(crate) fn render_decoration_rings(
        &self,
        renderer: &mut impl DecorationRenderer,
        push: &mut dyn FnMut(FocusRingRenderElement),
    ) {
        for ring in [&self.border, &self.focus_ring] {
            ring.render(renderer, Point::default(), push);
        }
    }

    pub(crate) fn render_rounded_corner_damage(&self, geo: Rectangle<f64, Logical>) -> ExtraDamage {
        self.rounded_corner_damage.render(geo)
    }
}

/// No client, layout owner, renderer or GPU handle crosses this update boundary.
pub(crate) struct DecorationUpdate {
    pub(crate) tile_size: Size<f64, Logical>,
    pub(crate) border_width: f64,
    pub(crate) border_visible: bool,
    pub(crate) draw_border_with_background: bool,
    pub(crate) is_active: bool,
    pub(crate) is_urgent: bool,
    pub(crate) view: Rectangle<f64, Logical>,
    pub(crate) radius: CornerRadius,
    pub(crate) window_radius: CornerRadius,
    pub(crate) scale: f64,
    pub(crate) alpha: f32,
    pub(crate) time: Option<Duration>,
    pub(crate) geometry_changed: bool,
}
