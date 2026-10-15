//! Deferred GLES capture readback. EGL and PBO resources stay on the event loop;
//! only completed, owned bytes can be sent to an encoder or file worker.

use std::marker::PhantomData;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context};
use smithay::backend::allocator::Fourcc;
use smithay::backend::egl::fence::EGLFence;
use smithay::backend::renderer::gles::{Capability, GlesMapping, GlesRenderer, GlesTexture};
use smithay::backend::renderer::{ContextId, ExportMem, Renderer, Texture};
use smithay::utils::{Buffer, Rectangle};

const MAX_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub(crate) enum GlesReadbackPoll {
    Pending,
    Ready(Vec<u8>),
}

#[derive(Debug)]
struct Transfer {
    mapping: GlesMapping,
    fence: EGLFence,
}

#[derive(Debug)]
pub(crate) struct PendingGlesReadback {
    transfer: Option<Transfer>,
    context: ContextId<GlesTexture>,
    // ContextId identifies texture-compatible shared contexts. Require the
    // original EGL context as well so a poll cannot change resource ownership.
    egl_context: usize,
    deadline: Instant,
    expected_len: usize,
    // EGLFence can be Send, but this operation deliberately has thread ownership.
    _event_loop_only: PhantomData<Rc<()>>,
}

impl PendingGlesReadback {
    /// The pinned Smithay PBO/map path requires GLES 3 and EGL fence support.
    /// Callers can select their explicit synchronous compatibility path when
    /// this is false; this helper never silently maps an unfinished transfer.
    pub(crate) fn supports(renderer: &GlesRenderer) -> bool {
        renderer.capabilities().contains(&Capability::Fencing)
            && renderer.capabilities().contains(&Capability::ExportFence)
    }

    pub(crate) fn begin(
        renderer: &mut GlesRenderer,
        texture: &GlesTexture,
        region: Rectangle<i32, Buffer>,
        format: Fourcc,
    ) -> anyhow::Result<Self> {
        ensure!(
            Self::supports(renderer),
            "GLES deferred readback is unsupported"
        );
        // Smithay map_texture computes its length as width * height * 4. Do not
        // admit narrower pixel formats or dimensions that overflow that i32 math.
        ensure!(
            matches!(
                format,
                Fourcc::Abgr8888 | Fourcc::Xbgr8888 | Fourcc::Argb8888 | Fourcc::Xrgb8888
            ),
            "GLES deferred readback requires a packed 32-bit capture format"
        );
        let expected_len = capture_len(region, texture.size())?;
        let deadline = Instant::now() + MAX_WAIT;
        let context = renderer.context_id();
        let egl_context = renderer.egl_context().get_context_handle() as usize;
        let mapping = renderer
            .copy_texture(texture, region, format)
            .context("copying GLES capture into pixel pack buffer")?;
        // copy_texture leaves this exact GLES context current. Insert the fence
        // after ReadPixels, rather than reusing the earlier render-completion
        // fence, so it also covers the PBO transfer.
        let fence = EGLFence::create(renderer.egl_context().display())
            .context("creating GLES capture transfer fence")?;
        renderer
            .with_context(|gl| unsafe { gl.Flush() })
            .context("flushing GLES capture transfer")?;

        Ok(Self {
            transfer: Some(Transfer { mapping, fence }),
            context,
            egl_context,
            deadline,
            expected_len,
            _event_loop_only: PhantomData,
        })
    }

    /// Poll without blocking on GPU completion. Mapping and the final byte copy
    /// still execute on this renderer's owner thread after the transfer signals.
    /// A completion or error consumes the PBO; dropping the helper cancels it.
    pub(crate) fn poll(&mut self, renderer: &mut GlesRenderer) -> anyhow::Result<GlesReadbackPoll> {
        self.poll_at(renderer, Instant::now())
    }

