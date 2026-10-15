use std::cell::OnceCell;

use anyhow::Context as _;
use realand_config::BlockOutFrom;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::{Kind, RenderElement};
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::{Bind, Offscreen, Renderer, RendererSuper, Texture};
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use super::{render_to_encompassing_texture, ToRenderElement};
use crate::render_helpers::solid_color::SolidColorBuffer;
use crate::render_helpers::texture::TextureBuffer;
use crate::render_helpers::vulkan::{native::NativeRenderer, VulkanTexture};
use crate::render_helpers::{BakedBuffer, RenderCtx, RenderTarget};

/// Pixel geometry and the scale at which this lazy snapshot cache was rendered.
#[derive(Debug)]
pub struct SnapshotTexture<T> {
    texture: (T, Rectangle<i32, Physical>),
    scale: Scale<f64>,
}

/// The owned buffer and lazy texture cache produced when freezing a snapshot.
type FrozenTextureContents<T> = (BakedBuffer<TextureBuffer<T>>, SnapshotTexture<T>);

/// Snapshot of a render.
#[derive(Debug)]
pub struct RenderSnapshot<C, B, T = GlesTexture> {
    /// Contents for a normal render.
    ///
    /// Relative to the geometry.
    pub contents: Vec<C>,

    /// Contents that are not blocked out, but the background is blocked out.
    ///
    /// If `None` then the background doesn't have any blocked-out surfaces, and normal `contents`
    /// can be used instead.
    pub contents_with_blocked_out_bg: Option<Vec<C>>,

    /// Blocked-out contents.
    ///
    /// Relative to the geometry.
    pub blocked_out_contents: Vec<B>,

    /// Where the contents were blocked out from at the time of the snapshot.
    pub block_out_from: Option<BlockOutFrom>,

    /// Visual size of the element at the point of the snapshot.
    pub size: Size<f64, Logical>,

    /// Contents rendered into a texture (lazily).
    pub texture: OnceCell<Option<SnapshotTexture<T>>>,

    /// Contents with blocked-out bg rendered into a texture (lazily).
    pub texture_with_blocked_out_bg: OnceCell<Option<SnapshotTexture<T>>>,

    /// Blocked-out contents rendered into a texture (lazily).
    pub blocked_out_texture: OnceCell<Option<SnapshotTexture<T>>>,
}

impl<C, B, T> RenderSnapshot<C, B, T> {
    /// Scale of the cache selected by `texture()`, not the latest requested scale.
    /// The texture can outlive the output scale on which it was first rendered.
    pub fn texture_scale(&self, target: RenderTarget) -> Option<Scale<f64>> {
        let cache = if target.should_block_out(self.block_out_from) {
            &self.blocked_out_texture
        } else if target != RenderTarget::Output && self.contents_with_blocked_out_bg.is_some() {
            &self.texture_with_blocked_out_bg
        } else {
            &self.texture
        };
        cache.get()?.as_ref().map(|texture| texture.scale)
    }
}

