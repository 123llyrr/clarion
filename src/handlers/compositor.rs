use std::collections::hash_map::Entry;

use clarion_ipc::PositionChange;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::input::pointer::{CursorImageStatus, CursorImageSurfaceData};
use smithay::reexports::calloop::Interest;
use smithay::reexports::wayland_server::protocol::wl_buffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::commit_timing::CommitTimerStateUserData;
use smithay::wayland::compositor::{
    add_blocker, add_post_commit_hook, add_pre_commit_hook, get_parent, is_sync_subsurface,
    remove_pre_commit_hook, with_states, Blocker, BlockerState, BufferAssignment,
    CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
};
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::drm_syncobj::DrmSyncobjCachedState;
use smithay::wayland::shell::xdg::ToplevelCachedState;
use smithay::wayland::shm::{ShmHandler, ShmState};

use super::xdg_shell::add_mapped_toplevel_pre_commit_hook;
use crate::compositor::{CastTarget, ClientState, LockState, State};
use crate::handlers::XDG_ACTIVATION_TOKEN_TIMEOUT;
use crate::layout::{ActivateWindow, AddWindowTarget, LayoutElement as _};
use crate::utils::transaction::Transaction;
use crate::utils::{is_mapped, send_scale_transform};
use crate::window::{InitialConfigureState, Mapped, ResolvedWindowRules, Unmapped};

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.clarion.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn new_subsurface(&mut self, surface: &WlSurface, parent: &WlSurface) {
        let mut root = parent.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }

        // The root of a subsurface tree can itself be an xdg or input-method popup. Such a root
        // is not in the main layout, so resolve its parent output through the popup manager.
        let output = self.clarion.output_for_root(&root).or_else(|| {
            self.clarion
                .popups
                .find_popup(&root)
                .and_then(|popup| self.output_for_popup(&popup))
        });
        if let Some(output) = output {
            let scale = output.current_scale();
            let transform = output.current_transform();
            with_states(surface, |data| {
                send_scale_transform(surface, data, scale, transform);
            });
        }
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        self.add_default_dmabuf_pre_commit_hook(surface);
        // This hook is installed before a client can create its commit-timer object,
        // so it observes the timestamp before Smithay's managed hook consumes it.
        add_pre_commit_hook::<Self, _>(surface, |state, _, surface| {
            let timestamp = with_states(surface, |states| {
                states
                    .data_map
                    .get::<CommitTimerStateUserData>()
                    .and_then(|timing| timing.borrow().timestamp)
            });
            if let Some(timestamp) = timestamp {
                state.clarion.schedule_commit_timing(surface, timestamp);
            }
        });
        add_post_commit_hook::<Self, _>(surface, |state, _, surface| {
            state
                .clarion
                .fifo
                .committed(surface, crate::utils::get_monotonic_time());
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        let _span = clarion_tracy_span!("CompositorHandler::commit");
        let _span = trace_span!("commit", surface = %surface.id()).entered();
        trace!("commit");

        // This callback runs after acquire/transaction blockers have cleared.
        // Snapshot the previous renderer buffers before Smithay replaces them,
        // consuming the animation serial only for the commit being applied.
        // A pre-commit snapshot can be overwritten by another blocked commit,
        // and cannot see the content of commits still waiting ahead of it.
        if let Some((mapped, output)) = self.clarion.layout.find_window_and_output_mut(surface) {
            if mapped.toplevel().wl_surface() == surface {
                let serial = with_states(surface, |states| {
                    states
                        .cached_state
                        .get::<ToplevelCachedState>()
                        .current()
                        .last_acked
                        .as_ref()
                        .map(|configure| configure.serial)
                });
                if serial.is_some_and(|serial| mapped.should_animate_commit(serial)) {
                    let scale = output
                        .map_or(1., |output| output.current_scale().fractional_scale())
                        .into();
                    self.backend
                        .with_renderer_for_output(output, |renderer| match renderer {
                            crate::render_helpers::renderer::AnyRenderer::Gles(renderer) => {
                                mapped.store_animation_snapshot(renderer, scale);
                            }
                            crate::render_helpers::renderer::AnyRenderer::Native(renderer) => {
                                mapped.store_native_animation_snapshot(renderer, scale);
                            }
                        });
                }
            }
        }

        on_commit_buffer_handler::<Self>(surface);
        crate::render_helpers::vulkan::native::record_surface_tree_buffer_owners(surface);
        self.backend.early_import(surface);

        let mut root_surface = surface.clone();
        while let Some(parent) = get_parent(&root_surface) {
            root_surface = parent;
        }

        // Update the cached root surface.
        self.clarion
            .root_surface
            .insert(surface.clone(), root_surface.clone());

        if is_sync_subsurface(surface) {
            return;
        }

        if surface == &root_surface {
            // This is a root surface commit. It might have mapped a previously-unmapped toplevel.
            if let Entry::Occupied(entry) = self.clarion.unmapped_windows.entry(surface.clone()) {
                if is_mapped(surface) {
                    // The toplevel got mapped.
                    let Unmapped {
                        window,
                        state,
                        activation_token_data,
                    } = entry.remove();

                    window.on_commit();

                    let toplevel = window.toplevel().expect("no X11 support");

                    let (
                        rules,
                        width,
                        height,
                        is_full_width,
                        output,
                        workspace_id,
                        is_pending_maximized,
                    ) = if let InitialConfigureState::Configured {
                        rules,
                        width,
                        height,
                        floating_width: _,
                        floating_height: _,
                        is_full_width,
                        output,
                        workspace_name,
                        is_pending_maximized,
                    } = state
                    {
                        // Check that the output is still connected.
                        let output =
                            output.filter(|o| self.clarion.layout.monitor_for_output(o).is_some());

                        // Check that the workspace still exists.
                        let workspace_id = workspace_name
                            .as_deref()
                            .and_then(|n| self.clarion.layout.find_workspace_by_name(n))
                            .map(|(_, ws)| ws.id());

                        (
                            rules,
                            width,
                            height,
                            is_full_width,
                            output,
                            workspace_id,
                            is_pending_maximized,
                        )
                    } else {
                        // Can happen when a surface unmaps by attaching a null buffer while
                        // there are in-flight pending configures.
                        debug!("window mapped without proper initial configure");
                        (
                            ResolvedWindowRules::default(),
                            None,
                            None,
                            false,
                            None,
                            None,
                            false,
                        )
                    };

                    // The GTK about dialog sets min/max size after the initial configure but
                    // before mapping, so we need to compute open_floating at the last possible
                    // moment, that is here.
                    let is_floating = rules.compute_open_floating(toplevel);

                    // Figure out if we should activate the window.
                    let activate = rules.open_focused.map(|focus| {
                        if focus {
                            ActivateWindow::Yes
                        } else {
                            ActivateWindow::No
                        }
                    });
                    let activate = activate.unwrap_or_else(|| {
                        // Check the token timestamp again in case the window took a while between
                        // requesting activation and mapping.
                        let token = activation_token_data.filter(|token| {
                            token.timestamp.elapsed() < XDG_ACTIVATION_TOKEN_TIMEOUT
                        });
                        if token.is_some() {
                            ActivateWindow::Yes
                        } else {
                            let config = self.clarion.config.borrow();
                            if config.debug.strict_new_window_focus_policy {
                                ActivateWindow::No
                            } else {
                                ActivateWindow::Smart
                            }
                        }
                    });

                    let parent = toplevel
                        .parent()
                        .and_then(|parent| self.clarion.layout.find_window_and_output(&parent))
                        // Only consider the parent if we configured the window for the same
                        // output.
                        //
                        // Normally when we're following the parent, the configured output will be
                        // None. If the configured output is set, that means it was set explicitly
                        // by a window rule or a fullscreen request.
                        .filter(|(_, parent_output)| {
                            parent_output.is_none()
                                || output.is_none()
                                || output.as_ref() == *parent_output
                        })
                        .map(|(mapped, _)| mapped.window.clone());

                    // The mapped pre-commit hook deals with dma-bufs on its own.
                    self.remove_default_dmabuf_pre_commit_hook(surface);
                    let hook = add_mapped_toplevel_pre_commit_hook(toplevel);
                    let mapped = {
                        let config = self.clarion.config.borrow();
                        Mapped::new(
                            window,
                            rules,
                            hook,
                            &config,
                            self.clarion.scene_resources.allocator(),
                        )
                    };
                    let window = mapped.window.clone();

                    let target = if let Some(p) = &parent {
                        // Open dialogs next to their parent window.
                        AddWindowTarget::NextTo(p)
                    } else if let Some(id) = workspace_id {
                        AddWindowTarget::Workspace(id)
                    } else if let Some(output) = &output {
                        AddWindowTarget::Output(output)
                    } else {
                        AddWindowTarget::Auto
                    };
                    let output = self.clarion.layout.add_window(
                        mapped,
                        target,
                        width,
                        height,
                        is_full_width,
                        is_floating,
                        activate,
                    );
                    let output = output.cloned();

                    // The window state cannot contain Fullscreen and Maximized at once. Therefore,
                    // if the window ended up fullscreen, then we only know that it is also
                    // maximized from the is_pending_maximized variable. Tell the layout about it
                    // here so that unfullscreening the window makes it maximized.
                    if let Some((mapped, _)) = self.clarion.layout.find_window_and_output(surface) {
                        if mapped.pending_sizing_mode().is_fullscreen() && is_pending_maximized {
                            self.clarion.layout.set_maximized(&window, true);
                        }
                    } else {
                        error!("layout is missing the window that we just added");
                    }

                    if let Some(output) = output {
                        self.clarion.layout.start_open_animation_for_window(&window);

                        let new_focus = self.clarion.layout.focus().map(|m| &m.window);
                        if new_focus == Some(&window) {
                            // We activated the newly opened window.
                            self.maybe_warp_cursor_to_focus();
                            self.clarion.layer_shell_on_demand_focus = None;
                        }

                        self.clarion.queue_redraw(&output);
                    }
                    return;
                }

                // The toplevel remains unmapped.
                trace!("toplevel remains unmapped");
                let unmapped = entry.get();
                if unmapped.needs_initial_configure() {
                    let toplevel = unmapped.window.toplevel().expect("no x11 support").clone();
                    self.queue_initial_configure(toplevel);
                }
                return;
            }

            // This is a commit of a previously-mapped root or a non-toplevel root.
            if let Some((mapped, output)) = self.clarion.layout.find_window_and_output(surface) {
                let window = mapped.window.clone();
                let output = output.cloned();

                let id = mapped.id();
                let has_animation_snapshot = mapped.has_animation_snapshot();

                // This is a commit of a previously-mapped toplevel.
                let is_mapped = is_mapped(surface);

                // Must start the close animation before window.on_commit().
                let transaction = Transaction::new();
                if !is_mapped {
                    let blocker = transaction.blocker();
                    self.backend
                        .with_renderer_for_output(output.as_ref(), |renderer| {
                            self.clarion
                                .layout
                                .start_close_animation_for_window(renderer, &window, blocker);
                        });
                }

                self.clarion
                    .layout
                    .find_window_and_output(surface)
                    .unwrap()
                    .0
                    .on_surface_commit();

                if !is_mapped {
                    // The toplevel got unmapped.
                    //
                    // Test client: wleird-unmap.
                    trace!("toplevel got unmapped");

                    let active_window = self.clarion.layout.focus().map(|m| &m.window);
                    let was_active = active_window == Some(&window);

                    self.clarion
                        .stop_casts_for_target(CastTarget::Window { id: id.get() });

                    self.clarion.window_mru_ui.remove_window(id);
                    self.clarion
                        .layout
                        .remove_window(&window, transaction.clone());
                    self.add_default_dmabuf_pre_commit_hook(surface);

                    // If this is the only instance, then this transaction will complete
                    // immediately, so no need to set the timer.
                    if !transaction.is_last() {
                        transaction.register_deadline_timer(&self.clarion.event_loop);
                    }

                    if was_active {
                        self.maybe_warp_cursor_to_focus();
                    }

                    // Newly-unmapped toplevels must perform the initial commit-configure sequence
                    // afresh.
                    let unmapped = Unmapped::new(window);
                    self.clarion
                        .unmapped_windows
                        .insert(surface.clone(), unmapped);

                    if let Some(output) = output {
                        self.clarion.queue_redraw(&output);
                        self.clarion.queue_redraw_mru_output();
                    }
                    return;
                }

                let (serial, buffer_delta) = with_states(surface, |states| {
                    let buffer_delta = states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .buffer_delta
                        .take();

                    let serial = states
                        .cached_state
                        .get::<ToplevelCachedState>()
                        .current()
                        .last_acked
                        .as_ref()
                        .map(|c| c.serial);
                    (serial, buffer_delta)
                });
                if serial.is_none() {
                    error!("commit on a mapped surface without a configured serial");
                }

                // The toplevel remains mapped.
                self.clarion
                    .window_mru_ui
                    .update_window(&self.clarion.layout, id);
                if has_animation_snapshot {
                    self.backend
                        .with_renderer_for_output(output.as_ref(), |renderer| {
                            self.clarion
                                .layout
                                .prepare_resize_retarget(renderer, &window);
                        });
                }
                self.clarion.layout.update_window(&window, serial);

                // Move the toplevel according to the attach offset.
                if let Some(delta) = buffer_delta {
                    if delta.x != 0 || delta.y != 0 {
                        let (x, y) = delta.to_f64().into();
                        self.clarion.layout.move_floating_window(
                            Some(&window),
                            PositionChange::AdjustFixed(x),
                            PositionChange::AdjustFixed(y),
                            false,
                        );
                    }
                }

                // Popup placement depends on window size which might have changed.
                self.update_reactive_popups(&window);

                if let Some(output) = output {
                    self.clarion.queue_redraw(&output);
                    self.clarion.queue_redraw_mru_output();
                }
                return;
            }

            // This is a commit of a non-toplevel root.

            // This might be a popup.
            self.popups_handle_commit(surface);
        }

        // This is a commit of a non-root or a non-toplevel root.
        let root_window_output = self.clarion.layout.find_window_and_output(&root_surface);
        if let Some((mapped, output)) = root_window_output {
            let window = mapped.window.clone();
            let output = output.cloned();
            mapped.on_surface_commit();
            self.clarion
                .window_mru_ui
                .update_window(&self.clarion.layout, mapped.id());
            self.clarion.layout.update_window(&window, None);
            if let Some(output) = output {
                self.clarion.queue_redraw(&output);
                self.clarion.queue_redraw_mru_output();
            }
            return;
        }

        // This might be a popup unsync subsurface.
        if let Some(popup) = self.clarion.popups.find_popup(&root_surface) {
            if let Some(output) = self.output_for_popup(&popup) {
                self.clarion.queue_redraw(&output.clone());
            }
            return;
        }

        // This might be a layer-shell surface.
        if self.layer_shell_handle_commit(surface) {
            return;
        }

        // This might be a cursor surface.
        if matches!(
            &self.clarion.cursor_manager.cursor_image(),
            CursorImageStatus::Surface(s) if s == &root_surface
        ) {
            // In case the cursor surface has been committed handle the role specific
            // buffer offset by applying the offset on the cursor image hotspot
            if surface == &root_surface {
                with_states(surface, |states| {
                    let cursor_image_attributes = states.data_map.get::<CursorImageSurfaceData>();

                    if let Some(mut cursor_image_attributes) =
                        cursor_image_attributes.map(|attrs| attrs.lock().unwrap())
                    {
                        let buffer_delta = states
                            .cached_state
                            .get::<SurfaceAttributes>()
                            .current()
                            .buffer_delta
                            .take();
                        if let Some(buffer_delta) = buffer_delta {
                            cursor_image_attributes.hotspot -= buffer_delta;
                        }
                    }
                });
            }

            // FIXME: granular redraws for cursors.
            self.clarion.queue_redraw_all();
            return;
        }

        // This might be a DnD icon surface.
        if matches!(&self.clarion.dnd_icon, Some(icon) if icon.surface == root_surface) {
            let dnd_icon = self.clarion.dnd_icon.as_mut().unwrap();

            // In case the dnd surface has been committed handle the role specific
            // buffer offset by applying the offset on the dnd icon offset
            if surface == &dnd_icon.surface {
                with_states(&dnd_icon.surface, |states| {
                    let buffer_delta = states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .buffer_delta
                        .take()
                        .unwrap_or_default();
                    dnd_icon.offset += buffer_delta;
                });
            }

            // FIXME: granular redraws for cursors.
            self.clarion.queue_redraw_all();
            return;
        }

        // This might be a lock surface.
        for (output, state) in &self.clarion.output_state {
            if let Some(lock_surface) = &state.lock_surface {
                if lock_surface.wl_surface() == &root_surface {
                    if matches!(
                        self.clarion.lock_state,
                        LockState::WaitingForSurfaces { .. }
                    ) {
                        self.clarion.maybe_continue_to_locking();
                    } else {
                        self.clarion.queue_redraw(&output.clone());
                    }

                    return;
                }
            }
        }

        // This message can trigger for lock surfaces that had a commit right after we unlocked
        // the session, but that's ok, we don't need to handle them.
        trace!("commit on an unrecognized surface: {surface:?}, root: {root_surface:?}");
    }

    fn destroyed(&mut self, surface: &WlSurface) {
        // Smithay calls this before PrivateSurfaceData::cleanup resets its
        // renderer state. Preserve the original attachment through the close
        // preparation below, then always clear this surface-local slot (also
        // while unwinding) before the destruction callback returns.
        struct ClearBufferOwner<'a>(&'a WlSurface);
        impl Drop for ClearBufferOwner<'_> {
            fn drop(&mut self) {
                crate::render_helpers::vulkan::native::clear_surface_buffer_owner(self.0);
            }
        }
        let clear_buffer_owner = ClearBufferOwner(surface);
        self.clarion.destroy_commit_timing(surface);
        self.clarion.destroy_fifo_surface(surface);
        self.clarion.stop_signal.wakeup();
        // Clients may destroy their subsurfaces before the main surface. Ensure we have a snapshot
        // when that happens, so that the closing animation includes all these subsurfaces.
        //
        // Test client: alacritty with CSD <= 0.13 (it was fixed in winit afterwards:
        // https://github.com/rust-windowing/winit/pull/3625).
        //
        // This is still not perfect, as this function is called already after the (first)
        // subsurface is destroyed; in the case of alacritty, this is the top CSD shadow. But, it
        // gets most of the job done.
        if let Some(root) = self.clarion.root_surface.get(surface) {
            if let Some((mapped, output)) = self.clarion.layout.find_window_and_output(root) {
                let window = mapped.window.clone();
                let output = output.cloned();
                self.store_unmap_snapshot(&window, output.as_ref());
            }
        }
        drop(clear_buffer_owner);

        self.clarion
            .root_surface
            .retain(|k, v| k != surface && v != surface);

        // The object destruction order is not guaranteed to follow the logical role order. So for
        // example when a client disconnects unexpectedly, WlSurface::destroyed() may be called
        // before XdgShellHandler::toplevel_destroyed(). In this case, the surface will *not* have
        // the default dmabuf pre-commit hook: it will still have the toplevel pre-commit hook.
        //
        // So, this may come out empty, and then the toplevel pre-commit hook will be removed in the
        // subsequent toplevel_destroyed() call.
        if let Some(hook) = self.clarion.dmabuf_pre_commit_hook.remove(surface) {
            remove_pre_commit_hook(surface, &hook);
        }
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.clarion.shm_state
    }
}

