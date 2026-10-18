use std::cell::RefCell;

use anyhow::Context as _;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::backend::renderer::element::{
    Element, Id, Kind, RenderElement, RenderElementStates, UnderlyingStorage,
};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, GlesRenderer, GlesTexture};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::utils::{
    CommitCounter, DamageBag, DamageSet, DamageSnapshot, OpaqueRegions,
};
use smithay::backend::renderer::{
    Bind as _, Color32F, ContextId, Frame as _, Offscreen as _, Renderer, Texture as _,
};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use super::encompassing_geo;
use super::renderer::AsGlesFrame as _;
use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};

/// Render a native animation snapshot while preserving the source surface visibility.
/// Callers use these states to deliver frame callbacks through the final offscreen element.
pub fn render_native_offscreen(
    renderer: &mut super::vulkan::native::NativeRenderer,
    scale: Scale<f64>,
    elements: &[impl RenderElement<super::vulkan::native::NativeRenderer>],
) -> anyhow::Result<(
    super::vulkan::VulkanTexture,
    SyncPoint,
    Rectangle<i32, Physical>,
    RenderElementStates,
)> {
    let geo = encompassing_geo(scale, elements.iter());
    let elements = elements
        .iter()
        .map(|element| {
            RelocateRenderElement::from_element(element, geo.loc.upscale(-1), Relocate::Relative)
        })
        .collect::<Vec<_>>();
    let mut texture = super::create_texture(renderer, geo.size, Fourcc::Abgr8888)?;
    let mut damage = OutputDamageTracker::new(geo.size, scale, Transform::Normal);
    let (sync, states) = {
        let mut target = renderer.bind(&mut texture)?;
        let result = damage
            .render_output(renderer, &mut target, 0, &elements, Color32F::TRANSPARENT)
            .context("error rendering native animation snapshot")?;
        (result.sync, result.states)
    };
    renderer.recycle_offscreen(texture.clone());
    Ok((texture, sync, geo, states))
}

/// Buffer for offscreen rendering.
#[derive(Debug)]
pub struct OffscreenBuffer {
    id: Id,

    /// The cached texture buffer.
    ///
    /// Lazily created when `render` is called. Recreated when necessary.
    inner: RefCell<Option<Inner>>,
}

/// Vulkan counterpart of [`OffscreenBuffer`]: a persistent offscreen target
/// with its own damage tracker, so long-running effects (workspace cube,
/// wobble) only redraw what changed instead of re-rendering the full scene
/// every frame. The target image stays bound to this buffer across frames;
/// the renderer's layout barriers order each re-render against the previous
/// frame's sampling without stalling the compositor on a CPU fence wait.
#[derive(Debug)]
pub struct NativeOffscreenBuffer {
    inner: RefCell<Option<NativeInner>>,
}

#[derive(Debug)]
struct NativeInner {
    texture: super::vulkan::VulkanTexture,
    renderer_context_id: ContextId<super::vulkan::VulkanTexture>,
    scale: Scale<f64>,
    damage: OutputDamageTracker,
    /// Commit counter mirroring the GLES buffer: bumped only by real redraws.
    outer_damage: DamageBag<i32, Buffer>,
}

/// Result of a persistent native offscreen render: the texture, its GPU sync
/// point, the encompassing geometry, frame-callback states, and a commit that
/// only advances when pixels actually changed.
pub type NativeOffscreenRender = (
    super::vulkan::VulkanTexture,
    SyncPoint,
    Rectangle<i32, Physical>,
    RenderElementStates,
    CommitCounter,
);

impl NativeOffscreenBuffer {
    /// Drop the cached render target and its damage history.
    pub fn clear(&self) {
        self.inner.borrow_mut().take();
    }

