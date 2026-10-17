//! Scene-owned retained window pixels, independent of layout payload ownership.
//!
//! A platform instance owns the registry. Layout adapters hold weak allocation
//! capabilities, and each tile holds only a stable registration key plus a weak
//! registry reference. Moving or parking a tile keeps its registration; dropping
//! it removes the entry. Temporary render leases keep an in-progress operation
//! valid without making a tile an owner of any retained GPU cache.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Deref;
use std::rc::{Rc, Weak};

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size};

use crate::layout::LayoutElementRenderSnapshot;
use crate::render_helpers::offscreen::{NativeOffscreenBuffer, OffscreenBuffer};
use crate::render_helpers::resize::CapturedResizeTexture;
use crate::render_helpers::{RenderCtx, RenderTarget};
use crate::scene::tile::TileRenderSnapshot;

/// A cache pair belongs to the scene even while a layout's numeric animation
/// reverses or is replaced. Neither backend shares cached pixels with the other.
#[derive(Debug, Default)]
pub struct AnimationBuffers {
    pub(crate) gles: OffscreenBuffer,
    pub(crate) native: NativeOffscreenBuffer,
}

/// A close-copy source either retains the exact Wayland attachment, or keeps
/// the original synchronous contract of the raw DMA-BUF API.
#[derive(Debug)]
pub enum NativeCloseSource {
    Owned(crate::render_helpers::vulkan::native::OwnedDmabufSource),
    Unowned(smithay::backend::allocator::dmabuf::Dmabuf),
}

impl NativeCloseSource {
    pub(crate) fn dmabuf(&self) -> &smithay::backend::allocator::dmabuf::Dmabuf {
        match self {
            Self::Owned(source) => source.dmabuf(),
            Self::Unowned(dmabuf) => dmabuf,
        }
    }
}

impl AnimationBuffers {
    pub(crate) fn clear(&self) {
        self.gles.clear();
        self.native.clear();
    }
}

#[derive(Debug)]
pub(crate) struct ResizeImages {
    pub(crate) snapshot: ResizeSnapshot,
    pub(crate) buffers: AnimationBuffers,
}

/// Scene failures request numeric-effect retirement from the layout owner.
/// Repeated failures coalesce until the next owner animation update.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EffectRetirements {
    pub(crate) wobble: bool,
    pub(crate) genie: bool,
}

impl EffectRetirements {
    pub(crate) fn any(self) -> bool {
        self.wobble || self.genie
    }
}

/// Retained GPU images only. Desktop identity, layout decisions, animation
/// trajectories and protocol policy are deliberately absent. Frozen elements
/// retain the client-buffer leases needed by their eventual GPU submissions.
#[derive(Debug, Default)]
pub(crate) struct WindowRenderResources {
    pub(crate) effect_retirements: Cell<EffectRetirements>,
    effect_wakeup: Weak<Cell<bool>>,
    pub(crate) decorations: RefCell<crate::scene::window_decorations::WindowDecorations>,
    pub(crate) wobble_forecast: RefCell<crate::scene::wobble::WobbleForecast>,
    pub(crate) genie_mesh: RefCell<crate::scene::genie::GenieCache>,
    pub(crate) open: AnimationBuffers,
    pub(crate) alpha: AnimationBuffers,
    pub(crate) wobble: AnimationBuffers,
    pub(crate) genie: [AnimationBuffers; RenderTarget::COUNT],
    pub(crate) resize: RefCell<Option<ResizeImages>>,
    pub(crate) pending_resize: RefCell<Option<(ResizeSnapshot, Option<ResizeSnapshot>)>>,
    pub(crate) unmap: RefCell<Option<TileRenderSnapshot>>,
    pub(crate) native_unmap: RefCell<Option<NativeTileSnapshot>>,
    /// Captured in pre-commit even if the complete three-target snapshot fails.
    /// It survives renderer replacement: the lease has no renderer context.
    pub(crate) native_close_source:
        RefCell<Option<crate::render_helpers::vulkan::native::OwnedDmabufSource>>,
}

impl WindowRenderResources {
    pub(crate) fn request_effect_retirement(&self, request: EffectRetirements) {
        if request.any() {
            if let Some(wakeup) = self.effect_wakeup.upgrade() {
                wakeup.set(true);
            }
        }
        let pending = self.effect_retirements.get();
        self.effect_retirements.set(EffectRetirements {
            wobble: pending.wobble || request.wobble,
            genie: pending.genie || request.genie,
        });
    }

