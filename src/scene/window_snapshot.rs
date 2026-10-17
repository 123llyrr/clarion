//! Scene-owned window capture and close preparation.
//!
//! Capture consumes the same restricted projection as display composition; it
//! never receives a layout tile, workspace, or desktop owner. Resource handles
//! identify retained pixels independently of the lifetime of a layout payload.

use std::cell::RefCell;
use std::rc::Rc;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Point, Scale, Transform};

use super::tile::{TileRenderSnapshot, TileScene};
use super::window_resources::{NativeCloseSource, NativeTileSnapshot, WindowResourceHandle};
use crate::layout::LayoutElement;
use crate::render_helpers::effect_buffer::EffectBuffer;
use crate::render_helpers::renderer::AnyRenderer;
use crate::render_helpers::snapshot::RenderSnapshot;
use crate::render_helpers::vulkan::native::{NativeBusy, NativeError, NativeRenderer};
use crate::render_helpers::xray::{Xray, XrayPos};
use crate::render_helpers::{render_to_encompassing_texture, RenderCtx, RenderTarget};

/// Transfer an existing pre-unmap capture, or capture the projected live scene.
pub(crate) fn snapshot_native<W: LayoutElement>(
    scene: &TileScene<'_, W>,
    resources: &WindowResourceHandle,
    renderer: &mut NativeRenderer,
) -> anyhow::Result<NativeTileSnapshot> {
    if let Some(snapshot) = resources.lease().native_unmap.borrow_mut().take() {
        return Ok(snapshot);
    }
    capture_native(scene, renderer, None, XrayPos::default())
}

/// Keep each privacy target independent, including its backdrop and geometry.
///
/// Native submissions retain the exact Wayland buffer attachments they sample.
/// The renderer still provides its synchronous fallback for unowned sources;
/// moving capture into the scene does not relax that lifetime contract.
pub(crate) fn capture_native<W: LayoutElement>(
    scene: &TileScene<'_, W>,
    renderer: &mut NativeRenderer,
    xray: Option<&Xray>,
    xray_pos: XrayPos,
) -> anyhow::Result<NativeTileSnapshot> {
    // Do not reset the outer output's retry state. A local capture scope records
    // only this capture's preparation and merges both flags when it is dropped.
    let mut renderer = renderer.capture_scope();
    let scale = Scale::from(scene.scale());
    let mut targets = Vec::with_capacity(3);
    for target in [
        RenderTarget::Output,
        RenderTarget::Screencast,
        RenderTarget::ScreenCapture,
    ] {
        let mut elements = Vec::new();
        scene.render(
            RenderCtx {
                renderer: &mut *renderer,
                target,
                xray,
            },
            Point::from((0., 0.)),
            xray_pos,
            false,
            &mut |elem| elements.push(elem),
        );
        // A deferred TileScene can intentionally push no elements. Capturing
        // that as a successful empty texture would suppress the retained
        // pre-commit source fallback and lose the closing window's pixels.
        if renderer.scene_was_deferred() {
            return Err(NativeError::Busy(NativeBusy::Frame).into());
        }
        let (texture, _sync, geo) = render_to_encompassing_texture(
            &mut *renderer,
            scale,
            Transform::Normal,
            Fourcc::Abgr8888,
            &elements,
        )?;
        if renderer.scene_was_deferred() {
            return Err(NativeError::Busy(NativeBusy::Frame).into());
        }
        targets.push((texture, geo.loc.to_f64().to_logical(scale)));
    }
    Ok(NativeTileSnapshot {
        targets: targets.try_into().unwrap(),
    })
}

/// Pin the attachment while the pre-commit hook still sees its old Buffer.
/// This must happen before any fallible rendering, including Busy or a renderer
/// epoch change that prevents a complete three-target snapshot.
pub(crate) fn store_native_unmap_if_empty<W: LayoutElement>(
    scene: &TileScene<'_, W>,
    resources: &WindowResourceHandle,
    renderer: &mut NativeRenderer,
    xray: Option<&mut Xray>,
    _xray_has_blocked_out_layers: bool,
    xray_pos: XrayPos,
) -> bool {
    let lease = resources.lease();
    if lease.native_close_source.borrow().is_none() {
        let source = scene.window().native_dmabuf_source();
        *lease.native_close_source.borrow_mut() = source;
    }
    if lease.native_unmap.borrow().is_none() {
        match capture_native(scene, renderer, xray.as_deref(), xray_pos) {
            Ok(snapshot) => *lease.native_unmap.borrow_mut() = Some(snapshot),
            Err(err) => warn!("error capturing native unmap snapshot: {err:?}"),
        }
    }
    let ready = lease.native_unmap.borrow().is_some();
    ready
}

