//! Window composition from renderer-free state and scene-owned resources.

use core::f64;
use std::ops::Deref;

use clarion_config::{Color, CornerRadius, GradientInterpolation};
use smithay::backend::renderer::element::{Element, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Rectangle, Scale, Size};

use super::focus_ring::FocusRingRenderElement;
use super::opening_window::{
    render_opening_window_gles, render_opening_window_native, OpeningWindowRenderElement,
};
use crate::clarion_render_elements;
use crate::layout::window_presentation::{AlphaAnimation, WindowPresentationState};
use crate::layout::{LayoutElement, LayoutElementRenderElement};
use crate::render_helpers::background_effect::BackgroundEffectElement;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::clipped_surface::ClippedSurfaceRenderElement;
use crate::render_helpers::damage::ExtraDamage;
use crate::render_helpers::mesh::MeshRenderElement;
use crate::render_helpers::offscreen::{OffscreenData, OffscreenRenderElement};
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::resize::ResizeRenderElement;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::snapshot::RenderSnapshot;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use crate::scene::window_resources::{EffectRetirements, WindowResourceLease};

clarion_render_elements! {
    TileRenderElement<R> => {
        LayoutElement = LayoutElementRenderElement<R>,
        FocusRing = FocusRingRenderElement,
        SolidColor = SolidColorRenderElement,
        Opening = OpeningWindowRenderElement,
        PrimaryGpu = crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement,
        Resize = ResizeRenderElement,
        Border = BorderRenderElement,
        Shadow = ShadowRenderElement,
        ClippedSurface = ClippedSurfaceRenderElement<R>,
        Offscreen = OffscreenRenderElement,
        ExtraDamage = ExtraDamage,
        BackgroundEffect = BackgroundEffectElement,
        Mesh = MeshRenderElement,
    }
}

pub type TileRenderSnapshot =
    RenderSnapshot<TileRenderElement<GlesRenderer>, TileRenderElement<GlesRenderer>>;

/// Window rendering has no access to Tile identity, focus operations or layouts.
/// A single lease is reused by normal, popup, effect and capture passes.
pub(crate) struct TileScene<'a, W: LayoutElement> {
    pub(crate) window: &'a W,
    pub(crate) state: &'a WindowPresentationState,
    pub(crate) resources: WindowResourceLease,
}

impl<W: LayoutElement> Deref for TileScene<'_, W> {
    type Target = WindowPresentationState;
    fn deref(&self) -> &Self::Target {
        self.state
    }
}

#[derive(Clone, Copy)]
pub(crate) enum NativeEffect {
    Wobble,
    Genie,
}