impl<T> RenderSnapshot<BakedBuffer<TextureBuffer<T>>, BakedBuffer<SolidColorBuffer>, T>
where
    T: Texture + Clone,
    BakedBuffer<TextureBuffer<T>>: ToRenderElement,
{
    /// Replace client-backed source elements with completed, compositor-owned copies.
    /// This is for animated commits whose old client buffers may be released as soon
    /// as the renderer applies the new buffer. Ordinary `texture()` calls remain lazy and do
    /// not incur this CPU wait.
    pub fn freeze<R>(self, renderer: &mut R, scale: Scale<f64>) -> anyhow::Result<Self>
    where
        R: Renderer<TextureId = T> + Offscreen<T> + Bind<T>,
        R::Error: Send + Sync + 'static,
        <BakedBuffer<TextureBuffer<T>> as ToRenderElement>::RenderElement: RenderElement<R>,
    {
        self.freeze_with_copy_completion(renderer, scale, wait_for_snapshot_copy)
    }

    fn freeze_with_copy_completion<R>(
        self,
        renderer: &mut R,
        scale: Scale<f64>,
        complete_copy: impl Fn(&SyncPoint),
    ) -> anyhow::Result<Self>
    where
        R: Renderer<TextureId = T> + Offscreen<T> + Bind<T>,
        R::Error: Send + Sync + 'static,
        <BakedBuffer<TextureBuffer<T>> as ToRenderElement>::RenderElement: RenderElement<R>,
    {
        let (contents, texture) =
            freeze_texture_contents(renderer, scale, &self.contents, &complete_copy)
                .context("error freezing snapshot contents")?;
        let background = self
            .contents_with_blocked_out_bg
            .as_ref()
            .map(|contents| freeze_texture_contents(renderer, scale, contents, &complete_copy))
            .transpose()
            .context("error freezing snapshot with blocked-out background")?;
        let (background, background_texture) = match background {
            Some((contents, texture)) => (Some(vec![contents]), OnceCell::from(Some(texture))),
            None => (None, OnceCell::new()),
        };

        Ok(Self {
            contents: vec![contents],
            contents_with_blocked_out_bg: background,
            // Solid-color elements own their contents already, and preserve the
            // target's privacy rule without another GPU copy.
            blocked_out_contents: self.blocked_out_contents,
            block_out_from: self.block_out_from,
            size: self.size,
            // The owned copy is already a shader-ready cache. Share its
            // owned storage and exact physical bounds instead of copying again.
            // texture_scale() lets callers adapt it after an output scale change.
            texture: OnceCell::from(Some(texture)),
            texture_with_blocked_out_bg: background_texture,
            blocked_out_texture: OnceCell::new(),
        })
    }
}

impl
    RenderSnapshot<
        BakedBuffer<TextureBuffer<VulkanTexture>>,
        BakedBuffer<SolidColorBuffer>,
        VulkanTexture,
    >
{
    /// Freeze an owned copy on the native renderer's graphics queue.
    ///
    /// NativeFrame::finish retains sampled Wayland buffers and GPU images until
    /// completion, or waits when it cannot obtain those buffer owners. Later
    /// native draws use the same queue and image barriers to consume this copy.
    /// An additional CPU wait would stall every output on the resize commit.
    /// This optimization is restricted to NativeRenderer: other renderers must
    /// complete their copies before the replaced client storage is released.
    pub fn freeze_native(
        self,
        renderer: &mut NativeRenderer,
        scale: Scale<f64>,
    ) -> anyhow::Result<Self> {
        self.freeze_with_copy_completion(renderer, scale, |_| {})
    }
}

