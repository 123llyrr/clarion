//! Window-grid composition from a single renderer-free presentation projection.
//!
//! The projection fixes front-to-back source order, geometry, selection and
//! transition poses. Composition owns all drawing and cache access without
//! retaining a desktop owner or an iterator that can recover one.

use smithay::backend::renderer::element::utils::{
    Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::{Element as _, Kind};
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::layout::monitor::{MonitorInnerRenderElement, MonitorRenderElement};
use crate::layout::scene_source::{
    ClosingRenderSource, ShadowRenderSource, TileRenderSource, WorkspaceScene,
};
use crate::layout::workspace::WorkspaceId;
use crate::layout::LayoutElement;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::solid_color::SolidColorRenderElement;
use crate::render_helpers::texture::TextureRenderElement;
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use crate::scene::monitor_resources::MonitorResources;
use crate::scene::retained::ResourceLease;

#[derive(Debug, Clone, Copy)]
pub(crate) struct SpreadTilePlacement {
    pub(crate) geometry: Rectangle<f64, Logical>,
    pub(crate) zoom: f64,
    pub(crate) focused: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SpreadClosingPlacement {
    pub(crate) source_position: Point<f64, Logical>,
    pub(crate) position: Point<f64, Logical>,
    pub(crate) zoom: f64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SpreadWorkspacePlacement {
    pub(crate) id: WorkspaceId,
    pub(crate) geometry: Rectangle<f64, Logical>,
    pub(crate) zoom: f64,
    pub(crate) visible: bool,
    pub(crate) desktop_geometry: Rectangle<f64, Logical>,
    pub(crate) desktop_visible: bool,
}

pub(crate) struct SpreadGeometry {
    pub(crate) scale: f64,
    pub(crate) view_size: Size<f64, Logical>,
    pub(crate) progress: f32,
    pub(crate) shadow_alpha: Option<f32>,
    pub(crate) tiles: Vec<SpreadTilePlacement>,
    pub(crate) closing: Vec<SpreadClosingPlacement>,
    pub(crate) workspaces: Vec<SpreadWorkspacePlacement>,
}

pub(crate) struct SpreadWorkspaceSources<'a> {
    pub(crate) background: SolidColorRenderElement,
    pub(crate) shadow: ShadowRenderSource<'a>,
}

pub(crate) struct SpreadComposition<'a, W: LayoutElement> {
    pub(crate) geometry: SpreadGeometry,
    pub(crate) tiles: Vec<TileRenderSource<'a, W>>,
    pub(crate) closing: Vec<ClosingRenderSource<'a>>,
    pub(crate) workspaces: Vec<SpreadWorkspaceSources<'a>>,
    pub(crate) title: String,
    pub(crate) resources: ResourceLease<MonitorResources>,
}

impl<'a, W: LayoutElement> SpreadComposition<'a, W> {
    /// The lower-layer pass consumes this same captured geometry and stable
    /// background identities; it never traverses the desktop again.
    pub(crate) fn workspace_scenes(&self) -> impl Iterator<Item = WorkspaceScene> + '_ {
        self.geometry
            .workspaces
            .iter()
            .zip(&self.workspaces)
            .filter(|(placement, _)| placement.visible)
            .map(|(placement, sources)| WorkspaceScene {
                namespace: placement.id.get() as usize,
                geometry: placement.geometry,
                zoom: placement.zoom,
                background: sources.background.clone(),
            })
    }

    pub(crate) fn render<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let _span = clarion_tracy_span!("Scene::compose_spread");
        let scale = self.geometry.scale;
        for (placement, source) in self.geometry.closing.iter().zip(&self.closing) {
            let origin = placement.source_position.to_physical_precise_round(scale);
            for element in source.render(
                ctx.r(),
                Rectangle::from_size(self.geometry.view_size),
                scale.into(),
            ) {
                let element = RescaleRenderElement::from_element(
                    MonitorInnerRenderElement::SpreadClosing(element),
                    origin,
                    placement.zoom,
                );
                push(RelocateRenderElement::from_element(
                    element,
                    placement.position.to_physical_precise_round(scale) - origin,
                    Relocate::Relative,
                ));
            }
        }

        self.render_header(ctx.r(), push);
        for (placement, source) in self.geometry.tiles.iter().zip(&self.tiles) {
            if placement.zoom <= 0. {
                continue;
            }
            source.render(
                ctx.r(),
                Point::default(),
                XrayPos::new(placement.geometry.loc, placement.zoom),
                placement.focused,
                &mut |element| {
                    let element = RescaleRenderElement::from_element(
                        MonitorInnerRenderElement::SpreadTile(element),
                        Point::default(),
                        placement.zoom,
                    );
                    push(RelocateRenderElement::from_element(
                        element,
                        placement.geometry.loc.to_physical_precise_round(scale),
                        Relocate::Relative,
                    ));
                },
            );
        }

        // Keep the same background buffer identity at both closed endpoints.
        // Its full desktop geometry is independent of the grid thumbnail pose.
        let progress = self.geometry.progress;
        if progress < 1. {
            for (placement, sources) in self.geometry.workspaces.iter().zip(&self.workspaces) {
                if !placement.desktop_visible {
                    continue;
                }
                let background = &sources.background;
                let element = SolidColorRenderElement::new(
                    background.id().clone(),
                    placement.desktop_geometry,
                    background.current_commit(),
                    background.color() * (1. - progress),
                    Kind::Unspecified,
                );
                push(at_output_origin(MonitorInnerRenderElement::SolidColor(
                    element,
                )));
            }
        }
    }

    fn render_header<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let scale = self.geometry.scale;
        let renderer_context = ctx.renderer.context_id().erased();
        let mut cache = self.resources.spread_header.borrow_mut();
        if cache.as_ref().is_none_or(|(text, s, context, _)| {
            text != &self.title || *s != scale.to_bits() || context != &renderer_context
        }) {
            // Invalidate first: a failed regeneration must never render a
            // texture imported by a previous renderer context or epoch.
            *cache = None;
            if let Ok(texture) =
                crate::ui::mru::generate_title_texture(ctx.renderer, &self.title, scale)
            {
                *cache = Some((
                    self.title.clone(),
                    scale.to_bits(),
                    renderer_context,
                    texture,
                ));
            }
        }
        let Some((_, _, _, texture)) = cache.as_ref() else {
            return;
        };
        let element = match texture {
            crate::ui::mru::MruTexture::Gles(buffer) => {
                PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
                    buffer.clone(),
                    (24., 24.),
                    self.geometry.progress,
                    None,
                    None,
                    Kind::Unspecified,
                ))
            }
            crate::ui::mru::MruTexture::Vulkan(buffer) => {
                PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
                    buffer.clone(),
                    (24., 24.),
                    self.geometry.progress,
                    None,
                    None,
                    Kind::Unspecified,
                ))
            }
        };
        push(at_output_origin(MonitorInnerRenderElement::SpreadText(
            element,
        )));
    }

    pub(crate) fn render_shadows<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let Some(alpha) = self.geometry.shadow_alpha else {
            return;
        };
        for (placement, sources) in self.geometry.workspaces.iter().zip(&self.workspaces) {
            if !placement.visible {
                continue;
            }
            sources.shadow.render(renderer, &mut |element| {
                let element = RescaleRenderElement::from_element(
                    MonitorInnerRenderElement::Shadow(element.with_alpha(alpha)),
                    Point::default(),
                    placement.zoom,
                );
                push(RelocateRenderElement::from_element(
                    element,
                    placement
                        .geometry
                        .loc
                        .to_physical_precise_round(self.geometry.scale),
                    Relocate::Relative,
                ));
            });
        }
    }
}

fn at_output_origin<R: ClarionRenderer>(
    element: MonitorInnerRenderElement<R>,
) -> MonitorRenderElement<R> {
    RelocateRenderElement::from_element(
        RescaleRenderElement::from_element(element, Point::default(), 1.),
        Point::default(),
        Relocate::Relative,
    )
}