/// Prefer the pre-commit attachment lease to a newer live buffer. The raw API
/// remains the final fallback and keeps its existing synchronous copy contract.
pub(crate) fn take_native_close_source<W: LayoutElement>(
    window: &W,
    resources: &WindowResourceHandle,
) -> Option<NativeCloseSource> {
    let retained = resources.lease().native_close_source.borrow_mut().take();
    retained
        .or_else(|| window.native_dmabuf_source())
        .map(NativeCloseSource::Owned)
        .or_else(|| window.dmabuf().map(NativeCloseSource::Unowned))
}

pub(crate) fn store_gles_unmap_if_empty<W: LayoutElement>(
    scene: &TileScene<'_, W>,
    resources: &WindowResourceHandle,
    renderer: &mut GlesRenderer,
    xray: Option<&mut Xray>,
    xray_has_blocked_out_layers: bool,
    xray_pos: XrayPos,
) {
    let lease = resources.lease();
    if lease.unmap.borrow().is_some() {
        return;
    }
    let snapshot = capture_gles(scene, renderer, xray, xray_has_blocked_out_layers, xray_pos);
    *lease.unmap.borrow_mut() = Some(snapshot);
}

pub(crate) fn store_any_unmap_if_empty<W: LayoutElement>(
    scene: &TileScene<'_, W>,
    resources: &WindowResourceHandle,
    renderer: &mut AnyRenderer<'_>,
    xray: Option<&mut Xray>,
    xray_has_blocked_out_layers: bool,
    xray_pos: XrayPos,
) {
    match renderer {
        AnyRenderer::Gles(gles) => {
            store_gles_unmap_if_empty(
                scene,
                resources,
                gles,
                xray,
                xray_has_blocked_out_layers,
                xray_pos,
            );
        }
        AnyRenderer::Native(native) => {
            let _ = store_native_unmap_if_empty(
                scene,
                resources,
                native,
                xray,
                xray_has_blocked_out_layers,
                xray_pos,
            );
        }
    }
}

/// GLES retains elements and resolves textures lazily when closing starts.
/// Output stays unfiltered; captures use a filtered background whenever any
/// background layer is private, without multiplying every target combination.
pub(crate) fn capture_gles<W: LayoutElement>(
    scene: &TileScene<'_, W>,
    renderer: &mut GlesRenderer,
    mut xray: Option<&mut Xray>,
    xray_has_blocked_out_layers: bool,
    xray_pos: XrayPos,
) -> TileRenderSnapshot {
    let _span = clarion_tracy_span!("Scene::capture_window_snapshot");
    let mut contents = Vec::new();
    scene.render(
        RenderCtx {
            target: RenderTarget::Output,
            renderer,
            xray: xray.as_deref(),
        },
        Point::from((0., 0.)),
        xray_pos,
        false,
        &mut |elem| contents.push(elem),
    );

    // The scoped route restores the shared Xray slots on both normal completion
    // and unwind. Captured elements retain their selected Rc independently.
    let xray = xray
        .take()
        .map(|xray| SnapshotXrayRoute::new(xray, xray_has_blocked_out_layers));
    let mut contents_with_blocked_out_bg = None;
    if xray_has_blocked_out_layers {
        if let Some(xray) = &xray {
            let mut elements = Vec::new();
            scene.render(
                RenderCtx {
                    target: RenderTarget::Output,
                    renderer,
                    xray: Some(xray.view()),
                },
                Point::from((0., 0.)),
                xray_pos,
                false,
                &mut |elem| elements.push(elem),
            );
            contents_with_blocked_out_bg = Some(elements);
        }
    }

    let mut blocked_out_contents = Vec::new();
    scene.render(
        RenderCtx {
            target: RenderTarget::Screencast,
            renderer,
            xray: xray.as_ref().map(SnapshotXrayRoute::view),
        },
        Point::from((0., 0.)),
        xray_pos,
        false,
        &mut |elem| blocked_out_contents.push(elem),
    );
    drop(xray);

    RenderSnapshot {
        contents,
        contents_with_blocked_out_bg,
        blocked_out_contents,
        block_out_from: scene.window().rules().block_out_from,
        size: scene.animated_tile_size(),
        texture: Default::default(),
        texture_with_blocked_out_bg: Default::default(),
        blocked_out_texture: Default::default(),
    }
}

/// A non-unmap commit invalidates all prior close preparation. Drop the exact
/// attachment outside the RefCell borrow, since its final Buffer may release.
pub(crate) fn take_unmap_snapshot(resources: &WindowResourceHandle) -> Option<TileRenderSnapshot> {
    let lease = resources.lease();
    let source = lease.native_close_source.borrow_mut().take();
    drop(source);
    lease.native_unmap.borrow_mut().take();
    let snapshot = lease.unmap.borrow_mut().take();
    snapshot
}