    fn poll_at(
        &mut self,
        renderer: &mut GlesRenderer,
        now: Instant,
    ) -> anyhow::Result<GlesReadbackPoll> {
        let result = self.try_poll(renderer, now);
        if !matches!(&result, Ok(GlesReadbackPoll::Pending)) {
            self.transfer = None;
        }
        result
    }

    fn try_poll(
        &self,
        renderer: &mut GlesRenderer,
        now: Instant,
    ) -> anyhow::Result<GlesReadbackPoll> {
        let transfer = self
            .transfer
            .as_ref()
            .context("GLES readback already finished")?;
        ensure!(
            self.context == renderer.context_id()
                && self.egl_context == renderer.egl_context().get_context_handle() as usize,
            "GLES capture renderer context was replaced"
        );
        ensure!(
            now < self.deadline,
            "GLES capture transfer exceeded five seconds"
        );
        let reached = transfer
            .fence
            .client_wait(Some(Duration::ZERO), false)
            .context("polling GLES capture transfer fence")?;
        if !reached {
            return Ok(GlesReadbackPoll::Pending);
        }

        // No extra row reversal: preserve mapping_to_vec's existing screenshot
        // contract. TextureMapping::flipped describes texture sampling only.
        let bytes = renderer
            .map_texture(&transfer.mapping)
            .context("mapping completed GLES capture transfer")?;
        ensure!(
            bytes.len() == self.expected_len,
            "invalid GLES capture byte length"
        );
        Ok(GlesReadbackPoll::Ready(bytes.to_vec()))
    }
}