impl State {
    pub fn add_default_dmabuf_pre_commit_hook(&mut self, surface: &WlSurface) {
        if !surface.is_alive() {
            error!("tried to add dmabuf pre-commit hook for a dead surface");
            return;
        }

        let hook = add_pre_commit_hook::<Self, _>(surface, move |state, _dh, surface| {
            let (maybe_dmabuf, acquire_point) = with_states(surface, |surface_data| {
                let dmabuf = surface_data
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .pending()
                    .buffer
                    .as_ref()
                    .and_then(|assignment| match assignment {
                        BufferAssignment::NewBuffer(buffer) => get_dmabuf(buffer).cloned().ok(),
                        _ => None,
                    });
                let acquire = surface_data
                    .cached_state
                    .get::<DrmSyncobjCachedState>()
                    .pending()
                    .acquire_point
                    .clone();
                (dmabuf, acquire)
            });
            if let Some(dmabuf) = maybe_dmabuf {
                if let Some(acquire_point) = acquire_point {
                    // Explicit sync uses the client's timeline, not the
                    // implicit DMA-BUF reservation fence. The commit must be
                    // ready before Smithay imports or renders the buffer.
                    match acquire_point.generate_blocker() {
                        Ok((blocker, source)) => {
                            if let Some(client) = surface.client() {
                                let res = state.clarion.event_loop.insert_source(
                                    source,
                                    move |_, _, state| {
                                        let display_handle = state.clarion.display_handle.clone();
                                        state
                                            .client_compositor_state(&client)
                                            .blocker_cleared(state, &display_handle);
                                        Ok(())
                                    },
                                );
                                if res.is_ok() {
                                    add_blocker(surface, blocker);
                                    trace!("added explicit syncobj acquire blocker");
                                } else {
                                    error!("failed to register syncobj acquire source; cancelling commit");
                                    add_blocker(surface, FailedAcquireBlocker);
                                }
                            }
                        }
                        Err(err) => {
                            error!(
                                ?err,
                                "failed to create syncobj acquire blocker; cancelling commit"
                            );
                            add_blocker(surface, FailedAcquireBlocker);
                        }
                    }
                    return;
                }
                state.add_implicit_dmabuf_blocker(surface, &dmabuf);
            }
        });

        let s = surface.clone();
        if let Some(prev) = self.clarion.dmabuf_pre_commit_hook.insert(s, hook) {
            error!("tried to add dmabuf pre-commit hook when there was already one");
            remove_pre_commit_hook(surface, &prev);
        }
    }