    /// Render `elements` into the persistent target, redrawing only the damage
    /// accumulated since the previous call. Returns the texture alongside the
    /// damage-bag commit: identical commits guarantee identical pixels, even
    /// though the texture identity no longer changes between frames.
    pub fn render(
        &self,
        renderer: &mut super::vulkan::native::NativeRenderer,
        scale: Scale<f64>,
        elements: &[impl RenderElement<super::vulkan::native::NativeRenderer>],
    ) -> anyhow::Result<NativeOffscreenRender> {
        let _latency =
            crate::utils::latency::Span::new(crate::utils::latency::Operation::NativeOffscreen);
        let _span = clarion_tracy_span!("NativeOffscreenBuffer::render");

        let geo = encompassing_geo(scale, elements.iter());
        let elements = elements
            .iter()
            .map(|element| {
                RelocateRenderElement::from_element(
                    element,
                    geo.loc.upscale(-1),
                    Relocate::Relative,
                )
            })
            .collect::<Vec<_>>();

        // Guard against empty elements producing a zero size.
        let mut target_size = geo.size;
        if target_size.w == 0 || target_size.h == 0 {
            target_size = Size::new(1, 1);
        }

        let mut inner = self.inner.borrow_mut();
        let recreate = match inner.as_ref() {
            Some(inner) => {
                inner.texture.width() < target_size.w as u32
                    || inner.texture.height() < target_size.h as u32
                    || inner.renderer_context_id != renderer.context_id()
                    || inner.texture.color.description != renderer.scene_color_description()
                    || inner.texture.format()
                        != Some(renderer.scene_buffer_format(Fourcc::Abgr8888))
            }
            None => true,
        };
        if recreate {
            // Hand the replaced image back to the pool instead of freeing it.
            if let Some(old) = inner.take() {
                renderer.recycle_offscreen(old.texture);
            }
            // Animated bounds can grow by a pixel each frame. Small capacity
            // buckets avoid allocating a new image for every such step. The
            // returned geometry, not the allocation size, defines visible UVs.
            let capacity = Size::<i32, Physical>::from((
                target_size
                    .w
                    .checked_add(63)
                    .map(|size| size & !63)
                    .unwrap_or(target_size.w),
                target_size
                    .h
                    .checked_add(63)
                    .map(|size| size & !63)
                    .unwrap_or(target_size.h),
            ));
            let texture = renderer
                .create_offscreen_capacity(
                    Fourcc::Abgr8888,
                    capacity.to_logical(1).to_buffer(1, Transform::Normal),
                )
                .or_else(|err| {
                    if capacity == target_size {
                        Err(err)
                    } else {
                        // Rounding can cross the device's image-size limit.
                        renderer.create_offscreen_capacity(
                            Fourcc::Abgr8888,
                            target_size.to_logical(1).to_buffer(1, Transform::Normal),
                        )
                    }
                })
                .context("error creating texture")?;
            // Keep a bounded pool reference while this scene owns the target.
            // clear() can then release it for the next window without another
            // cold allocation. Shared scene/GPU/snapshot owners prevent reuse.
            renderer.recycle_offscreen(texture.clone());
            let capacity = Size::from((texture.width() as i32, texture.height() as i32));
            let _ = inner.insert(NativeInner {
                texture,
                renderer_context_id: renderer.context_id(),
                scale,
                damage: OutputDamageTracker::new(capacity, scale, Transform::Normal),
                outer_damage: DamageBag::default(),
            });
        }
        let inner = inner.as_mut().unwrap();

        let texture_size: Size<i32, Physical> =
            Size::from((inner.texture.width() as i32, inner.texture.height() as i32));

        // Recreate the damage tracker if the scale changes, mirroring the GLES buffer.
        if inner.scale != scale {
            inner.scale = scale;
            inner.damage = OutputDamageTracker::new(texture_size, scale, Transform::Normal);
            inner.outer_damage = DamageBag::default();
        }

        let res = {
            let mut target = renderer.bind(&mut inner.texture)?;
            inner
                .damage
                .render_output(renderer, &mut target, 1, &elements, Color32F::TRANSPARENT)
                .context("error rendering native offscreen buffer")?
        };

        // Feed the redrawn region into the commit counter, same conversion as
        // the GLES buffer: tracker output is Physical space, which is Buffer
        // space when rendering into a texture.
        if let Some(damage) = res.damage {
            let size = texture_size.to_logical(1);
            let damage = damage
                .iter()
                .map(|rect| rect.to_logical(1).to_buffer(1, Transform::Normal, &size));
            inner.outer_damage.add(damage);
        }

        Ok((
            inner.texture.clone(),
            res.sync,
            geo,
            res.states,
            inner.outer_damage.current_commit(),
        ))
    }
}