impl<C, B, T> RenderSnapshot<C, B, T>
where
    C: ToRenderElement,
    B: ToRenderElement,
{
    pub fn texture<R>(
        &self,
        ctx: RenderCtx<R>,
        scale: Scale<f64>,
    ) -> Option<&(T, Rectangle<i32, Physical>)>
    where
        R: Renderer<TextureId = T> + Offscreen<T> + Bind<T>,
        R::Error: Send + Sync + 'static,
        C::RenderElement: RenderElement<R>,
        B::RenderElement: RenderElement<R>,
    {
        if ctx.target.should_block_out(self.block_out_from) {
            self.blocked_out_texture.get_or_init(|| {
                let _span = realand_tracy_span!("RenderSnapshot::texture");

                let elements: Vec<_> = self
                    .blocked_out_contents
                    .iter()
                    .map(|baked| {
                        baked.to_render_element(Point::from((0., 0.)), scale, 1., Kind::Unspecified)
                    })
                    .collect();

                match render_to_encompassing_texture(
                    ctx.renderer,
                    scale,
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    &elements,
                ) {
                    Ok((texture, _sync_point, geo)) => Some(SnapshotTexture {
                        texture: (texture, geo),
                        scale,
                    }),
                    Err(err) => {
                        warn!("error rendering blocked-out contents to texture: {err:?}");
                        None
                    }
                }
            })
        } else if ctx.target != RenderTarget::Output && self.contents_with_blocked_out_bg.is_some()
        {
            let contents = self.contents_with_blocked_out_bg.as_ref().unwrap();
            self.texture_with_blocked_out_bg.get_or_init(|| {
                let _span = realand_tracy_span!("RenderSnapshot::texture");

                let elements: Vec<_> = contents
                    .iter()
                    .map(|baked| {
                        baked.to_render_element(Point::from((0., 0.)), scale, 1., Kind::Unspecified)
                    })
                    .collect();

                match render_to_encompassing_texture(
                    ctx.renderer,
                    scale,
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    &elements,
                ) {
                    Ok((texture, _sync_point, geo)) => Some(SnapshotTexture {
                        texture: (texture, geo),
                        scale,
                    }),
                    Err(err) => {
                        warn!("error rendering contents with blocked-out bg to texture: {err:?}");
                        None
                    }
                }
            })
        } else {
            self.texture.get_or_init(|| {
                let _span = realand_tracy_span!("RenderSnapshot::texture");

                let elements: Vec<_> = self
                    .contents
                    .iter()
                    .map(|baked| {
                        baked.to_render_element(Point::from((0., 0.)), scale, 1., Kind::Unspecified)
                    })
                    .collect();

                match render_to_encompassing_texture(
                    ctx.renderer,
                    scale,
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    &elements,
                ) {
                    Ok((texture, _sync_point, geo)) => Some(SnapshotTexture {
                        texture: (texture, geo),
                        scale,
                    }),
                    Err(err) => {
                        warn!("error rendering contents to texture: {err:?}");
                        None
                    }
                }
            })
        }
        .as_ref()
        .map(|texture| &texture.texture)
    }
}

fn freeze_texture_contents<R>(
    renderer: &mut R,
    scale: Scale<f64>,
    contents: &[BakedBuffer<TextureBuffer<R::TextureId>>],
    complete_copy: &impl Fn(&SyncPoint),
) -> anyhow::Result<FrozenTextureContents<R::TextureId>>
where
    R: Renderer
        + Offscreen<<R as RendererSuper>::TextureId>
        + Bind<<R as RendererSuper>::TextureId>,
    R::Error: Send + Sync + 'static,
    R::TextureId: Clone,
    BakedBuffer<TextureBuffer<R::TextureId>>: ToRenderElement,
    <BakedBuffer<TextureBuffer<R::TextureId>> as ToRenderElement>::RenderElement: RenderElement<R>,
{
    let elements = contents
        .iter()
        .map(|baked| baked.to_render_element(Point::from((0., 0.)), scale, 1., Kind::Unspecified))
        .collect::<Vec<_>>();
    let (texture, sync, geo) = render_to_encompassing_texture(
        renderer,
        scale,
        Transform::Normal,
        Fourcc::Abgr8888,
        &elements,
    )?;
    complete_copy(&sync);
    let cached = SnapshotTexture {
        texture: (texture.clone(), geo),
        scale,
    };
    let contents = BakedBuffer {
        buffer: TextureBuffer::from_texture(
            renderer,
            texture,
            scale,
            Transform::Normal,
            Vec::new(),
        ),
        location: geo.loc.to_f64().to_logical(scale),
        src: None,
        dst: None,
    };
    Ok((contents, cached))
}

fn wait_for_snapshot_copy(sync: &SyncPoint) {
    // EINTR does not mean the GPU has stopped reading the old client buffer.
    // Retry until completion before the commit can release that storage. This
    // deliberate wait is restricted to explicit snapshot freezing, never render().
    while sync.wait().is_err() {}
}

#[cfg(test)]
mod tests {
    use std::os::fd::OwnedFd;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use smithay::backend::renderer::sync::{Fence, Interrupted};

    use super::*;

