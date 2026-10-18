//! Stage card composition from immutable geometry and narrow render sources.
//!
//! Layout captures each visible card once, in front-to-back order. This scene
//! completes a card's layers, windows, background and shadow before the card
//! behind it, without retaining a desktop or workspace owner.
use clarion_config::{Color, CornerRadius, GradientInterpolation};
use smithay::backend::renderer::element::utils::{
    CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::utils::{Logical, Physical, Point, Rectangle, Size};

use crate::layout::monitor::{MonitorInnerRenderElement, MonitorRenderElement};
use crate::layout::scene_source::{InsertHintRenderSource, StageLayers, WorkspaceScene};
use crate::layout::workspace::{WorkspaceId, WorkspaceRenderElement};
use crate::layout::LayoutElement;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::texture::TextureRenderElement;
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use crate::scene::monitor_resources::MonitorResources;
use crate::scene::retained::ResourceLease;
use crate::scene::workspace_composition::{Channel, WorkspaceSources};

/// Identity and numeric card pose, with separately captured window sources.
pub(crate) struct StageCard<'a, W: LayoutElement> {
    pub(crate) id: WorkspaceId,
    pub(crate) workspace_index: usize,
    pub(crate) geometry: Rectangle<f64, Logical>,
    pub(crate) zoom: f64,
    pub(crate) crop: Rectangle<i32, Physical>,
    pub(crate) highlight: f64,
    pub(crate) sources: WorkspaceSources<'a, W>,
}

/// Display-only rail data; click/drop policy and workspace traversal stay in layout.
pub(crate) struct StageRailItem {
    pub(crate) geometry: Rectangle<f64, Logical>,
    pub(crate) clip: Rectangle<f64, Logical>,
    pub(crate) text: String,
    pub(crate) subtitle: Option<String>,
    pub(crate) navigation: bool,
    pub(crate) enabled: bool,
    pub(crate) highlight: f32,
}

pub(crate) struct StageComposition<'a, W: LayoutElement> {
    pub(crate) cards: Vec<StageCard<'a, W>>,
    pub(crate) rail: Vec<StageRailItem>,
    pub(crate) scale: f64,
    pub(crate) alpha: f32,
    pub(crate) active_color: [f32; 4],
    pub(crate) workspace_count: usize,
    pub(crate) insert_hint: Option<(WorkspaceId, InsertHintRenderSource<'a>, Point<f64, Logical>)>,
    pub(crate) resources: ResourceLease<MonitorResources>,
}

