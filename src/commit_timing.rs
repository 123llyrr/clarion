//! Protocol and scheduling support for `wp_commit_timing_v1`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use calloop::RegistrationToken;
use smithay::reexports::rustix::time::{Nsecs, Secs, Timespec};
use smithay::reexports::wayland_protocols::wp::commit_timing::v1::server::{
    wp_commit_timer_v1::{self, WpCommitTimerV1},
    wp_commit_timing_manager_v1::{self, WpCommitTimingManagerV1},
};
use smithay::reexports::wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, Client, DataInit, DisplayHandle, New,
    Resource, Weak,
};
use smithay::utils::{Monotonic, Time};
use smithay::wayland::commit_timing::{
    CommitTimerBarrierStateUserData, CommitTimerStateUserData, Timestamp,
};
use smithay::wayland::compositor::{add_blocker, add_pre_commit_hook, with_states};
use smithay::wayland::{Dispatch2, GlobalDispatch2};

use crate::compositor::State;

/// Commit-timing global with the protocol validation missing from Smithay's
/// current managed implementation.
#[derive(Debug)]
pub struct CommitTimingManagerState {
    _global: GlobalId,
}

impl CommitTimingManagerState {
    pub fn new(display: &DisplayHandle) -> Self {
        let global =
            display.create_global::<State, WpCommitTimingManagerV1, _>(1, CommitTimingManagerData);
        Self { _global: global }
    }
}

#[derive(Debug, Clone, Copy)]
struct CommitTimingManagerData;

impl GlobalDispatch2<WpCommitTimingManagerV1, State> for CommitTimingManagerData {
    fn bind(
        &self,
        _state: &mut State,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<WpCommitTimingManagerV1>,
        data_init: &mut DataInit<'_, State>,
    ) {
        data_init.init(resource, *self);
    }
}

impl Dispatch2<WpCommitTimingManagerV1, State> for CommitTimingManagerData {
    fn request(
        &self,
        _state: &mut State,
        _client: &Client,
        resource: &WpCommitTimingManagerV1,
        request: wp_commit_timing_manager_v1::Request,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            wp_commit_timing_manager_v1::Request::GetTimer { id, surface } => {
                let (install_hook, has_active_timer) = with_states(&surface, |states| {
                    let Some(marker) = states.data_map.get::<RefCell<CommitTimerMarker>>() else {
                        return (true, false);
                    };
                    let mut marker = marker.borrow_mut();
                    let active = marker.0.as_ref().is_some_and(Resource::is_alive);
                    if !active {
                        marker.0 = None;
                    }
                    (false, active)
                });

                if has_active_timer {
                    resource.post_error(
                        wp_commit_timing_manager_v1::Error::CommitTimerExists,
                        "the surface already has a commit timer object",
                    );
                    return;
                }

                if install_hook {
                    add_pre_commit_hook::<State, _>(&surface, |_, _, surface| {
                        let timestamp = with_states(surface, |states| {
                            states
                                .data_map
                                .get::<CommitTimerStateUserData>()
                                .and_then(|state| state.borrow_mut().timestamp.take())
                        });
                        if let Some(timestamp) = timestamp {
                            let barrier = with_states(surface, |states| {
                                states
                                    .data_map
                                    .get_or_insert(CommitTimerBarrierStateUserData::default)
                                    .lock()
                                    .unwrap()
                                    .register(timestamp)
                            });
                            add_blocker(surface, barrier);
                        }
                    });
                }

                let timer = data_init.init(id, CommitTimerData(surface.downgrade()));
                with_states(&surface, |states| {
                    states
                        .data_map
                        .get_or_insert(|| RefCell::new(CommitTimerMarker(None)))
                        .borrow_mut()
                        .0 = Some(timer);
                });
            }
            wp_commit_timing_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

struct CommitTimerMarker(Option<WpCommitTimerV1>);

#[derive(Debug)]
struct CommitTimerData(Weak<WlSurface>);

impl CommitTimerData {
    fn clear_marker(&self, resource: &WpCommitTimerV1) {
        let Ok(surface) = self.0.upgrade() else {
            return;
        };
        with_states(&surface, |states| {
            let Some(marker) = states.data_map.get::<RefCell<CommitTimerMarker>>() else {
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

impl Dispatch2<WpCommitTimerV1, State> for CommitTimerData {
    fn request(
        &self,
        _state: &mut State,
        _client: &Client,
        resource: &WpCommitTimerV1,
        request: wp_commit_timer_v1::Request,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            wp_commit_timer_v1::Request::SetTimestamp {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => {
                let Ok(surface) = self.0.upgrade() else {
                    resource.post_error(
                        wp_commit_timer_v1::Error::SurfaceDestroyed,
                        "the associated surface has been destroyed",
                    );
                    return;
                };

                if tv_nsec >= 1_000_000_000 {
                    resource.post_error(
                        wp_commit_timer_v1::Error::InvalidTimestamp,
                        format!("tv_nsec must be below 1000000000, got {tv_nsec}"),
                    );
                    return;
                }

                let seconds = ((tv_sec_hi as u64) << 32) | u64::from(tv_sec_lo);
                // Smithay's Timestamp uses a signed Timespec internally. Values
                // above that range are valid on the wire but unreachable for the
                // compositor clock, so keep them at the latest representable time.
                let seconds = seconds.min(Secs::MAX as u64);
                let timestamp = Timestamp::from(Time::<Monotonic>::from(Timespec {
                    tv_sec: seconds as Secs,
                    tv_nsec: tv_nsec as Nsecs,
                }));

                let duplicate = with_states(&surface, move |states| {
                    let mut timing = states
                        .data_map
                        .get_or_insert(CommitTimerStateUserData::default)
                        .borrow_mut();
                    if timing.timestamp.is_some() {
                        true
                    } else {
                        timing.timestamp = Some(timestamp);
                        false
                    }
                });
                if duplicate {
                    resource.post_error(
                        wp_commit_timer_v1::Error::TimestampExists,
                        "the surface already has a timestamp for this commit",
                    );
                }
            }
            wp_commit_timer_v1::Request::Destroy => self.clear_marker(resource),
            _ => unreachable!(),
        }
    }

    fn destroyed(
        &self,
        _state: &mut State,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        resource: &WpCommitTimerV1,
    ) {
        self.clear_marker(resource);
    }
}

pub(crate) struct ScheduledCommit {
    pub deadline: Duration,
    pub release_until: Timestamp,
    pub token: Option<RegistrationToken>,
}

#[derive(Default)]
pub(crate) struct CommitTimingScheduler {
    pub scheduled: HashMap<WlSurface, ScheduledCommit>,
}