    pub(crate) fn add_implicit_dmabuf_blocker(&mut self, surface: &WlSurface, dmabuf: &Dmabuf) {
        // The only generation error is AlreadyReady: no watcher is needed then.
        // A successfully generated source, however, represents a pending writer.
        if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ) {
            if let Some(client) = surface.client() {
                let res = self
                    .clarion
                    .event_loop
                    .insert_source(source, move |_, _, state| {
                        let display_handle = state.clarion.display_handle.clone();
                        state
                            .client_compositor_state(&client)
                            .blocker_cleared(state, &display_handle);
                        Ok(())
                    });
                if let Err(err) = res {
                    error!(error = %err.error, "failed to register implicit DMA-BUF acquire source; cancelling commit");
                    add_blocker(surface, FailedAcquireBlocker);
                } else {
                    add_blocker(surface, blocker);
                    trace!("added default dmabuf blocker");
                }
            }
        }
    }

    pub fn remove_default_dmabuf_pre_commit_hook(&mut self, surface: &WlSurface) {
        if let Some(hook) = self.clarion.dmabuf_pre_commit_hook.remove(surface) {
            remove_pre_commit_hook(surface, &hook);
        } else {
            error!("tried to remove dmabuf pre-commit hook but there was none");
        }
    }
}

pub(super) struct FailedAcquireBlocker;

impl Blocker for FailedAcquireBlocker {
    fn state(&self) -> BlockerState {
        BlockerState::Cancelled
    }
}
