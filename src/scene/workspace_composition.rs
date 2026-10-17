//! Ordinary desktop/Expo composition from one immutable layout projection.
//!
//! Geometry and ordered sources are captured once. This module owns the four
//! drawing channels, clipping and transforms; it never receives a Monitor or
//! Workspace and cannot traverse or mutate desktop state.
use smallvec::SmallVec;
use smithay::backend::renderer::element::utils::{
    CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size};

use crate::layout::dwindle::DwindleSpaceRenderElement;
use crate::layout::floating::FloatingSpaceRenderElement;
use crate::layout::monitor::{MonitorInnerRenderElement, MonitorRenderElement};
use crate::layout::scene_source::{
    ClosingRenderSource, InsertHintRenderSource, ShadowRenderSource, TabRenderSource,
    TileRenderSource,
};
use crate::layout::scrolling::ScrollingSpaceRenderElement;
use crate::layout::workspace::{WorkspaceId, WorkspaceRenderElement};
use crate::layout::LayoutElement;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use crate::scene::monitor_resources::MonitorResources;
use crate::scene::retained::ResourceLease;

/// This is the established front-to-back ordering, including moving windows
/// whose destination workspace is outside the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Channel {
    FloatingMoving,
    FloatingNormal,
    TilingMoving,
    TilingNormal,
}
impl Channel {
    pub(crate) const ORDER: [Self; 4] = [
        Self::FloatingMoving,
        Self::FloatingNormal,
        Self::TilingMoving,
        Self::TilingNormal,
    ];
    pub(crate) fn normal(self) -> bool {
        matches!(self, Self::FloatingNormal | Self::TilingNormal)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TileSpace {
    Floating,
    Scrolling,
    Dwindle,
}

pub(crate) enum Item<'a, W: LayoutElement> {
    Tile {
        source: TileRenderSource<'a, W>,
        position: Point<f64, Logical>,
        focused: bool,
        space: TileSpace,
    },
    Closing {
        source: ClosingRenderSource<'a>,
        view: Rectangle<f64, Logical>,
        scale: Scale<f64>,
        space: TileSpace,
    },
    Genie {
        source: TileRenderSource<'a, W>,
        origin: Point<f64, Logical>,
    },
    Tab {
        source: TabRenderSource<'a>,
        position: Point<f64, Logical>,
    },
}

pub(crate) struct OrderedItem<'a, W: LayoutElement> {
    pub(crate) channel: Channel,
    pub(crate) item: Item<'a, W>,
}

/// Numeric geometry/identity only; retained pixels are separate capabilities.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WorkspacePlacement {
    pub(crate) id: WorkspaceId,
    pub(crate) geometry: Rectangle<f64, Logical>,
    pub(crate) zoom: f64,
    pub(crate) normal_visible: bool,
}

pub(crate) struct GeometryPlan {
    pub(crate) scale: f64,
    pub(crate) placements: SmallVec<[WorkspacePlacement; 2]>,
    pub(crate) normal_crop: Rectangle<i32, Physical>,
    pub(crate) moving_crop: Rectangle<i32, Physical>,
    pub(crate) shadow_alpha: Option<f32>,
    pub(crate) expo: Option<ExpoBorders>,
}

pub(crate) struct ExpoBorders {
    pub(crate) alpha: f32,
    pub(crate) selected: usize,
    pub(crate) active_color: [f32; 4],
}

pub(crate) struct WorkspaceSources<'a, W: LayoutElement> {
    pub(crate) items: Vec<OrderedItem<'a, W>>,
    pub(crate) background: SolidColorRenderElement,
    pub(crate) shadow: ShadowRenderSource<'a>,
}

