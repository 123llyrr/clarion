//! Refresh constraints for wp-fifo. Smithay owns the commit blockers; we signal only
//! barriers included in a rendered frame, or old barriers that cannot make progress.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use smithay::desktop::utils::surface_primary_scanout_output;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::fifo::v1::server::{
    wp_fifo_manager_v1::{self, WpFifoManagerV1},
    wp_fifo_v1::{self, WpFifoV1},
};
use smithay::reexports::wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, Client, DataInit, DisplayHandle, New,
    Resource, Weak,
};
use smithay::wayland::compositor::{
    add_blocker, add_pre_commit_hook, is_sync_subsurface, with_states, Barrier,
};
use smithay::wayland::fifo::{FifoBarrierCachedState, FifoCachedState};
use smithay::wayland::{Dispatch2, GlobalDispatch2};

use crate::compositor::State;

/// FIFO global with precise protocol-error ownership and resource cleanup.
#[derive(Debug)]
pub struct FifoManagerState {
    _global: GlobalId,
}

impl FifoManagerState {
    pub fn new(display: &DisplayHandle) -> Self {
        let global = display.create_global::<State, WpFifoManagerV1, _>(1, FifoManagerData);
        Self { _global: global }
    }
}

#[derive(Debug, Clone, Copy)]
struct FifoManagerData;

impl GlobalDispatch2<WpFifoManagerV1, State> for FifoManagerData {
    fn bind(
        &self,
        _state: &mut State,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<WpFifoManagerV1>,
        data_init: &mut DataInit<'_, State>,
    ) {
        data_init.init(resource, *self);
    }
}

