use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::env;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use calloop::channel::Channel;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Physical, Point, Rectangle, Size, Transform};
use smithay::wayland::compositor::with_states;
use xcursor::parser::Image;

#[path = "cursor/worker.rs"]
mod worker;
pub(crate) use worker::CursorLoadCompletion;

/// Some default looking `left_ptr` icon.
static FALLBACK_CURSOR_DATA: &[u8] = include_bytes!("../resources/cursor.rgba");

enum CacheEntry {
    Ready(Option<Rc<XCursor>>),
    Pending,
    Deferred,
}
type XCursorCache = HashMap<(CursorIcon, i32), CacheEntry>;

pub struct CursorManager {
    theme: Arc<worker::ThemeSpec>,
    epoch: u64,
    worker: worker::Worker,
    completions: Option<Channel<CursorLoadCompletion>>,
    fallback: Rc<XCursor>,
    size: u8,
    current_cursor: CursorImageStatus,
    named_cursor_cache: RefCell<XCursorCache>,
}

impl CursorManager {
    pub fn new(theme: &str, size: u8) -> Self {
        Self::ensure_env(theme, size);

        let (worker, completions) = worker::Worker::new();
        Self {
            theme: Arc::new(worker::ThemeSpec::new(theme)),
            epoch: 1,
            worker,
            completions: Some(completions),
            fallback: Rc::new(Self::fallback_cursor()),
            size,
            current_cursor: CursorImageStatus::default_named(),
            named_cursor_cache: Default::default(),
        }
    }

    /// Reload the cursor theme.
    pub fn reload(&mut self, theme: &str, size: u8) {
        Self::ensure_env(theme, size);
        self.epoch = self
            .epoch
            .checked_add(1)
            .expect("cursor theme epoch exhausted");
        self.worker.epoch.store(self.epoch, Ordering::Release);
        self.theme = Arc::new(worker::ThemeSpec::new(theme));
        self.size = size;
        self.named_cursor_cache.get_mut().clear();
    }

    /// Install once in the owner event loop; the receiver contains owned asset results only.
    pub(crate) fn take_completion_source(&mut self) -> Option<Channel<CursorLoadCompletion>> {
        self.completions.take()
    }

    /// Returns whether the visible named cursor may need a redraw. No GPU or desktop state is
    /// touched by the worker. Deferred requests retry only after a completion frees capacity.
    pub(crate) fn apply_completion(&mut self, completion: CursorLoadCompletion) -> bool {
        let cache = self.named_cursor_cache.get_mut();
        let mut deferred = false;
        cache.retain(|_, entry| {
            if matches!(entry, CacheEntry::Deferred) {
                deferred = true;
                false
            } else {
                true
            }
        });
        if completion.epoch != self.epoch {
            return deferred && matches!(self.current_cursor, CursorImageStatus::Named(_));
        }
        let key = (completion.icon, completion.scale);
        if !matches!(cache.get(&key), Some(CacheEntry::Pending)) {
            return deferred && matches!(self.current_cursor, CursorImageStatus::Named(_));
        }
        let cursor = match completion.images {
            Ok(images) if !images.is_empty() => Some(Rc::new(XCursor::new(images))),
            Ok(_) => {
                warn!("cursor worker returned no images");
                None
            }
            Err(error) => {
                warn!(
                    "error loading cursor {:?}@{}: {error}",
                    completion.icon, completion.scale
                );
                None
            }
        };
        // Bound retained decoded assets; texture upload remains owner-thread/cache specific.
        const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
        let bytes = |cursor: &XCursor| {
            cursor
                .images
                .iter()
                .map(|image| image.pixels_rgba.len() + image.pixels_argb.len())
                .sum::<usize>()
        };
        let incoming = cursor.as_deref().map_or(0, &bytes);
        let mut retained = cache
            .values()
            .filter_map(|entry| match entry {
                CacheEntry::Ready(Some(cursor)) => Some(bytes(cursor)),
                _ => None,
            })
            .sum::<usize>();
        if retained + incoming > MAX_CACHE_BYTES {
            cache.retain(|cached_key, entry| {
                if *cached_key != key && retained + incoming > MAX_CACHE_BYTES {
                    if let CacheEntry::Ready(Some(cursor)) = entry {
                        retained -= bytes(cursor);
                        return false;
                    }
                }
                true
            });
        }
        cache.insert(key, CacheEntry::Ready(cursor));
        matches!(self.current_cursor, CursorImageStatus::Named(icon) if deferred || icon == completion.icon || completion.icon == CursorIcon::Default)
    }

