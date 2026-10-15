use std::sync::{Arc, Mutex};

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::background_effect::{
    self, BackgroundEffectSurfaceCachedState, ExtBackgroundEffectHandler,
};
use smithay::wayland::compositor::{
    add_post_commit_hook, with_states, Cacheable, RegionAttributes, SurfaceData,
};

use crate::compositor::State;
use crate::utils::region::region_to_non_overlapping_rects;

/// Per-surface cache for processed blur region (non-overlapping rects).
#[derive(Default)]
struct CachedBlurRegionUserData(Mutex<CachedBlurRegionInner>);

/// Travel with the protocol state through acquire fences and synchronized
/// subsurfaces. An older blocked commit must not consume a newer mask change.
#[derive(Default)]
struct BlurRegionChanged(bool);

impl Cacheable for BlurRegionChanged {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        Self(std::mem::take(&mut self.0))
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        into.0 |= self.0;
    }
}

#[derive(Default)]
struct CachedBlurRegionInner {
    /// Whether the region must be recomputed.
    dirty: bool,
    /// Whether the post-commit hook has been registered for this surface.
    hook_registered: bool,
    /// Cached non-overlapping rects in surface-local coordinates.
    ///
    /// `None` means there's no blur region.
    rects: Option<Arc<Vec<Rectangle<i32, Logical>>>>,
}

/// Gets the cached blur region for a surface, lazily recomputing if dirty.
pub fn get_cached_blur_region(states: &SurfaceData) -> Option<Arc<Vec<Rectangle<i32, Logical>>>> {
    let cache = states
        .data_map
        .get_or_insert_threadsafe(CachedBlurRegionUserData::default);
    let mut guard = cache.0.lock().unwrap();

    if guard.dirty {
        guard.dirty = false;
        recompute_blur_region(states, &mut guard);
    }

    guard.rects.clone()
}

fn recompute_blur_region(states: &SurfaceData, inner: &mut CachedBlurRegionInner) {
    let cached = &states.cached_state;

    let rects = if let Some(arc) = &mut inner.rects {
        if Arc::strong_count(arc) > 1 {
            debug!("cloning rects due to non-unique reference");
        }
        arc
    } else {
        inner.rects.insert(Arc::new(Vec::new()))
    };
    let rects = Arc::make_mut(rects);

    if cached.has::<BackgroundEffectSurfaceCachedState>() {
        let mut guard = cached.get::<BackgroundEffectSurfaceCachedState>();
        if let Some(region) = &guard.current().blur_region {
            region_to_non_overlapping_rects(region, rects);
        } else {
            inner.rects = None;
        }
        return;
    }

    inner.rects = None;
}

fn mark_blur_region_pending_dirty(wl_surface: &WlSurface) {
    let register_hook = with_states(wl_surface, |states| {
        states.cached_state.get::<BlurRegionChanged>().pending().0 = true;
        let cache = states
            .data_map
            .get_or_insert_threadsafe(CachedBlurRegionUserData::default);
        let mut guard = cache.0.lock().unwrap();
        if guard.hook_registered {
            false
        } else {
            guard.hook_registered = true;
            true
        }
    });

    if register_hook {
        add_post_commit_hook::<State, _>(wl_surface, |_state, _dh, surface| {
            with_states(surface, |states| {
                let changed =
                    std::mem::take(&mut states.cached_state.get::<BlurRegionChanged>().current().0);
                if !changed {
                    return;
                }
                if let Some(cache) = states.data_map.get::<CachedBlurRegionUserData>() {
                    let mut guard = cache.0.lock().unwrap();
                    guard.dirty = true;

                    crate::render_helpers::background_effect::damage_surface(states);
                } else {
                    error!("unexpected missing CachedBlurRegionUserData");
                }
            });
        });
    }
}

impl ExtBackgroundEffectHandler for State {
    fn capabilities(&self) -> background_effect::Capability {
        background_effect::Capability::Blur
    }

    fn set_blur_region(&mut self, wl_surface: WlSurface, _region: RegionAttributes) {
        mark_blur_region_pending_dirty(&wl_surface);
    }

    fn unset_blur_region(&mut self, wl_surface: WlSurface) {
        mark_blur_region_pending_dirty(&wl_surface);
    }
}