impl Dispatch2<WpFifoManagerV1, State> for FifoManagerData {
    fn request(
        &self,
        _state: &mut State,
        _client: &Client,
        resource: &WpFifoManagerV1,
        request: wp_fifo_manager_v1::Request,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            wp_fifo_manager_v1::Request::GetFifo { id, surface } => {
                let (install_hook, has_active_fifo) = with_states(&surface, |states| {
                    let Some(marker) = states.data_map.get::<RefCell<FifoMarker>>() else {
                        return (true, false);
                    };
                    let mut marker = marker.borrow_mut();
                    let active = marker.0.as_ref().is_some_and(Resource::is_alive);
                    if !active {
                        marker.0 = None;
                    }
                    (false, active)
                });

                if has_active_fifo {
                    resource.post_error(
                        wp_fifo_manager_v1::Error::AlreadyExists,
                        "the surface already has a fifo object",
                    );
                    return;
                }

                if install_hook {
                    add_pre_commit_hook::<State, _>(&surface, |_, _, surface| {
                        let barrier = with_states(surface, |states| {
                            let fifo = *states.cached_state.get::<FifoCachedState>().pending();
                            let barrier = fifo
                                .wait_barrier
                                .then(|| {
                                    states
                                        .cached_state
                                        .get::<FifoBarrierCachedState>()
                                        .pending()
                                        .barrier
                                        .take()
                                })
                                .flatten();
                            if fifo.set_barrier {
                                states
                                    .cached_state
                                    .get::<FifoBarrierCachedState>()
                                    .pending()
                                    .barrier = Some(Barrier::new(false));
                            }
                            barrier
                        });

                        if let Some(barrier) = barrier {
                            if !barrier.is_signaled() && !is_sync_subsurface(surface) {
                                add_blocker(surface, barrier);
                            }
                        }
                    });
                }

                let fifo = data_init.init(id, FifoData(surface.downgrade()));
                with_states(&surface, |states| {
                    states
                        .data_map
                        .get_or_insert(|| RefCell::new(FifoMarker(None)))
                        .borrow_mut()
                        .0 = Some(fifo);
                });
            }
            wp_fifo_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

struct FifoMarker(Option<WpFifoV1>);

#[derive(Debug)]
struct FifoData(Weak<WlSurface>);

impl FifoData {
    fn clear_marker(&self, resource: &WpFifoV1) {
        let Ok(surface) = self.0.upgrade() else {
            return;
        };
        with_states(&surface, |states| {
            let Some(marker) = states.data_map.get::<RefCell<FifoMarker>>() else {
                return;
            };
            let mut marker = marker.borrow_mut();
            if marker
                .0
                .as_ref()
                .is_some_and(|active| active.id() == resource.id())
            {
                marker.0 = None;
            }
        });
    }
}

impl Dispatch2<WpFifoV1, State> for FifoData {
    fn request(
        &self,
        _state: &mut State,
        _client: &Client,
        resource: &WpFifoV1,
        request: wp_fifo_v1::Request,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            wp_fifo_v1::Request::SetBarrier => {
                let Ok(surface) = self.0.upgrade() else {
                    resource.post_error(
                        wp_fifo_v1::Error::SurfaceDestroyed,
                        "the associated surface has been destroyed",
                    );
                    return;
                };
                with_states(&surface, |states| {
                    states
                        .cached_state
                        .get::<FifoCachedState>()
                        .pending()
                        .set_barrier = true;
                });
            }
            wp_fifo_v1::Request::WaitBarrier => {
                let Ok(surface) = self.0.upgrade() else {
                    resource.post_error(
                        wp_fifo_v1::Error::SurfaceDestroyed,
                        "the associated surface has been destroyed",
                    );
                    return;
                };
                with_states(&surface, |states| {
                    states
                        .cached_state
                        .get::<FifoCachedState>()
                        .pending()
                        .wait_barrier = true;
                });
            }
            wp_fifo_v1::Request::Destroy => self.clear_marker(resource),
            _ => unreachable!(),
        }
    }

    fn destroyed(
        &self,
        _state: &mut State,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        resource: &WpFifoV1,
    ) {
        self.clear_marker(resource);
    }
}

// The existing one-second fallback timer checks this deadline. A barrier therefore
// cannot stall a hidden, unmapped, suspended or disconnected surface indefinitely.
const MAX_WAIT: Duration = Duration::from_secs(1);

#[derive(Clone)]
struct Pending {
    barrier: Barrier,
    client: Client,
    applied_at: Duration,
}

struct EstimatedFrame {
    deadline: Option<Instant>,
    barriers: Vec<(WlSurface, Pending)>,
}

#[derive(Default)]
pub(crate) struct FifoState {
    pending: HashMap<WlSurface, Pending>,
    estimated_frames: HashMap<Output, Vec<EstimatedFrame>>,
}

impl FifoState {
    pub fn committed(&mut self, surface: &WlSurface, now: Duration) {
        let barrier = with_states(surface, |states| {
            states
                .cached_state
                .get::<FifoBarrierCachedState>()
                .current()
                .barrier
                .clone()
        });
        let Some(barrier) = barrier.filter(|barrier| !barrier.is_signaled()) else {
            self.pending.remove(surface);
            return;
        };
        // Commits without set_barrier can carry the same cached barrier. They must
        // not extend its fallback deadline indefinitely.
        if self
            .pending
            .get(surface)
            .is_some_and(|p| p.barrier == barrier)
        {
            return;
        }
        if let Some(client) = surface.client() {
            self.pending.insert(
                surface.clone(),
                Pending {
                    barrier,
                    client,
                    applied_at: now,
                },
            );
        }
    }

    #[cfg(test)]
    pub fn frame_rendered(&mut self, output: &Output, defer: bool, tx: &Sender<Client>) -> bool {
        self.frame_rendered_at(output, defer, None, tx)
    }

    /// A nested output has no KMS vblank, so each empty frame retains its own refresh deadline.
    pub fn frame_rendered_at(
        &mut self,
        output: &Output,
        defer: bool,
        deadline: Option<Instant>,
        tx: &Sender<Client>,
    ) -> bool {
        let frame = self
            .pending
            .iter()
            .filter_map(|(surface, pending)| {
                let visible = with_states(surface, |states| {
                    surface_primary_scanout_output(surface, states).as_ref() == Some(output)
                });
                visible.then(|| (surface.clone(), pending.clone()))
            })
            .collect::<Vec<_>>();
        if defer {
            // Preserve snapshots from earlier frames. Replacing them could both release a new
            // barrier at an older frame's deadline and lose an older barrier entirely.
            let frame = frame
                .into_iter()
                .filter(|(_, pending)| !pending.barrier.is_signaled())
                .collect::<Vec<_>>();
            if frame.is_empty() {
                return false;
            }
            let queued = self.estimated_frames.entry(output.clone()).or_default();
            queued.retain_mut(|estimated| {
                estimated
                    .barriers
                    .retain(|(_, pending)| !pending.barrier.is_signaled());
                !estimated.barriers.is_empty()
            });
            let barriers = frame
                .into_iter()
                .filter(|(surface, pending)| {
                    !queued
                        .iter()
                        .flat_map(|frame| &frame.barriers)
                        .any(|(old, p)| old == surface && p.barrier == pending.barrier)
                })
                .collect::<Vec<_>>();
            if !barriers.is_empty() {
                queued.push(EstimatedFrame { deadline, barriers });
            }
            false
        } else {
            // A submitted frame also supersedes any older empty-frame estimate. Signal their
            // barriers even if a newer commit replaced the current per-surface barrier.
            let old = self.estimated_frames.remove(output).unwrap_or_default();
            let mut released = false;
            for estimated in old {
                released |= self.signal(estimated.barriers, tx);
            }
            self.signal(frame, tx) | released
        }
    }