impl<W: LayoutElement> StageComposition<'_, W> {
    /// Lower-layer namespaces reuse the very same card pose and background.
    pub(crate) fn workspace_scenes(&self) -> impl Iterator<Item = WorkspaceScene> + '_ {
        self.cards.iter().map(|card| WorkspaceScene {
            namespace: card.id.get() as usize,
            geometry: card.geometry,
            zoom: card.zoom,
            background: card.sources.background.clone(),
        })
    }

    pub(crate) fn render<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        focus_ring: bool,
        layers: &mut StageLayers<'_, R>,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let _span = clarion_tracy_span!("Scene::compose_stage");
        self.render_rail(ctx.r(), push);
        for card in &self.cards {
            self.render_border(ctx.renderer, card, push);
            self.render_layers(ctx.r(), card, true, layers, push);
            let xray = XrayPos::new(card.geometry.loc, card.zoom);
            for channel in Channel::ORDER {
                for source in card
                    .sources
                    .items
                    .iter()
                    .filter(|source| source.channel == channel)
                {
                    source.item.render(
                        ctx.r(),
                        xray,
                        focus_ring,
                        &mut |element: WorkspaceRenderElement<R>| {
                            if let Some(element) =
                                CropRenderElement::from_element(element, self.scale, card.crop)
                            {
                                push(transform(
                                    MonitorInnerRenderElement::from(element),
                                    card,
                                    self.scale,
                                ));
                            }
                        },
                    );
                }
                if channel == Channel::FloatingNormal {
                    if let Some((id, source, position)) = &self.insert_hint {
                        if *id == card.id {
                            source.render(ctx.renderer, *position, &mut |element| {
                                if let Some(element) =
                                    CropRenderElement::from_element(element, self.scale, card.crop)
                                {
                                    push(transform(
                                        MonitorInnerRenderElement::InsertHint(element),
                                        card,
                                        self.scale,
                                    ));
                                }
                            });
                        }
                    }
                }
            }
            self.render_layers(ctx.r(), card, false, layers, push);
            push(transform(
                MonitorInnerRenderElement::SolidColor(card.sources.background.clone()),
                card,
                self.scale,
            ));
            card.sources.shadow.render(ctx.renderer, &mut |element| {
                push(transform(
                    MonitorInnerRenderElement::Shadow(element.with_alpha(self.alpha)),
                    card,
                    self.scale,
                ));
            });
        }
    }

    fn render_layers<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        card: &StageCard<'_, W>,
        popups: bool,
        layers: &mut StageLayers<'_, R>,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        layers(ctx, card.id, card.geometry, popups, &mut |element| {
            if let Some(element) = CropRenderElement::from_element(element, self.scale, card.crop) {
                push(transform(
                    MonitorInnerRenderElement::StageLayer(element),
                    card,
                    self.scale,
                ));
            }
        });
    }

    fn render_border<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        card: &StageCard<'_, W>,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let geometry = card.geometry;
        let width = 1. + 2. * card.highlight;
        if geometry.size.w <= width * 2. || geometry.size.h <= width * 2. {
            return;
        }
        if BorderRenderElement::has_shader(renderer) {
            self.render_glass_border(card, width, push);
        } else {
            self.render_solid_border(card, width, push);
        }
    }

    /// Translucent glass edge: a soft accent glow behind a brighter highlight
    /// line. The stage card shape is unchanged; only the material changes.
    fn render_glass_border<R: ClarionRenderer>(
        &self,
        card: &StageCard<'_, W>,
        width: f64,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let geometry = card.geometry;
        let active = Color::from_array_premul(self.active_color);
        let tinted = |alpha: f32| Color::new_unpremul(active.r, active.g, active.b, alpha);
        let white = |alpha: f32| Color::new_unpremul(1., 1., 1., alpha);
        let angle = (135f32 - 90.).to_radians();
        let gradient = GradientInterpolation::default();

        let mut cache = self.resources.stage_glass_borders.borrow_mut();
        cache.resize_with(self.workspace_count, || {
            [BorderRenderElement::empty(), BorderRenderElement::empty()]
        });
        let [glow, line] = &mut cache[card.workspace_index];

        let glow_width = 5.;
        let glow_loc = geometry.loc - Point::from((glow_width, glow_width));
        let glow_size = geometry.size + Size::from((glow_width * 2., glow_width * 2.));
        glow.update(
            glow_size,
            Rectangle::from_size(glow_size),
            gradient,
            white(0.10),
            tinted(0.05),
            angle,
            Rectangle::from_size(glow_size),
            glow_width as f32,
            CornerRadius::default(),
            self.scale as f32,
            self.alpha,
        );
        line.update(
            geometry.size,
            Rectangle::from_size(geometry.size),
            gradient,
            white(0.62),
            tinted(0.34),
            angle,
            Rectangle::from_size(geometry.size),
            width as f32,
            CornerRadius::default(),
            self.scale as f32,
            self.alpha,
        );

        push(untransformed(MonitorInnerRenderElement::Border(
            glow.clone().with_location(glow_loc),
        )));
        push(untransformed(MonitorInnerRenderElement::Border(
            line.clone().with_location(geometry.loc),
        )));
    }

    fn render_solid_border<R: ClarionRenderer>(
        &self,
        card: &StageCard<'_, W>,
        width: f64,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let geometry = card.geometry;
        let passive_color = [0.25, 0.25, 0.30, 0.8];
        let color = std::array::from_fn(|index| {
            passive_color[index]
                + (self.active_color[index] - passive_color[index]) * card.highlight as f32
        });
        let mut cache = self.resources.stage_borders.borrow_mut();
        cache.resize_with(self.workspace_count, || {
            std::array::from_fn(|_| SolidColorBuffer::new(Size::from((0., 0.)), [0.; 4]))
        });
        let positions = [
            (geometry.loc, Size::from((geometry.size.w, width))),
            (
                geometry.loc + Point::from((0., geometry.size.h - width)),
                Size::from((geometry.size.w, width)),
            ),
            (
                geometry.loc + Point::from((0., width)),
                Size::from((width, geometry.size.h - width * 2.)),
            ),
            (
                geometry.loc + Point::from((geometry.size.w - width, width)),
                Size::from((width, geometry.size.h - width * 2.)),
            ),
        ];
        for (buffer, (location, size)) in cache[card.workspace_index].iter_mut().zip(positions) {
            buffer.update(size, color);
            push(untransformed(MonitorInnerRenderElement::SolidColor(
                SolidColorRenderElement::from_buffer(
                    buffer,
                    location,
                    self.alpha,
                    Kind::Unspecified,
                ),
            )));
        }
    }

    fn render_rail<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        if self.alpha <= 0. {
            return;
        }
        let scale = self.scale;
        let mut texts = self.resources.stage_rail_text.borrow_mut();
        let mut buffers = self.resources.stage_rail_buffers.borrow_mut();
        texts.resize_with(self.rail.len() * 2, || None);
        buffers.resize_with(self.rail.len(), || {
            SolidColorBuffer::new(Size::from((0., 0.)), [0.; 4])
        });
        let renderer_context = ctx.renderer.context_id().erased();
        for (index, item) in self.rail.iter().enumerate() {
            let alpha = self.alpha * if item.enabled { 1. } else { 0.4 };
            for (line, text) in [Some(&item.text), item.subtitle.as_ref()]
                .into_iter()
                .enumerate()
            {
                let Some(text) = text else {
                    continue;
                };
                let cache = &mut texts[index * 2 + line];
                if cache.as_ref().is_none_or(|(old, s, context, _)| {
                    old != text || *s != scale.to_bits() || context != &renderer_context
                }) {
                    *cache = None;
                    if let Ok(texture) =
                        crate::ui::mru::generate_title_texture(ctx.renderer, text, scale)
                    {
                        *cache = Some((
                            text.clone(),
                            scale.to_bits(),
                            renderer_context.clone(),
                            texture,
                        ));
                    }
                }
                let Some((_, _, _, texture)) = cache else {
                    continue;
                };
                let text_size = match texture {
                    crate::ui::mru::MruTexture::Gles(buffer) => buffer.logical_size(),
                    crate::ui::mru::MruTexture::Vulkan(buffer) => buffer.logical_size(),
                }
                .upscale(if line == 0 { 1. } else { 0.85 });
                let y = if item.subtitle.is_some() {
                    7. + line as f64 * 22.
                } else {
                    (item.geometry.size.h - text_size.h) / 2.
                };
                let x = if item.navigation {
                    (item.geometry.size.w - text_size.w) / 2.
                } else {
                    10.
                };
                let location = item.geometry.loc + Point::from((x, y));
                let text_alpha = alpha * if line == 0 { 1. } else { 0.72 };
                let element = match texture {
                    crate::ui::mru::MruTexture::Gles(buffer) => {
                        PrimaryGpuTextureRenderElement::from(
                            TextureRenderElement::from_texture_buffer(
                                buffer.clone(),
                                location,
                                text_alpha,
                                None,
                                Some(text_size),
                                Kind::Unspecified,
                            ),
                        )
                    }
                    crate::ui::mru::MruTexture::Vulkan(buffer) => {
                        PrimaryGpuTextureRenderElement::from(
                            TextureRenderElement::from_texture_buffer(
                                buffer.clone(),
                                location,
                                text_alpha,
                                None,
                                Some(text_size),
                                Kind::Unspecified,
                            ),
                        )
                    }
                };
                let crop = Rectangle::new(
                    item.geometry.loc + Point::from((4., 0.)),
                    Size::from(((item.geometry.size.w - 8.).max(0.), item.geometry.size.h)),
                )
                .intersection(item.clip)
                .unwrap_or_default()
                .to_physical_precise_round(scale);
                if let Some(element) = CropRenderElement::from_element(element, scale, crop) {
                    push(untransformed(MonitorInnerRenderElement::StageText(element)));
                }
            }
            let active_color = [0.12, 0.25, 0.40, 0.95];
            let passive_color = [0.06, 0.07, 0.09, 0.9];
            let color = std::array::from_fn(|index| {
                passive_color[index] + (active_color[index] - passive_color[index]) * item.highlight
            });
            let Some(background) = Rectangle::new(
                item.geometry.loc + Point::from((2., 0.)),
                Size::from(((item.geometry.size.w - 4.).max(0.), item.geometry.size.h)),
            )
            .intersection(item.clip) else {
                continue;
            };
            buffers[index].update(background.size, color);
            push(untransformed(MonitorInnerRenderElement::SolidColor(
                SolidColorRenderElement::from_buffer(
                    &buffers[index],
                    background.loc,
                    alpha,
                    Kind::Unspecified,
                ),
            )));
        }
    }
}

fn transform<W: LayoutElement, R: ClarionRenderer>(
    element: MonitorInnerRenderElement<R>,
    card: &StageCard<'_, W>,
    scale: f64,
) -> MonitorRenderElement<R> {
    RelocateRenderElement::from_element(
        RescaleRenderElement::from_element(element, Point::default(), card.zoom),
        card.geometry.loc.to_physical_precise_round(scale),
        Relocate::Relative,
    )
}

fn untransformed<R: ClarionRenderer>(
    element: MonitorInnerRenderElement<R>,
) -> MonitorRenderElement<R> {
    RelocateRenderElement::from_element(
        RescaleRenderElement::from_element(element, Point::default(), 1.),
        Point::default(),
        Relocate::Relative,
    )
}
