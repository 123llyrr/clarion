use std::ptr;

use anyhow::{ensure, Context as _};
use realand_config::BlockOutFrom;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Buffer, Fourcc};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::backend::renderer::element::{Element, Kind, RenderElement, RenderElementStates};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::{
    Bind, Color32F, ExportMem, Frame, Offscreen, Renderer, RendererSuper, Texture,
};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::shm;
use solid_color::{SolidColorBuffer, SolidColorRenderElement};

use self::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use self::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::renderer::AsGlesRenderer;
use crate::render_helpers::xray::Xray;

pub mod background_effect;
pub mod blur;
pub mod border;
pub mod clipped_surface;
pub mod damage;
mod damage_region;
pub mod debug;
pub mod effect_buffer;
pub mod framebuffer_effect;
pub mod gradient_fade_texture;
mod liquid_glass;
pub mod memory;
pub mod mesh;
pub mod offscreen;
pub mod particles;
pub mod primary_gpu_texture;
pub mod render_elements;
pub mod renderer;
pub mod resize;
pub mod resources;
pub mod shader_element;
pub mod shaders;
pub mod shadow;
pub mod snapshot;
pub mod solid_color;
pub mod surface;
pub(crate) mod sync;
pub mod texture;
pub(crate) mod transition_capture;
pub mod vulkan;
pub mod xray;

/// A rendering context.
///
/// Bundles together things needed by most rendering code.
pub struct RenderCtx<'a, R> {
    pub renderer: &'a mut R,
    pub target: RenderTarget,
    pub xray: Option<&'a Xray>,
}

impl<'a, R> RenderCtx<'a, R> {
    /// Reborrows this context with a smaller lifetime.
    #[inline]
    pub fn r<'b>(&'b mut self) -> RenderCtx<'b, R> {
        RenderCtx {
            renderer: self.renderer,
            target: self.target,
            xray: self.xray,
        }
    }
}

impl<'a, R: AsGlesRenderer> RenderCtx<'a, R> {
    pub fn as_gles<'b>(&'b mut self) -> Option<RenderCtx<'b, GlesRenderer>> {
        self.renderer.as_gles_renderer().map(|renderer| RenderCtx {
            renderer,
            target: self.target,
            xray: self.xray,
        })
    }
}

impl<'a, R: crate::render_helpers::renderer::AsNativeRenderer> RenderCtx<'a, R> {
    pub fn as_native<'b>(
        &'b mut self,
    ) -> Option<RenderCtx<'b, crate::render_helpers::vulkan::native::NativeRenderer>> {
        self.renderer
            .as_native_renderer()
            .map(|renderer| RenderCtx {
                renderer,
                target: self.target,
                xray: self.xray,
            })
    }
}

/// What we're rendering for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderTarget {
    /// Rendering to display on screen.
    Output = 0,
    /// Rendering for a screencast.
    Screencast,
    /// Rendering for any other screen capture.
    ScreenCapture,
}

/// Buffer with location, src and dst.
#[derive(Debug)]
pub struct BakedBuffer<B> {
    pub buffer: B,
    pub location: Point<f64, Logical>,
    pub src: Option<Rectangle<f64, Logical>>,
    pub dst: Option<Size<i32, Logical>>,
}

pub trait ToRenderElement {
    type RenderElement;

    fn to_render_element(
        &self,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
        alpha: f32,
        kind: Kind,
    ) -> Self::RenderElement;
}

impl RenderTarget {
    pub const COUNT: usize = 3;

    pub fn should_block_out(self, block_out_from: Option<BlockOutFrom>) -> bool {
        match block_out_from {
            None => false,
            Some(BlockOutFrom::Screencast) => self == RenderTarget::Screencast,
            Some(BlockOutFrom::ScreenCapture) => self != RenderTarget::Output,
        }
    }
}

impl ToRenderElement for BakedBuffer<TextureBuffer<GlesTexture>> {
    type RenderElement = PrimaryGpuTextureRenderElement;

