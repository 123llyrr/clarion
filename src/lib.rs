#[macro_use]
extern crate tracing;

// Unit and integration tests do not necessarily enter the compositor's
// `main`, which is where Tracy is normally started. In manual-lifetime mode,
// initialize it lazily at the instrumentation boundary. `Client::start()` is
// internally synchronized, so this is safe when several test threads race to
// execute their first span; in normal builds the helper is inlined away.
#[inline]
fn ensure_tracy_client() {
    #[cfg(feature = "profile-with-tracy-ondemand")]
    let _ = tracy_client::Client::running().unwrap_or_else(tracy_client::Client::start);
}

macro_rules! clarion_tracy_span {
    ($($args:tt)*) => {{
        $crate::ensure_tracy_client();
        tracy_client::span!($($args)*)
    }};
}

#[cfg(feature = "dbus")]
pub mod a11y;
pub mod animation;
pub mod backend;
pub mod cli;
mod commit_timing;
pub mod compositor;
pub mod cursor;
#[cfg(feature = "dbus")]
pub mod dbus;
mod fifo;
pub mod frame_clock;
pub mod frame_pipeline;
mod frame_stats;
pub mod gestures;
pub mod handlers;
pub mod idle;
pub mod input;
pub mod ipc;
pub mod layer;
pub mod layout;
mod output_preview;
pub mod output_schedule;
pub(crate) mod overview;
pub(crate) mod platform;
pub mod protocols;
pub mod render_helpers;
pub mod rubber_band;
pub mod scene;
#[cfg(feature = "xdp-gnome-screencast")]
pub mod screencasting;
pub mod ui;
pub mod utils;
pub mod window;

#[cfg(test)]
mod tests;