impl Default for NativeOffscreenBuffer {
    fn default() -> Self {
        NativeOffscreenBuffer {
            inner: RefCell::new(None),
        }
    }
}

#[derive(Debug)]
struct Inner {
    /// The texture with offscreened contents.
    texture: GlesTexture,
    /// Id of the renderer context that the texture comes from.
    renderer_context_id: ContextId<GlesTexture>,
    /// Scale of the texture.
    scale: Scale<f64>,
    /// Damage tracker for drawing to the texture.
    damage: OutputDamageTracker,
    /// Damage of this offscreen element itself facing outside.
    outer_damage: DamageBag<i32, Buffer>,
}

#[derive(Debug, Clone)]
pub struct OffscreenRenderElement {
    id: Id,
    texture: GlesTexture,
    renderer_context_id: ContextId<GlesTexture>,
    scale: Scale<f64>,
    damage: DamageSnapshot<i32, Buffer>,
    offset: Point<f64, Logical>,
    src_size: Size<i32, Buffer>,
    alpha: f32,
    kind: Kind,
}

#[derive(Debug, Clone)]
pub struct OffscreenData {
    /// Id of the offscreen element.
    pub id: Id,
    /// States for the render into the offscreen buffer.
    pub states: RenderElementStates,
}

impl OffscreenBuffer {
    /// Drop the cached render target and its damage history.
    ///
    /// Offscreen buffers are intentionally lazy, so callers should release them once a
    /// short-lived effect finishes. Otherwise every window that has used an effect keeps its
    /// largest texture alive indefinitely.
    pub fn clear(&self) {
        self.inner.borrow_mut().take();
    }