impl<W: LayoutElement> TileScene<'_, W> {
    pub(crate) fn window(&self) -> &W {
        self.window
    }
    pub(crate) fn scale(&self) -> f64 {
        self.scale
    }
    fn resolved_corner_radius(&self) -> CornerRadius {
        self.state.resolved_corner_radius(self.window)
    }
    fn resolved_clip_to_geometry(&self) -> bool {
        self.state.resolved_clip_to_geometry(self.window)
    }
    fn rounding_power(&self) -> f32 {
        self.state.rounding_power()
    }
    fn fullscreen_progress(&self) -> f64 {
        self.state.fullscreen_progress()
    }
    fn expanded_progress(&self) -> f64 {
        self.state.expanded_progress()
    }
    fn visual_border_width(&self) -> Option<f64> {
        self.state.visual_border_width()
    }
    fn window_loc(&self) -> Point<f64, Logical> {
        self.state.window_loc(self.window)
    }
    fn window_size(&self) -> Size<f64, Logical> {
        self.state.window_size(self.window)
    }
    fn animated_window_size(&self) -> Size<f64, Logical> {
        self.state.animated_window_size(self.window)
    }
    pub(crate) fn animated_tile_size(&self) -> Size<f64, Logical> {
        self.state.animated_tile_size(self.window)
    }
    fn bob_offset(&self) -> Point<f64, Logical> {
        self.state.bob_offset(self.window)
    }

    fn render_inner<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        location: Point<f64, Logical>,
        mut xray_pos: XrayPos,
        focus_ring: bool,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) {
        let _span = clarion_tracy_span!("Tile::render_inner");

        let decorations = self.resources.decorations.borrow();
        let scale = Scale::from(self.scale);
        let fullscreen_progress = self.fullscreen_progress();
        let expanded_progress = self.expanded_progress();

        let win_alpha = if self.window.is_ignoring_opacity_window_rule() {
            1.
        } else {
            let alpha = self.window.rules().opacity.unwrap_or(1.).clamp(0., 1.);

            // Interpolate towards alpha = 1. at fullscreen.
            let p = fullscreen_progress as f32;
            alpha * (1. - p) + 1. * p
        };

        // This is here rather than in render_offset() because render_offset() is currently assumed
        // by the code to be temporary. So, for example, interactive move will try to "grab" the
        // tile at its current render offset and reset the render offset to zero by cancelling the
        // tile move animations. On the other hand, bob_offset() is not resettable, so adding it in
        // render_offset() would cause obvious animation glitches.
        //
        // This isn't to say that adding it here is perfect; indeed, it kind of breaks view_rect
        // passed to update_render_elements(). But, it works well enough for what it is.
        let bob_offset = self.bob_offset();
        let location = location + bob_offset;
        xray_pos = xray_pos.offset(bob_offset);

        let window_loc = self.window_loc();
        let window_size = self.window_size();
        let animated_window_size = self.animated_window_size();
        let window_render_loc = location + window_loc;
        let area = Rectangle::new(window_render_loc, animated_window_size);
        xray_pos = xray_pos.offset(window_loc);

        let rules = self.window.rules();

        // Clip to geometry including during the fullscreen animation to help with buggy clients
        // that submit a full-sized buffer before acking the fullscreen state (Firefox).
        let clip_to_geometry = fullscreen_progress < 1. && self.resolved_clip_to_geometry();
        let radius = self
            .resolved_corner_radius()
            .scaled_by(1. - expanded_progress as f32);

        // Popups go on top, whether it's resize or not.
        self.window.render_popups(
            ctx.r(),
            window_render_loc,
            scale,
            win_alpha,
            xray_pos,
            &mut |elem| push(elem.into()),
        );

        let mut pushed_resize = false;
        let resources = &self.resources;
        let resize_images = resources.resize.borrow();
        if let (Some(resize), Some(images)) = (&self.resize_animation, resize_images.as_ref()) {
            if ResizeRenderElement::has_shader(ctx.renderer) {
                if let Some(mut ctx) = ctx.as_gles() {
                    if let Some((texture_from, snapshot_size)) =
                        images
                            .snapshot
                            .gles_texture(ctx.renderer, ctx.target, scale)
                    {
                        let mut window_elements = Vec::new();
                        self.window.render_normal(
                            ctx.r(),
                            Point::from((0., 0.)),
                            scale,
                            1.,
                            &mut |elem| window_elements.push(elem),
                        );

                        let current = images
                            .buffers
                            .gles
                            .render(ctx.renderer, scale, &window_elements)
                            .map_err(|err| warn!("error rendering window to texture: {err:?}"))
                            .ok();

                        // Clip blocked-out resizes unconditionally because they use solid color
                        // render elements.
                        let clip_to_geometry = if images.snapshot.blocked_out(ctx.target)
                            && ctx.target.should_block_out(rules.block_out_from)
                        {
                            true
                        } else {
                            clip_to_geometry
                        };

                        if let Some((elem_current, _sync_point, mut data)) = current {
                            let texture_current = elem_current.texture().clone();
                            // The offset and size are computed in physical pixels and converted to
                            // logical with the same `scale`, so converting them back with rounding
                            // inside the geometry() call gives us the same physical result back.
                            let texture_current_geo = elem_current.geometry(scale);

                            let elem = ResizeRenderElement::new(
                                area,
                                scale,
                                texture_from.clone(),
                                snapshot_size,
                                (texture_current, texture_current_geo),
                                window_size,
                                resize.anim.value() as f32,
                                resize.anim.clamped_value().clamp(0., 1.) as f32,
                                radius,
                                clip_to_geometry,
                                win_alpha,
                            );

                            // We're drawing the resize shader, not the offscreen directly.
                            data.id = elem.id().clone();

                            // This is not a problem for split popups as the code will look for them
                            // by original id when it doesn't find them
                            // on the offscreen.
                            self.window.set_offscreen_data(Some(data));
                            push(elem.into());
                            pushed_resize = true;
                        }
                    }
                }
            }
            if let Some(renderer) = ctx.renderer.as_native_renderer() {
                let result = (|| -> anyhow::Result<_> {
                    let mut native_ctx = RenderCtx {
                        renderer,
                        target: ctx.target,
                        xray: ctx.xray,
                    };
                    let (texture_from, snapshot_size) = images
                        .snapshot
                        .native_texture(native_ctx.renderer, ctx.target, scale)
                        .ok_or_else(|| anyhow::anyhow!("missing native resize snapshot"))?;
                    let mut elements = Vec::new();
                    self.window.render_normal(
                        native_ctx.r(),
                        Point::from((0., 0.)),
                        scale,
                        1.,
                        &mut |elem| elements.push(elem),
                    );
                    // The persistent target keeps its allocation across the whole
                    // resize; the damage tracker redraws only what the client
                    // actually changed, and the sampling barriers order each frame
                    // against the previous one.
                    let (texture, _sync, geo, states, _commit) =
                        images
                            .buffers
                            .native
                            .render(native_ctx.renderer, scale, &elements)?;
                    let clip = clip_to_geometry
                        || (images.snapshot.blocked_out(ctx.target)
                            && ctx.target.should_block_out(rules.block_out_from));
                    let elem = ResizeRenderElement::new_native(
                        native_ctx.renderer,
                        self.options
                            .animations
                            .window_resize
                            .custom_shader
                            .as_deref(),
                        area,
                        scale,
                        texture_from,
                        snapshot_size,
                        (texture, geo),
                        window_size,
                        resize.anim.value() as f32,
                        resize.anim.clamped_value().clamp(0., 1.) as f32,
                        radius,
                        clip,
                        win_alpha,
                    )?;
                    Ok((elem, states))
                })();
                match result {
                    Ok((elem, states)) => {
                        self.window.set_offscreen_data(Some(OffscreenData {
                            id: elem.id().clone(),
                            states,
                        }));
                        push(elem.into());
                        pushed_resize = true;
                    }
                    Err(err)
                        if crate::render_helpers::vulkan::native::NativeError::is_backpressure(
                            err.as_ref(),
                        ) =>
                    {
                        // The frame coordinator retries this scene after preparation.
                        // Drawing the final client here would skip the resize pose.
                        return;
                    }
                    Err(err) => warn!("error rendering native resize: {err:#}"),
                }
            }
        }

        // If we're not resizing, render the window itself.
        let has_border_shader = BorderRenderElement::has_shader(ctx.renderer);
        if !pushed_resize {
            let geo = Rectangle::new(window_render_loc, window_size);
            let radius = radius.fit_to(window_size.w as f32, window_size.h as f32);

            let clip_shader = ClippedSurfaceRenderElement::clip_program(ctx.renderer);
            let rounding_power = self.rounding_power();
            let clip = |elem| match elem {
                LayoutElementRenderElement::Wayland(elem) => {
                    // If we should clip to geometry, render a clipped window.
                    if clip_to_geometry {
                        if let Some(shader) = clip_shader.clone() {
                            if ClippedSurfaceRenderElement::will_clip(&elem, scale, geo, radius) {
                                return ClippedSurfaceRenderElement::new(
                                    elem,
                                    scale,
                                    geo,
                                    shader.clone(),
                                    radius,
                                )
                                .with_rounding_power(rounding_power)
                                .into();
                            }
                        }
                    }

                    // Otherwise, render it normally.
                    LayoutElementRenderElement::Wayland(elem).into()
                }
                LayoutElementRenderElement::SolidColor(elem) => {
                    // In this branch we're rendering a blocked-out window with a solid
                    // color. We need to render it with a rounded corner shader even if
                    // clip_to_geometry is false, because in this case we're assuming that
                    // the unclipped window CSD already has corners rounded to the
                    // user-provided radius, so our blocked-out rendering should match that
                    // radius.
                    if radius != CornerRadius::default() && has_border_shader {
                        return BorderRenderElement::new(
                            geo.size,
                            Rectangle::from_size(geo.size),
                            GradientInterpolation::default(),
                            Color::from_color32f(elem.color()),
                            Color::from_color32f(elem.color()),
                            0.,
                            Rectangle::from_size(geo.size),
                            0.,
                            radius,
                            scale.x as f32,
                            1.,
                        )
                        .with_rounding_power(rounding_power)
                        .with_location(geo.loc)
                        .into();
                    }

                    // Otherwise, render the solid color as is.
                    LayoutElementRenderElement::SolidColor(elem).into()
                }
                elem @ LayoutElementRenderElement::BackgroundEffect(_) => {
                    // This is only used on popups for now. If subsurface blur is implemented, this
                    // will need to be handled somehow.
                    error!("background effect clipping is unimplemented");
                    elem.into()
                }
            };

            if clip_to_geometry && clip_shader.is_some() {
                let damage = decorations.render_rounded_corner_damage(geo);
                push(damage.into());
            }

            self.window
                .render_normal(ctx.r(), window_render_loc, scale, win_alpha, &mut |elem| {
                    push(clip(elem))
                });

            // Dim inactive windows if enabled.
            let dim_enabled = self.options.layout.dim_inactive;
            let rule_dim = self.window.rules().dim_inactive;
            let should_dim =
                rule_dim.unwrap_or(dim_enabled) && !focus_ring && fullscreen_progress < 1.0;
            if should_dim {
                let factor = self
                    .window
                    .rules()
                    .dim_factor
                    .unwrap_or(self.options.layout.dim_factor)
                    .clamp(0.0, 1.0);
                let dim_alpha = (1.0 - factor) * (1.0 - fullscreen_progress as f32);
                if dim_alpha > 0.001 {
                    if has_border_shader {
                        let dim_elem = BorderRenderElement::new(
                            geo.size,
                            Rectangle::from_size(geo.size),
                            GradientInterpolation::default(),
                            Color::from_rgba8_unpremul(0, 0, 0, 255),
                            Color::from_rgba8_unpremul(0, 0, 0, 255),
                            0.,
                            Rectangle::from_size(geo.size),
                            0.,
                            radius,
                            scale.x as f32,
                            dim_alpha,
                        )
                        .with_rounding_power(self.rounding_power())
                        .with_location(geo.loc);
                        push(dim_elem.into());
                    } else {
                        let black_buffer = SolidColorBuffer::new(geo.size, [0., 0., 0., 1.]);
                        let dim_elem = SolidColorRenderElement::from_buffer(
                            &black_buffer,
                            geo.loc,
                            dim_alpha,
                            Kind::Unspecified,
                        );
                        push(dim_elem.into());
                    }
                }
            }
        }

        if fullscreen_progress > 0. {
            let alpha = fullscreen_progress as f32;

            // During the un/fullscreen animation, render a border element in order to use the
            // animated corner radius.
            if fullscreen_progress < 1. && has_border_shader {
                let border_width = self.visual_border_width().unwrap_or(0.);
                let radius = self
                    .resolved_corner_radius()
                    .expanded_by(border_width as f32)
                    .scaled_by(1. - expanded_progress as f32);

                let size = decorations.fullscreen_backdrop.size();
                let color = decorations.fullscreen_backdrop.color();
                let elem = BorderRenderElement::new(
                    size,
                    Rectangle::from_size(size),
                    GradientInterpolation::default(),
                    Color::from_color32f(color),
                    Color::from_color32f(color),
                    0.,
                    Rectangle::from_size(size),
                    0.,
                    radius,
                    scale.x as f32,
                    alpha,
                )
                .with_rounding_power(self.rounding_power())
                .with_location(location);
                push(elem.into());
            } else {
                let elem = SolidColorRenderElement::from_buffer(
                    &decorations.fullscreen_backdrop,
                    location,
                    alpha,
                    Kind::Unspecified,
                );
                push(elem.into());
            }
        }

        decorations.render_border(ctx.renderer, location, self.visual_border_width(), push);

        // Hide the focus ring when maximized/fullscreened. It's not normally visible anyway due to
        // being outside the monitor or obscured by a solid colored bar, but it is visible under
        // semitransparent bars in maximized state (which is a bit weird) and in the overview (also
        // a bit weird).
        decorations.render_focus_ring(
            ctx.renderer,
            location,
            focus_ring && expanded_progress < 1.,
            push,
        );

        decorations.render_shadow(ctx.renderer, location, expanded_progress < 1., push);

        let surface_anim_scale = animated_window_size / window_size;
        self.window.render_background_effect(
            ctx.r(),
            area,
            self.scale,
            clip_to_geometry,
            surface_anim_scale,
            radius,
            xray_pos,
            &mut |elem| push(elem.into()),
        );
    }

    pub fn render<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        location: Point<f64, Logical>,
        xray_pos: XrayPos,
        focus_ring: bool,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) {
        if self.render_genie(ctx.r(), xray_pos, push) {
            return;
        }
        let wobble_active = !self.resources.effect_retirements.get().wobble
            && self.wobble.borrow().is_active()
            && !self.options.animations.off
            && !self.options.animations.window_wobble.off;

        if wobble_active {
            if let Some(mut ctx) = ctx.as_gles() {
                if MeshRenderElement::is_available(ctx.renderer) {
                    let scale = Scale::from(self.scale);
                    let mut elements = Vec::new();
                    self.render_without_wobble(
                        ctx.r(),
                        Point::from((0., 0.)),
                        xray_pos,
                        focus_ring,
                        &mut |element| elements.push(element),
                    );

                    match self
                        .resources
                        .wobble
                        .gles
                        .render(ctx.renderer, scale, &elements)
                    {
                        Ok((element, _sync, mut data)) => {
                            let offset = element.offset();
                            let source_area =
                                Rectangle::new(location + offset, element.logical_size());
                            let source_commit = element.current_commit();
                            let source_texture = u64::from(element.texture().tex_id());
                            let mesh = self.resources.wobble_forecast.borrow_mut().sample(
                                &self.wobble.borrow(),
                                source_area,
                                self.options.animations.window_wobble,
                                self.clock.now(),
                                source_commit,
                                source_texture,
                            );
                            if let Some(mesh) = mesh {
                                let element = MeshRenderElement::new(
                                    mesh.id,
                                    mesh.commit,
                                    element.texture().clone(),
                                    element.src().size,
                                    mesh.area,
                                    mesh.vertices,
                                    1.,
                                    Kind::Unspecified,
                                );
                                data.id = element.id().clone();
                                self.window().set_offscreen_data(Some(data));
                                push(element.into());
                                return;
                            }
                            self.clear_wobble_pixels();
                        }
                        Err(err) => {
                            warn!("error rendering elastic window to texture: {err:?}");
                            self.request_wobble_retirement();
                        }
                    }
                } else {
                    self.request_wobble_retirement();
                }
            } else if let Some(mut ctx) = ctx.as_native() {
                let scale = Scale::from(self.scale);
                let mut elements = Vec::new();
                self.render_without_wobble(
                    ctx.r(),
                    Point::from((0., 0.)),
                    xray_pos,
                    focus_ring,
                    &mut |element| elements.push(element),
                );
                match self
                    .resources
                    .wobble
                    .native
                    .render(ctx.renderer, scale, &elements)
                {
                    Ok((texture, _sync, geo, states, source_commit)) => {
                        // Native submissions share a queue; sampling barriers order this
                        // snapshot without stalling the compositor on a CPU fence wait.
                        // The persistent tracker redraws only what changed since the
                        // previous wobble frame.
                        let offset = geo.loc.to_f64().to_logical(scale);
                        let source_area =
                            Rectangle::new(location + offset, geo.size.to_f64().to_logical(scale));
                        // A pooled image keeps its identity when pixels change; the
                        // damage-bag commit marks real content updates instead.
                        let source_texture = texture.cache_key();
                        let mesh = self.resources.wobble_forecast.borrow_mut().sample(
                            &self.wobble.borrow(),
                            source_area,
                            self.options.animations.window_wobble,
                            self.clock.now(),
                            source_commit,
                            source_texture,
                        );
                        if let Some(mesh) = mesh {
                            let used = smithay::utils::Size::from((
                                f64::from(geo.size.w),
                                f64::from(geo.size.h),
                            ));
                            let element = MeshRenderElement::new_vulkan(
                                mesh.id,
                                mesh.commit,
                                texture,
                                used,
                                mesh.area,
                                mesh.vertices,
                                1.,
                                Kind::Unspecified,
                            );
                            let data = crate::render_helpers::offscreen::OffscreenData {
                                id: element.id().clone(),
                                states,
                            };
                            self.window().set_offscreen_data(Some(data));
                            push(element.into());
                            return;
                        }
                        self.clear_wobble_pixels();
                    }
                    Err(err) => {
                        if self.handle_native_effect_error(NativeEffect::Wobble, err.as_ref()) {
                            return;
                        }
                        warn!("error rendering Vulkan elastic window to texture: {err:?}");
                    }
                }
            } else {
                self.request_wobble_retirement();
            }
        }

        self.render_without_wobble(ctx, location, xray_pos, focus_ring, push);
    }

    pub fn render_genie<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        xray_pos: XrayPos,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) -> bool {
        if self.resources.effect_retirements.get().genie {
            return false;
        }
        let Some(pose) = self.genie.borrow().as_ref().map(|genie| genie.pose()) else {
            return false;
        };
        let idx = ctx.target as usize;
        let scale = Scale::from(self.scale);
        if let Some(mut ctx) = ctx.as_gles() {
            if !MeshRenderElement::is_available(ctx.renderer) {
                self.request_genie_retirement();
                return false;
            }
            let mut elements = Vec::new();
            self.render_without_wobble(ctx.r(), (0., 0.).into(), xray_pos, false, &mut |element| {
                elements.push(element)
            });
            match self.resources.genie[idx]
                .gles
                .render(ctx.renderer, scale, &elements)
            {
                Ok((element, _sync, mut data)) => {
                    let texture_area = Rectangle::new(element.offset(), element.logical_size());
                    let mesh = self.resources.genie_mesh.borrow_mut().sample(
                        &pose,
                        texture_area,
                        self.animated_tile_size(),
                        element.current_commit(),
                        u64::from(element.texture().tex_id()),
                    );
                    let element = MeshRenderElement::new(
                        mesh.id,
                        mesh.commit,
                        element.texture().clone(),
                        element.src().size,
                        mesh.area,
                        mesh.vertices,
                        mesh.alpha,
                        Kind::Unspecified,
                    );
                    data.id = element.id().clone();
                    self.window.set_offscreen_data(Some(data));
                    push(element.into());
                    return true;
                }
                Err(err) => warn!("error rendering Genie window to texture: {err:?}"),
            }
        } else if let Some(mut ctx) = ctx.as_native() {
            let mut elements = Vec::new();
            self.render_without_wobble(ctx.r(), (0., 0.).into(), xray_pos, false, &mut |element| {
                elements.push(element)
            });
            match self.resources.genie[idx]
                .native
                .render(ctx.renderer, scale, &elements)
            {
                Ok((texture, _sync, geo, states, source_commit)) => {
                    let texture_area = geo.to_f64().to_logical(scale);
                    let mesh = self.resources.genie_mesh.borrow_mut().sample(
                        &pose,
                        texture_area,
                        self.animated_tile_size(),
                        source_commit,
                        texture.cache_key(),
                    );
                    let used: smithay::utils::Size<f64, smithay::utils::Buffer> =
                        (f64::from(geo.size.w), f64::from(geo.size.h)).into();
                    let element = MeshRenderElement::new_vulkan(
                        mesh.id,
                        mesh.commit,
                        texture,
                        used,
                        mesh.area,
                        mesh.vertices,
                        mesh.alpha,
                        Kind::Unspecified,
                    );
                    self.window.set_offscreen_data(Some(OffscreenData {
                        id: element.id().clone(),
                        states,
                    }));
                    push(element.into());
                    return true;
                }
                Err(err) => {
                    if self.handle_native_effect_error(NativeEffect::Genie, err.as_ref()) {
                        // Keep the parked window on the exclusive effect route;
                        // its ordinary contents must not escape during a retry.
                        return true;
                    }
                    warn!("error rendering Vulkan Genie window to texture: {err:?}");
                }
            }
        }
        self.request_genie_retirement();
        false
    }

    pub(crate) fn clear_wobble_pixels(&self) {
        self.resources.wobble_forecast.borrow_mut().clear();
        self.resources.wobble.gles.clear();
        self.resources.wobble.native.clear();
        self.window().set_offscreen_data(None);
    }

    fn request_wobble_retirement(&self) {
        self.resources.request_effect_retirement(EffectRetirements {
            wobble: true,
            ..EffectRetirements::default()
        });
        self.clear_wobble_pixels();
    }

    fn request_genie_retirement(&self) {
        self.resources.request_effect_retirement(EffectRetirements {
            genie: true,
            ..EffectRetirements::default()
        });
        self.clear_genie_pixels();
    }

    pub(crate) fn handle_native_effect_error(
        &self,
        effect: NativeEffect,
        error: &(dyn std::error::Error + 'static),
    ) -> bool {
        if crate::render_helpers::vulkan::native::NativeError::is_backpressure(error) {
            return true;
        }
        match effect {
            NativeEffect::Wobble => self.request_wobble_retirement(),
            NativeEffect::Genie => self.request_genie_retirement(),
        }
        false
    }

    fn render_without_wobble<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        location: Point<f64, Logical>,
        xray_pos: XrayPos,
        focus_ring: bool,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) {
        let _span = clarion_tracy_span!("Tile::render");

        let scale = Scale::from(self.scale);

        let tile_alpha = self
            .alpha_animation
            .as_ref()
            .map_or(1., AlphaAnimation::value) as f32;

        let mut pushed = false;
        self.window().set_offscreen_data(None);

        if let Some(open) = &self.open_animation {
            let pose = open.pose();
            if let Some(mut ctx) = ctx.as_gles() {
                let mut elements = Vec::new();
                self.render_inner(
                    ctx.r(),
                    Point::new(0., 0.),
                    xray_pos,
                    focus_ring,
                    &mut |elem| elements.push(elem),
                );
                match render_opening_window_gles(
                    &pose,
                    ctx.renderer,
                    &self.resources.open,
                    &elements,
                    self.animated_tile_size(),
                    location,
                    scale,
                    tile_alpha,
                ) {
                    Ok((elem, data)) => {
                        self.window().set_offscreen_data(Some(data));
                        push(elem.into());
                        pushed = true;
                    }
                    Err(err) => {
                        warn!("error rendering window opening animation: {err:?}");
                    }
                }
            } else if let Some(mut ctx) = ctx.as_native() {
                let mut elements = Vec::new();
                self.render_inner(
                    ctx.r(),
                    Point::new(0., 0.),
                    xray_pos,
                    focus_ring,
                    &mut |elem| elements.push(elem),
                );
                match render_opening_window_native(
                    &pose,
                    ctx.renderer,
                    &self.resources.open,
                    self.options.animations.window_open.custom_shader.as_deref(),
                    &elements,
                    self.animated_tile_size(),
                    location,
                    scale,
                    tile_alpha,
                ) {
                    Ok((elem, data)) => {
                        self.window().set_offscreen_data(Some(data));
                        push(elem.into());
                        pushed = true;
                    }
                    Err(err) => {
                        if crate::render_helpers::vulkan::native::NativeError::is_backpressure(
                            err.as_ref(),
                        ) {
                            return;
                        }
                        warn!("error rendering Vulkan window opening animation: {err:?}");
                    }
                }
            } else {
                let current_scale = pose.scale;
                let tile_size = self.animated_tile_size();
                let center: Point<f64, Logical> = Point::new(
                    location.x + tile_size.w * 0.5,
                    location.y + tile_size.h * 0.5,
                );
                let scaled_loc = Point::new(
                    center.x - tile_size.w * 0.5 * current_scale,
                    center.y - tile_size.h * 0.5 * current_scale,
                );
                self.render_inner(ctx.r(), scaled_loc, xray_pos, focus_ring, &mut |elem| {
                    push(elem)
                });
                pushed = true;
            }
        } else if self.alpha_animation.is_some() {
            if let Some(mut ctx) = ctx.as_gles() {
                let mut elements = Vec::new();
                self.render_inner(
                    ctx.r(),
                    Point::new(0., 0.),
                    xray_pos,
                    focus_ring,
                    &mut |elem| elements.push(elem),
                );
                match self
                    .resources
                    .alpha
                    .gles
                    .render(ctx.renderer, scale, &elements)
                {
                    Ok((elem, _sync, data)) => {
                        let offset = elem.offset();
                        let elem = elem.with_alpha(tile_alpha).with_offset(location + offset);

                        self.window().set_offscreen_data(Some(data));
                        push(elem.into());
                        pushed = true;
                    }
                    Err(err) => {
                        warn!("error rendering tile to offscreen for alpha animation: {err:?}");
                    }
                }
            } else if let Some(mut ctx) = ctx.as_native() {
                let mut elements = Vec::new();
                self.render_inner(
                    ctx.r(),
                    Point::new(0., 0.),
                    xray_pos,
                    focus_ring,
                    &mut |elem| elements.push(elem),
                );
                match self
                    .resources
                    .alpha
                    .native
                    .render(ctx.renderer, scale, &elements)
                {
                    Ok((texture, _sync, geo, states, _commit)) => {
                        let offset = geo.loc.to_f64().to_logical(scale);
                        let used = Size::from((
                            f64::from(geo.size.w) / scale.x,
                            f64::from(geo.size.h) / scale.y,
                        ));
                        let buf = crate::render_helpers::texture::TextureBuffer::from_texture(
                            ctx.renderer,
                            texture,
                            scale,
                            smithay::utils::Transform::Normal,
                            Vec::new(),
                        );
                        // The pooled allocation may be larger than this frame's
                        // encompassing geometry; only the [0, geo.size) prefix is
                        // this window.
                        let elem =
                            crate::render_helpers::texture::TextureRenderElement::from_texture_buffer(
                                buf,
                                location + offset,
                                tile_alpha,
                                Some(Rectangle::new(Point::from((0., 0.)), used)),
                                Some(used),
                                Kind::Unspecified,
                            );
                        self.window().set_offscreen_data(Some(OffscreenData {
                            id: elem.id().clone(),
                            states,
                        }));
                        push(
                            crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement::from(
                                elem,
                            )
                            .into(),
                        );
                        pushed = true;
                    }
                    Err(err) => {
                        if crate::render_helpers::vulkan::native::NativeError::is_backpressure(
                            err.as_ref(),
                        ) {
                            return;
                        }
                        warn!(
                            "error rendering Vulkan tile to offscreen for alpha animation: {err:?}"
                        );
                    }
                }
            } else {
                self.render_inner(ctx.r(), location, xray_pos, focus_ring, &mut |elem| {
                    push(elem)
                });
                pushed = true;
            }
        }

        if !pushed {
            self.render_inner(ctx, location, xray_pos, focus_ring, &mut |elem| push(elem));
        }
    }

    pub(crate) fn clear_genie_pixels(&self) {
        self.resources.genie_mesh.borrow_mut().clear();
        for buffers in &self.resources.genie {
            buffers.clear();
        }
        self.window.set_offscreen_data(None);
    }

    #[cfg(test)]
    pub(crate) fn render_decoration_rings(
        &self,
        renderer: &mut impl crate::render_helpers::renderer::DecorationRenderer,
        push: &mut dyn FnMut(FocusRingRenderElement),
    ) {
        self.resources
            .decorations
            .borrow()
            .render_decoration_rings(renderer, push);
    }
}