fn capture_len(
    region: Rectangle<i32, Buffer>,
    texture_size: smithay::utils::Size<i32, Buffer>,
) -> anyhow::Result<usize> {
    ensure!(
        region.loc.x >= 0
            && region.loc.y >= 0
            && region.size.w > 0
            && region.size.h > 0
            && region
                .loc
                .x
                .checked_add(region.size.w)
                .is_some_and(|x| x <= texture_size.w)
            && region
                .loc
                .y
                .checked_add(region.size.h)
                .is_some_and(|y| y <= texture_size.h),
        "GLES capture region is outside its texture"
    );
    let len = region
        .size
        .w
        .checked_mul(region.size.h)
        .and_then(|n| n.checked_mul(4));
    len.map(|n| n as usize)
        .context("GLES capture region exceeds PBO length limit")
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::backend::allocator::gbm::GbmDevice;
    use smithay::backend::egl::{EGLContext, EGLDisplay};
    use smithay::backend::renderer::ImportMem;
    use std::fs::OpenOptions;
    use std::sync::Arc;

    fn setup(with_fences: bool) -> GlesRenderer {
        let path = std::env::var("REALAND_GLES_TEST_NODE")
            .expect("set REALAND_GLES_TEST_NODE to a DRM render node");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let gbm = GbmDevice::new(Arc::new(file)).unwrap();
        let display = unsafe { EGLDisplay::new(gbm).unwrap() };
        let context = EGLContext::new(&display).unwrap();
        eprintln!("GLES READBACK HARDWARE: {path}");
        if with_fences {
            unsafe { GlesRenderer::new(context).unwrap() }
        } else {
            unsafe { GlesRenderer::with_capabilities(context, []).unwrap() }
        }
    }

    fn finish(job: &mut PendingGlesReadback, renderer: &mut GlesRenderer) -> Vec<u8> {
        loop {
            match job.poll(renderer).unwrap() {
                GlesReadbackPoll::Pending => std::thread::sleep(Duration::from_millis(1)),
                GlesReadbackPoll::Ready(bytes) => return bytes,
            }
        }
    }

    #[test]
    fn capture_region_rejects_empty_out_of_bounds_and_i32_byte_overflow() {
        let size = (12, 9).into();
        assert_eq!(
            capture_len(Rectangle::new((3, 2).into(), (5, 4).into()), size).unwrap(),
            80
        );
        for region in [
            Rectangle::new((-1, 0).into(), (2, 2).into()),
            Rectangle::new((11, 8).into(), (2, 2).into()),
            Rectangle::new((0, 0).into(), (0, 2).into()),
            Rectangle::new((i32::MAX, 0).into(), (1, 2).into()),
        ] {
            assert!(capture_len(region, size).is_err());
        }
        let huge = (32_768, 32_768).into();
        assert!(capture_len(Rectangle::from_size(huge), huge).is_err());
    }

    #[test]
    #[ignore = "requires a DRM render node and EGL fences"]
    fn gles_deferred_readback_preserves_crop_channels_rows_and_independent_buffers() {
        let mut renderer = setup(true);
        assert!(PendingGlesReadback::supports(&renderer));
        let size = (7, 5).into();
        let mut source = Vec::new();
        for y in 0..5u8 {
            for x in 0..7u8 {
                source.extend_from_slice(&[x * 20 + 1, y * 30 + 2, x + y * 7 + 3, 255]);
            }
        }
        let texture = renderer
            .import_memory(&source, Fourcc::Abgr8888, size, false)
            .unwrap();
        let crops = [
            Rectangle::from_size(size),
            Rectangle::new((2, 1).into(), (3, 3).into()),
        ];
        // Begin both before either is mapped; each must retain its own PBO data.
        let mut jobs = crops.map(|crop| {
            PendingGlesReadback::begin(&mut renderer, &texture, crop, Fourcc::Abgr8888).unwrap()
        });
        for (job, crop) in jobs.iter_mut().zip(crops) {
            let actual = finish(job, &mut renderer);
            let expected: Vec<_> = (crop.loc.y..crop.loc.y + crop.size.h)
                .flat_map(|y| {
                    let start = ((y * size.w + crop.loc.x) * 4) as usize;
                    source[start..start + crop.size.w as usize * 4]
                        .iter()
                        .copied()
                })
                .collect();
            assert_eq!(
                actual, expected,
                "readback changed crop, rows, channels or buffer ownership"
            );
            assert!(
                job.poll(&mut renderer).is_err(),
                "completed pixels must not be delivered twice"
            );
        }
        let cancel =
            PendingGlesReadback::begin(&mut renderer, &texture, crops[0], Fourcc::Abgr8888)
                .unwrap();
        drop(cancel);
        let mut next =
            PendingGlesReadback::begin(&mut renderer, &texture, crops[0], Fourcc::Abgr8888)
                .unwrap();
        assert_eq!(finish(&mut next, &mut renderer), source);
    }

    #[test]
    #[ignore = "requires a DRM render node and EGL fences"]
    fn gles_deferred_readback_rejects_context_replacement_deadline_and_unsupported_fences() {
        let mut renderer = setup(true);
        let texture = renderer
            .import_memory(&[1, 2, 3, 255], Fourcc::Abgr8888, (1, 1).into(), false)
            .unwrap();
        let region = Rectangle::from_size((1, 1).into());
        let mut expired =
            PendingGlesReadback::begin(&mut renderer, &texture, region, Fourcc::Abgr8888).unwrap();
        assert!(expired
            .poll_at(&mut renderer, expired.deadline)
            .unwrap_err()
            .to_string()
            .contains("five seconds"));
        assert!(expired.transfer.is_none());
        let mut replaced =
            PendingGlesReadback::begin(&mut renderer, &texture, region, Fourcc::Abgr8888).unwrap();
        let mut replacement = setup(true);
        assert!(replaced
            .poll(&mut replacement)
            .unwrap_err()
            .to_string()
            .contains("context was replaced"));
        assert!(replaced.transfer.is_none());
        let mut unsupported = setup(false);
        assert!(!PendingGlesReadback::supports(&unsupported));
        assert!(
            PendingGlesReadback::begin(&mut unsupported, &texture, region, Fourcc::Abgr8888)
                .is_err()
        );
        assert!(
            PendingGlesReadback::begin(&mut renderer, &texture, region, Fourcc::Rgb565).is_err()
        );
    }
}