    /// Checks if the cursor WlSurface is alive, and if not, cleans it up.
    pub fn check_cursor_image_surface_alive(&mut self) {
        if let CursorImageStatus::Surface(surface) = &self.current_cursor {
            if !surface.alive() {
                self.current_cursor = CursorImageStatus::default_named();
            }
        }
    }

    /// Get the current rendering cursor.
    pub fn get_render_cursor(&self, scale: i32) -> RenderCursor {
        match self.current_cursor.clone() {
            CursorImageStatus::Hidden => RenderCursor::Hidden,
            CursorImageStatus::Surface(surface) => {
                let hotspot = with_states(&surface, |states| {
                    states
                        .data_map
                        .get::<CursorImageSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .hotspot
                });

                RenderCursor::Surface { hotspot, surface }
            }
            CursorImageStatus::Named(icon) => self.get_render_cursor_named(icon, scale),
        }
    }

    fn get_render_cursor_named(&self, icon: CursorIcon, scale: i32) -> RenderCursor {
        self.get_cursor_with_name(icon, scale)
            .map(|cursor| RenderCursor::Named {
                icon,
                scale,
                cursor,
            })
            .unwrap_or_else(|| RenderCursor::Named {
                icon: Default::default(),
                scale,
                cursor: self.get_default_cursor(scale),
            })
    }