    pub fn render(
        &self,
        renderer: &mut GlesRenderer,
        scale: Scale<f64>,
        elements: &[impl RenderElement<GlesRenderer>],
    ) -> anyhow::Result<(OffscreenRenderElement, SyncPoint, OffscreenData)> {
        let _span = clarion_tracy_span!("OffscreenBuffer::render");

        let geo = encompassing_geo(scale, elements.iter());
        let elements = Vec::from_iter(elements.iter().map(|ele| {
            RelocateRenderElement::from_element(ele, geo.loc.upscale(-1), Relocate::Relative)
        }));

        // Guard against empty elements producing a zero size.
        let mut src_size = geo.size;
        if src_size.w == 0 || src_size.h == 0 {
            src_size = Size::new(1, 1);
        }

        let src_size = src_size.to_logical(1).to_buffer(1, Transform::Normal);
        let offset = geo.loc.to_f64().to_logical(scale);

        let mut inner = self.inner.borrow_mut();

        // Check if we need to create or recreate the texture.
        let size_string;
        let mut reason = "";
        if let Some(Inner {
            texture,
            renderer_context_id,
            ..
        }) = inner.as_mut()
        {
            let old_size = texture.size();
            if old_size.w < src_size.w || old_size.h < src_size.h {
                size_string = format!(
                    "size increased from {} × {} to {} × {}",
                    old_size.w, old_size.h, src_size.w, src_size.h
                );
                reason = &size_string;

                *inner = None;
            } else if !texture.is_unique_reference() {
                reason = "not unique";

                *inner = None;
            } else if *renderer_context_id != renderer.context_id() {
                reason = "renderer id changed";

                *inner = None;
            }
        } else {
            reason = "first render";
        }

        let inner = if let Some(inner) = inner.as_mut() {
            inner
        } else {
            trace!("creating new texture: {reason}");
            let span = clarion_tracy_span!("creating offscreen buffer");
            span.emit_text(reason);

            let texture: GlesTexture = renderer
                .create_buffer(Fourcc::Abgr8888, src_size)
                .context("error creating texture")?;

            let buffer_size = src_size.to_logical(1, Transform::Normal).to_physical(1);
            let damage = OutputDamageTracker::new(buffer_size, scale, Transform::Normal);

            inner.insert(Inner {
                texture,
                renderer_context_id: renderer.context_id(),
                scale,
                damage,
                outer_damage: DamageBag::default(),
            })
        };

        // When leaving the old texture as is, its size might be bigger than src_size.
        let texture_size = inner.texture.size();
        let buffer_size = texture_size.to_logical(1, Transform::Normal).to_physical(1);

        // Recreate the damage tracker if the scale changes. We already recreate it for buffer size
        // changes, and transform is always Normal.
        if inner.scale != scale {
            inner.scale = scale;

            trace!("recreating damage tracker due to scale change");
            inner.damage = OutputDamageTracker::new(buffer_size, scale, Transform::Normal);
            inner.outer_damage = DamageBag::default();
        }

        let res = {
            let mut target = renderer.bind(&mut inner.texture)?;
            inner
                .damage
                .render_output(renderer, &mut target, 1, &elements, Color32F::TRANSPARENT)
                .context("error rendering")?
        };

        // Add the resulting damage to the outer tracker.
        if let Some(damage) = res.damage {
            // OutputDamageTracker gives us Physical coordinate space, but it's actually the Buffer
            // space because we were rendering to a texture.
            let size = buffer_size.to_logical(1);
            let damage = damage
                .iter()
                .map(|rect| rect.to_logical(1).to_buffer(1, Transform::Normal, &size));
            inner.outer_damage.add(damage);
        }

        let elem = OffscreenRenderElement {
            id: self.id.clone(),
            texture: inner.texture.clone(),
            renderer_context_id: inner.renderer_context_id.clone(),
            scale,
            damage: inner.outer_damage.snapshot(),
            offset,
            src_size,
            alpha: 1.,
            kind: Kind::Unspecified,
        };

        let data = OffscreenData {
            id: self.id.clone(),
            states: res.states,
        };

        Ok((elem, res.sync, data))
    }
}

impl Default for OffscreenBuffer {
    fn default() -> Self {
        OffscreenBuffer {
            inner: RefCell::new(None),
            id: Id::new(),
        }
    }
}

impl OffscreenRenderElement {
    pub fn texture(&self) -> &GlesTexture {
        &self.texture
    }

    pub fn offset(&self) -> Point<f64, Logical> {
        self.offset
    }

    pub fn with_alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha;
        self
    }

    pub fn with_offset(mut self, offset: Point<f64, Logical>) -> Self {
        self.offset = offset;
        self
    }

    pub fn logical_size(&self) -> Size<f64, Logical> {
        self.src_size
            .to_f64()
            .to_logical(self.scale, Transform::Normal)
    }

    fn damage_since(&self, commit: Option<CommitCounter>) -> DamageSet<i32, Buffer> {
        self.damage
            .damage_since(commit)
            .unwrap_or_else(|| DamageSet::from_slice(&[Rectangle::from_size(self.texture.size())]))
    }
}

impl Element for OffscreenRenderElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.damage.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        let logical_geo = Rectangle::new(self.offset, self.logical_size());
        logical_geo.to_physical_precise_round(scale)
    }

    fn transform(&self) -> Transform {
        Transform::Normal
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size(self.src_size).to_f64()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        let texture_size = self.texture.size().to_f64();
        let src = self.src();

        self.damage_since(commit)
            .into_iter()
            .filter_map(|region| {
                let mut region = region.to_f64().intersection(src)?;

                region.loc -= src.loc;
                region = region.upscale(texture_size / src.size);

                let logical = region.to_logical(self.scale, Transform::Normal, &src.size);
                Some(logical.to_physical_precise_up(scale))
            })
            .collect()
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        self.kind
    }
}

