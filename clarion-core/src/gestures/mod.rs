//! Device-independent gesture math shared by input adapters and window layout.
//!
//! Trackers consume numeric deltas and timestamps; they do not dispatch input,
//! mutate compositor state, or depend on Wayland devices. Keep device routing in
//! `input` so layout never needs to import the input adapter layer.

pub mod scroll_swipe_gesture;
pub mod scroll_tracker;
pub mod swipe_tracker;
