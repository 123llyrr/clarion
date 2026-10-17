//! Native dual-Kawase blur with reusable intermediate images.
use std::sync::Arc;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::{Bind, Color32F, ContextId, Frame, Offscreen, Renderer, Texture};
use smithay::utils::{Buffer, Rectangle, Size, Transform};

use super::native::{NativeError, NativeFrame, NativeRenderer};
use super::VulkanTexture;
use crate::render_helpers::blur::BlurOptions;

const MAX_RETAINED_OUTPUTS: usize = 8;
const MAX_RETAINED_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;

/// Evict the oldest cache entries until the new allocation fits. Driver padding
/// counts toward the byte budget; an oversized output remains usable but is not
/// retained after it is replaced.
fn output_pool_evictions(
    mut allocation_sizes: impl ExactSizeIterator<Item = u64> + Clone,
    incoming: u64,
) -> Option<usize> {
    if incoming > MAX_RETAINED_OUTPUT_BYTES {
        return None;
    }
    let mut count = allocation_sizes.len();
    let mut bytes: u128 = allocation_sizes.clone().map(u128::from).sum();
    let mut evictions = 0;
    while count >= MAX_RETAINED_OUTPUTS
        || bytes + u128::from(incoming) > u128::from(MAX_RETAINED_OUTPUT_BYTES)
    {
        bytes -= u128::from(allocation_sizes.next()?);
        count -= 1;
        evictions += 1;
    }
    Some(evictions)
}

#[derive(Debug, Default)]
pub struct NativeBlur {
    context: Option<ContextId<VulkanTexture>>,
    // Full-resolution output, then successively smaller intermediate images.
    textures: Vec<VulkanTexture>,
    // Replaced full-resolution outputs, including ones still owned by a frame
    // or snapshot. Only a uniquely owned image is eligible for reuse. The
    // current output and intermediate chain are separate from this cache.
    output_pool: Vec<VulkanTexture>,
}

/// Required target pixels for each down/up pass, in execution order.
///
/// Keep every texture and viewport at its original size: changing their size
/// changes Kawase's sample phase, notably for odd dimensions. Only the scissor
/// is reduced. Walking dependencies backwards includes every bilinear tap of
/// every later pass; pixels outside these rectangles are never sampled.
pub(crate) fn pass_regions(
    size: Size<i32, Buffer>,
    options: BlurOptions,
    visible: Rectangle<i32, Buffer>,
) -> Vec<Rectangle<i32, Buffer>> {
    let passes = usize::from(options.passes.clamp(1, 31));
    let mut sizes = Vec::with_capacity(passes + 1);
    sizes.push(size);
    for _ in 0..passes {
        let last = *sizes.last().unwrap();
        sizes.push(Size::from(((last.w / 2).max(1), (last.h / 2).max(1))));
    }
    let mut regions = vec![Rectangle::default(); 2 * passes];
    let Some(mut needed) = visible.intersection(Rectangle::from_size(size)) else {
        return regions;
    };
    // Match the shader's f32 offset, then conservatively round outwards.
    let offset = f64::from(options.offset as f32);
    for step in (0..regions.len()).rev() {
        let up = step >= passes;
        let target = if up { 2 * passes - 1 - step } else { step + 1 };
        let source = if up { target + 1 } else { target - 1 };
        let input = sizes[source];
        let output = sizes[target];
        regions[step] = needed;
        // Upsampling's outer taps reach offset source texels. Downsampling's
        // taps use half an *output* texel. An extra two source texels cover
        // bilinear neighbours and floating point interpolation/rounding.
        let ratio_x = f64::from(input.w) / f64::from(output.w);
        let ratio_y = f64::from(input.h) / f64::from(output.h);
        let radius_x = if up { offset } else { offset * 0.5 * ratio_x } + 2.;
        let radius_y = if up { offset } else { offset * 0.5 * ratio_y } + 2.;
        let left = (f64::from(needed.loc.x) * ratio_x - radius_x)
            .floor()
            .max(0.) as i32;
        let top = (f64::from(needed.loc.y) * ratio_y - radius_y)
            .floor()
            .max(0.) as i32;
        let right = (f64::from(needed.loc.x + needed.size.w) * ratio_x + radius_x)
            .ceil()
            .min(f64::from(input.w)) as i32;
        let bottom = (f64::from(needed.loc.y + needed.size.h) * ratio_y + radius_y)
            .ceil()
            .min(f64::from(input.h)) as i32;
        needed = Rectangle::from_extremities((left, top), (right, bottom));
    }
    regions
}