    fn to_render_element(
        &self,
        location: Point<f64, Logical>,
        _scale: Scale<f64>,
        alpha: f32,
        kind: Kind,
    ) -> Self::RenderElement {
        let elem = TextureRenderElement::from_texture_buffer(
            self.buffer.clone(),
            location + self.location,
            alpha,
            self.src,
            self.dst.map(|dst| dst.to_f64()),
            kind,
        );
        PrimaryGpuTextureRenderElement::from(elem)
    }
}

impl ToRenderElement for BakedBuffer<TextureBuffer<vulkan::VulkanTexture>> {
    type RenderElement = TextureRenderElement<vulkan::VulkanTexture>;

    fn to_render_element(
        &self,
        location: Point<f64, Logical>,
        _scale: Scale<f64>,
        alpha: f32,
        kind: Kind,
    ) -> Self::RenderElement {
        TextureRenderElement::from_texture_buffer(
            self.buffer.clone(),
            location + self.location,
            alpha,
            self.src,
            self.dst.map(|dst| dst.to_f64()),
            kind,
        )
    }
}

impl ToRenderElement for BakedBuffer<SolidColorBuffer> {
    type RenderElement = SolidColorRenderElement;

    fn to_render_element(
        &self,
        location: Point<f64, Logical>,
        _scale: Scale<f64>,
        alpha: f32,
        kind: Kind,
    ) -> Self::RenderElement {
        SolidColorRenderElement::from_buffer(&self.buffer, location + self.location, alpha, kind)
    }
}

pub fn encompassing_geo(
    scale: Scale<f64>,
    elements: impl Iterator<Item = impl Element>,
) -> Rectangle<i32, Physical> {
    elements
        .map(|ele| ele.geometry(scale))
        .reduce(|a, b| a.merge(b))
        .unwrap_or_default()
}

pub fn create_texture<R: Renderer + Offscreen<<R as RendererSuper>::TextureId>>(
    renderer: &mut R,
    size: Size<i32, Physical>,
    fourcc: Fourcc,
) -> Result<R::TextureId, R::Error> {
    let buffer_size = size.to_logical(1).to_buffer(1, Transform::Normal);
    renderer.create_buffer(fourcc, buffer_size)
}

pub fn copy_framebuffer<R: ExportMem>(
    renderer: &mut R,
    target: &R::Framebuffer<'_>,
    fourcc: Fourcc,
) -> Result<R::TextureMapping, R::Error>
where
    for<'a> R::Framebuffer<'a>: Texture,
{
    renderer.copy_framebuffer(target, Rectangle::from_size(target.size()), fourcc)
}

pub fn render_to_encompassing_texture<R>(
    renderer: &mut R,
    scale: Scale<f64>,
    transform: Transform,
    fourcc: Fourcc,
    elements: &[impl RenderElement<R>],
) -> anyhow::Result<(R::TextureId, SyncPoint, Rectangle<i32, Physical>)>
where
    R: Renderer
        + Offscreen<<R as RendererSuper>::TextureId>
        + Bind<<R as RendererSuper>::TextureId>,
    R::Error: Send + Sync + 'static,
{
    let geo = encompassing_geo(scale, elements.iter());
    let elements = elements.iter().rev().map(|ele| {
        RelocateRenderElement::from_element(ele, geo.loc.upscale(-1), Relocate::Relative)
    });

    let (texture, sync_point) =
        render_to_texture(renderer, geo.size, scale, transform, fourcc, elements)?;

    Ok((texture, sync_point, geo))
}

pub fn render_to_texture<R>(
    renderer: &mut R,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    transform: Transform,
    fourcc: Fourcc,
    elements: impl Iterator<Item = impl RenderElement<R>>,
) -> anyhow::Result<(R::TextureId, SyncPoint)>
where
    R: Renderer
        + Offscreen<<R as RendererSuper>::TextureId>
        + Bind<<R as RendererSuper>::TextureId>,
    R::Error: Send + Sync + 'static,
{
    let _span = realand_tracy_span!();

    let mut texture = create_texture(renderer, size, fourcc).context("error creating texture")?;

    let sync_point = {
        let mut target = renderer
            .bind(&mut texture)
            .context("error binding texture")?;

        render_elements(renderer, &mut target, size, scale, transform, elements)?
    };

    Ok((texture, sync_point))
}