    pub(crate) fn take_effect_retirements(&self) -> EffectRetirements {
        self.effect_retirements.take()
    }

    pub(crate) fn clear_wobble_retirement(&self) {
        self.effect_retirements.set(EffectRetirements {
            wobble: false,
            ..self.effect_retirements.get()
        });
    }

    pub(crate) fn clear_genie_retirement(&self) {
        self.effect_retirements.set(EffectRetirements {
            genie: false,
            ..self.effect_retirements.get()
        });
    }

    /// Retire stale native images while leaving independent GLES caches alive.
    /// Submitted elements retain their own GPU completion leases.
    pub(crate) fn clear_native(&self) {
        self.resize.borrow_mut().take();
        self.pending_resize.borrow_mut().take();
        self.native_unmap.borrow_mut().take();
        self.open.native.clear();
        self.alpha.native.clear();
        self.wobble.native.clear();
        for buffers in &self.genie {
            buffers.native.clear();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct WindowResourceKey(u64);

#[derive(Debug, Default)]
struct RegistryInner {
    next_key: Cell<u64>,
    effect_wakeup: Rc<Cell<bool>>,
    entries: RefCell<HashMap<WindowResourceKey, Rc<WindowRenderResources>>>,
}

/// Strong scene-resource owner for one compositor/platform instance.
/// It intentionally does not implement Clone; adapters receive weak allocators.
#[derive(Debug, Default)]
pub struct WindowResourceRegistry {
    inner: Rc<RegistryInner>,
}

impl WindowResourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn take_effect_wakeup(&self) -> bool {
        self.inner.effect_wakeup.replace(false)
    }

    pub fn allocator(&self) -> WindowResourceAllocator {
        WindowResourceAllocator {
            owner: Rc::downgrade(&self.inner),
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner.entries.borrow().len()
    }
}

#[derive(Debug, Clone)]
pub struct WindowResourceAllocator {
    owner: Weak<RegistryInner>,
}

impl WindowResourceAllocator {
    #[cfg(test)]
    pub(crate) fn owner_is_alive(&self) -> bool {
        self.owner.strong_count() > 0
    }

    pub fn allocate(&self) -> WindowResourceHandle {
        let owner = self.owner.upgrade().expect("scene resource owner is alive");
        let next = owner
            .next_key
            .get()
            .checked_add(1)
            .expect("scene resource keys exhausted");
        owner.next_key.set(next);
        let key = WindowResourceKey(next);
        let previous = owner.entries.borrow_mut().insert(
            key,
            Rc::new(WindowRenderResources {
                effect_wakeup: Rc::downgrade(&owner.effect_wakeup),
                ..WindowRenderResources::default()
            }),
        );
        debug_assert!(previous.is_none());
        WindowResourceHandle {
            key,
            owner: self.owner.clone(),
        }
    }
}

/// Sole registration token, containing no strong resource or registry reference.
/// Non-Clone prevents an old payload drop from removing a newer live token.
#[derive(Debug)]
pub struct WindowResourceHandle {
    key: WindowResourceKey,
    owner: Weak<RegistryInner>,
}

impl WindowResourceHandle {
    #[cfg(test)]
    pub(crate) fn registration_key(&self) -> u64 {
        self.key.0
    }

    pub(crate) fn lease(&self) -> WindowResourceLease {
        let owner = self.owner.upgrade().expect("scene resource owner is alive");
        let resources = owner
            .entries
            .borrow()
            .get(&self.key)
            .cloned()
            .expect("window is registered with the scene");
        WindowResourceLease(resources)
    }
}

impl Drop for WindowResourceHandle {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            // Resource destructors can retire other handles. Never invoke them
            // while holding the registry's RefCell borrow.
            let removed = owner.entries.borrow_mut().remove(&self.key);
            drop(removed);
        }
    }
}

/// Temporary access for one synchronous update/render, never stored in a tile.
#[derive(Debug)]
pub(crate) struct WindowResourceLease(Rc<WindowRenderResources>);

#[cfg(test)]
impl WindowResourceLease {
    pub(crate) fn liveness_probe(&self) -> Weak<WindowRenderResources> {
        Rc::downgrade(&self.0)
    }
}

impl Deref for WindowResourceLease {
    type Target = WindowRenderResources;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A cached texture together with the physical rectangle it occupies and its logical size.
///
/// Keeping this shape behind a name makes the GLES and native resize paths easier to read while
/// ensuring they continue to return the same data to their callers.
type ResizeTextureInput<T> = ((T, Rectangle<i32, Physical>), Size<f64, Logical>);

/// Separate snapshots preserve each capture target's privacy rules during closing.
#[derive(Debug)]
pub struct NativeTileSnapshot {
    pub targets: [(
        crate::render_helpers::vulkan::VulkanTexture,
        Point<f64, Logical>,
    ); 3],
}

#[derive(Debug)]
pub(crate) enum ResizeSnapshot {
    Gles(LayoutElementRenderSnapshot),
    Native(LayoutElementRenderSnapshot<crate::render_helpers::vulkan::VulkanTexture>),
    CompositedGles(
        [(
            CapturedResizeTexture<smithay::backend::renderer::gles::GlesTexture>,
            bool,
        ); RenderTarget::COUNT],
    ),
    CompositedNative(
        [(
            CapturedResizeTexture<crate::render_helpers::vulkan::VulkanTexture>,
            bool,
        ); RenderTarget::COUNT],
    ),
}
impl ResizeSnapshot {
    pub(crate) fn size(&self) -> Size<f64, Logical> {
        match self {
            Self::Gles(s) => s.size,
            Self::Native(s) => s.size,
            Self::CompositedGles(s) => s[0].0.size(),
            Self::CompositedNative(s) => s[0].0.size(),
        }
    }