impl NativeBlur {
    fn retain_output(&mut self, texture: VulkanTexture) {
        let Some(evictions) = output_pool_evictions(
            self.output_pool.iter().map(|t| t.inner.allocation_size),
            texture.inner.allocation_size,
        ) else {
            return;
        };
        drop(self.output_pool.drain(..evictions));
        self.output_pool.push(texture);
    }

    fn prepare_textures(
        &mut self,
        id: ContextId<VulkanTexture>,
        size: Size<i32, Buffer>,
        passes: usize,
        mut create: impl FnMut(Size<i32, Buffer>) -> Result<VulkanTexture, NativeError>,
    ) -> Result<(), NativeError> {
        if self.context.as_ref() != Some(&id)
            || self.textures.first().is_some_and(|t| t.size() != size)
        {
            // Owners in submitted frames or snapshots remain alive independently.
            self.textures.clear();
            self.output_pool.clear();
            self.context = Some(id);
        }
        let mut level_size = size;
        for i in 0..=passes {
            if i == self.textures.len() {
                self.textures.push(create(level_size)?);
            }
            level_size.w = (level_size.w / 2).max(1);
            level_size.h = (level_size.h / 2).max(1);
        }
        self.textures.truncate(passes + 1);
        // A completed older output can rotate back in while the most recent
        // output is still sampled by the GPU or retained by another caller.
        if Arc::strong_count(&self.textures[0].inner) != 1 {
            let replacement = match self
                .output_pool
                .iter()
                .position(|t| Arc::strong_count(&t.inner) == 1)
            {
                Some(index) => self.output_pool.remove(index),
                None => create(size)?,
            };
            let previous = std::mem::replace(&mut self.textures[0], replacement);
            self.retain_output(previous);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn retained_output_allocations(&self) -> Vec<u64> {
        self.output_pool
            .iter()
            .map(|t| t.inner.allocation_size)
            .collect()
    }

    /// All passes remain on the renderer's queue. The returned completion can
    /// be exported when consumers need to wait outside that queue.
    pub fn render(
        &mut self,
        renderer: &mut NativeRenderer,
        source: &VulkanTexture,
        options: BlurOptions,
    ) -> Result<(VulkanTexture, SyncPoint), NativeError> {
        let _latency =
            crate::utils::latency::Span::new(crate::utils::latency::Operation::NativeBlur);
        let job = self.prepare_in_frame(renderer, source, options)?;
        let mut output = job.textures[0].clone();
        let size = output.size();
        let mut target = renderer.bind(&mut output)?;
        let mut frame = renderer.render(
            &mut target,
            size.to_logical(1, Transform::Normal).to_physical(1),
            Transform::Normal,
        )?;
        let last = job.render_intermediates(&mut frame)?;
        frame.clear(
            Color32F::TRANSPARENT,
            &[Rectangle::from_size(frame.output_size())],
        )?;
        frame.draw_blur(
            last,
            [
                0.5 * job.offset / last.width() as f32,
                0.5 * job.offset / last.height() as f32,
            ],
            true,
        )?;
        let done = frame.finish()?;
        Ok((job.textures[0].clone(), done))
    }

    fn prepare(
        &mut self,
        renderer: &mut NativeRenderer,
        source: &VulkanTexture,
        options: BlurOptions,
    ) -> Result<(), NativeError> {
        if !options.offset.is_finite()
            || options.offset < 0.
            || options.offset > f64::from(f32::MAX)
        {
            return Err(NativeError::Other("invalid blur sample offset".into()));
        }
        let context = renderer
            .vulkan()
            .ctx
            .as_ref()
            .ok_or_else(|| NativeError::Other("missing Vulkan context".into()))?;
        if !Arc::ptr_eq(context, &source.inner.ctx) {
            return Err(NativeError::Other(
                "blur source belongs to another Vulkan device".into(),
            ));
        }
        renderer.retire_completed()?;
        let id = renderer.context_id();
        let passes = usize::from(options.passes.clamp(1, 31));
        self.prepare_textures(id, source.size(), passes, |size| {
            renderer.create_buffer(Fourcc::Abgr8888, size)
        })
    }

    /// Allocate before borrowing a frame, then enqueue after capturing its source.
    /// The prepared job owns its images and can be consumed only once, preserving
    /// previous outputs when the cache is prepared again.
    pub fn prepare_in_frame(
        &mut self,
        renderer: &mut NativeRenderer,
        source: &VulkanTexture,
        options: BlurOptions,
    ) -> Result<PreparedBlur, NativeError> {
        self.prepare(renderer, source, options)?;
        Ok(PreparedBlur {
            source: source.clone(),
            textures: self.textures.clone(),
            offset: options.offset as f32,
        })
    }

    pub fn prepare_in_native_frame(
        &mut self,
        frame: &NativeFrame<'_>,
        source: &VulkanTexture,
        options: BlurOptions,
    ) -> Result<PreparedBlur, NativeError> {
        if !options.offset.is_finite()
            || options.offset < 0.
            || options.offset > f64::from(f32::MAX)
        {
            return Err(NativeError::Other("invalid blur sample offset".into()));
        }
        if !Arc::ptr_eq(&frame.context(), &source.inner.ctx) {
            return Err(NativeError::Other(
                "blur source belongs to another Vulkan device".into(),
            ));
        }
        let id = frame.context_id();
        let passes = usize::from(options.passes.clamp(1, 31));
        self.prepare_textures(id, source.size(), passes, |size| {
            frame.create_buffer(Fourcc::Abgr8888, size)
        })?;
        Ok(PreparedBlur {
            source: source.clone(),
            textures: self.textures.clone(),
            offset: options.offset as f32,
        })
    }
}

/// A blur job prepared outside a frame, executed in its ordered command stream.
pub struct PreparedBlur {
    source: VulkanTexture,
    textures: Vec<VulkanTexture>,
    offset: f32,
}

impl PreparedBlur {
    fn render_intermediates<'a>(
        &'a self,
        frame: &mut NativeFrame<'_>,
    ) -> Result<&'a VulkanTexture, NativeError> {
        let passes = self.textures.len() - 1;
        for i in 1..=passes {
            let input = if i == 1 {
                &self.source
            } else {
                &self.textures[i - 1]
            };
            frame.blur_pass(input, &self.textures[i], self.offset, false)?;
        }
        for i in (1..passes).rev() {
            frame.blur_pass(&self.textures[i + 1], &self.textures[i], self.offset, true)?;
        }
        Ok(&self.textures[1])
    }