type XrayBuffer = Rc<RefCell<EffectBuffer>>;

/// Temporary routing of one target's backdrop, preserving the original alias
/// identities. ScreenCapture is never overwritten by the GLES optimization.
struct SnapshotXrayRoute<'a> {
    xray: &'a mut Xray,
    restore_idx: usize,
    background: XrayBuffer,
    backdrop: XrayBuffer,
}

impl<'a> SnapshotXrayRoute<'a> {
    fn new(xray: &'a mut Xray, has_blocked_out_layers: bool) -> Self {
        let (destination, source) = if has_blocked_out_layers {
            (RenderTarget::Output, RenderTarget::Screencast)
        } else {
            (RenderTarget::Screencast, RenderTarget::Output)
        };
        let restore_idx = destination as usize;
        let background = Rc::clone(&xray.background[restore_idx]);
        let backdrop = Rc::clone(&xray.backdrop[restore_idx]);
        xray.background[restore_idx] = Rc::clone(&xray.background[source as usize]);
        xray.backdrop[restore_idx] = Rc::clone(&xray.backdrop[source as usize]);
        Self {
            xray,
            restore_idx,
            background,
            backdrop,
        }
    }

    fn view(&self) -> &Xray {
        self.xray
    }
}

impl Drop for SnapshotXrayRoute<'_> {
    fn drop(&mut self) {
        std::mem::swap(
            &mut self.xray.background[self.restore_idx],
            &mut self.background,
        );
        std::mem::swap(
            &mut self.xray.backdrop[self.restore_idx],
            &mut self.backdrop,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use super::*;
    use crate::scene::window_resources::WindowResourceRegistry;

    fn verify_xray_route(has_blocked_out_layers: bool, unwind: bool) {
        let mut xray = Xray::new();
        let backgrounds = xray.background.clone();
        let backdrops = xray.backdrop.clone();
        let (destination, source) = if has_blocked_out_layers {
            (RenderTarget::Output, RenderTarget::Screencast)
        } else {
            (RenderTarget::Screencast, RenderTarget::Output)
        };

        let result = catch_unwind(AssertUnwindSafe(|| {
            let route = SnapshotXrayRoute::new(&mut xray, has_blocked_out_layers);
            for target in [
                RenderTarget::Output,
                RenderTarget::Screencast,
                RenderTarget::ScreenCapture,
            ] {
                let expected = if target == destination {
                    source
                } else {
                    target
                };
                assert!(Rc::ptr_eq(
                    &route.view().background[target as usize],
                    &backgrounds[expected as usize],
                ));
                assert!(Rc::ptr_eq(
                    &route.view().backdrop[target as usize],
                    &backdrops[expected as usize],
                ));
            }
            if unwind {
                panic!("injected capture unwind");
            }
        }));
        assert_eq!(result.is_err(), unwind);
        for index in 0..RenderTarget::COUNT {
            assert!(Rc::ptr_eq(&xray.background[index], &backgrounds[index]));
            assert!(Rc::ptr_eq(&xray.backdrop[index], &backdrops[index]));
        }
    }

    #[test]
    fn private_background_routes_only_output_to_filtered_screencast() {
        verify_xray_route(true, false);
    }

    #[test]
    fn public_background_reuses_output_for_screencast() {
        verify_xray_route(false, false);
    }

    #[test]
    fn both_background_routes_restore_shared_aliases_after_capture_unwind() {
        for has_blocked_out_layers in [false, true] {
            verify_xray_route(has_blocked_out_layers, true);
        }
    }

    #[test]
    fn consuming_unmap_snapshot_preserves_pixels_after_registration_is_removed() {
        let owner = WindowResourceRegistry::new();
        let token = owner.allocator().allocate();
        *token.lease().unmap.borrow_mut() = Some(RenderSnapshot {
            contents: Vec::new(),
            contents_with_blocked_out_bg: Some(Vec::new()),
            blocked_out_contents: Vec::new(),
            block_out_from: Some(clarion_config::BlockOutFrom::ScreenCapture),
            size: (360., 240.).into(),
            texture: Default::default(),
            texture_with_blocked_out_bg: Default::default(),
            blocked_out_texture: Default::default(),
        });
        let snapshot = take_unmap_snapshot(&token).unwrap();
        assert!(take_unmap_snapshot(&token).is_none());
        drop(token);
        assert_eq!(owner.len(), 0);
        assert_eq!(snapshot.size, (360., 240.).into());
        assert_eq!(
            snapshot.block_out_from,
            Some(clarion_config::BlockOutFrom::ScreenCapture),
        );
        assert!(snapshot.contents_with_blocked_out_bg.is_some());
    }
}