    fn scale_test_snapshot() -> RenderSnapshot<(), (), ()> {
        RenderSnapshot {
            contents: Vec::new(),
            contents_with_blocked_out_bg: None,
            blocked_out_contents: Vec::new(),
            block_out_from: None,
            size: Size::from((300., 200.)),
            texture: OnceCell::new(),
            texture_with_blocked_out_bg: OnceCell::new(),
            blocked_out_texture: OnceCell::new(),
        }
    }

    fn cached_texture_at(scale: f64) -> Option<SnapshotTexture<()>> {
        Some(SnapshotTexture {
            texture: (
                (),
                Rectangle::new(Point::from((-20, -10)), Size::from((640, 480))),
            ),
            scale: Scale::from(scale),
        })
    }

    #[test]
    fn snapshot_texture_scale_tracks_the_selected_privacy_cache() {
        let mut snapshot = scale_test_snapshot();
        snapshot.texture.set(cached_texture_at(2.)).unwrap();
        snapshot
            .texture_with_blocked_out_bg
            .set(cached_texture_at(1.5))
            .unwrap();
        snapshot
            .blocked_out_texture
            .set(cached_texture_at(1.25))
            .unwrap();
        let targets = [
            RenderTarget::Output,
            RenderTarget::Screencast,
            RenderTarget::ScreenCapture,
        ];
        for (background, block_out, expected) in [
            (false, None, [2., 2., 2.]),
            (true, None, [2., 1.5, 1.5]),
            (true, Some(BlockOutFrom::Screencast), [2., 1.25, 1.5]),
            (true, Some(BlockOutFrom::ScreenCapture), [2., 1.25, 1.25]),
        ] {
            snapshot.contents_with_blocked_out_bg = background.then(Vec::new);
            snapshot.block_out_from = block_out;
            for (target, scale) in targets.into_iter().zip(expected) {
                assert_eq!(snapshot.texture_scale(target), Some(Scale::from(scale)));
            }
        }
    }

    #[test]
    fn snapshot_texture_scale_does_not_fallback_from_missing_or_failed_cache() {
        let mut snapshot = scale_test_snapshot();
        snapshot.texture.set(cached_texture_at(2.)).unwrap();
        snapshot.contents_with_blocked_out_bg = Some(Vec::new());
        snapshot.block_out_from = Some(BlockOutFrom::Screencast);
        assert_eq!(
            snapshot.texture_scale(RenderTarget::Output),
            Some(Scale::from(2.))
        );
        assert_eq!(snapshot.texture_scale(RenderTarget::Screencast), None);
        assert_eq!(snapshot.texture_scale(RenderTarget::ScreenCapture), None);
        snapshot.texture_with_blocked_out_bg.set(None).unwrap();
        snapshot.blocked_out_texture.set(None).unwrap();
        assert_eq!(snapshot.texture_scale(RenderTarget::Screencast), None);
        assert_eq!(snapshot.texture_scale(RenderTarget::ScreenCapture), None);
    }

    #[derive(Debug)]
    struct InterruptedCopy {
        calls: Arc<AtomicUsize>,
    }

    impl Fence for InterruptedCopy {
        fn is_signaled(&self) -> bool {
            self.calls.load(Ordering::SeqCst) >= 5
        }

        fn wait(&self) -> Result<(), Interrupted> {
            if self.calls.fetch_add(1, Ordering::SeqCst) < 4 {
                Err(Interrupted)
            } else {
                Ok(())
            }
        }

        fn is_exportable(&self) -> bool {
            false
        }

        fn export(&self) -> Option<OwnedFd> {
            None
        }
    }

    #[test]
    fn freezing_waits_for_copy_completion_despite_repeated_interruptions() {
        let calls = Arc::new(AtomicUsize::new(0));
        let sync = SyncPoint::from(InterruptedCopy {
            calls: calls.clone(),
        });
        wait_for_snapshot_copy(&sync);
        assert_eq!(calls.load(Ordering::SeqCst), 5);
        assert!(sync.is_reached());
    }
}