    pub fn render(self, frame: &mut NativeFrame<'_>) -> Result<VulkanTexture, NativeError> {
        let last = self.render_intermediates(frame)?;
        frame.blur_pass(last, &self.textures[0], self.offset, true)?;
        Ok(self.textures[0].clone())
    }

    /// Produce only a known sampled area. The caller must not read other pixels
    /// of the returned texture, which can contain previous-frame or undefined data.
    pub fn render_region(
        self,
        frame: &mut NativeFrame<'_>,
        visible: Rectangle<i32, Buffer>,
    ) -> Result<VulkanTexture, NativeError> {
        // Preserve the original full-effect path, including its allocation cost.
        if visible == Rectangle::from_size(self.source.size()) {
            return self.render(frame);
        }
        let passes = self.textures.len() - 1;
        let regions = pass_regions(
            self.source.size(),
            BlurOptions {
                passes: passes as u8,
                offset: f64::from(self.offset),
            },
            visible,
        );
        for (step, region) in regions.into_iter().enumerate() {
            if region.is_empty() {
                continue;
            }
            let up = step >= passes;
            let target = if up { 2 * passes - 1 - step } else { step + 1 };
            let source = if step == 0 {
                &self.source
            } else if up {
                &self.textures[target + 1]
            } else {
                &self.textures[target - 1]
            };
            frame.blur_pass_region(source, &self.textures[target], self.offset, up, region)?;
        }
        Ok(self.textures[0].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_pool_bounds_driver_padded_allocations_and_oversized_outputs() {
        let mib = 1024 * 1024;
        // Three logical 16 MiB outputs fit, but their actual 24 MiB Vulkan
        // allocations require retiring the oldest before retaining another.
        assert_eq!(
            output_pool_evictions([24 * mib, 24 * mib].into_iter(), 24 * mib),
            Some(1)
        );
        assert_eq!(
            output_pool_evictions([8 * mib; MAX_RETAINED_OUTPUTS].into_iter(), 8 * mib),
            Some(1)
        );
        assert_eq!(
            output_pool_evictions([1].into_iter(), MAX_RETAINED_OUTPUT_BYTES),
            Some(1)
        );
        assert_eq!(
            output_pool_evictions([1].into_iter(), MAX_RETAINED_OUTPUT_BYTES + 1),
            None
        );
        // Do not wrap an allocation ledger before applying the limit.
        assert_eq!(
            output_pool_evictions([u64::MAX, u64::MAX].into_iter(), 1),
            Some(2)
        );

        let mut retained = Vec::<u64>::new();
        for incoming in [1, 24 * mib, 24 * mib, 24 * mib, 65 * mib, 1]
            .into_iter()
            .chain(std::iter::repeat_n(1, 12))
        {
            let Some(evictions) = output_pool_evictions(retained.iter().copied(), incoming) else {
                assert!(incoming > MAX_RETAINED_OUTPUT_BYTES);
                continue;
            };
            if evictions > 0 {
                // Evict only as many oldest entries as necessary.
                let fewer = &retained[evictions - 1..];
                assert!(
                    fewer.len() + 1 > MAX_RETAINED_OUTPUTS
                        || fewer.iter().sum::<u64>() + incoming > MAX_RETAINED_OUTPUT_BYTES
                );
            }
            drop(retained.drain(..evictions));
            retained.push(incoming);
            assert!(retained.len() <= MAX_RETAINED_OUTPUTS);
            assert!(retained.iter().sum::<u64>() <= MAX_RETAINED_OUTPUT_BYTES);
        }
        assert_eq!(retained, [1; MAX_RETAINED_OUTPUTS]);
    }

    #[test]
    fn regional_blur_passes_cover_all_bilinear_taps() {
        for size in [(1, 1), (1, 97), (101, 1), (127, 93), (128, 96)] {
            for passes in [1, 3, 31] {
                for offset in [0., 1.25, 8.] {
                    let size: Size<i32, Buffer> = size.into();
                    let options = BlurOptions { passes, offset };
                    let visible = Rectangle::new((size.w / 3, size.h / 3).into(), (3, 7).into());
                    let regions = pass_regions(size, options, visible);
                    let mut sizes = vec![size];
                    for _ in 0..passes {
                        let previous = *sizes.last().unwrap();
                        sizes.push(Size::from((
                            (previous.w / 2).max(1),
                            (previous.h / 2).max(1),
                        )));
                    }
                    let passes = usize::from(passes);
                    for step in 1..regions.len() {
                        let up = step >= passes;
                        let target = if up { 2 * passes - 1 - step } else { step + 1 };
                        let source = if up { target + 1 } else { target - 1 };
                        let r = regions[step];
                        let dependency = regions[step - 1];
                        for (lo, len, input, output, dep_lo, dep_len) in [
                            (
                                r.loc.x,
                                r.size.w,
                                sizes[source].w,
                                sizes[target].w,
                                dependency.loc.x,
                                dependency.size.w,
                            ),
                            (
                                r.loc.y,
                                r.size.h,
                                sizes[source].h,
                                sizes[target].h,
                                dependency.loc.y,
                                dependency.size.h,
                            ),
                        ] {
                            for pixel in lo..lo + len {
                                for sign in [-1., 0., 1.] {
                                    let uv = (pixel as f32 + 0.5) / output as f32;
                                    let tap = if up {
                                        offset as f32 / input as f32
                                    } else {
                                        0.5 * offset as f32 / output as f32
                                    };
                                    let sample = (uv + sign * tap) * input as f32 - 0.5;
                                    for neighbour in
                                        [sample.floor() as i32, sample.floor() as i32 + 1]
                                    {
                                        let neighbour = neighbour.clamp(0, input - 1);
                                        assert!((dep_lo..dep_lo + dep_len).contains(&neighbour),
                                            "missing tap: size={size:?} passes={passes} offset={offset} step={step}");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn regional_blur_empty_and_full_plans_and_menu_bar_work() {
        let size: Size<i32, Buffer> = (1920, 1080).into();
        let options = BlurOptions {
            passes: 3,
            offset: 2.,
        };
        let empty = pass_regions(size, options, Rectangle::default());
        assert!(empty.iter().all(Rectangle::is_empty));
        let full = pass_regions(size, options, Rectangle::from_size(size));
        let bar = pass_regions(size, options, Rectangle::from_size((1920, 24).into()));
        let area = |regions: &[Rectangle<i32, Buffer>]| {
            regions
                .iter()
                .map(|r| r.size.w as usize * r.size.h as usize)
                .sum::<usize>()
        };
        assert_eq!(area(&full), 3_402_000);
        eprintln!(
            "WORK: 1080p/24px bar/3 passes/offset2 blur fragment area {} -> {}",
            area(&full),
            area(&bar)
        );
        assert!(
            area(&bar) * 8 < area(&full),
            "bar={} full={}",
            area(&bar),
            area(&full)
        );
    }
}