pub fn render_and_download<R>(
    renderer: &mut R,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    transform: Transform,
    fourcc: Fourcc,
    elements: impl Iterator<Item = impl RenderElement<R>>,
) -> anyhow::Result<R::TextureMapping>
where
    R: Renderer
        + ExportMem
        + Offscreen<<R as RendererSuper>::TextureId>
        + Bind<<R as RendererSuper>::TextureId>,
    R::Error: Send + Sync + 'static,
    for<'a> R::Framebuffer<'a>: Texture,
{
    let _span = realand_tracy_span!();

    let mut texture = create_texture(renderer, size, fourcc).context("error creating texture")?;
    let mut target = renderer
        .bind(&mut texture)
        .context("error binding texture")?;

    let _sync = render_elements(renderer, &mut target, size, scale, transform, elements)
        .context("error rendering")?;

    copy_framebuffer(renderer, &target, fourcc).context("error copying framebuffer")
}

pub fn render_to_vec<R>(
    renderer: &mut R,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    transform: Transform,
    fourcc: Fourcc,
    elements: impl Iterator<Item = impl RenderElement<R>>,
) -> anyhow::Result<Vec<u8>>
where
    R: Renderer
        + ExportMem
        + Offscreen<<R as RendererSuper>::TextureId>
        + Bind<<R as RendererSuper>::TextureId>,
    R::Error: Send + Sync + 'static,
    for<'a> R::Framebuffer<'a>: Texture,
{
    let _span = realand_tracy_span!();

    let mapping = render_and_download(renderer, size, scale, transform, fourcc, elements)
        .context("error rendering")?;
    mapping_to_vec(renderer, &mapping)
}

/// Read pixels from a compositor-rendered capture target in its existing row order.
/// The GLES projection already makes these rows Y-down. `TextureMapping::flipped`
/// describes texture sampling orientation, not an extra flip to apply to screenshots.
pub fn mapping_to_vec<R>(renderer: &mut R, mapping: &R::TextureMapping) -> anyhow::Result<Vec<u8>>
where
    R: ExportMem,
    R::Error: Send + Sync + 'static,
{
    let copy = renderer
        .map_texture(mapping)
        .context("error mapping texture")?;
    Ok(copy.to_vec())
}

pub fn render_to_dmabuf<R>(
    renderer: &mut R,
    damage_tracker: &mut OutputDamageTracker,
    mut dmabuf: Dmabuf,
    elements: &[impl RenderElement<R>],
    states: RenderElementStates,
) -> anyhow::Result<SyncPoint>
where
    R: Renderer + Bind<Dmabuf>,
    R::Error: Send + Sync + 'static,
{
    let _span = realand_tracy_span!();
    let (size, _scale, _transform) = damage_tracker.mode().try_into().unwrap();
    ensure!(
        dmabuf.width() == size.w as u32 && dmabuf.height() == size.h as u32,
        "invalid buffer size"
    );

    let mut target = renderer.bind(&mut dmabuf).context("error binding dmabuf")?;
    let res = damage_tracker
        .render_output_with_states(
            renderer,
            &mut target,
            0,
            elements,
            Color32F::TRANSPARENT,
            states,
        )
        .context("error rendering to dmabuf")?;
    Ok(res.sync)
}

#[derive(Clone, Copy, Debug)]
struct ShmCaptureLayout {
    offset: usize,
    stride: usize,
    row: usize,
    rows: usize,
}

fn shm_capture_layout(
    size: Size<i32, Physical>,
    len: usize,
    data: shm::BufferData,
) -> anyhow::Result<ShmCaptureLayout> {
    ensure!(
        data.format == wl_shm::Format::Xrgb8888
            && data.width == size.w
            && data.height == size.h
            && size.w > 0
            && size.h > 0
            && data.offset >= 0
            && data.stride > 0,
        "invalid SHM capture format or dimensions"
    );
    let row = (size.w as usize)
        .checked_mul(4)
        .context("capture row overflow")?;
    let offset = data.offset as usize;
    let stride = data.stride as usize;
    let rows = size.h as usize;
    let end = (rows - 1)
        .checked_mul(stride)
        .and_then(|n| n.checked_add(row))
        .and_then(|n| n.checked_add(offset))
        .context("capture layout overflow")?;
    ensure!(
        stride >= row && end <= len,
        "SHM capture exceeds pool bounds"
    );
    Ok(ShmCaptureLayout {
        offset,
        stride,
        row,
        rows,
    })
}