impl RenderElement<GlesRenderer> for OffscreenRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dest: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        if frame.context_id() != self.renderer_context_id {
            warn!("trying to render texture from different renderer");
            return Ok(());
        }

        frame.render_texture_from_to(
            &self.texture,
            src,
            dest,
            damage,
            opaque_regions,
            Transform::Normal,
            self.alpha,
            None,
            &[],
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // If scanout for things other than Wayland buffers is implemented, this will need to take
        // the target GPU into account.
        None
    }
}

impl<'render> RenderElement<TtyRenderer<'render>> for OffscreenRenderElement {
    fn draw(
        &self,
        frame: &mut TtyFrame<'_, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), TtyRendererError<'render>> {
        let gles_frame = frame.as_gles_frame();
        RenderElement::<GlesRenderer>::draw(
            &self,
            gles_frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )?;
        Ok(())
    }

    fn underlying_storage(
        &self,
        _renderer: &mut TtyRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        // If scanout for things other than Wayland buffers is implemented, this will need to take
        // the target GPU into account.
        None
    }
}

impl
    smithay::backend::renderer::element::RenderElement<
        crate::render_helpers::vulkan::native::NativeRenderer,
    > for OffscreenRenderElement
{
    fn draw(
        &self,
        _frame: &mut crate::render_helpers::vulkan::native::NativeFrame<'_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), crate::render_helpers::vulkan::native::NativeError> {
        Ok(())
    }

    fn underlying_storage(
        &self,
        _renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
    ) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod native_color_cache_tests {
    use super::*;
    use crate::color::{ColorDescription, HdrOutputSettings};
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    use crate::render_helpers::vulkan::hardware_tests::setup;
    use crate::render_helpers::vulkan::native::NativeRenderer;

    #[test]
    #[ignore = "requires a real DRM render node"]
    fn native_offscreen_color_cache_rebuilds_for_sdr_hdr_and_headroom_changes() {
        let (vk, reference, allocator) = setup();
        drop(reference);
        drop(allocator);
        let mut renderer = NativeRenderer::new(vk).unwrap();
        renderer
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        let buffer = NativeOffscreenBuffer::default();
        let source = SolidColorBuffer::new((32., 24.), [0.75, 0.5, 0.25, 1.]);
        let elements = [SolidColorRenderElement::from_buffer(
            &source,
            (0., 0.),
            1.,
            Kind::Unspecified,
        )];
        let outputs = [
            None,
            Some(ColorDescription::srgb()),
            Some(HdrOutputSettings::default().description()),
            Some(
                HdrOutputSettings {
                    peak_luminance: 500.,
                    ..Default::default()
                }
                .description(),
            ),
            None,
        ];
        let mut previous: Option<super::super::vulkan::VulkanTexture> = None;
        for output in outputs {
            let _scope = renderer.output_color_scope(output);
            let (texture, sync, _, _, _) = buffer
                .render(&mut renderer, Scale::from(1.), &elements)
                .unwrap();
            sync.wait().unwrap();
            assert_eq!(
                texture.color.description,
                renderer.scene_color_description()
            );
            assert_eq!(
                texture.format(),
                Some(renderer.scene_buffer_format(Fourcc::Abgr8888))
            );
            if let Some(previous) = previous.take() {
                assert_ne!(
                    texture.image(),
                    previous.image(),
                    "a retained old scene must not be overwritten or relabeled"
                );
            }
            let (same, sync, _, _, _) = buffer
                .render(&mut renderer, Scale::from(1.), &elements)
                .unwrap();
            sync.wait().unwrap();
            assert_eq!(
                same.image(),
                texture.image(),
                "unchanged scene should retain its cache"
            );
            previous = Some(texture);
        }
    }
}