    #[cfg(test)]
    pub fn has_estimated_frame(&self, output: &Output) -> bool {
        self.estimated_frames.contains_key(output)
    }

    pub fn estimated_vblank(&mut self, output: &Output, tx: &Sender<Client>) -> bool {
        self.estimated_frames.remove(output).is_some_and(|frames| {
            frames.into_iter().fold(false, |released, frame| {
                self.signal(frame.barriers, tx) | released
            })
        })
    }

    pub fn next_estimated_deadline(&self, output: &Output) -> Option<Instant> {
        self.estimated_frames
            .get(output)?
            .iter()
            .filter_map(|frame| frame.deadline)
            .min()
    }

    pub fn estimated_vblank_due(
        &mut self,
        output: &Output,
        now: Instant,
        tx: &Sender<Client>,
    ) -> bool {
        let Some(frames) = self.estimated_frames.remove(output) else {
            return false;
        };
        let mut later = Vec::new();
        let mut released = false;
        for frame in frames {
            if frame.deadline.is_none_or(|deadline| deadline <= now) {
                released |= self.signal(frame.barriers, tx);
            } else {
                later.push(frame);
            }
        }
        if !later.is_empty() {
            self.estimated_frames.insert(output.clone(), later);
        }
        released
    }

    pub fn expire(&mut self, now: Duration, tx: &Sender<Client>) {
        // The periodic fallback also bounds the lifetime of nested frame snapshots if calloop
        // could not arm their short timer. Only nested frames carry an Instant deadline.
        let instant = Instant::now();
        let due_outputs = self
            .estimated_frames
            .iter()
            .filter(|(_, frames)| {
                frames
                    .iter()
                    .any(|frame| frame.deadline.is_some_and(|deadline| deadline <= instant))
            })
            .map(|(output, _)| output.clone())
            .collect::<Vec<_>>();
        for output in due_outputs {
            self.estimated_vblank_due(&output, instant, tx);
        }
        let expired = self
            .pending
            .iter()
            .filter(|(_, p)| now.saturating_sub(p.applied_at) >= MAX_WAIT)
            .map(|(surface, p)| (surface.clone(), p.clone()))
            .collect();
        self.signal(expired, tx);
        self.estimated_frames.retain(|_, frame| {
            frame.retain_mut(|estimated| {
                estimated.barriers.retain(|(_, p)| !p.barrier.is_signaled());
                !estimated.barriers.is_empty()
            });
            !frame.is_empty()
        });
    }

    pub fn destroyed(&mut self, surface: &WlSurface, tx: &Sender<Client>) {
        if let Some(pending) = self.pending.remove(surface) {
            // Other surfaces in the same client transaction can wait for this one.
            self.signal(vec![(surface.clone(), pending)], tx);
        }
        self.estimated_frames.retain(|_, frame| {
            frame.retain_mut(|estimated| {
                estimated.barriers.retain(|(s, _)| s != surface);
                !estimated.barriers.is_empty()
            });
            !frame.is_empty()
        });
    }

    fn signal(&mut self, frame: Vec<(WlSurface, Pending)>, tx: &Sender<Client>) -> bool {
        let mut released = false;
        for (surface, pending) in frame {
            let needs_signal = !pending.barrier.is_signaled();
            if needs_signal {
                pending.barrier.signal();
                released = true;
            }
            // An older frame must never remove a newer commit's barrier.
            if self
                .pending
                .get(&surface)
                .is_some_and(|p| p.barrier == pending.barrier)
            {
                self.pending.remove(&surface);
            }
            if needs_signal {
                let _ = tx.send(pending.client);
            }
        }
        released
    }
}
