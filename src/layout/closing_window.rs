//! Denial-derived close phases and particles: Copyright (C) 2026 Doctor Logix.
//! GPL-3.0-or-later; see resources/licenses/denial-GPL-3.0-or-later.txt.

use std::cell::Cell;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::rc::Rc;

use anyhow::Context as _;
use glam::{Mat3, Vec2};
use realand_config::{BlockOutFrom, WindowCloseEffect};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::utils::{
    Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::{Id, Kind, RenderElement};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture, Uniform};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::Texture;
use smithay::utils::{Logical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::compositor::{Blocker, BlockerState};

use crate::animation::Animation;
use crate::realand_render_elements;
use crate::render_helpers::particles::{ParticleRenderElement, ParticleVertex};
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::RealandRenderer;
use crate::render_helpers::shader_element::ShaderRenderElement;
use crate::render_helpers::shaders::{mat3_uniform, ProgramType, Shaders};
use crate::render_helpers::snapshot::RenderSnapshot;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::vulkan::native::NativeRenderer;
use crate::render_helpers::vulkan::VulkanTexture;
use crate::render_helpers::{render_to_encompassing_texture, RenderCtx, RenderTarget};
use crate::utils::transaction::TransactionBlocker;

#[derive(Debug, Clone)]
pub enum ClosingBuffer {
    Gles(TextureBuffer<GlesTexture>),
    Vulkan(TextureBuffer<VulkanTexture>),
}

impl ClosingBuffer {
    pub fn texture_scale(&self) -> Scale<f64> {
        match self {
            Self::Gles(b) => b.texture_scale(),
            Self::Vulkan(b) => b.texture_scale(),
        }
    }

    pub fn texture_physical_size(&self) -> Size<i32, smithay::utils::Buffer> {
        match self {
            Self::Gles(b) => b.texture().size(),
            Self::Vulkan(b) => b.texture().size(),
        }
    }

    pub fn to_render_element(
        &self,
        offset: Point<f64, Logical>,
        alpha: f32,
    ) -> PrimaryGpuTextureRenderElement {
        match self {
            Self::Gles(b) => {
                let elem = TextureRenderElement::from_texture_buffer(
                    b.clone(),
                    offset,
                    alpha,
                    None,
                    None,
                    Kind::Unspecified,
                );
                PrimaryGpuTextureRenderElement::from(elem)
            }
            Self::Vulkan(b) => {
                let elem = TextureRenderElement::from_texture_buffer(
                    b.clone(),
                    offset,
                    alpha,
                    None,
                    None,
                    Kind::Unspecified,
                );
                PrimaryGpuTextureRenderElement::from(elem)
            }
        }
    }
}

#[derive(Debug)]
pub struct ClosingWindow {
    /// Contents of the window.
    buffer: ClosingBuffer,
    native_capture_buffers: Option<[(ClosingBuffer, Point<f64, Logical>); 2]>,
    native_shader_source: Option<String>,

    /// Contents that are not blocked out, but the background is blocked out.
    ///
    /// If `None` then the background doesn't have any blocked-out surfaces, and normal `buffer`
    /// can be used instead.
    buffer_with_blocked_out_bg: Option<ClosingBuffer>,

    /// Blocked-out contents of the window.
    blocked_out_buffer: ClosingBuffer,

    /// Where the window should be blocked out from.
    block_out_from: Option<BlockOutFrom>,

    /// Size of the window geometry.
    geo_size: Size<f64, Logical>,

    /// Position in the workspace.
    pos: Point<f64, Logical>,

    /// How much the texture should be offset.
    buffer_offset: Point<f64, Logical>,

    /// How much the texture with blocked-out bg should be offset.
    buffer_with_blocked_out_bg_offset: Point<f64, Logical>,

    /// How much the blocked-out texture should be offset.
    blocked_out_buffer_offset: Point<f64, Logical>,

    /// The closing animation.
    anim_state: AnimationState,

    /// Random seed for the shader.
    random_seed: f32,

    /// Optional 3D Glide transform layered around the close shader.
    glide: realand_config::Glide,

    /// Selected built-in effect. User shaders retain their historical full-view canvas.
    effect: WindowCloseEffect,
    built_in_effect: bool,

    /// Whether Vulkan renderer backend is active.
    is_vulkan: bool,
    /// Cloned DMA-BUF of the closing window, if available.
    dmabuf: Option<smithay::backend::allocator::dmabuf::Dmabuf>,

    /// Denial's stable 11 by 8 particle grid, generated once for this closing window.
    particles: Vec<ExplosionParticle>,
    particle_id: Id,
    particle_commit: Cell<CommitCounter>,
}

realand_render_elements! {
    ClosingWindowRenderElement => {
        Texture = RelocateRenderElement<RescaleRenderElement<PrimaryGpuTextureRenderElement>>,
        Shader = ShaderRenderElement,
        Particles = ParticleRenderElement,
    }
}

#[derive(Debug)]
enum AnimationState {
    Waiting {
        /// Blocker for a transaction before starting the animation.
        blocker: TransactionBlocker,
        anim: Animation,
    },
    Animating(Animation),
}

impl AnimationState {
    pub fn new(blocker: TransactionBlocker, anim: Animation) -> Self {
        if blocker.state() == BlockerState::Pending {
            Self::Waiting { blocker, anim }
        } else {
            // This actually doesn't normally happen because the window is removed only after the
            // closing animation is created. Though, it does happen with disable-transactions debug
            // flag.
            Self::Animating(anim)
        }
    }

    fn are_animations_ongoing(&self, built_in_effect: bool) -> bool {
        match self {
            Self::Waiting { .. } => true,
            // Built-in shaders, particles and the texture fallback all use clamped progress.
            // Once it reaches 1 the snapshot is fully transparent, so release it without waiting
            // for the spring's invisible oscillations to settle. Custom shaders can use those.
            Self::Animating(anim) if built_in_effect => !anim.is_clamped_done(),
            Self::Animating(anim) => !anim.is_done(),
        }
    }
}

impl ClosingWindow {
    pub fn translate(&mut self, delta: Point<f64, Logical>) {
        self.pos += delta;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new<E: RenderElement<GlesRenderer>>(
        renderer: &mut GlesRenderer,
        snapshot: RenderSnapshot<E, E>,
        scale: Scale<f64>,
        geo_size: Size<f64, Logical>,
        pos: Point<f64, Logical>,
        blocker: TransactionBlocker,
        anim: Animation,
        glide: realand_config::Glide,
        effect: WindowCloseEffect,
        has_custom_shader: bool,
        is_vulkan: bool,
        dmabuf: Option<smithay::backend::allocator::dmabuf::Dmabuf>,
    ) -> anyhow::Result<Self> {
        let _span = realand_tracy_span!("ClosingWindow::new");

        let mut render_to_texture = |elements: Vec<E>| -> anyhow::Result<_> {
            let (texture, _sync_point, geo) = render_to_encompassing_texture(
                renderer,
                scale,
                Transform::Normal,
                Fourcc::Abgr8888,
                &elements,
            )
            .context("error rendering to texture")?;

            let buffer = TextureBuffer::from_texture(
                renderer,
                texture,
                scale,
                Transform::Normal,
                Vec::new(),
            );

            let offset = geo.loc.to_f64().to_logical(scale);

            Ok((buffer, offset))
        };

        let (buffer, buffer_offset) =
            render_to_texture(snapshot.contents).context("error rendering contents")?;
        let (buffer_with_blocked_out_bg, buffer_with_blocked_out_bg_offset) =
            if let Some(contents) = snapshot.contents_with_blocked_out_bg {
                let (buffer, offset) = render_to_texture(contents)
                    .context("error rendering contents with blocked-out bg")?;
                (Some(ClosingBuffer::Gles(buffer)), offset)
            } else {
                (None, Point::default())
            };
        let (blocked_out_buffer, blocked_out_buffer_offset) =
            render_to_texture(snapshot.blocked_out_contents)
                .context("error rendering blocked-out contents")?;

        let buffer = ClosingBuffer::Gles(buffer);
        let blocked_out_buffer = ClosingBuffer::Gles(blocked_out_buffer);

        let particle_seed = fastrand::u64(..);
        let particles = if !has_custom_shader && effect == WindowCloseEffect::Explosion {
            build_particles(particle_seed)
        } else {
            Vec::new()
        };

        Ok(Self {
            buffer,
            buffer_with_blocked_out_bg,
            blocked_out_buffer,
            block_out_from: snapshot.block_out_from,
            native_capture_buffers: None,
            native_shader_source: None,
            geo_size,
            pos,
            buffer_offset,
            buffer_with_blocked_out_bg_offset,
            blocked_out_buffer_offset,
            anim_state: AnimationState::new(blocker, anim),
            random_seed: (particle_seed as u32) as f32 / u32::MAX as f32,
            glide,
            effect,
            built_in_effect: !has_custom_shader,
            is_vulkan,
            dmabuf,
            particles,
            particle_id: Id::new(),
            particle_commit: Cell::new(CommitCounter::default()),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_native(
        renderer: &mut NativeRenderer,
        scale: Scale<f64>,
        geo_size: Size<f64, Logical>,
        pos: Point<f64, Logical>,
        blocker: TransactionBlocker,
        anim: Animation,
        glide: realand_config::Glide,
        effect: WindowCloseEffect,
        custom_shader: Option<&str>,
        snapshot: Option<super::tile::NativeTileSnapshot>,
        dmabuf: Option<smithay::backend::allocator::dmabuf::Dmabuf>,
    ) -> anyhow::Result<Self> {
        let _span = realand_tracy_span!("ClosingWindow::new_native");
        let has_custom_shader = custom_shader.is_some();

        // A failed snapshot must never expose an unfiltered DMA-BUF to captures.
        // Omit the animation on captures when there is no trusted snapshot.
        let mut native_capture_buffers = None;
        let (buffer, buffer_offset) = if let Some(snapshot) = snapshot {
            let [(texture, offset), cast, capture] = snapshot.targets;
            native_capture_buffers = Some([cast, capture].map(|(texture, offset)| {
                (
                    ClosingBuffer::Vulkan(TextureBuffer::from_texture(
                        renderer,
                        texture,
                        scale,
                        Transform::Normal,
                        Vec::new(),
                    )),
                    offset,
                )
            }));
            let buf = TextureBuffer::from_texture(
                renderer,
                texture,
                scale,
                Transform::Normal,
                Vec::new(),
            );
            (ClosingBuffer::Vulkan(buf), offset)
        } else if let Some(ref dmabuf) = dmabuf {
            use smithay::backend::renderer::ImportDma;
            let imported = renderer
                .import_dmabuf(dmabuf, None)
                .context("error importing dmabuf in native closing window")?;
            // The client may reuse its wl_buffer as soon as the close transaction releases it.
            // Keep an independent image for the entire animation, just like NativeTileSnapshot
            // does. Rendering the imported DMA-BUF into an offscreen texture also waits for the
            // external sample to finish before the client can overwrite the source.
            let size = imported.size();
            let source = TextureBuffer::from_texture(
                renderer,
                imported,
                scale,
                Transform::Normal,
                Vec::new(),
            );
            let element = TextureRenderElement::from_texture_buffer(
                source,
                Point::default(),
                1.,
                None,
                None,
                Kind::Unspecified,
            );
            let (texture, _sync) = crate::render_helpers::render_to_texture(
                renderer,
                Size::from((size.w, size.h)),
                scale,
                Transform::Normal,
                Fourcc::Abgr8888,
                std::iter::once(element),
            )
            .context("error snapshotting dmabuf in native closing window")?;
            let buf = TextureBuffer::from_texture(
                renderer,
                texture,
                scale,
                Transform::Normal,
                Vec::new(),
            );
            (ClosingBuffer::Vulkan(buf), Point::default())
        } else {
            use smithay::backend::renderer::{Bind, Color32F, Frame, Offscreen, Renderer};
            let phys_size = geo_size.to_physical_precise_round(scale);
            let mut texture = renderer
                .create_buffer(Fourcc::Abgr8888, Size::from((phys_size.w, phys_size.h)))
                .context("error creating fallback texture for native closing window")?;
            // The fallback path has no window contents (the buffer was not a
            // DMA-BUF). Render a clear so the animation never samples an
            // UNDEFINED image, which shows driver garbage (often reddish
            // noise) instead of a fading window.
            {
                let mut target = renderer
                    .bind(&mut texture)
                    .context("error binding fallback texture for native closing window")?;
                let mut frame = renderer
                    .render(&mut target, phys_size, Transform::Normal)
                    .context("error rendering fallback texture clear")?;
                frame.clear(
                    Color32F::from([0., 0., 0., 1.]),
                    &[Rectangle::from_size(phys_size)],
                )?;
                // The completion is tracked by the renderer (async export or
                // an in-frame wait); the sync point itself is not needed here.
                drop(frame.finish()?);
            }
            let buf = TextureBuffer::from_texture(
                renderer,
                texture,
                scale,
                Transform::Normal,
                Vec::new(),
            );
            (ClosingBuffer::Vulkan(buf), Point::default())
        };

        let particle_seed = fastrand::u64(..);
        let particles = if !has_custom_shader && effect == WindowCloseEffect::Explosion {
            build_particles(particle_seed)
        } else {
            Vec::new()
        };

        Ok(Self {
            blocked_out_buffer: buffer.clone(),
            buffer_with_blocked_out_bg: None,
            buffer,
            block_out_from: None,
            native_capture_buffers,
            native_shader_source: Some(crate::render_helpers::shaders::animation_source(
                ProgramType::Close,
                crate::render_helpers::shaders::resolve_close_shader(custom_shader, effect),
            )),
            geo_size,
            pos,
            buffer_offset,
            buffer_with_blocked_out_bg_offset: Point::default(),
            blocked_out_buffer_offset: Point::default(),
            anim_state: AnimationState::new(blocker, anim),
            random_seed: (particle_seed as u32) as f32 / u32::MAX as f32,
            glide,
            effect,
            built_in_effect: !has_custom_shader,
            is_vulkan: true,
            dmabuf,
            particles,
            particle_id: Id::new(),
            particle_commit: Cell::new(CommitCounter::default()),
        })
    }

    pub fn advance_animations(&mut self) {
        match &mut self.anim_state {
            AnimationState::Waiting { blocker, anim } => {
                if blocker.state() != BlockerState::Pending {
                    let anim = anim.restarted_fresh(0., 1.);
                    self.anim_state = AnimationState::Animating(anim);
                }
            }
            AnimationState::Animating(_anim) => (),
        }
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.anim_state.are_animations_ongoing(self.built_in_effect)
    }

    #[inline]
    pub fn pos(&self) -> Point<f64, Logical> {
        self.pos
    }

    pub(crate) fn stable_id(&self) -> &Id {
        &self.particle_id
    }

    #[inline]
    pub fn geo_size(&self) -> Size<f64, Logical> {
        self.geo_size
    }

    #[inline]
    pub fn effect(&self) -> WindowCloseEffect {
        self.effect
    }

    #[inline]
    pub fn is_vulkan(&self) -> bool {
        self.is_vulkan
    }

    #[inline]
    pub fn dmabuf(&self) -> Option<&smithay::backend::allocator::dmabuf::Dmabuf> {
        self.dmabuf.as_ref()
    }

    #[inline]
    pub fn progress(&self) -> f32 {
        match &self.anim_state {
            AnimationState::Waiting { .. } => 0.0,
            AnimationState::Animating(anim) => anim.value() as f32,
        }
    }

    pub fn render<R: RealandRenderer>(
        &self,
        mut ctx: RenderCtx<'_, R>,
        view_rect: Rectangle<f64, Logical>,
        scale: Scale<f64>,
    ) -> Vec<ClosingWindowRenderElement> {
        let _ = (self.is_vulkan, &self.dmabuf);

        let capture = if self.is_vulkan && ctx.target != RenderTarget::Output {
            let Some(buffers) = &self.native_capture_buffers else {
                return Vec::new();
            };
            Some(&buffers[ctx.target as usize - 1])
        } else {
            None
        };
        let (buffer, offset) = if let Some((buffer, offset)) = capture {
            (buffer, *offset)
        } else if ctx.target.should_block_out(self.block_out_from) {
            (&self.blocked_out_buffer, self.blocked_out_buffer_offset)
        } else if ctx.target != RenderTarget::Output && self.buffer_with_blocked_out_bg.is_some() {
            (
                self.buffer_with_blocked_out_bg.as_ref().unwrap(),
                self.buffer_with_blocked_out_bg_offset,
            )
        } else {
            (&self.buffer, self.buffer_offset)
        };

        let anim = match &self.anim_state {
            AnimationState::Waiting { .. } => {
                let elem = buffer.to_render_element(Point::from((0., 0.)), 1.);
                let elem = RescaleRenderElement::from_element(elem, Point::from((0, 0)), 1.);

                let location = self.pos + offset - view_rect.loc;
                let elem = RelocateRenderElement::from_element(
                    elem,
                    location.to_physical_precise_round(scale),
                    Relocate::Relative,
                );

                return vec![elem.into()];
            }
            AnimationState::Animating(anim) => anim,
        };

        let progress = anim.value();
        let clamped_progress = anim.clamped_value().clamp(0., 1.);

        // The built-in explosion body is fully transparent after 38%. Keeping only the
        // independently bounded particles avoids shading and damaging a large empty quad.
        if self.built_in_effect
            && self.effect == WindowCloseEffect::Explosion
            && clamped_progress >= 0.38
        {
            let relative = self.pos - view_rect.loc;
            let pos = relative.to_physical_precise_round(scale).to_logical(scale) + offset;
            let texture_scale = buffer.texture_scale();
            let texture_size = buffer.texture_physical_size();
            let size = Size::from((
                f64::from(texture_size.w) / texture_scale.x,
                f64::from(texture_size.h) / texture_scale.y,
            ));
            return self
                .particle_element(clamped_progress, pos, size, scale)
                .map(ClosingWindowRenderElement::from)
                .into_iter()
                .collect();
        }

        if let Some(gles) = ctx.as_gles() {
            if let ClosingBuffer::Gles(ref gles_buf) = buffer {
                if Shaders::get(gles.renderer)
                    .program(ProgramType::Close)
                    .is_some()
                {
                    // Round to physical pixels relative to the view position. This is similar to what
                    // happens when rendering normal windows.
                    let relative = self.pos - view_rect.loc;
                    let pos =
                        view_rect.loc + relative.to_physical_precise_round(scale).to_logical(scale);

                    let texture_scale = gles_buf.texture_scale();
                    let texture_physical_size = gles_buf.texture().size();
                    let texture_size = Size::from((
                        f64::from(texture_physical_size.w) / texture_scale.x,
                        f64::from(texture_physical_size.h) / texture_scale.y,
                    ));
                    let effect_rect = if self.built_in_effect {
                        // Denial transforms the complete RepaintBoundary snapshot, including window
                        // decorations and shadows.
                        Rectangle::new(pos + offset, texture_size)
                    } else {
                        Rectangle::new(pos, self.geo_size)
                    };
                    let effect_pos_in_view = effect_rect.loc - view_rect.loc;

                    let (area, element_location) = if self.built_in_effect {
                        let longest = effect_rect.size.w.max(effect_rect.size.h);
                        let effect_padding = match self.effect {
                            // Particles carry their own bounds; only allow for the 2.5% punch here.
                            WindowCloseEffect::Explosion => longest * 0.015 + 2.,
                            WindowCloseEffect::Implode => longest * 0.04 + 2.,
                            WindowCloseEffect::Fade => 2.,
                        };
                        let glide_padding = if self.glide.off {
                            0.
                        } else {
                            longest * 0.1 + 2.
                        };
                        let padding = effect_padding.max(glide_padding);
                        let padding = Point::from((padding, padding))
                            .to_physical_precise_round(scale)
                            .to_logical(scale);
                        let area = Rectangle::new(
                            effect_rect.loc - padding,
                            Size::from((
                                effect_rect.size.w + padding.x * 2.,
                                effect_rect.size.h + padding.y * 2.,
                            )),
                        );
                        let element_location = area.loc - view_rect.loc;
                        (area, element_location)
                    } else {
                        // Custom close shaders historically receive the entire view and may intentionally
                        // draw far outside the window.
                        (view_rect, Point::from((0., 0.)))
                    };

                    let area_loc = Vec2::new(area.loc.x as f32, area.loc.y as f32);
                    let area_size = Vec2::new(area.size.w as f32, area.size.h as f32);

                    let effect_loc = Vec2::new(effect_rect.loc.x as f32, effect_rect.loc.y as f32);
                    let effect_size =
                        Vec2::new(effect_rect.size.w as f32, effect_rect.size.h as f32);

                    let input_to_geo = Mat3::from_scale(area_size / effect_size)
                        * Mat3::from_translation((area_loc - effect_loc) / area_size);

                    let tex_scale = Vec2::new(texture_scale.x as f32, texture_scale.y as f32);
                    let tex_loc = Vec2::new(offset.x as f32, offset.y as f32);
                    let tex_size = Vec2::new(
                        texture_physical_size.w as f32,
                        texture_physical_size.h as f32,
                    ) / tex_scale;

                    let geo_to_tex = if self.built_in_effect {
                        Mat3::IDENTITY
                    } else {
                        Mat3::from_translation(-tex_loc / tex_size)
                            * Mat3::from_scale(effect_size / tex_size)
                    };

                    let shader = ShaderRenderElement::new(
                        ProgramType::Close,
                        area.size,
                        None,
                        scale.x as f32,
                        1.,
                        Rc::new([
                            mat3_uniform("realand_input_to_geo", input_to_geo),
                            Uniform::new("realand_geo_size", effect_size.to_array()),
                            mat3_uniform("realand_geo_to_tex", geo_to_tex),
                            Uniform::new("realand_progress", progress as f32),
                            Uniform::new("realand_clamped_progress", clamped_progress as f32),
                            Uniform::new("realand_random_seed", self.random_seed),
                            Uniform::new(
                                "realand_glide_enabled",
                                if self.glide.off { 0. } else { 1. },
                            ),
                            Uniform::new(
                                "realand_glide_tilt",
                                self.glide.tilt_deg.to_radians() as f32,
                            ),
                            Uniform::new("realand_glide_depth", self.glide.depth as f32),
                        ]),
                        HashMap::from([(String::from("realand_tex"), gles_buf.texture().clone())]),
                        Kind::Unspecified,
                    )
                    .with_location(element_location);

                    let mut elements = Vec::with_capacity(2);
                    if self.built_in_effect
                        && self.effect == WindowCloseEffect::Explosion
                        && ParticleRenderElement::is_available(gles.renderer)
                    {
                        if let Some(particles) = self.particle_element(
                            clamped_progress,
                            effect_pos_in_view,
                            effect_rect.size,
                            scale,
                        ) {
                            // Render-element vectors are front-to-back. Denial paints particles in the
                            // foreground and the flash/window body underneath them.
                            elements.push(particles.into());
                        }
                    }
                    elements.push(shader.into());
                    return elements;
                }
            }
        }

        if let (Some(renderer), ClosingBuffer::Vulkan(buf)) =
            (ctx.renderer.as_native_renderer(), buffer)
        {
            let result = (|| -> anyhow::Result<Vec<ClosingWindowRenderElement>> {
                // Round to physical pixels relative to the view position. This is similar to what
                // happens when rendering normal windows.
                let relative = self.pos - view_rect.loc;
                let pos =
                    view_rect.loc + relative.to_physical_precise_round(scale).to_logical(scale);

                let texture_scale = buf.texture_scale();
                let texture_physical_size = buf.texture().size();
                let texture_size = Size::from((
                    f64::from(texture_physical_size.w) / texture_scale.x,
                    f64::from(texture_physical_size.h) / texture_scale.y,
                ));
                let effect_rect = if self.built_in_effect {
                    // Denial transforms the complete RepaintBoundary snapshot, including window
                    // decorations and shadows.
                    Rectangle::new(pos + offset, texture_size)
                } else {
                    Rectangle::new(pos, self.geo_size)
                };
                let effect_pos_in_view = effect_rect.loc - view_rect.loc;

                let (area, element_location) = if self.built_in_effect {
                    let longest = effect_rect.size.w.max(effect_rect.size.h);
                    let effect_padding = match self.effect {
                        // Particles carry their own bounds; only allow for the 2.5% punch here.
                        WindowCloseEffect::Explosion => longest * 0.015 + 2.,
                        WindowCloseEffect::Implode => longest * 0.04 + 2.,
                        WindowCloseEffect::Fade => 2.,
                    };
                    let glide_padding = if self.glide.off {
                        0.
                    } else {
                        longest * 0.1 + 2.
                    };
                    let padding = effect_padding.max(glide_padding);
                    let padding = Point::from((padding, padding))
                        .to_physical_precise_round(scale)
                        .to_logical(scale);
                    let area = Rectangle::new(
                        effect_rect.loc - padding,
                        Size::from((
                            effect_rect.size.w + padding.x * 2.,
                            effect_rect.size.h + padding.y * 2.,
                        )),
                    );
                    let element_location = area.loc - view_rect.loc;
                    (area, element_location)
                } else {
                    // Custom close shaders historically receive the entire view and may intentionally
                    // draw far outside the window.
                    (view_rect, Point::from((0., 0.)))
                };

                let area_loc = Vec2::new(area.loc.x as f32, area.loc.y as f32);
                let area_size = Vec2::new(area.size.w as f32, area.size.h as f32);

                let effect_loc = Vec2::new(effect_rect.loc.x as f32, effect_rect.loc.y as f32);
                let effect_size = Vec2::new(effect_rect.size.w as f32, effect_rect.size.h as f32);

                let input_to_geo = Mat3::from_scale(area_size / effect_size)
                    * Mat3::from_translation((area_loc - effect_loc) / area_size);

                let tex_scale = Vec2::new(texture_scale.x as f32, texture_scale.y as f32);
                let tex_loc = Vec2::new(offset.x as f32, offset.y as f32);
                let tex_size = Vec2::new(
                    texture_physical_size.w as f32,
                    texture_physical_size.h as f32,
                ) / tex_scale;

                let geo_to_tex = if self.built_in_effect {
                    Mat3::IDENTITY
                } else {
                    Mat3::from_translation(-tex_loc / tex_size)
                        * Mat3::from_scale(effect_size / tex_size)
                };

                let fallback_source;
                let shader_src = match self.native_shader_source.as_deref() {
                    Some(src) => src,
                    None => {
                        fallback_source = crate::render_helpers::shaders::animation_source(
                            ProgramType::Close,
                            crate::render_helpers::shaders::resolve_close_shader(None, self.effect),
                        );
                        &fallback_source
                    }
                };
                let shader = ShaderRenderElement::new_native(
                    renderer,
                    ProgramType::Close,
                    shader_src,
                    area.size,
                    scale.x as f32,
                    1.,
                    Rc::new([
                        mat3_uniform("realand_input_to_geo", input_to_geo),
                        Uniform::new("realand_geo_size", effect_size.to_array()),
                        mat3_uniform("realand_geo_to_tex", geo_to_tex),
                        Uniform::new("realand_progress", progress as f32),
                        Uniform::new("realand_clamped_progress", clamped_progress as f32),
                        Uniform::new("realand_random_seed", self.random_seed),
                        Uniform::new(
                            "realand_glide_enabled",
                            if self.glide.off { 0. } else { 1. },
                        ),
                        Uniform::new(
                            "realand_glide_tilt",
                            self.glide.tilt_deg.to_radians() as f32,
                        ),
                        Uniform::new("realand_glide_depth", self.glide.depth as f32),
                    ]),
                    HashMap::from([(String::from("realand_tex"), buf.texture().clone())]),
                    Kind::Unspecified,
                )?
                .with_location(element_location);

                let mut elements = Vec::with_capacity(2);
                if self.built_in_effect && self.effect == WindowCloseEffect::Explosion {
                    if let Some(particles) = self.particle_element(
                        clamped_progress,
                        effect_pos_in_view,
                        effect_rect.size,
                        scale,
                    ) {
                        // Render-element vectors are front-to-back. Denial paints particles in the
                        // foreground and the flash/window body underneath them.
                        elements.push(particles.into());
                    }
                }
                elements.push(shader.into());
                Ok(elements)
            })();
            match result {
                Ok(elements) => return elements,
                Err(err) => warn!("error rendering native closing shader: {err:#}"),
            }
        }

        let elem = buffer.to_render_element(Point::from((0., 0.)), 1. - clamped_progress as f32);

        let center = self.geo_size.to_point().downscale(2.);
        let elem = RescaleRenderElement::from_element(
            elem,
            (center - offset).to_physical_precise_round(scale),
            ((1. - clamped_progress) / 5. + 0.8).max(0.),
        );

        let location = self.pos + offset - view_rect.loc;
        let elem = RelocateRenderElement::from_element(
            elem,
            location.to_physical_precise_round(scale),
            Relocate::Relative,
        );

        let mut elements = vec![elem.into()];

        let relative = self.pos - view_rect.loc;
        let pos = view_rect.loc + relative.to_physical_precise_round(scale).to_logical(scale);
        let texture_scale = buffer.texture_scale();
        let texture_physical_size = buffer.texture_physical_size();
        let texture_size = Size::from((
            f64::from(texture_physical_size.w) / texture_scale.x,
            f64::from(texture_physical_size.h) / texture_scale.y,
        ));
        let effect_rect = if self.built_in_effect {
            Rectangle::new(pos + offset, texture_size)
        } else {
            Rectangle::new(pos, self.geo_size)
        };
        let effect_pos_in_view = effect_rect.loc - view_rect.loc;

        if self.built_in_effect && self.effect == WindowCloseEffect::Explosion {
            if let Some(particles) = self.particle_element(
                clamped_progress,
                effect_pos_in_view,
                effect_rect.size,
                scale,
            ) {
                elements.insert(0, particles.into());
            }
        }

        elements
    }

    fn particle_element(
        &self,
        progress: f64,
        window_pos: Point<f64, Logical>,
        window_size: Size<f64, Logical>,
        scale: Scale<f64>,
    ) -> Option<ParticleRenderElement> {
        if self.particles.is_empty() {
            return None;
        }

        let glide = ParticleGlide::new(window_size, self.glide, progress);
        let center = [window_size.w * 0.5, window_size.h * 0.5];
        let travel_scale = (window_size.w.min(window_size.h) * 0.54).clamp(96., 220.);
        let mut vertices = Vec::with_capacity(self.particles.len() * 6);

        for (index, particle) in self.particles.iter().enumerate() {
            let local = interval(progress, particle.delay, 1.);
            if local <= 0. || local >= 1. {
                continue;
            }

            let travel = 1. - (1. - local).powi(3);
            let gravity = local * local * travel_scale * 0.34;
            let start = [particle.x * window_size.w, particle.y * window_size.h];
            let radial = [start[0] - center[0], start[1] - center[1]];
            let radial_distance = radial[0].hypot(radial[1]);
            let direction = if radial_distance < 0.001 {
                particle.direction
            } else {
                [radial[0] / radial_distance, radial[1] / radial_distance]
            };
            let jitter = particle.direction.map(|component| component * 0.34);
            let position = [
                start[0] + (direction[0] + jitter[0]) * travel_scale * particle.speed * travel,
                start[1]
                    + (direction[1] + jitter[1]) * travel_scale * particle.speed * travel
                    + gravity,
            ];
            let opacity = (local * PI).sin().clamp(0., 1.) * particle.opacity;
            let rgb = DENIAL_PARTICLE_PALETTE[index % DENIAL_PARTICLE_PALETTE.len()];
            let color = [
                (rgb[0] * opacity) as f32,
                (rgb[1] * opacity) as f32,
                (rgb[2] * opacity) as f32,
                opacity as f32,
            ];
            let width = particle.size * (1. - local * 0.28);
            let aspect = particle.aspect;
            let rotation = particle.rotation + particle.spin * travel;
            let (sine, cosine) = rotation.sin_cos();
            let inv_width = (1. / (width * scale.x).max(1.)) as f32;

            let corners = [
                (-0.5, -aspect * 0.5),
                (0.5, -aspect * 0.5),
                (-0.5, aspect * 0.5),
                (0.5, aspect * 0.5),
            ]
            .map(|(local_x, local_y)| {
                let x = local_x * width;
                let y = local_y * width;
                let point = glide.transform([
                    position[0] + x * cosine - y * sine,
                    position[1] + x * sine + y * cosine,
                ]);
                let point = [window_pos.x + point[0], window_pos.y + point[1]];
                ParticleVertex {
                    // Normalized below after deriving the exact animated bounds.
                    position: [point[0] as f32, point[1] as f32],
                    local: [local_x as f32, local_y as f32],
                    color,
                    shape: 0.,
                    inv_width,
                    aspect: aspect as f32,
                }
            });
            for corner in [0, 2, 3, 0, 3, 1] {
                vertices.push(corners[corner]);
            }
        }

        let first = vertices.first()?;
        let mut min = [f64::from(first.position[0]), f64::from(first.position[1])];
        let mut max = min;
        for vertex in &vertices[1..] {
            let position = vertex.position.map(f64::from);
            min[0] = min[0].min(position[0]);
            min[1] = min[1].min(position[1]);
            max[0] = max[0].max(position[0]);
            max[1] = max[1].max(position[1]);
        }
        let margin = (2. / scale.x.max(0.001)).max(0.5);
        let area = Rectangle::new(
            Point::from((min[0] - margin, min[1] - margin)),
            Size::from((
                (max[0] - min[0] + margin * 2.).max(1.),
                (max[1] - min[1] + margin * 2.).max(1.),
            )),
        );
        for vertex in &mut vertices {
            vertex.position = [
                ((f64::from(vertex.position[0]) - area.loc.x) / area.size.w) as f32,
                ((f64::from(vertex.position[1]) - area.loc.y) / area.size.h) as f32,
            ];
        }

        let mut commit = self.particle_commit.get();
        commit.increment();
        self.particle_commit.set(commit);
        Some(ParticleRenderElement::new(
            self.particle_id.clone(),
            commit,
            area,
            vertices,
            Kind::Unspecified,
        ))
    }
}

/// Perspective coefficients shared by all fragment vertices in a frame.
struct ParticleGlide {
    size: Size<f64, Logical>,
    enabled: bool,
    sine: f64,
    cosine: f64,
    depth: f64,
}

impl ParticleGlide {
    fn new(size: Size<f64, Logical>, glide: realand_config::Glide, amount: f64) -> Self {
        let enabled = !glide.off && amount > 0. && size.w > 0. && size.h > 0.;
        let (sine, cosine) = if enabled {
            (-glide.tilt_deg.to_radians() * amount).sin_cos()
        } else {
            (0., 1.)
        };
        Self {
            size,
            enabled,
            sine,
            cosine: cosine.max(0.05),
            depth: glide.depth * amount,
        }
    }

    fn transform(&self, point: [f64; 2]) -> [f64; 2] {
        if !self.enabled {
            return point;
        }
        let plane_x = point[0] / self.size.w - 0.5;
        let plane_y = point[1] / self.size.h - 0.5;
        let mut perspective = 1. + self.depth + plane_y * self.sine;
        if perspective.abs() < 0.02 {
            perspective = if perspective.is_sign_negative() {
                -0.02
            } else {
                0.02
            };
        }
        [
            (plane_x / perspective + 0.5) * self.size.w,
            (plane_y * self.cosine / perspective + 0.5) * self.size.h,
        ]
    }
}

#[derive(Debug, Clone, Copy)]
struct ExplosionParticle {
    x: f64,
    y: f64,
    direction: [f64; 2],
    speed: f64,
    size: f64,
    aspect: f64,
    delay: f64,
    rotation: f64,
    spin: f64,
    opacity: f64,
}

const DENIAL_PARTICLE_PALETTE: [[f64; 3]; 6] = [
    [244. / 255., 239. / 255., 244. / 255.],
    [208. / 255., 188. / 255., 1.],
    [1., 204. / 255., 102. / 255.],
    [1., 92. / 255., 108. / 255.],
    [142. / 255., 230. / 255., 193. / 255.],
    [52. / 255., 58. / 255., 69. / 255.],
];

fn interval(value: f64, begin: f64, end: f64) -> f64 {
    ((value - begin) / (end - begin)).clamp(0., 1.)
}

/// Dart VM's seeded `Random`: 64-bit Thomas Wang seed mix plus MWC state.
struct DartRandom {
    state: u64,
}

impl DartRandom {
    fn new(seed: u64) -> Self {
        let mut state = seed;
        state = (!state).wrapping_add(state.wrapping_shl(21));
        state ^= state >> 24;
        state = state.wrapping_mul(265);
        state ^= state >> 14;
        state = state.wrapping_mul(21);
        state ^= state >> 28;
        state = state.wrapping_add(state.wrapping_shl(31));
        if state == 0 {
            state = 0x5a17;
        }
        let mut random = Self { state };
        for _ in 0..4 {
            random.next_state();
        }
        random
    }

    fn next_state(&mut self) {
        const MULTIPLIER: u64 = 0xffff_da61;
        let low = self.state & u64::from(u32::MAX);
        let high = self.state >> 32;
        self.state = MULTIPLIER.wrapping_mul(low).wrapping_add(high);
    }

    fn next_power_of_two(&mut self, max: u32) -> u32 {
        debug_assert!(max.is_power_of_two());
        self.next_state();
        self.state as u32 & (max - 1)
    }

    fn next_double(&mut self) -> f64 {
        const POW_2_27: f64 = 134_217_728.;
        const POW_2_53: f64 = 9_007_199_254_740_992.;
        (f64::from(self.next_power_of_two(1 << 26)) * POW_2_27
            + f64::from(self.next_power_of_two(1 << 27)))
            / POW_2_53
    }
}

fn build_particles(seed: u64) -> Vec<ExplosionParticle> {
    let mut random = DartRandom::new(seed);
    (0..88)
        .map(|index| {
            let column = index % 11;
            let row = index / 11;
            ExplosionParticle {
                x: ((column as f64 + 0.25 + random.next_double() * 0.5) / 11.).clamp(0.04, 0.96),
                y: ((row as f64 + 0.20 + random.next_double() * 0.6) / 8.).clamp(0.04, 0.96),
                direction: {
                    let (sine, cosine) = (random.next_double() * PI * 2.).sin_cos();
                    [cosine, sine]
                },
                speed: 0.42 + random.next_double() * 0.90,
                size: 3. + random.next_double() * 8.,
                aspect: 0.45 + random.next_double() * 1.7,
                delay: 0.04 + random.next_double() * 0.15,
                rotation: random.next_double() * PI,
                spin: (random.next_double() - 0.5) * PI * 3.6,
                opacity: 0.72 + random.next_double() * 0.28,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{Clock, Spring, SpringParams};
    use crate::utils::transaction::Transaction;
    use std::time::Duration;

    #[test]
    fn builtin_close_releases_its_snapshot_after_the_last_visible_spring_frame() {
        for damping_ratio in [0., 0.4] {
            let mut clock = Clock::with_time(Duration::ZERO);
            let spring = Spring {
                from: 0.,
                to: 1.,
                initial_velocity: 0.,
                params: SpringParams::new(damping_ratio, 400., 0.0001),
            };
            let visible_end = spring.clamped_duration().unwrap();
            let anim = Animation::spring(clock.clone(), spring);
            assert!(visible_end < anim.duration());
            let state = AnimationState::new(TransactionBlocker::completed(), anim.clone());

            clock.set_unadjusted(visible_end - Duration::from_millis(1));
            assert!(state.are_animations_ongoing(true));
            assert!(state.are_animations_ongoing(false));

            clock.set_unadjusted(visible_end);
            assert_eq!(anim.clamped_value(), 1.);
            assert!(!state.are_animations_ongoing(true));
            assert!(
                state.are_animations_ongoing(false),
                "custom shaders retain their spring overshoot"
            );

            // Custom shaders can intentionally keep an undamped spring alive.
            if damping_ratio == 0. {
                assert_eq!(anim.duration(), Duration::MAX);
                clock.set_unadjusted(Duration::from_secs(10));
                assert!(state.are_animations_ongoing(false));
            } else {
                clock.set_unadjusted(anim.end_time());
                assert!(!state.are_animations_ongoing(false));
            }
        }
    }

    #[test]
    fn builtin_close_releases_its_snapshot_at_the_first_overshooting_easing_endpoint() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let config = realand_config::Animation {
            off: false,
            kind: realand_config::animations::Kind::Easing(
                realand_config::animations::EasingParams {
                    duration_ms: 1000,
                    curve: realand_config::animations::Curve::CubicBezier(0.05, 0.9, 0.1, 1.05),
                },
            ),
        };
        let animation = Animation::new(clock.clone(), 0., 1., 0., config);
        let state = AnimationState::new(TransactionBlocker::completed(), animation.clone());
        let first_visible_end = (1..1000)
            .find(|millis| animation.value_at(Duration::from_millis(*millis)) >= 1.)
            .unwrap();

        clock.set_unadjusted(Duration::from_millis(first_visible_end - 1));
        assert!(state.are_animations_ongoing(true));
        clock.set_unadjusted(Duration::from_millis(first_visible_end));
        assert!(!state.are_animations_ongoing(true));
        assert!(state.are_animations_ongoing(false));
        assert!(animation.value() > 1.);

        clock.set_unadjusted(Duration::from_millis(1000));
        assert!(!state.are_animations_ongoing(false));
    }

    #[test]
    fn close_reduced_motion_finishes_active_motion_but_preserves_transaction_waiting() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let anim = Animation::ease(
            clock.clone(),
            0.,
            1.,
            0.,
            1000,
            crate::animation::Curve::Linear,
        );
        let active = AnimationState::new(TransactionBlocker::completed(), anim.clone());
        let transaction = Transaction::new();
        let waiting = AnimationState::new(transaction.blocker(), anim);
        clock.set_unadjusted(Duration::from_millis(16));
        assert!(active.are_animations_ongoing(true));
        assert!(waiting.are_animations_ongoing(true));

        clock.set_complete_instantly(true);
        assert!(!active.are_animations_ongoing(true));
        assert!(!active.are_animations_ongoing(false));
        assert!(waiting.are_animations_ongoing(true));
    }

    #[test]
    fn denial_particle_generation_matches_grid_and_ranges() {
        let particles = build_particles(0x1234_5678);
        assert_eq!(particles.len(), 88);
        for particle in &particles {
            assert!((0.04..=0.96).contains(&particle.x));
            assert!((0.04..=0.96).contains(&particle.y));
            assert!((0.42..1.32).contains(&particle.speed));
            assert!((3.0..11.0).contains(&particle.size));
            assert!((0.45..2.15).contains(&particle.aspect));
        }
    }

    #[test]
    fn dart_random_is_repeatable() {
        let mut first = DartRandom::new(42);
        let mut second = DartRandom::new(42);
        for _ in 0..32 {
            assert_eq!(first.next_double(), second.next_double());
        }
    }

    #[test]
    fn close_glide_transforms_particle_geometry_with_the_snapshot() {
        let size = Size::from((800., 600.));
        let glide = realand_config::Glide::default();
        assert_eq!(
            ParticleGlide::new(size, glide, 0.7).transform([400., 300.]),
            [400., 300.]
        );

        let transformed = ParticleGlide::new(size, glide, 0.7).transform([100., 80.]);
        assert!(transformed.into_iter().all(f64::is_finite));
        assert_ne!(transformed, [100., 80.]);

        assert_eq!(
            ParticleGlide::new(size, realand_config::Glide { off: true, ..glide }, 0.7)
                .transform([100., 80.]),
            [100., 80.]
        );
    }
}
