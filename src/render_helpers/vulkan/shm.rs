//! Wayland SHM snapshots and surface-local native texture reuse.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak as RcWeak};
use std::sync::Weak;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::{
    buffer_type, BufferType, ContextId, ImportAll, ImportDmaWl, ImportMemWl, Renderer, Texture,
};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::utils::{Buffer, Rectangle};
use smithay::wayland::compositor::SurfaceData;
use smithay::wayland::shm::{shm_format_to_fourcc, with_buffer_contents};

use super::native::{NativeError, NativeRenderer};
use super::VulkanTexture;

const MAX_DAMAGE_REGIONS: usize = 64;

/// Vulkan uploads require disjoint destinations. Keep the subtraction work
/// bounded even when a client supplies many overlapping or fragmented regions;
/// copying their bounding rectangle from SHM preserves the same final pixels.
fn upload_regions(
    damage: &[Rectangle<i32, Buffer>],
    width: u32,
    height: u32,
) -> Vec<Rectangle<i32, Buffer>> {
    let rectangle = |rect: ash::vk::Rect2D| {
        Rectangle::new(
            (rect.offset.x, rect.offset.y).into(),
            (rect.extent.width as i32, rect.extent.height as i32).into(),
        )
    };
    let fallback = || {
        super::damage::bounding_rect(damage, width, height)
            .map(rectangle)
            .into_iter()
            .collect()
    };
    if damage.len() > MAX_DAMAGE_REGIONS {
        return fallback();
    }
    let mut regions = Vec::new();
    for rect in damage {
        let clipped = super::damage::clip_rect(
            rect.loc.x,
            rect.loc.y,
            rect.size.w,
            rect.size.h,
            width,
            height,
        );
        if clipped.extent.width == 0 || clipped.extent.height == 0 {
            continue;
        }
        let mut fragments = vec![rectangle(clipped)];
        for previous in &regions {
            let mut remaining = Vec::new();
            for fragment in fragments {
                remaining.extend(fragment.subtract_rect(*previous));
                if regions.len() + remaining.len() > MAX_DAMAGE_REGIONS {
                    return fallback();
                }
            }
            fragments = remaining;
        }
        if regions.len() + fragments.len() > MAX_DAMAGE_REGIONS {
            return fallback();
        }
        regions.extend(fragments);
    }
    regions
}

// Surface-local cache; each renderer mutates it only on its owning thread.
// A surface may rotate between several buffers: cache its last rendered texture,
// not a texture per buffer whose pixels can be several commits out of date.
#[derive(Default)]
struct ShmCache(Rc<RefCell<ShmCacheMap>>);

type ShmCacheMap = HashMap<ContextId<VulkanTexture>, ShmCacheEntry>;

struct ShmCacheEntry {
    renderer: Weak<()>,
    texture: VulkanTexture,
}

/// A weak surface-cache registration owned by one renderer. Dropping the
/// renderer removes its texture even if this surface is hidden indefinitely.
pub(super) struct ShmCacheRegistration {
    cache: RcWeak<RefCell<ShmCacheMap>>,
    id: ContextId<VulkanTexture>,
}

impl std::fmt::Debug for ShmCacheRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShmCacheRegistration")
            .field("id", &self.id)
            .finish()
    }
}

impl Drop for ShmCacheRegistration {
    fn drop(&mut self) {
        if let Some(cache) = self.cache.upgrade() {
            cache.borrow_mut().remove(&self.id);
        }
    }
}

impl NativeRenderer {
    fn track_shm_cache(&mut self, cache: &ShmCache) {
        self.shm_cache_imports = self.shm_cache_imports.wrapping_add(1);
        if self.shm_cache_imports.is_multiple_of(256) {
            self.shm_cache_registrations
                .retain(|_, registration| registration.cache.strong_count() != 0);
        }
        let key = Rc::as_ptr(&cache.0) as usize;
        if self
            .shm_cache_registrations
            .get(&key)
            .is_some_and(|registration| registration.cache.strong_count() != 0)
        {
            return;
        }
        self.shm_cache_registrations.insert(
            key,
            ShmCacheRegistration {
                cache: Rc::downgrade(&cache.0),
                id: self.context_id(),
            },
        );
    }
}

impl Drop for NativeRenderer {
    fn drop(&mut self) {
        self.shm_cache_registrations.clear();
    }
}

impl ShmCache {
    fn prune(&self) {
        self.0
            .borrow_mut()
            .retain(|_, entry| entry.renderer.strong_count() != 0);
    }

    fn get(&self, id: &ContextId<VulkanTexture>) -> Option<VulkanTexture> {
        self.prune();
        self.0.borrow().get(id).map(|entry| entry.texture.clone())
    }

    fn insert(&self, id: ContextId<VulkanTexture>, renderer: Weak<()>, texture: VulkanTexture) {
        self.prune();
        self.0
            .borrow_mut()
            .insert(id, ShmCacheEntry { renderer, texture });
    }

    fn remove(&self, id: &ContextId<VulkanTexture>) {
        self.prune();
        self.0.borrow_mut().remove(id);
    }
}

