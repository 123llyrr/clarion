//! Realand's device-independent desktop core.
//!
//! This crate owns desktop identity and geometry. It does not send Wayland
//! requests or access a renderer; platform adapters consume its effects.

pub mod bsp;
pub mod canvas;
pub mod column;
pub mod deformation;
pub mod desktop;
pub mod dnd_hold;
pub mod floating;
pub mod frame_clock;
pub mod geometry;
pub mod gestures;
pub mod input;
pub mod layout;
pub mod management;
pub mod motion;
pub mod move_controls;
pub mod navigation;
pub mod output_identity;
pub mod overview;
pub mod policy_memory;
pub mod rate_clock;
pub mod region;
pub mod resize;
pub mod rubber_band;
pub mod scene;
pub mod scheduling;
pub mod scrolling;
pub mod session_match;
pub mod spread;
pub mod spring;
pub mod stage_drag;
pub mod trajectory;
pub mod viewport;
pub mod workspace_state;
pub mod workspace_switch;

pub use desktop::{CoreEffect, CoreEvent, DesktopCore, Layer, SpaceId, WindowRecord};
pub use geometry::{Axis, Rect, Size};
pub use layout::LayoutMode;