pub(crate) struct Composition<'a, W: LayoutElement> {
    pub(crate) geometry: GeometryPlan,
    pub(crate) workspaces: SmallVec<[WorkspaceSources<'a, W>; 2]>,
    pub(crate) insert_hint: Option<(WorkspaceId, InsertHintRenderSource<'a>, Point<f64, Logical>)>,
    pub(crate) resources: ResourceLease<MonitorResources>,
}

impl<W: LayoutElement> Composition<'_, W> {
    pub(crate) fn render<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        focus_ring: bool,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let _span = clarion_tracy_span!("Scene::compose_workspaces");
        let scale = self.geometry.scale;
        for channel in Channel::ORDER {
            let crop = if channel.normal() {
                self.geometry.normal_crop
            } else {
                self.geometry.moving_crop
            };
            for (placement, sources) in self.geometry.placements.iter().zip(&self.workspaces) {
                if placement.geometry.size.w <= 0.
                    || placement.geometry.size.h <= 0.
                    || (channel.normal() && !placement.normal_visible)
                {
                    continue;
                }
                let xray = XrayPos::new(placement.geometry.loc, placement.zoom);
                let mut push_workspace = |element: WorkspaceRenderElement<R>| {
                    if let Some(element) = CropRenderElement::from_element(element, scale, crop) {
                        let element = MonitorInnerRenderElement::from(element);
                        push(transform(element, placement, scale));
                    }
                };
                for source in sources
                    .items
                    .iter()
                    .filter(|source| source.channel == channel)
                {
                    source
                        .item
                        .render(ctx.r(), xray, focus_ring, &mut push_workspace);
                }
                if channel == Channel::FloatingNormal {
                    if let Some((id, source, location)) = &self.insert_hint {
                        if *id == placement.id {
                            source.render(ctx.renderer, *location, &mut |element| {
                                if let Some(element) =
                                    CropRenderElement::from_element(element, scale, crop)
                                {
                                    push(transform(
                                        MonitorInnerRenderElement::InsertHint(element),
                                        placement,
                                        scale,
                                    ));
                                }
                            });
                        }
                    }
                }
            }
        }
        self.render_expo_borders(push);
    }

    pub(crate) fn render_shadows<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let Some(alpha) = self.geometry.shadow_alpha else {
            return;
        };
        for (placement, sources) in self.geometry.placements.iter().zip(&self.workspaces) {
            if placement.normal_visible {
                sources.shadow.render(renderer, &mut |element| {
                    push(transform(
                        MonitorInnerRenderElement::Shadow(element.with_alpha(alpha)),
                        placement,
                        self.geometry.scale,
                    ));
                });
            }
        }
    }

    fn render_expo_borders<R: ClarionRenderer>(
        &self,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let Some(expo) = &self.geometry.expo else {
            return;
        };
        if expo.alpha <= 0.05 {
            return;
        }
        let mut cache = self.resources.expo_borders.borrow_mut();
        cache.resize_with(self.geometry.placements.len(), || {
            std::array::from_fn(|_| SolidColorBuffer::new((0., 0.), [0.; 4]))
        });
        for (index, placement) in self.geometry.placements.iter().enumerate() {
            let geometry = placement.geometry;
            let width = if index == expo.selected { 4. } else { 1. };
            let color = if index == expo.selected {
                expo.active_color
            } else {
                [0.25, 0.25, 0.30, 0.6]
            };
            if geometry.size.w <= width * 2. || geometry.size.h <= width * 2. {
                continue;
            }
            let loc = geometry.loc;
            let size = geometry.size;
            let rectangles = [
                Rectangle::new(loc, Size::from((size.w, width))),
                Rectangle::new(
                    (loc.x, loc.y + size.h - width).into(),
                    (size.w, width).into(),
                ),
                Rectangle::new(
                    (loc.x, loc.y + width).into(),
                    (width, size.h - width * 2.).into(),
                ),
                Rectangle::new(
                    (loc.x + size.w - width, loc.y + width).into(),
                    (width, size.h - width * 2.).into(),
                ),
            ];
            for (buffer, rectangle) in cache[index].iter_mut().zip(rectangles) {
                buffer.update(rectangle.size, color);
                let element = SolidColorRenderElement::from_buffer(
                    buffer,
                    rectangle.loc,
                    expo.alpha,
                    Kind::Unspecified,
                );
                let element = RescaleRenderElement::from_element(
                    MonitorInnerRenderElement::SolidColor(element),
                    Point::default(),
                    1.,
                );
                push(RelocateRenderElement::from_element(
                    element,
                    Point::default(),
                    Relocate::Relative,
                ));
            }
        }
    }
}

fn transform<R: ClarionRenderer>(
    element: MonitorInnerRenderElement<R>,
    placement: &WorkspacePlacement,
    scale: f64,
) -> MonitorRenderElement<R> {
    RelocateRenderElement::from_element(
        RescaleRenderElement::from_element(element, Point::default(), placement.zoom),
        placement.geometry.loc.to_physical_precise_round(scale),
        Relocate::Relative,
    )
}

impl<W: LayoutElement> Item<'_, W> {
    pub(crate) fn render<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        xray: XrayPos,
        focus_ring: bool,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) {
        match self {
            Self::Tile {
                source,
                position,
                focused,
                space,
            } => source.render(
                ctx,
                *position,
                xray.offset(*position),
                focus_ring && *focused,
                &mut |element| {
                    push(match space {
                        TileSpace::Floating => FloatingSpaceRenderElement::Tile(element).into(),
                        TileSpace::Scrolling => ScrollingSpaceRenderElement::Tile(element).into(),
                        TileSpace::Dwindle => DwindleSpaceRenderElement::Tile(element).into(),
                    });
                },
            ),
            Self::Closing {
                source,
                view,
                scale,
                space,
            } => {
                for element in source.render(ctx.r(), *view, *scale) {
                    push(match space {
                        TileSpace::Floating => {
                            FloatingSpaceRenderElement::ClosingWindow(element).into()
                        }
                        TileSpace::Scrolling => {
                            ScrollingSpaceRenderElement::ClosingWindow(element).into()
                        }
                        TileSpace::Dwindle => {
                            DwindleSpaceRenderElement::ClosingWindow(element).into()
                        }
                    });
                }
            }
            Self::Genie { source, origin } => {
                source.render_genie(ctx, xray.offset(*origin), &mut |element| {
                    push(WorkspaceRenderElement::Minimized(element))
                });
            }
            Self::Tab { source, position } => {
                source.render(ctx.renderer, *position, &mut |element| {
                    push(ScrollingSpaceRenderElement::<R>::TabIndicator(element).into())
                })
            }
        }
    }
}