    pub fn current_cursor_animation_bounds(&self, scale: i32) -> Option<Rectangle<f64, Logical>> {
        match &self.current_cursor {
            CursorImageStatus::Hidden | CursorImageStatus::Surface(_) => None,
            CursorImageStatus::Named(icon) => {
                let cursor = self
                    .get_cursor_with_name(*icon, scale)
                    .unwrap_or_else(|| self.get_default_cursor(scale));
                cursor
                    .is_animated_cursor()
                    .then(|| cursor.animation_bounds(scale))
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn set_named_cursor_for_test(&mut self, scale: i32, images: Vec<Image>) {
        let icon = CursorIcon::Wait;
        self.named_cursor_cache.get_mut().insert(
            (icon, scale),
            CacheEntry::Ready(Some(Rc::new(XCursor::new(images)))),
        );
        self.current_cursor = CursorImageStatus::Named(icon);
    }

    /// Get named cursor for the given `icon` and `scale`.
    pub fn get_cursor_with_name(&self, icon: CursorIcon, scale: i32) -> Option<Rc<XCursor>> {
        let mut cache = self.named_cursor_cache.borrow_mut();
        match cache.get(&(icon, scale)) {
            Some(CacheEntry::Ready(cursor)) => return cursor.clone(),
            Some(CacheEntry::Pending | CacheEntry::Deferred) => return None,
            None => (),
        }
        let size = u32::try_from(scale)
            .ok()
            .and_then(|scale| u32::from(self.size).checked_mul(scale))?;
        // Named cursor enum and finite output scales bound normal keys; retain a defensive cap
        // for temporary output-scale churn. Pending jobs still have their completion identity.
        if cache.len() >= 256 {
            let key = cache
                .iter()
                .find_map(|(key, value)| matches!(value, CacheEntry::Ready(_)).then_some(*key))?;
            cache.remove(&key);
        }
        let request = worker::Request {
            epoch: self.epoch,
            icon,
            scale,
            size,
            theme: self.theme.clone(),
        };
        let entry = if self.worker.request(request).is_ok() {
            CacheEntry::Pending
        } else {
            CacheEntry::Deferred
        };
        cache.insert((icon, scale), entry);
        None
    }

    /// Get default cursor.
    pub fn get_default_cursor(&self, scale: i32) -> Rc<XCursor> {
        // The default cursor always has a fallback.
        self.get_cursor_with_name(CursorIcon::Default, scale)
            .unwrap_or_else(|| self.fallback.clone())
    }

    /// Currently used cursor_image as a cursor provider.
    pub fn cursor_image(&self) -> &CursorImageStatus {
        &self.current_cursor
    }

    /// Set new cursor image provider.
    pub fn set_cursor_image(&mut self, cursor: CursorImageStatus) {
        self.current_cursor = cursor;
    }

    /// Set the common XCURSOR env variables.
    fn ensure_env(theme: &str, size: u8) {
        env::set_var("XCURSOR_THEME", theme);
        env::set_var("XCURSOR_SIZE", size.to_string());
    }

    fn fallback_cursor() -> XCursor {
        let images = vec![Image {
            size: 32,
            width: 64,
            height: 64,
            xhot: 1,
            yhot: 1,
            delay: 0,
            pixels_rgba: Vec::from(FALLBACK_CURSOR_DATA),
            pixels_argb: vec![],
        }];

        XCursor::new(images)
    }
}

#[cfg(test)]
mod asset_tests {
    use super::*;
    fn completion(
        manager: &CursorManager,
        icon: CursorIcon,
        scale: i32,
        images: Result<Vec<Image>, String>,
    ) -> CursorLoadCompletion {
        CursorLoadCompletion {
            epoch: manager.epoch,
            icon,
            scale,
            images,
        }
    }
    #[test]
    fn pending_asset_has_immediate_embedded_fallback_and_owner_completion_preserves_pixels() {
        let mut manager = CursorManager::new("clarion-test-missing-theme", 32);
        manager.set_cursor_image(CursorImageStatus::Named(CursorIcon::Wait));
        assert!(manager.get_cursor_with_name(CursorIcon::Wait, 2).is_none());
        let first = manager.get_default_cursor(2);
        assert_eq!(first.frames()[0].pixels_rgba, FALLBACK_CURSOR_DATA);
        for _ in 0..20 {
            assert!(manager.get_cursor_with_name(CursorIcon::Wait, 2).is_none());
        }
        let mut image = CursorManager::fallback_cursor().images.remove(0);
        image.delay = 21;
        image.xhot = 3;
        image.pixels_rgba[0] = 172;
        let result = completion(&manager, CursorIcon::Wait, 2, Ok(vec![image]));
        assert!(manager.apply_completion(result));
        let current = manager.get_cursor_with_name(CursorIcon::Wait, 2).unwrap();
        assert_eq!(current.frames()[0].pixels_rgba[0], 172);
        assert_eq!(
            (current.frames()[0].xhot, current.frames()[0].delay),
            (3, 21)
        );
    }
    #[test]
    fn negative_cache_and_empty_completion_keep_valid_fallback_without_per_frame_requests() {
        let mut manager = CursorManager::new("clarion-test-missing-theme", 32);
        manager.set_cursor_image(CursorImageStatus::Named(CursorIcon::Wait));
        manager.get_cursor_with_name(CursorIcon::Wait, 1);
        let failed = completion(&manager, CursorIcon::Wait, 1, Err("absent".into()));
        assert!(manager.apply_completion(failed));
        for _ in 0..20 {
            assert!(manager.get_cursor_with_name(CursorIcon::Wait, 1).is_none());
        }
        assert!(matches!(
            manager
                .named_cursor_cache
                .borrow()
                .get(&(CursorIcon::Wait, 1)),
            Some(CacheEntry::Ready(None))
        ));
        manager.get_cursor_with_name(CursorIcon::Default, 1);
        let empty = completion(&manager, CursorIcon::Default, 1, Ok(Vec::new()));
        assert!(manager.apply_completion(empty));
        assert_eq!(manager.get_default_cursor(1).frames().len(), 1);
    }
    #[test]
    fn theme_epoch_rejects_old_pixels_and_only_current_pending_key_can_publish() {
        let mut manager = CursorManager::new("old", 32);
        manager.get_cursor_with_name(CursorIcon::Wait, 1);
        let old = completion(
            &manager,
            CursorIcon::Wait,
            1,
            Ok(CursorManager::fallback_cursor().images),
        );
        manager.reload("new", 48);
        assert!(!manager.apply_completion(old));
        assert!(manager.get_cursor_with_name(CursorIcon::Wait, 1).is_none());
        let unknown = completion(
            &manager,
            CursorIcon::Wait,
            2,
            Ok(CursorManager::fallback_cursor().images),
        );
        assert!(!manager.apply_completion(unknown));
    }
    #[test]
    fn invisible_completion_updates_negative_cache_without_requesting_redraw() {
        let mut manager = CursorManager::new("missing", 32);
        manager.get_cursor_with_name(CursorIcon::Wait, 1);
        manager.set_cursor_image(CursorImageStatus::Hidden);
        let result = completion(&manager, CursorIcon::Wait, 1, Err("absent".into()));
        assert!(!manager.apply_completion(result));
        assert!(matches!(
            manager
                .named_cursor_cache
                .borrow()
                .get(&(CursorIcon::Wait, 1)),
            Some(CacheEntry::Ready(None))
        ));
    }
    #[test]
    fn retired_theme_completion_retries_deferred_new_theme_without_old_pixels() {
        let mut manager = CursorManager::new("old", 32);
        let old = completion(
            &manager,
            CursorIcon::Wait,
            1,
            Ok(CursorManager::fallback_cursor().images),
        );
        manager.reload("new", 48);
        manager
            .named_cursor_cache
            .get_mut()
            .insert((CursorIcon::Default, 1), CacheEntry::Deferred);
        assert!(manager.apply_completion(old));
        assert!(!manager
            .named_cursor_cache
            .borrow()
            .contains_key(&(CursorIcon::Default, 1)));
        assert!(manager.get_cursor_with_name(CursorIcon::Wait, 1).is_none());
    }
}

/// The cursor prepared for renderer.
pub enum RenderCursor {
    Hidden,
    Surface {
        hotspot: Point<i32, Logical>,
        surface: WlSurface,
    },
    Named {
        icon: CursorIcon,
        scale: i32,
        cursor: Rc<XCursor>,
    },
}

type TextureCache = HashMap<(CursorIcon, i32), Vec<MemoryRenderBuffer>>;

#[derive(Default)]
pub struct CursorTextureCache {
    cache: RefCell<TextureCache>,
}

impl CursorTextureCache {
    pub fn clear(&mut self) {
        self.cache.get_mut().clear();
    }

    pub fn get(
        &self,
        icon: CursorIcon,
        scale: i32,
        cursor: &XCursor,
        idx: usize,
    ) -> MemoryRenderBuffer {
        self.cache
            .borrow_mut()
            .entry((icon, scale))
            .or_insert_with(|| {
                cursor
                    .frames()
                    .iter()
                    .map(|frame| {
                        MemoryRenderBuffer::from_slice(
                            &frame.pixels_rgba,
                            Fourcc::Argb8888,
                            (frame.width as i32, frame.height as i32),
                            scale,
                            Transform::Normal,
                            None,
                        )
                    })
                    .collect()
            })[idx]
            .clone()
    }
}

// The XCursorBuffer implementation is inspired by `wayland-rs`, thus provided under MIT license.

/// The state of the `NamedCursor`.
pub struct XCursor {
    /// The image for the underlying named cursor.
    images: Vec<Image>,
    /// The total duration of the animation.
    animation_duration: u32,
    /// Bounds relative to the pointer, cached for the requested cursor scale.
    animation_bounds: Cell<Option<(i32, Rectangle<f64, Logical>)>>,
}

impl XCursor {
    fn new(images: Vec<Image>) -> Self {
        let animation_duration = images
            .iter()
            .fold(0u32, |acc, image| acc.saturating_add(image.delay));
        Self {
            images,
            animation_duration,
            animation_bounds: Cell::new(None),
        }
    }

    fn animation_bounds(&self, scale: i32) -> Rectangle<f64, Logical> {
        if let Some((cached_scale, bounds)) = self.animation_bounds.get() {
            if cached_scale == scale {
                return bounds;
            }
        }

        // Use every frame: a later frame may cross an output edge even when the
        // currently displayed one does not. Match render_pointer's integer
        // hotspot conversion before accounting for the output's fractional scale.
        let bounds = self
            .images
            .iter()
            .map(|image| {
                let location = Self::hotspot(image).to_logical(scale).upscale(-1).to_f64();
                let size = Size::<f64, Physical>::from((image.width as f64, image.height as f64))
                    .to_logical(scale as f64);
                Rectangle::new(location, size)
            })
            .reduce(|bounds, frame| bounds.merge(frame))
            .unwrap();
        self.animation_bounds.set(Some((scale, bounds)));
        bounds
    }

    /// Given a time, calculate which frame to show, and how much time remains until the next frame.
    ///
    /// Time will wrap, so if for instance the cursor has an animation lasting 100ms,
    /// then calling this function with 5ms and 105ms as input gives the same output.
    pub fn frame(&self, mut millis: u32) -> (usize, &Image) {
        if self.animation_duration == 0 {
            return (0, &self.images[0]);
        }

        millis %= self.animation_duration;

        let mut res = 0;
        for (i, img) in self.images.iter().enumerate() {
            if millis < img.delay {
                res = i;
                break;
            }
            millis -= img.delay;
        }

        (res, &self.images[res])
    }

    /// Get the frames for the given `XCursor`.
    pub fn frames(&self) -> &[Image] {
        &self.images
    }

    /// Check whether the cursor is animated.
    pub fn is_animated_cursor(&self) -> bool {
        self.images.len() > 1
    }

    /// Get hotspot for the given `image`.
    pub fn hotspot(image: &Image) -> Point<i32, Physical> {
        (image.xhot as i32, image.yhot as i32).into()
    }
}
