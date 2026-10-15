//! Capture submission stays on the compositor thread. Vulkan transfers may
//! finish on a worker; GLES PBOs are polled by their owning EGL event loop.

use anyhow::{ensure, Context};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{ExportMem, Texture};
use smithay::utils::{Buffer, Rectangle};

use crate::render_helpers::renderer::RealandRenderer;
use crate::render_helpers::vulkan::native::{NativeRenderer, PendingNativeReadback};

use super::gles_readback::PendingGlesReadback;

#[derive(Debug)]
pub(crate) enum PendingPixels {
    Native(PendingNativeReadback),
    Gles(PendingGlesReadback),
    Ready(Vec<u8>),
}

pub(crate) trait DeferredCaptureRenderer: RealandRenderer {
    fn enqueue_pixels(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, Buffer>,
        format: Fourcc,
    ) -> anyhow::Result<PendingPixels>;
}

impl DeferredCaptureRenderer for NativeRenderer {
    fn enqueue_pixels(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, Buffer>,
        format: Fourcc,
    ) -> anyhow::Result<PendingPixels> {
        Ok(PendingPixels::Native(
            self.enqueue_readback(texture, region, format)
                .context("submitting Vulkan capture readback")?,
        ))
    }
}

impl DeferredCaptureRenderer for GlesRenderer {
    fn enqueue_pixels(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, Buffer>,
        format: Fourcc,
    ) -> anyhow::Result<PendingPixels> {
        if PendingGlesReadback::supports(self) {
            return Ok(PendingPixels::Gles(PendingGlesReadback::begin(
                self, texture, region, format,
            )?));
        }
        validate_fallback_capture(region, texture.size(), format)?;
        warn!("GLES capture fences unavailable; using synchronous readback compatibility path");
        let mapping = self
            .copy_texture(texture, region, format)
            .context("copying GLES capture without fence support")?;
        Ok(PendingPixels::Ready(crate::render_helpers::mapping_to_vec(
            self, &mapping,
        )?))
    }
}

fn validate_fallback_capture(
    region: Rectangle<i32, Buffer>,
    texture_size: smithay::utils::Size<i32, Buffer>,
    format: Fourcc,
) -> anyhow::Result<()> {
    // The legacy GLES map length is an i32 packed-four-byte product. Apply the
    // same bounds as the deferred path before its synchronous fallback.
    ensure!(
        matches!(
            format,
            Fourcc::Abgr8888 | Fourcc::Xbgr8888 | Fourcc::Argb8888 | Fourcc::Xrgb8888
        ),
        "GLES capture requires a packed 32-bit format"
    );
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
                .is_some_and(|y| y <= texture_size.h)
            && region
                .size
                .w
                .checked_mul(region.size.h)
                .and_then(|n| n.checked_mul(4))
                .is_some(),
        "invalid GLES capture region or byte length"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synchronous_fallback_rejects_unsupported_formats_and_overflow_before_gpu_access() {
        let size = (20, 10).into();
        assert!(validate_fallback_capture(
            Rectangle::new((2, 3).into(), (5, 6).into()),
            size,
            Fourcc::Xrgb8888
        )
        .is_ok());
        for region in [
            Rectangle::new((-1, 0).into(), (2, 2).into()),
            Rectangle::new((19, 9).into(), (2, 2).into()),
            Rectangle::from_size((0, 1).into()),
            Rectangle::new((i32::MAX, 0).into(), (1, 1).into()),
            Rectangle::from_size((32_768, 32_768).into()),
        ] {
            assert!(validate_fallback_capture(region, size, Fourcc::Xrgb8888).is_err());
        }
        assert!(
            validate_fallback_capture(Rectangle::from_size(size), size, Fourcc::Rgb565).is_err()
        );
    }
}