/// Validate the client buffer, then render a capture texture without downloading
/// it. The returned texture can be submitted to DeferredCaptureRenderer. No SHM
/// mapping or SIGBUS guard is retained during GPU work or asynchronous readback.
pub(crate) fn prepare_shm_texture<R>(
    renderer: &mut R,
    damage_tracker: &mut OutputDamageTracker,
    buffer: &WlBuffer,
    elements: &[impl RenderElement<R>],
    states: RenderElementStates,
) -> anyhow::Result<R::TextureId>
where
    R: Renderer
        + Offscreen<<R as RendererSuper>::TextureId>
        + Bind<<R as RendererSuper>::TextureId>,
    R::Error: Send + Sync + 'static,
{
    let _span = realand_tracy_span!();
    let (size, _, _): (Size<i32, Physical>, Scale<f64>, Transform) = damage_tracker
        .mode()
        .try_into()
        .context("invalid capture output mode")?;
    shm::with_buffer_contents(buffer, |_, len, data| shm_capture_layout(size, len, data))
        .context("expected SHM capture buffer")??;
    let mut texture = create_texture(renderer, size, Fourcc::Xrgb8888)
        .context("error creating SHM capture texture")?;
    {
        let mut target = renderer
            .bind(&mut texture)
            .context("error binding SHM capture texture")?;
        damage_tracker
            .render_output_with_states(
                renderer,
                &mut target,
                0,
                elements,
                Color32F::TRANSPARENT,
                states,
            )
            .context("error rendering SHM capture")?;
    }
    Ok(texture)
}

/// Complete a deferred SHM capture on the protocol-owning thread. Revalidate the
/// buffer after GPU completion, then write only active pixels in existing row
/// order. Buffer prefix and padding are preserved.
pub(crate) fn write_shm_pixels(
    buffer: &WlBuffer,
    size: Size<i32, Physical>,
    bytes: &[u8],
) -> anyhow::Result<()> {
    shm::with_buffer_contents_mut(buffer, |ptr, len, data| {
        let layout = shm_capture_layout(size, len, data)?;
        // SAFETY: layout bounds were checked against this guarded pool mapping.
        unsafe { copy_shm_pixels(ptr, layout, bytes) }
    })
    .context("SHM capture write failed")?
}

/// The caller must provide a live writable pool covering the validated layout.
unsafe fn copy_shm_pixels(
    pool: *mut u8,
    layout: ShmCaptureLayout,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let required = layout
        .row
        .checked_mul(layout.rows)
        .context("capture size overflow")?;
    ensure!(bytes.len() >= required, "incomplete capture readback");
    for y in 0..layout.rows {
        // Only overwrite active pixels, preserving pool prefix and row padding.
        unsafe {
            ptr::copy_nonoverlapping(
                bytes.as_ptr().add(y * layout.row),
                pool.add(layout.offset + y * layout.stride),
                layout.row,
            );
        }
    }
    Ok(())
}

/// Synchronous compatibility wrapper; production capture submits the prepared
/// texture through DeferredCaptureRenderer and writes only after completion.
pub fn render_to_shm<R>(
    renderer: &mut R,
    damage_tracker: &mut OutputDamageTracker,
    buffer: &WlBuffer,
    elements: &[impl RenderElement<R>],
    states: RenderElementStates,
) -> anyhow::Result<()>
where
    R: Renderer
        + ExportMem
        + Offscreen<<R as RendererSuper>::TextureId>
        + Bind<<R as RendererSuper>::TextureId>,
    R::Error: Send + Sync + 'static,
    for<'a> R::Framebuffer<'a>: Texture,
{
    let texture = prepare_shm_texture(renderer, damage_tracker, buffer, elements, states)?;
    let (size, _, _): (Size<i32, Physical>, Scale<f64>, Transform) = damage_tracker
        .mode()
        .try_into()
        .context("invalid capture output mode")?;
    let mapping = renderer
        .copy_texture(
            &texture,
            Rectangle::from_size((size.w, size.h).into()),
            Fourcc::Xrgb8888,
        )
        .context("error copying SHM capture texture")?;
    let bytes = renderer
        .map_texture(&mapping)
        .context("error mapping SHM capture texture")?;
    write_shm_pixels(buffer, size, bytes)
}