    pub(crate) fn blocked_out(&self, target: RenderTarget) -> bool {
        match self {
            Self::Gles(s) => target.should_block_out(s.block_out_from),
            Self::Native(s) => target.should_block_out(s.block_out_from),
            Self::CompositedGles(s) => s[target as usize].1,
            Self::CompositedNative(s) => s[target as usize].1,
        }
    }

    pub(crate) fn gles_texture(
        &self,
        renderer: &mut GlesRenderer,
        target: RenderTarget,
        scale: Scale<f64>,
    ) -> Option<ResizeTextureInput<smithay::backend::renderer::gles::GlesTexture>> {
        match self {
            Self::Gles(s) => {
                let texture = s
                    .texture(
                        RenderCtx {
                            renderer,
                            target,
                            xray: None,
                        },
                        scale,
                    )?
                    .clone();
                let cached_scale = s.texture_scale(target)?;
                let size = Size::from((
                    s.size.w * cached_scale.x / scale.x,
                    s.size.h * cached_scale.y / scale.y,
                ));
                Some((texture, size))
            }
            Self::CompositedGles(s) => Some(s[target as usize].0.as_input(scale)),
            _ => None,
        }
    }

    pub(crate) fn native_texture(
        &self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        target: RenderTarget,
        scale: Scale<f64>,
    ) -> Option<ResizeTextureInput<crate::render_helpers::vulkan::VulkanTexture>> {
        match self {
            Self::Native(s) => {
                let texture = s
                    .texture(
                        RenderCtx {
                            renderer,
                            target,
                            xray: None,
                        },
                        scale,
                    )?
                    .clone();
                let cached_scale = s.texture_scale(target)?;
                let size = Size::from((
                    s.size.w * cached_scale.x / scale.x,
                    s.size.h * cached_scale.y / scale.y,
                ));
                Some((texture, size))
            }
            Self::CompositedNative(s) => Some(s[target as usize].0.as_input(scale)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_helpers::snapshot::RenderSnapshot;

    #[test]
    fn effect_retirement_requests_coalesce_until_the_owner_takes_them() {
        let resources = WindowRenderResources::default();
        for _ in 0..3 {
            resources.request_effect_retirement(EffectRetirements {
                wobble: true,
                genie: false,
            });
        }
        resources.request_effect_retirement(EffectRetirements {
            wobble: false,
            genie: true,
        });
        assert_eq!(
            resources.take_effect_retirements(),
            EffectRetirements {
                wobble: true,
                genie: true
            },
        );
        assert_eq!(
            resources.take_effect_retirements(),
            EffectRetirements::default()
        );
    }

    #[test]
    fn owner_cleanup_preserves_another_pending_effect_retirement() {
        let resources = WindowRenderResources::default();
        let both = EffectRetirements {
            wobble: true,
            genie: true,
        };
        resources.request_effect_retirement(both);
        resources.clear_wobble_retirement();
        assert_eq!(
            resources.effect_retirements.get(),
            EffectRetirements {
                wobble: false,
                genie: true
            }
        );
        resources.request_effect_retirement(both);
        resources.clear_genie_retirement();
        assert_eq!(
            resources.effect_retirements.get(),
            EffectRetirements {
                wobble: true,
                genie: false
            }
        );
    }

    fn empty_snapshot<C, B>() -> RenderSnapshot<C, B> {
        RenderSnapshot {
            contents: Vec::new(),
            contents_with_blocked_out_bg: None,
            blocked_out_contents: Vec::new(),
            block_out_from: None,
            size: Size::from((320., 240.)),
            texture: Default::default(),
            texture_with_blocked_out_bg: Default::default(),
            blocked_out_texture: Default::default(),
        }
    }

    #[test]
    fn weak_tokens_and_allocators_do_not_keep_the_scene_owner_alive() {
        let owner = WindowResourceRegistry::new();
        let allocator = owner.allocator();
        let token = allocator.allocate();
        let lease = token.lease();
        let resources = Rc::downgrade(&lease.0);
        drop(lease);
        assert!(resources.upgrade().is_some());
        drop(owner);
        assert!(allocator.owner.upgrade().is_none());
        assert!(token.owner.upgrade().is_none());
        assert!(resources.upgrade().is_none());
        // Dropping parked or detached payloads after platform teardown is safe.
        drop(token);
    }

    #[test]
    fn removing_a_window_unregisters_it_but_preserves_an_in_progress_render_lease() {
        let owner = WindowResourceRegistry::new();
        let token = owner.allocator().allocate();
        let lease = token.lease();
        let resources = Rc::downgrade(&lease.0);
        assert_eq!(owner.len(), 1);
        drop(token);
        assert_eq!(owner.len(), 0);
        assert!(resources.upgrade().is_some());
        drop(lease);
        assert!(resources.upgrade().is_none());
    }

    #[test]
    fn transferring_or_parking_a_token_preserves_its_key_without_allocating() {
        fn transfer(token: WindowResourceHandle) -> WindowResourceHandle {
            token
        }
        let owner = WindowResourceRegistry::new();
        let token = owner.allocator().allocate();
        let key = token.key;
        *token.lease().unmap.borrow_mut() = Some(empty_snapshot());
        let mut parked = Some(transfer(token));
        assert_eq!(owner.len(), 1);
        let restored = parked.take().unwrap();
        assert!(parked.is_none());
        assert_eq!(restored.key, key);
        assert!(restored.lease().unmap.borrow().is_some());
        drop(restored);
        assert_eq!(owner.len(), 0);
    }

    #[test]
    fn retired_keys_are_never_reused_or_shared_between_platform_instances() {
        let owner = WindowResourceRegistry::new();
        let first = owner.allocator().allocate();
        let retired = first.key;
        drop(first);
        let current = owner.allocator().allocate();
        assert_ne!(current.key, retired);

        let another = WindowResourceRegistry::new();
        let other = another.allocator().allocate();
        drop(current);
        assert_eq!(owner.len(), 0);
        assert_eq!(another.len(), 1);
        assert!(other.lease().unmap.borrow().is_none());
    }

    #[test]
    fn taken_closing_snapshot_outlives_the_window_registration() {
        let owner = WindowResourceRegistry::new();
        let token = owner.allocator().allocate();
        let lease = token.lease();
        *lease.unmap.borrow_mut() = Some(empty_snapshot());
        let frozen = lease.unmap.borrow_mut().take().unwrap();
        drop(lease);
        drop(token);
        assert_eq!(owner.len(), 0);
        // Production frozen elements keep their texture/client-buffer leases.
        // Unregistering a tile cannot invalidate the transferred snapshot.
        assert_eq!(frozen.size, Size::from((320., 240.)));
        assert!(frozen.contents.is_empty());
    }

    #[test]
    fn native_epoch_reset_discards_resize_targets_without_touching_gles_unmap() {
        let owner = WindowResourceRegistry::new();
        let token = owner.allocator().allocate();
        let lease = token.lease();
        *lease.unmap.borrow_mut() = Some(empty_snapshot());
        *lease.pending_resize.borrow_mut() = Some((ResizeSnapshot::Gles(empty_snapshot()), None));
        lease.clear_native();
        assert!(lease.pending_resize.borrow().is_none());
        assert!(lease.resize.borrow().is_none());
        assert!(lease.native_unmap.borrow().is_none());
        assert!(lease.unmap.borrow().is_some());
    }
}