// Explicit dispatch keeps the native scene independent of ImportEgl, which
// Smithay's blanket ImportAll implementation requires in this build.
impl ImportAll for NativeRenderer {
    fn import_buffer(
        &mut self,
        buffer: &WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Option<Result<VulkanTexture, NativeError>> {
        let kind = buffer_type(buffer);
        if !matches!(kind, Some(BufferType::Shm)) {
            // Damage on a later SHM commit is relative to the intervening content,
            // not our older SHM texture. Force a full import when returning to SHM.
            if let Some(cache) = surface.and_then(|s| s.data_map.get::<ShmCache>()) {
                cache.remove(&self.context_id());
            }
        }
        match kind {
            Some(BufferType::Shm) => Some(self.import_shm_buffer(buffer, surface, damage)),
            Some(BufferType::Dma) => Some(self.import_dma_buffer(buffer, surface, damage)),
            _ => None,
        }
    }
}
impl ImportMemWl for NativeRenderer {
    fn import_shm_buffer(
        &mut self,
        buffer: &WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<VulkanTexture, NativeError> {
        let id = self.context_id();
        let cache = surface.map(|s| s.data_map.get_or_insert(ShmCache::default));
        let cached = cache.and_then(|c| c.get(&id));
        let snapshot = with_buffer_contents(buffer, |ptr, len, data| {
            let format = shm_format_to_fourcc(data.format)
                .filter(|f| {
                    matches!(
                        f,
                        Fourcc::Argb8888 | Fourcc::Xrgb8888 | Fourcc::Abgr8888 | Fourcc::Xbgr8888
                    )
                })
                .ok_or_else(|| NativeError::Other("unsupported Vulkan SHM format".into()))?;
            let invalid = || NativeError::Other("invalid SHM offset, dimensions or stride".into());
            if data.offset < 0 || data.width <= 0 || data.height <= 0 || data.stride <= 0 {
                return Err(invalid());
            }
            let row = (data.width as usize).checked_mul(4).ok_or_else(invalid)?;
            let stride = data.stride as usize;
            let offset = data.offset as usize;
            let end = (data.height as usize - 1)
                .checked_mul(stride)
                .and_then(|n| n.checked_add(row))
                .and_then(|n| n.checked_add(offset))
                .ok_or_else(invalid)?;
            if stride < row || end > len {
                return Err(invalid());
            }
            // Validate the new buffer metadata even on cache hits. Existing
            // textures need only the disjoint damaged rectangles copied out
            // while the SHM buffer is protected against truncation.
            let same_texture = cached.as_ref().is_some_and(|t| {
                t.width == data.width as u32
                    && t.height == data.height as u32
                    && t.format() == Some(format)
            });
            let mut regions = Vec::new();
            if same_texture {
                regions = upload_regions(damage, data.width as u32, data.height as u32);
                if regions.is_empty() {
                    return Ok((None, format, data.width, data.height, row, regions));
                }
            }
            let size = if same_texture {
                regions
                    .iter()
                    .try_fold(0usize, |total, rect: &Rectangle<i32, Buffer>| {
                        (rect.size.w as usize)
                            .checked_mul(4)
                            .and_then(|row| row.checked_mul(rect.size.h as usize))
                            .and_then(|bytes| total.checked_add(bytes))
                            .ok_or_else(invalid)
                    })?
            } else {
                row.checked_mul(data.height as usize).ok_or_else(invalid)?
            };
            let mut pixels = Vec::<u8>::new();
            pixels
                .try_reserve_exact(size)
                .map_err(|e| NativeError::Other(e.to_string()))?;
            // Never form a Rust reference into client-writable shared memory.
            // Smithay protects these raw reads against truncated-pool SIGBUS.
            // Write directly into reserved storage: zero-filling it first only
            // doubles CPU writes for every SHM upload. Keep len at zero until
            // every byte has been copied, including on the protected SIGBUS path.
            if same_texture {
                let mut position = 0;
                for region in &regions {
                    let patch_row = region.size.w as usize * 4;
                    for y in 0..region.size.h as usize {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                ptr.add(
                                    offset
                                        + (region.loc.y as usize + y) * stride
                                        + region.loc.x as usize * 4,
                                ),
                                pixels.as_mut_ptr().add(position),
                                patch_row,
                            );
                        }
                        position += patch_row;
                    }
                }
            } else {
                for y in 0..data.height as usize {
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            ptr.add(offset + y * stride),
                            pixels.as_mut_ptr().add(y * row),
                            row,
                        );
                    }
                }
            }
            // SAFETY: both branches initialize exactly `size` bytes: full rows
            // use row * height, packed regions use the checked sum of their row
            // sizes. The allocation covers size, and no slice of this storage
            // was exposed before the copy completed.
            unsafe { pixels.set_len(size) };
            Ok((Some(pixels), format, data.width, data.height, row, regions))
        })
        .map_err(|e| NativeError::Other(format!("SHM buffer access failed: {e:?}")))??;
        let (pixels, format, width, height, stride, regions) = snapshot;
        let previous = cached.filter(|t| {
            t.width == width as u32 && t.height == height as u32 && t.format() == Some(format)
        });
        let Some(pixels) = pixels else {
            return previous
                .ok_or_else(|| NativeError::Other("missing unchanged SHM texture".into()));
        };
        let permit = self.reserve_upload()?;
        let (texture, upload) = if let Some(texture) = previous {
            let upload = self
                .vulkan()
                .update_memory_texture_packed_regions_deferred(&texture, &pixels, &regions)?;
            (texture, upload)
        } else {
            // First use or size/format change needs all pixels even with empty damage.
            let (texture, upload) = self.vulkan().import_memory_texture_deferred(
                &pixels,
                format,
                width as u32,
                height as u32,
                stride,
            )?;
            (texture, Some(upload))
        };
        if let Some(upload) = upload {
            self.retain_shm_upload(upload, permit)?;
        }
        if let Some(cache) = cache {
            self.track_shm_cache(cache);
            cache.insert(id, self.shm_cache_lifetime(), texture.clone());
        }
        Ok(texture)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::render_helpers::vulkan::hardware_tests::setup;

    #[test]
    fn upload_damage_is_clipped_disjoint_and_preserves_covered_pixels() {
        let damage = [
            Rectangle::new((-5, -3).into(), (10, 8).into()),
            Rectangle::new((2, 2).into(), (10, 10).into()),
            Rectangle::new((i32::MAX, 0).into(), (i32::MAX, 1).into()),
        ];
        let regions = upload_regions(&damage, 16, 16);
        assert!(regions.len() <= MAX_DAMAGE_REGIONS);
        for (i, region) in regions.iter().enumerate() {
            assert!(Rectangle::from_size((16, 16).into()).contains_rect(*region));
            assert!(regions[i + 1..].iter().all(|r| !region.overlaps(*r)));
        }
        for y in 0..16 {
            for x in 0..16 {
                let point: smithay::utils::Point<i32, Buffer> = (x, y).into();
                // The final deliberately overflowing rectangle is out of bounds.
                assert_eq!(
                    regions.iter().any(|r| r.contains(point)),
                    damage[..2].iter().any(|r| r.contains(point)),
                );
            }
        }
    }

    #[test]
    fn upload_damage_bounds_large_and_fragmented_client_requests() {
        let damage: Vec<_> = (0..10_000)
            .map(|x| Rectangle::new((x, 0).into(), (1, 1).into()))
            .collect();
        assert_eq!(
            upload_regions(&damage, 20_000, 2),
            vec![Rectangle::new((0, 0).into(), (10_000, 1).into())]
        );

        // Fewer input rectangles can still exceed the fragment budget.
        let mut fragmented: Vec<_> = (0..32)
            .map(|x| Rectangle::new((x * 2, 0).into(), (1, 64).into()))
            .collect();
        fragmented.push(Rectangle::new((0, 1).into(), (64, 1).into()));
        fragmented.push(Rectangle::new((0, 3).into(), (64, 1).into()));
        assert_eq!(
            upload_regions(&fragmented, 64, 64),
            vec![Rectangle::from_size((64, 64).into())]
        );
        assert!(upload_regions(&[], 64, 64).is_empty());
        let outside = vec![Rectangle::new((100, 100).into(), (1, 1).into()); 100];
        assert!(upload_regions(&outside, 64, 64).is_empty());
    }

    #[test]
    #[ignore = "requires real DRM render node and Vulkan"]
    fn shm_cache_releases_dead_renderer_without_evicting_live_context() {
        let cache = ShmCache::default();
        let (first_vk, _first_gl, _first_allocator) = setup();
        let mut first = NativeRenderer::new(first_vk).unwrap();
        first.track_shm_cache(&cache);
        let first_texture = first.vulkan().create_solid_texture([255; 4]).unwrap();
        let old_context = Arc::downgrade(&first_texture.inner.ctx);
        cache.insert(
            first.context_id(),
            first.shm_cache_lifetime(),
            first_texture.clone(),
        );
        drop(first_texture);

        let (second_vk, _second_gl, _second_allocator) = setup();
        let mut second = NativeRenderer::new(second_vk).unwrap();
        second.track_shm_cache(&cache);
        let second_id = second.context_id();
        let second_texture = second.vulkan().create_solid_texture([128; 4]).unwrap();
        let live_context = Arc::downgrade(&second_texture.inner.ctx);
        cache.insert(
            second_id.clone(),
            second.shm_cache_lifetime(),
            second_texture.clone(),
        );
        drop(second_texture);
        assert_eq!(cache.0.borrow().len(), 2, "both renderers are active");
        drop(first);
        assert_eq!(cache.0.borrow().len(), 1, "drop removes old entry");
        assert!(old_context.upgrade().is_none(), "old GPU context leaked");
        assert!(cache.get(&second_id).is_some(), "live cache was evicted");

        drop(second);
        cache.prune();
        assert!(cache.0.borrow().is_empty());
        assert!(live_context.upgrade().is_none());
    }
}