pub fn clear_dmabuf<R>(renderer: &mut R, mut dmabuf: Dmabuf) -> anyhow::Result<SyncPoint>
where
    R: Renderer + Bind<Dmabuf>,
    R::Error: Send + Sync + 'static,
{
    let size = dmabuf.size();
    let size = size.to_logical(1, Transform::Normal).to_physical(1);
    let mut target = renderer.bind(&mut dmabuf).context("error binding dmabuf")?;
    let mut frame = renderer
        .render(&mut target, size, Transform::Normal)
        .context("error starting frame")?;
    frame
        .clear(Color32F::TRANSPARENT, &[Rectangle::from_size(size)])
        .context("error clearing")?;
    frame.finish().context("error finishing frame")
}

fn render_elements<R>(
    renderer: &mut R,
    target: &mut R::Framebuffer<'_>,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    transform: Transform,
    elements: impl Iterator<Item = impl RenderElement<R>>,
) -> anyhow::Result<SyncPoint>
where
    R: Renderer,
    R::Error: Send + Sync + 'static,
{
    let transform = transform.invert();
    let output_rect = Rectangle::from_size(transform.transform_size(size));

    let mut frame = renderer
        .render(target, size, transform)
        .context("error starting frame")?;

    frame
        .clear(Color32F::TRANSPARENT, &[output_rect])
        .context("error clearing")?;

    for element in elements {
        let src = element.src();
        let dst = element.geometry(scale);

        if let Some(mut damage) = output_rect.intersection(dst) {
            damage.loc -= dst.loc;

            let cache = UserDataMap::new();
            if element.is_framebuffer_effect() {
                element
                    .capture_framebuffer(&mut frame, src, dst, &cache)
                    .context("error in capture_framebuffer()")?;
            }
            element
                .draw(&mut frame, src, dst, &[damage], &[], Some(&cache))
                .context("error drawing element")?;
        }
    }

    frame.finish().context("error finishing frame")
}

#[cfg(test)]
mod shm_capture_tests {
    use super::*;

    fn data() -> shm::BufferData {
        shm::BufferData {
            offset: 7,
            width: 2,
            height: 3,
            stride: 12,
            format: wl_shm::Format::Xrgb8888,
        }
    }

    #[test]
    fn deferred_completion_preserves_prefix_padding_and_raw_row_order() {
        let size = (2, 3).into();
        let mut pool = vec![199; 45];
        let bytes: Vec<_> = (0..24).collect();
        let layout = shm_capture_layout(size, pool.len(), data()).unwrap();
        unsafe {
            copy_shm_pixels(pool.as_mut_ptr(), layout, &bytes).unwrap();
        }
        assert_eq!(&pool[..7], &[199; 7]);
        for row in 0..3 {
            assert_eq!(
                &pool[7 + row * 12..15 + row * 12],
                &bytes[row * 8..row * 8 + 8]
            );
            assert_eq!(&pool[15 + row * 12..19 + row * 12], &[199; 4]);
        }
    }

    #[test]
    fn completion_rejects_short_pool_or_pixels_before_writing() {
        let size = (2, 3).into();
        assert!(shm_capture_layout(size, 38, data()).is_err());
        let mut pool = vec![199; 45];
        let original = pool.clone();
        let layout = shm_capture_layout(size, pool.len(), data()).unwrap();
        assert!(unsafe { copy_shm_pixels(pool.as_mut_ptr(), layout, &[0; 23]) }.is_err());
        assert_eq!(pool, original);
        for invalid in [
            shm::BufferData {
                offset: -1,
                ..data()
            },
            shm::BufferData {
                stride: 7,
                ..data()
            },
            shm::BufferData { width: 3, ..data() },
            shm::BufferData {
                format: wl_shm::Format::Argb8888,
                ..data()
            },
        ] {
            assert!(shm_capture_layout(size, pool.len(), invalid).is_err());
        }
        assert!(shm_capture_layout((0, 3).into(), pool.len(), data()).is_err());
    }
}
