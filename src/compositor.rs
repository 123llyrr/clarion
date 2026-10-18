use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{env, mem, thread};

use _server_decoration::server::org_kde_kwin_server_decoration_manager::Mode as KdeDecorationsMode;
use anyhow::{bail, ensure, Context};
use calloop::futures::Scheduler;
use clarion_config::output::MaxBpc;
use clarion_config::{
    Config, FloatOrInt, Key, Modifiers, OutputName, TrackLayout, WarpMouseToFocusMode,
    WorkspaceReference, Xkb,
};
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::DrmDeviceFd;
use smithay::backend::input::Keycode;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::utils::{
    select_dmabuf_feedback, Relocate, RelocateRenderElement,
};
use smithay::backend::renderer::element::{
    default_primary_scanout_output_compare, Element, Id, PrimaryScanoutOutput, RenderElement,
    RenderElementStates,
};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Offscreen, RendererSuper, Texture};
use smithay::desktop::utils::{
    bbox_from_surface_tree, output_update, send_dmabuf_feedback_surface_tree,
    send_frames_surface_tree, surface_presentation_feedback_flags_from_states,
    surface_primary_scanout_output, take_presentation_feedback_surface_tree,
    under_from_surface_tree, update_surface_primary_scanout_output, with_surfaces_surface_tree,
    OutputPresentationFeedback,
};
use smithay::desktop::{
    find_popup_root_surface, layer_map_for_output, LayerSurface, PopupGrab, PopupManager,
    PopupUngrabStrategy, Space, Window, WindowSurfaceType,
};
use smithay::input::keyboard::{Layout as KeyboardLayout, XkbConfig};
use smithay::input::pointer::{
    CursorIcon, CursorImageStatus, CursorImageSurfaceData, Focus,
    GrabStartData as PointerGrabStartData, MotionEvent,
};
use smithay::input::tablet::TabletSeatTrait;
use smithay::input::{Seat, SeatState};
use smithay::output::{self, Output, OutputModeSource, PhysicalProperties, Subpixel, WeakOutput};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{
    Interest, LoopHandle, LoopSignal, Mode, PostAction, RegistrationToken,
};
use smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::WmCapabilities;
use smithay::reexports::wayland_protocols_misc::server_decoration as _server_decoration;
use smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;
use smithay::reexports::wayland_server::backend::{
    ClientData, ClientId, DisconnectReason, GlobalId,
};
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle, Resource};
use smithay::utils::{
    ClockSource, IsAlive as _, Logical, Monotonic, Physical, Point, Rectangle, Scale, Size, Time,
    Transform, SERIAL_COUNTER,
};
use smithay::wayland::background_effect::BackgroundEffectState;
use smithay::wayland::commit_timing::{CommitTimerBarrierStateUserData, Timestamp};
use smithay::wayland::compositor::{
    with_states, with_surface_tree_downward, CompositorClientState, CompositorHandler,
    CompositorState, HookId, SurfaceData, TraversalAction,
};
use smithay::wayland::content_type::ContentTypeState;
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::dmabuf::DmabufState;
use smithay::wayland::drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjState};
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::idle_inhibit::IdleInhibitManagerState;
use smithay::wayland::idle_notify::IdleNotifierState;
use smithay::wayland::input_method::InputMethodManagerState;
use smithay::wayland::keyboard_shortcuts_inhibit::{
    KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
};
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraintsState};
use smithay::wayland::pointer_gestures::PointerGesturesState;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::security_context::SecurityContextState;
use smithay::wayland::selection::data_device::{set_data_device_selection, DataDeviceState};
use smithay::wayland::selection::ext_data_control::DataControlState as ExtDataControlState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::selection::wlr_data_control::DataControlState as WlrDataControlState;
use smithay::wayland::session_lock::{LockSurface, SessionLockManagerState, SessionLocker};
use smithay::wayland::shell::kde::decoration::KdeDecorationState;
use smithay::wayland::shell::wlr_layer::{self, Layer, WlrLayerShellState};
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;
#[cfg(test)]
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use smithay::wayland::socket::ListeningSocketSource;
use smithay::wayland::tablet_manager::TabletManagerState;
use smithay::wayland::text_input::TextInputManagerState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;
use smithay::wayland::xdg_activation::XdgActivationState;
use smithay::wayland::xdg_foreign::XdgForeignState;
use wayland_server::protocol::wl_output::WlOutput;

#[cfg(feature = "dbus")]
use crate::a11y::A11y;
use crate::animation::Clock;
use crate::backend::tty::SurfaceDmabufFeedback;
use crate::backend::{Backend, Headless, RenderResult, RendererBackendType, Tty, Winit};
use crate::cursor::{CursorManager, CursorTextureCache, XCursor};
#[cfg(feature = "dbus")]
use crate::dbus::freedesktop_locale1::Locale1ToClarion;
#[cfg(feature = "dbus")]
use crate::dbus::freedesktop_login1::Login1ToClarion;
#[cfg(feature = "dbus")]
use crate::dbus::gnome_shell_introspect::{self, ClarionToIntrospect, IntrospectToClarion};
#[cfg(feature = "dbus")]
use crate::dbus::gnome_shell_screenshot::{ClarionToScreenshot, ScreenshotToClarion};
use crate::gestures::scroll_swipe_gesture::ScrollSwipeGesture;
use crate::gestures::scroll_tracker::ScrollTracker;
use crate::handlers::{configure_lock_surface, XDG_ACTIVATION_TOKEN_TIMEOUT};
use crate::input::pick_color_grab::PickColorGrab;
use crate::input::{
    apply_libinput_settings, mods_with_finger_scroll_binds, mods_with_mouse_binds,
    mods_with_tablet_stylus_binds, mods_with_wheel_binds, TabletData,
};
use crate::ipc::server::IpcServer;
use crate::layer::MappedLayer;
use crate::layout::workspace::{Workspace, WorkspaceId};
use crate::layout::{HitType, Layout, LayoutElement as _};
use crate::output_schedule::OutputSchedule;
pub use crate::output_schedule::RedrawState;
use crate::protocols::ext_workspace::{self, ExtWorkspaceManagerState};
use crate::protocols::foreign_toplevel::{self, ForeignToplevelManagerState};
use crate::protocols::gamma_control::GammaControlManagerState;
use crate::protocols::mutter_x11_interop::MutterX11InteropManagerState;
use crate::protocols::output_management::OutputManagementManagerState;
use crate::protocols::screencopy::{Screencopy, ScreencopyBuffer, ScreencopyManagerState};
use crate::protocols::virtual_pointer::VirtualPointerManagerState;
use crate::render_helpers::blur::BlurOptions;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::solid_color::SolidColorBuffer;
use crate::render_helpers::texture::TextureBuffer;
use crate::render_helpers::xray::{Xray, XrayPos};
use crate::render_helpers::{
    encompassing_geo, render_to_dmabuf, render_to_encompassing_texture, render_to_texture, shaders,
    RenderCtx, RenderTarget,
};
use crate::scene::{OutputRenderElements, PointerRenderElements, WindowScreenshotRenderElement};
#[cfg(feature = "xdp-gnome-screencast")]
use crate::screencasting::Screencasting;
use crate::ui::config_error_notification::ConfigErrorNotification;
use crate::ui::exit_confirm_dialog::ExitConfirmDialog;
use crate::ui::hotkey_overlay::HotkeyOverlay;
use crate::ui::mru::{MruCloseRequest, WindowMruUi};
use crate::ui::screen_transition::{self, ScreenTransition};
use crate::ui::screenshot_ui::{OutputScreenshot, ScreenshotUi};
use crate::utils::scale::{closest_representable_scale, guess_monitor_scale};
use crate::utils::spawning::{CHILD_DISPLAY, CHILD_ENV};
use crate::utils::vblank_throttle::VBlankThrottle;
use crate::utils::watcher::Watcher;
use crate::utils::xwayland::satellite::Satellite;
use crate::utils::{
    center, center_f64, expand_home, get_monotonic_time, ipc_transform_to_smithay, is_mapped,
    logical_output, make_screenshot_path, output_matches_name, output_size, panel_orientation,
    send_scale_transform, write_png_rgba8, xwayland,
};
use crate::window::mapped::MappedId;
use crate::window::{InitialConfigureState, Mapped, ResolvedWindowRules, Unmapped, WindowRef};

const CLEAR_COLOR_LOCKED: [f32; 4] = [0.3, 0.1, 0.1, 1.];

// We'll try to send frame callbacks at least once a second. We'll make a timer that fires once a
// second, so with the worst timing the maximum interval between two frame callbacks for a surface
// should be ~1.995 seconds.
const FRAME_CALLBACK_THROTTLE: Option<Duration> = Some(Duration::from_millis(995));

pub struct Clarion {
    pub config: Rc<RefCell<Config>>,

    /// Output config from the config file.
    ///
    /// This does not include transient output config changes done via IPC. It is only used when
    /// reloading the config from disk to determine if the output configuration should be reloaded
    /// (and transient changes dropped).
    pub config_file_output_config: clarion_config::Outputs,
    pub(crate) output_preview: Option<crate::output_preview::OutputPreview>,
    pub(crate) confirmed_output_preview: Option<crate::output_preview::ConfirmedOutputPreview>,
    pub(crate) last_cancelled_output_preview: Option<u64>,

    pub config_file_watcher: Option<Watcher>,

    pub event_loop: LoopHandle<'static, State>,
    pub scheduler: Scheduler<()>,
    runtime_executor:
        Option<crate::platform::event_sources::ScopedSource<calloop::futures::Executor<()>, State>>,
    runtime_display:
        Option<crate::platform::event_sources::ScopedSource<Generic<Display<State>>, State>>,
    pub(crate) shutting_down: bool,
    clients: crate::platform::wayland_shutdown::ClientRegistry,
    pub(crate) owned_tasks: crate::utils::owned_tasks::TaskPool,
    keymap_request: u64,
    #[cfg(test)]
    keymap_completions: u64,
    session_restore_request: u64,
    pub stop_signal: LoopSignal,
    pub display_handle: DisplayHandle,

    /// Whether clarion was run with `--session`
    pub is_session_instance: bool,

    /// Name of the Wayland socket.
    ///
    /// This is `None` when creating `Clarion` without a Wayland socket.
    pub socket_name: Option<OsString>,

    pub start_time: Instant,

    /// Whether the at-startup=true window rules are active.
    pub is_at_startup: bool,

    /// Clock for driving animations.
    pub clock: Clock,

    // Each workspace corresponds to a Space. Each workspace generally has one Output mapped to it,
    // however it may have none (when there are no outputs connected) or multiple (when mirroring).
    pub layout: Layout<Mapped>,
    pub(crate) scene_resources: crate::scene::resources::SceneResources,
    /// Retained connector invalidation drain; render damage uses scene elements.
    core_scene_outputs: Vec<String>,
    /// Reused drain for core requests; seat overrides stay in this protocol adapter.
    core_protocol_ops: Vec<crate::layout::CoreProtocolOp>,
    screenshot_tasks: crate::platform::capture::CaptureSlots,
    screencopy_tasks: crate::platform::capture::CaptureSlots,
    capture_scene_epoch: crate::platform::capture::CaptureEpoch,

    // This space does not actually contain any windows, but all outputs are mapped into it
    // according to their global position.
    pub global_space: Space<Window>,

    /// Mapped outputs, sorted by their name and position.
    pub sorted_outputs: Vec<Output>,

    // Windows which don't have a buffer attached yet.
    pub unmapped_windows: HashMap<WlSurface, Unmapped>,

    /// Layer surfaces which don't have a buffer attached yet.
    pub unmapped_layer_surfaces: HashSet<WlSurface>,

    /// Extra data for mapped layer surfaces.
    pub mapped_layer_surfaces: HashMap<LayerSurface, MappedLayer>,

    // Cached root surface for every surface, so that we can access it in destroyed() where the
    // normal get_parent() is cleared out.
    pub root_surface: HashMap<WlSurface, WlSurface>,

    // Dmabuf readiness pre-commit hook for a surface.
    pub dmabuf_pre_commit_hook: HashMap<WlSurface, HookId>,

    /// Clients to notify about their blockers being cleared.
    pub blocker_cleared_tx: Sender<Client>,
    pub blocker_cleared_rx: Receiver<Client>,

    pub output_state: HashMap<Output, OutputState>,

    // When false, we're idling with monitors powered off.
    pub monitors_active: bool,

    /// Whether the laptop lid is closed.
    ///
    /// Libinput guarantees that the lid switch starts in open state, and if it was closed during
    /// startup, libinput will immediately send a closed event.
    pub is_lid_closed: bool,

    pub devices: HashSet<input::Device>,
    pub tablets: HashMap<input::Device, TabletData>,
    pub touch: HashSet<input::Device>,

    // Smithay state.
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub kde_decoration_state: KdeDecorationState,
    pub layer_shell_state: WlrLayerShellState,
    pub session_lock_state: SessionLockManagerState,
    pub foreign_toplevel_state: ForeignToplevelManagerState,
    pub ext_workspace_state: ExtWorkspaceManagerState,
    pub screencopy_state: ScreencopyManagerState,
    pub output_management_state: OutputManagementManagerState,
    pub color_management_state: crate::protocols::color_management::ColorManagementState,
    pub viewporter_state: ViewporterState,
    pub background_effect_state: BackgroundEffectState,
    pub gtk_shell_state: crate::handlers::gtk_shell::GtkShellState,
    pub qt_app_menu_state: crate::handlers::qt_app_menu::QtAppMenuState,
    pub xdg_foreign_state: XdgForeignState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub dmabuf_state: DmabufState,
    pub drm_syncobj_state: Option<DrmSyncobjState>,
    pub fractional_scale_manager_state: FractionalScaleManagerState,
    pub content_type_state: ContentTypeState,
    pub commit_timing_manager_state: crate::commit_timing::CommitTimingManagerState,
    pub(crate) commit_timing: crate::commit_timing::CommitTimingScheduler,
    pub fifo_manager_state: crate::fifo::FifoManagerState,
    pub(crate) fifo: crate::fifo::FifoState,
    pub seat_state: SeatState<State>,
    pub tablet_state: TabletManagerState,
    pub text_input_state: TextInputManagerState,
    pub input_method_state: InputMethodManagerState,
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    pub virtual_keyboard_state: VirtualKeyboardManagerState,
    pub virtual_pointer_state: VirtualPointerManagerState,
    pub pointer_gestures_state: PointerGesturesState,
    pub relative_pointer_state: RelativePointerManagerState,
    pub pointer_constraints_state: PointerConstraintsState,
    pub idle_notifier_state: IdleNotifierState<State>,
    pub idle_inhibit_manager_state: IdleInhibitManagerState,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub wlr_data_control_state: WlrDataControlState,
    pub ext_data_control_state: ExtDataControlState,
    pub popups: PopupManager,
    pub popup_grab: Option<PopupGrabState>,
    pub presentation_state: PresentationState,
    pub security_context_state: SecurityContextState,
    pub gamma_control_manager_state: GammaControlManagerState,
    pub activation_state: XdgActivationState,
    pub mutter_x11_interop_state: MutterX11InteropManagerState,

    // This will not work as is outside of tests, so it is gated with #[cfg(test)] for now. In
    // particular, shaders will need to learn about the single pixel buffer. Also, it must be
    // verified that a black single-pixel-buffer background lets the foreground surface to be
    // unredirected.
    #[cfg(test)]
    pub single_pixel_buffer_state: SinglePixelBufferState,

    pub seat: Seat<State>,
    /// Scancodes of the keys to suppress.
    pub suppressed_keys: HashSet<Keycode>,
    /// Button codes of the mouse buttons to suppress.
    pub suppressed_buttons: HashSet<u32>,
    pub bind_cooldown_timers: HashMap<Key, RegistrationToken>,
    pub bind_repeat_timer: Option<RegistrationToken>,
    pub keyboard_focus: KeyboardFocus,
    pub layer_shell_on_demand_focus: Option<LayerSurface>,
    pub idle_inhibiting_surfaces: HashSet<WlSurface>,
    pub is_fdo_idle_inhibited: Arc<AtomicBool>,
    pub keyboard_shortcuts_inhibiting_surfaces: HashMap<WlSurface, KeyboardShortcutsInhibitor>,

    /// Most recent XKB settings from org.freedesktop.locale1.
    pub xkb_from_locale1: Option<Xkb>,

    pub cursor_manager: CursorManager,
    pub cursor_texture_cache: CursorTextureCache,
    pub cursor_shape_manager_state: CursorShapeManagerState,
    pub dnd_icon: Option<DndIcon>,
    /// Contents under pointer.
    ///
    /// Periodically updated: on motion and other events and in the loop callback. If you require
    /// the real up-to-date contents somewhere, it's better to recompute on the spot.
    ///
    /// This is not pointer focus. I.e. during a click grab, the pointer focus remains on the
    /// client with the grab, but this field will keep updating to the latest contents as if no
    /// grab was active.
    ///
    /// This is primarily useful for emitting pointer motion events for surfaces that move
    /// underneath the cursor on their own (i.e. when the tiling layout moves). In this case, not
    /// taking grabs into account is expected, because we pass the information to pointer.motion()
    /// which passes it down through grabs, which decide what to do with it as they see fit.
    pub pointer_contents: PointContents,
    pub pointer_visibility: PointerVisibility,
    pub pointer_inactivity_timer: Option<RegistrationToken>,
    /// Whether the pointer inactivity timer got reset this event loop iteration.
    ///
    /// Used for limiting the reset to once per iteration, so that it's not spammed with high
    /// resolution mice.
    pub pointer_inactivity_timer_got_reset: bool,
    /// Whether the (idle notifier) activity was notified this event loop iteration.
    ///
    /// Used for limiting the notify to once per iteration, so that it's not spammed with high
    /// resolution mice.
    pub notified_activity_this_iteration: bool,
    pub last_user_activity: Instant,
    /// Idle policy: inactivity turns into a lock or a suspend request.
    pub idle_policy: crate::idle::IdlePolicy,
    /// Pending panel switch-off request. Blanking belongs to the backend, which
    /// the core state cannot reach, so the owner drains this on its refresh.
    pub(crate) pending_dpms_blank: Option<bool>,
    hot_corners: crate::input::hot_corner::HotCornerState,
    pub pointer_constraint_position_hint: Option<Point<f64, Logical>>,
    pub tablet_cursor_location: Option<Point<f64, Logical>>,
    pub gesture_swipe_3f_cumulative: Option<(f64, f64)>,
    pub launcher_pinch: Option<crate::input::launcher_gesture::LauncherPinch>,
    pub overview_scroll_swipe_gesture: ScrollSwipeGesture,
    pub vertical_wheel_tracker: ScrollTracker,
    pub horizontal_wheel_tracker: ScrollTracker,
    pub mods_with_mouse_binds: HashSet<Modifiers>,
    pub mods_with_wheel_binds: HashSet<Modifiers>,
    pub mods_with_tablet_stylus_binds: HashSet<Modifiers>,
    pub vertical_finger_scroll_tracker: ScrollTracker,
    pub horizontal_finger_scroll_tracker: ScrollTracker,
    pub mods_with_finger_scroll_binds: HashSet<Modifiers>,

    pub lock_state: LockState,

    // State that we last sent to the logind LockedHint.
    pub locked_hint: Option<bool>,

    pub screenshot_ui: ScreenshotUi,
    pub config_error_notification: ConfigErrorNotification,
    pub hotkey_overlay: HotkeyOverlay,
    pub exit_confirm_dialog: ExitConfirmDialog,

    pub window_mru_ui: WindowMruUi,
    pub pending_mru_commit: Option<PendingMruCommit>,

    pub pick_window: Option<async_channel::Sender<Option<MappedId>>>,
    pub pick_color: Option<async_channel::Sender<Option<clarion_ipc::PickedColor>>>,

    pub debug_draw_opaque_regions: bool,
    pub debug_draw_damage: bool,

    #[cfg(feature = "dbus")]
    pub dbus: Option<crate::dbus::DBusServers>,
    #[cfg(feature = "dbus")]
    pub a11y_manager: Option<crate::dbus::freedesktop_a11y::Manager>,
    #[cfg(feature = "dbus")]
    pub a11y: A11y,
    #[cfg(feature = "dbus")]
    pub inhibit_power_key_fd: Option<zbus::zvariant::OwnedFd>,

    pub ipc_server: Option<IpcServer>,
    pub ipc_outputs_changed: bool,

    pub satellite: Option<Satellite>,
    pub(crate) satellite_probe_request: u64,

    #[cfg(feature = "xdp-gnome-screencast")]
    pub casting: Screencasting,
}

pub(crate) struct SessionRestoreToken {
    request: u64,
    lock_epoch: crate::platform::capture::CaptureStamp,
}

smithay::delegate_dispatch2!(State);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointerVisibility {
    /// The pointer is visible.
    Visible,
    /// The pointer is invisible, but retains its focus.
    ///
    /// This state is set temporarily after auto-hiding the pointer to keep tooltips open and grabs
    /// ongoing.
    Hidden,
    /// The pointer is invisible and cannot focus.
    ///
    /// Corresponds to a fully disabled pointer, for example after a touchscreen input, or after
    /// the pointer contents changed in a Hidden state.
    Disabled,
}

impl PointerVisibility {
    pub fn is_visible(&self) -> bool {
        matches!(self, Self::Visible)
    }
}

#[derive(Debug)]
pub struct DndIcon {
    pub surface: WlSurface,
    pub offset: Point<i32, Logical>,
}

pub struct OutputState {
    pub global: GlobalId,
    /// Last output coordinate system, independent of layer-shell rearrangements.
    render_geometry: OutputRenderGeometry,
    pub(crate) schedule: OutputSchedule,
    pub on_demand_vrr_enabled: bool,
    pub vblank_throttle: VBlankThrottle,
    /// Solid color buffer for the backdrop that we use instead of clearing to avoid damage
    /// tracking issues and make screenshots easier.
    pub backdrop_buffer: SolidColorBuffer,
    /// Separate element identity for the animated overview wallpaper shade.
    pub overview_shade_buffer: SolidColorBuffer,
    pub xray: Xray,
    pub lock_render_state: LockRenderState,
    pub lock_surface: Option<LockSurface>,
    pub lock_color_buffer: SolidColorBuffer,
    pub(crate) screen_transition: Option<ScreenTransition>,
    /// Damage tracker used for the debug damage visualization.
    pub debug_damage_tracker: OutputDamageTracker,
}

#[derive(Clone, Copy, PartialEq)]
struct OutputRenderGeometry {
    size: Size<i32, Physical>,
    integer_scale: i32,
    fractional_scale: f64,
    transform: Transform,
}

impl OutputRenderGeometry {
    fn new(output: &Output) -> Self {
        let scale = output.current_scale();
        Self {
            size: output.current_mode().unwrap().size,
            integer_scale: scale.integer_scale(),
            fractional_scale: scale.fractional_scale(),
            transform: output.current_transform(),
        }
    }
}

pub struct PopupGrabState {
    pub root: WlSurface,
    pub grab: PopupGrab<State>,
    pub has_keyboard_grab: bool,
}

// The surfaces here are always toplevel surfaces focused as far as clarion's logic is concerned,
// even when popup grabs are active (which means the real keyboard focus is on a popup descending
// from that toplevel surface).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyboardFocus {
    // Layout is focused by default if there's nothing else to focus.
    Layout { surface: Option<WlSurface> },
    LayerShell { surface: WlSurface },
    LockScreen { surface: Option<WlSurface> },
    ScreenshotUi,
    ExitConfirmDialog,
    Overview,
    Mru,
}

#[derive(Default, Clone, PartialEq)]
pub struct PointContents {
    // Output under point.
    pub output: Option<Output>,
    // Surface under point and its location in the global coordinate space.
    //
    // Can be `None` even when `window` is set, for example when the pointer is over the clarion
    // border around the window.
    pub surface: Option<(WlSurface, Point<f64, Logical>)>,
    // If surface belongs to a window, this is that window.
    pub window: Option<(Window, HitType)>,
    // If surface belongs to a layer surface, this is that layer surface.
    pub layer: Option<LayerSurface>,
    // Pointer is over a hot corner.
    pub hot_corner: Option<HotCornerKind>,
}

pub use crate::input::hot_corner::HotCornerKind;

#[derive(Debug, Default)]
pub enum LockState {
    #[default]
    Unlocked,
    WaitingForSurfaces {
        confirmation: SessionLocker,
        deadline_token: RegistrationToken,
    },
    Locking(SessionLocker),
    Locked(ExtSessionLockV1),
}

#[derive(PartialEq, Eq)]
pub enum LockRenderState {
    /// The output displays a normal session frame.
    Unlocked,
    /// The output displays a locked frame.
    Locked,
}

enum ScreencopySubmission {
    Dmabuf(SyncPoint),
    Shm(
        crate::platform::pixel_readback::PendingPixels,
        crate::platform::capture::CapturePermit,
    ),
}

// Not related to the one in Smithay.
//
// This state keeps track of when a surface last received a frame callback.
struct SurfaceFrameThrottlingState {
    /// Output and sequence that the frame callback was last sent at.
    last_sent_at: RefCell<Option<(Output, u32)>>,
}

pub enum CenterCoords {
    Separately,
    Both,
    // Force centering even if the cursor is already in the rectangle.
    BothAlways,
}

#[derive(Clone, PartialEq, Eq)]
pub enum CastTarget {
    // Dynamic cast before selecting anything.
    Nothing,
    Output {
        output: WeakOutput,
        /// Cached name of the output.
        name: String,
    },
    Window {
        id: u64,
    },
}

impl CastTarget {
    pub fn output(output: &Output) -> Self {
        Self::Output {
            output: output.downgrade(),
            name: output.name(),
        }
    }

    pub fn matches_output(&self, weak: &WeakOutput) -> bool {
        matches!(self, CastTarget::Output { output, .. } if output == weak)
    }

    pub fn matches(&self, ipc: &clarion_ipc::CastTarget) -> bool {
        use CastTarget::*;
        match (self, ipc) {
            (Nothing, clarion_ipc::CastTarget::Nothing {}) => true,
            (Output { name, .. }, clarion_ipc::CastTarget::Output { name: ipc_name }) => {
                name == ipc_name
            }
            (Window { id }, clarion_ipc::CastTarget::Window { id: ipc_id }) => id == ipc_id,
            _ => false,
        }
    }

    pub fn make_ipc(&self) -> clarion_ipc::CastTarget {
        use CastTarget::*;
        match self {
            Nothing => clarion_ipc::CastTarget::Nothing {},
            Output { name, .. } => clarion_ipc::CastTarget::Output { name: name.clone() },
            Window { id } => clarion_ipc::CastTarget::Window { id: *id },
        }
    }
}

/// Pending update to a window's focus timestamp.
#[derive(Debug)]
pub struct PendingMruCommit {
    id: MappedId,
    token: RegistrationToken,
    stamp: Duration,
}

impl Default for SurfaceFrameThrottlingState {
    fn default() -> Self {
        Self {
            last_sent_at: RefCell::new(None),
        }
    }
}

impl KeyboardFocus {
    pub fn surface(&self) -> Option<&WlSurface> {
        match self {
            KeyboardFocus::Layout { surface } => surface.as_ref(),
            KeyboardFocus::LayerShell { surface } => Some(surface),
            KeyboardFocus::LockScreen { surface } => surface.as_ref(),
            KeyboardFocus::ScreenshotUi => None,
            KeyboardFocus::ExitConfirmDialog => None,
            KeyboardFocus::Overview => None,
            KeyboardFocus::Mru => None,
        }
    }

    pub fn into_surface(self) -> Option<WlSurface> {
        match self {
            KeyboardFocus::Layout { surface } => surface,
            KeyboardFocus::LayerShell { surface } => Some(surface),
            KeyboardFocus::LockScreen { surface } => surface,
            KeyboardFocus::ScreenshotUi => None,
            KeyboardFocus::ExitConfirmDialog => None,
            KeyboardFocus::Overview => None,
            KeyboardFocus::Mru => None,
        }
    }

    pub fn is_layout(&self) -> bool {
        matches!(self, KeyboardFocus::Layout { .. })
    }

    pub fn is_overview(&self) -> bool {
        matches!(self, KeyboardFocus::Overview)
    }
}

pub struct State {
    pub backend: Backend,
    pub clarion: Clarion,
}

impl Drop for State {
    fn drop(&mut self) {
        self.prepare_shutdown();
    }
}

impl State {
    /// Cancel event-loop futures and detach the Wayland display source before
    /// dropping device owners. Call after dispatch returns; Drop also covers unwind and
    /// fixture teardown. This only closes this instance, never the global GPU
    /// lifecycle shared by other compositor fixtures.
    pub fn prepare_shutdown(&mut self) {
        if mem::replace(&mut self.clarion.shutting_down, true) {
            return;
        }
        // Take before Drop so repeated calls, including State::drop, are inert.
        // Cancel Async futures first; they can unregister nested loop sources.
        drop(self.clarion.runtime_executor.take());
        let clients = self.clarion.clients.snapshot();
        if let Some(mut display) = self.clarion.runtime_display.take() {
            // The owner is local, so destroyed callbacks can borrow all of
            // State without re-entering a borrow of this registered source.
            let result = display.with_source_mut(|source| {
                crate::platform::wayland_shutdown::disconnect_clients(source, self, clients)
            });
            if let Err(error) = result {
                warn!(
                    ?error,
                    "error dispatching Wayland client destruction during shutdown"
                );
            }
            drop(display);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Config,
        event_loop: LoopHandle<'static, State>,
        stop_signal: LoopSignal,
        display: Display<State>,
        headless: bool,
        create_wayland_socket: bool,
        is_session_instance: bool,
        requested_renderer: RendererBackendType,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let _span = clarion_tracy_span!("State::new");

        let config = Rc::new(RefCell::new(config));

        let has_display = env::var_os("WAYLAND_DISPLAY").is_some()
            || env::var_os("WAYLAND_SOCKET").is_some()
            || env::var_os("DISPLAY").is_some();

        let mut bootstrap =
            crate::platform::device_bridge::DeviceBootstrap::new(event_loop.clone());
        let mut backend = if headless {
            let mut headless = Headless::new(config.clone(), requested_renderer);
            if requested_renderer.is_vulkan() {
                if let Err(err) = headless.add_renderer() {
                    warn!("headless Vulkan backend unavailable ({err:#}); falling back to GLES");
                    headless.set_requested_renderer(RendererBackendType::Gles);
                    headless
                        .add_renderer()
                        .context("error initializing the headless GLES backend")?;
                }
            }
            Backend::Headless(headless)
        } else if has_display {
            let winit = Winit::new(config.clone(), &mut bootstrap, requested_renderer)?;
            Backend::Winit(winit)
        } else {
            let tty = Tty::new(config.clone(), &mut bootstrap, requested_renderer)
                .context("error initializing the TTY backend")?;
            Backend::Tty(tty)
        };

        let mut clarion = Clarion::new(
            config.clone(),
            event_loop,
            stop_signal,
            display,
            &backend,
            create_wayland_socket,
            is_session_instance,
        );
        backend.init(&mut crate::platform::device_bridge::DeviceCoordinator::new(
            &mut clarion,
        ));

        let mut state = Self { backend, clarion };

        if state.backend.supports_color_management() {
            state.backend.enable_color_management();
            let formats = state
                .backend
                .with_renderer(|renderer| {
                    use smithay::backend::renderer::ImportMemWl as _;
                    match renderer {
                        crate::render_helpers::renderer::AnyRenderer::Native(native) => {
                            native.shm_formats().collect::<Vec<_>>()
                        }
                        crate::render_helpers::renderer::AnyRenderer::Gles(_) => Vec::new(),
                    }
                })
                .unwrap_or_default();
            state.clarion.shm_state.update_formats(formats);
            state.clarion.color_management_state =
                crate::protocols::color_management::ColorManagementState::new::<State>(
                    &state.clarion.display_handle,
                    true,
                );
        }

        // Load the xkb_file config option if set by the user.
        state.load_xkb_file();
        // Initialize some IPC server state.
        state.ipc_keyboard_layouts_changed();
        // Focus the default monitor if set by the user.
        state.focus_default_monitor();

        Ok(state)
    }

    pub fn refresh_and_flush_clients(&mut self) {
        let _span = clarion_tracy_span!("State::refresh_and_flush_clients");

        {
            let _latency =
                crate::utils::latency::Span::new(crate::utils::latency::Operation::Refresh);
            self.refresh();
        }

        // Advance animations to the current time (not target render time) before rendering outputs
        // in order to clear completed animations and render elements. Even if we're not rendering,
        // it's good to advance every now and then so the workspace clean-up and animations don't
        // build up (the 1 second frame callback timer will call this line).
        {
            let _latency =
                crate::utils::latency::Span::new(crate::utils::latency::Operation::Animations);
            // Apply this batch's latest motion before event-time effect simulation;
            // presentation forecasts must never become the canonical wobble pose.
            self.clarion.layout.flush_pending_move();
            self.clarion.advance_animations();
        }

        self.clarion.flush_core_scene_invalidations();
        self.clarion.redraw_queued_outputs(&mut self.backend);
        // Motion may be coalesced inside redraw. Keep its final cleanup request
        // after submission, when queue_redraw can mark the pending vblank state.
        self.clarion.flush_core_scene_invalidations();

        {
            let _span = clarion_tracy_span!("flush_clients");
            let _latency =
                crate::utils::latency::Span::new(crate::utils::latency::Operation::Flush);
            self.clarion.display_handle.flush_clients().unwrap();
        }

        #[cfg(feature = "dbus")]
        self.clarion.update_locked_hint();

        // Clear the time so it's fetched afresh next iteration.
        self.clarion.clock.clear();
        self.clarion.pointer_inactivity_timer_got_reset = false;
        self.clarion.notified_activity_this_iteration = false;
    }

    // We monitor both libinput and logind: libinput is always there (including without DBus), but
    // it misses some switch events (e.g. after unsuspend) on some systems.
    pub fn set_lid_closed(&mut self, is_closed: bool) {
        if self.clarion.is_lid_closed == is_closed {
            return;
        }

        debug!("laptop lid {}", if is_closed { "closed" } else { "opened" });
        self.clarion.is_lid_closed = is_closed;
        self.backend.on_output_config_changed(
            &mut crate::platform::device_bridge::DeviceCoordinator::new(&mut self.clarion),
        );
    }

    fn refresh(&mut self) {
        let _span = clarion_tracy_span!("State::refresh");

        // The idle policy asks for panel blanking, but only the owner can reach
        // the backend. Drain it here so a wake-up is served on the next turn
        // rather than waiting for the one-second policy tick.
        if let Some(blanked) = self.clarion.pending_dpms_blank.take() {
            self.backend.set_outputs_blanked(blanked);
        }

        // Handle commits for surfaces whose blockers cleared this cycle. This should happen before
        // layout.refresh() since this is where these surfaces handle commits.
        self.notify_blocker_cleared();

        // These should be called periodically, before flushing the clients.
        self.clarion.popups.cleanup();
        self.refresh_popup_grab();
        self.update_keyboard_focus();

        // Should be called before refresh_layout() because that one will refresh other window
        // states and then send a pending configure.
        self.clarion.refresh_window_states();

        // Needs to be called after updating the keyboard focus.
        self.clarion.refresh_layout();

        self.clarion
            .cursor_manager
            .check_cursor_image_surface_alive();
        self.clarion.refresh_pointer_outputs();
        self.clarion.global_space.refresh();
        self.clarion.refresh_idle_inhibit();
        self.refresh_pointer_contents();
        foreign_toplevel::refresh(self);
        ext_workspace::refresh(self);

        #[cfg(feature = "xdp-gnome-screencast")]
        self.clarion.refresh_mapped_cast_outputs();
        // Should happen before refresh_window_rules(), but after anything that can start or stop
        // screencasts.
        #[cfg(feature = "xdp-gnome-screencast")]
        self.clarion.refresh_mapped_cast_window_rules();
        self.ipc_refresh_casts();

        self.clarion.refresh_window_rules();
        self.refresh_ipc_outputs();
        crate::protocols::color_management::ColorManagementState::refresh(self);
        self.ipc_refresh_layout();
        self.ipc_refresh_keyboard_layout_index();

        // Needs to be called after updating the keyboard focus.
        #[cfg(feature = "dbus")]
        self.clarion.refresh_a11y();
    }

    fn notify_blocker_cleared(&mut self) {
        let dh = self.clarion.display_handle.clone();
        while let Ok(client) = self.clarion.blocker_cleared_rx.try_recv() {
            trace!("calling blocker_cleared");
            self.client_compositor_state(&client)
                .blocker_cleared(self, &dh);
        }
    }

    pub fn move_cursor(&mut self, location: Point<f64, Logical>) {
        let mut under = match self.clarion.pointer_visibility {
            PointerVisibility::Disabled => PointContents::default(),
            _ => self.clarion.contents_under(location),
        };

        // Disable the hidden pointer if the contents underneath have changed.
        if !self.clarion.pointer_visibility.is_visible() && self.clarion.pointer_contents != under {
            self.clarion.pointer_visibility = PointerVisibility::Disabled;

            // When setting PointerVisibility::Hidden together with pointer contents changing,
            // we can change straight to nothing to avoid one frame of hover. Notably, this can
            // be triggered through warp-mouse-to-focus combined with hide-when-typing.
            under = PointContents::default();
        }

        self.clarion.pointer_contents.clone_from(&under);

        let pointer = &self.clarion.seat.get_pointer().unwrap();
        pointer.motion(
            self,
            under.surface,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: get_monotonic_time().as_millis() as u32,
            },
        );
        pointer.frame(self);

        self.clarion.maybe_activate_pointer_constraint();

        // We do not show the pointer on programmatic or keyboard movement.

        // FIXME: granular
        self.clarion.queue_redraw_all();
    }

    /// Moves cursor within the specified rectangle, only adjusting coordinates if needed.
    fn move_cursor_to_rect(&mut self, rect: Rectangle<f64, Logical>, mode: CenterCoords) -> bool {
        let pointer = &self.clarion.seat.get_pointer().unwrap();
        let cur_loc = pointer.current_location();
        let x_in_bound = cur_loc.x >= rect.loc.x && cur_loc.x <= rect.loc.x + rect.size.w;
        let y_in_bound = cur_loc.y >= rect.loc.y && cur_loc.y <= rect.loc.y + rect.size.h;

        let p = match mode {
            CenterCoords::Separately => {
                if x_in_bound && y_in_bound {
                    return false;
                } else if y_in_bound {
                    // adjust x
                    Point::from((rect.loc.x + rect.size.w / 2.0, cur_loc.y))
                } else if x_in_bound {
                    // adjust y
                    Point::from((cur_loc.x, rect.loc.y + rect.size.h / 2.0))
                } else {
                    // adjust x and y
                    center_f64(rect)
                }
            }
            CenterCoords::Both => {
                if x_in_bound && y_in_bound {
                    return false;
                } else {
                    // adjust x and y
                    center_f64(rect)
                }
            }
            CenterCoords::BothAlways => center_f64(rect),
        };

        self.move_cursor(p);
        true
    }

    pub fn move_cursor_to_focused_tile(&mut self, mode: CenterCoords) -> bool {
        if !self.clarion.keyboard_focus.is_layout() {
            return false;
        }

        if self.clarion.tablet_cursor_location.is_some() {
            return false;
        }

        let Some(output) = self.clarion.layout.active_output() else {
            return false;
        };
        let monitor = self.clarion.layout.monitor_for_output(output).unwrap();

        let mut rv = false;
        let rect = monitor.active_window_visual_rectangle();

        if let Some(rect) = rect {
            let output_geo = self.clarion.global_space.output_geometry(output).unwrap();
            let mut rect = rect;
            rect.loc += output_geo.loc.to_f64();
            rv = self.move_cursor_to_rect(rect, mode);
        }

        rv
    }

    pub fn focus_default_monitor(&mut self) {
        // Our default target is the first output in sorted order.
        let Some(mut target) = self.clarion.sorted_outputs.first().cloned() else {
            // No outputs are connected.
            return;
        };

        let config = self.clarion.config.borrow();
        for config in &config.outputs.0 {
            if !config.focus_at_startup {
                continue;
            }
            if let Some(output) = self.clarion.output_by_name_match(&config.name) {
                target = output.clone();
                break;
            }
        }
        drop(config);

        self.clarion.layout.focus_output(&target);
        self.move_cursor_to_output(&target);
    }

    /// Focus a specific window, taking care of a potential active output change and cursor
    /// warp.
    pub fn focus_window(&mut self, window: &Window) {
        let active_output = self.clarion.layout.active_output().cloned();

        self.clarion.layout.activate_window(window);

        let new_active = self.clarion.layout.active_output().cloned();
        if new_active != active_output {
            if !self.maybe_warp_cursor_to_focus_centered() {
                if let Some(output) = new_active {
                    self.move_cursor_to_output(&output);
                }
            }
        } else {
            self.maybe_warp_cursor_to_focus();
        }

        // FIXME: granular
        self.clarion.queue_redraw_all();
    }

    pub fn confirm_mru(&mut self) {
        if let Some(window) = self.clarion.close_mru(MruCloseRequest::Confirm) {
            // focus_window() will warp the cursor to the window only when the keyboard focus is on
            // the layout. However, right now the keyboard focus is still on the MRU (that we had
            // just closed) since it's only updated at the end of the event loop cycle. Force-update
            // the keyboard focus here to make cursor warping work.
            self.update_keyboard_focus();

            self.focus_window(&window);
        }
    }

    pub fn maybe_warp_cursor_to_focus(&mut self) -> bool {
        let focused = match self.clarion.config.borrow().input.warp_mouse_to_focus {
            None => return false,
            Some(inner) => match inner.mode {
                None => CenterCoords::Separately,
                Some(WarpMouseToFocusMode::CenterXy) => CenterCoords::Both,
                Some(WarpMouseToFocusMode::CenterXyAlways) => CenterCoords::BothAlways,
            },
        };
        self.move_cursor_to_focused_tile(focused)
    }

    pub fn maybe_warp_cursor_to_focus_centered(&mut self) -> bool {
        let focused = match self.clarion.config.borrow().input.warp_mouse_to_focus {
            None => return false,
            Some(inner) => match inner.mode {
                None => CenterCoords::Both,
                Some(WarpMouseToFocusMode::CenterXy) => CenterCoords::Both,
                Some(WarpMouseToFocusMode::CenterXyAlways) => CenterCoords::BothAlways,
            },
        };
        self.move_cursor_to_focused_tile(focused)
    }

    pub fn refresh_pointer_contents(&mut self) {
        // Don't move the mouse pointer while the user is interacting with the tablet, as it causes
        // unwanted jumps for the client.
        if self.clarion.tablet_cursor_location.is_some() {
            return;
        }

        let _span = clarion_tracy_span!("Clarion::refresh_pointer_contents");

        let pointer = &self.clarion.seat.get_pointer().unwrap();
        let location = pointer.current_location();

        if !self.clarion.exit_confirm_dialog.is_open()
            && !self.clarion.is_locked()
            && !self.clarion.screenshot_ui.is_open()
        {
            // Don't refresh cursor focus during transitions.
            if let Some((output, _)) = self.clarion.output_under(location) {
                if self
                    .clarion
                    .layout
                    .monitor_for_output(output)
                    .is_some_and(|monitor| monitor.are_transitions_ongoing())
                {
                    return;
                }
            }
        }

        if !self.update_pointer_contents() {
            return;
        }

        pointer.frame(self);

        // Pointer motion from a surface to nothing triggers a cursor change to default, which
        // means we may need to redraw.

        // FIXME: granular
        self.clarion.queue_redraw_all();
    }

    pub fn update_pointer_contents(&mut self) -> bool {
        let _span = clarion_tracy_span!("Clarion::update_pointer_contents");

        let pointer = &self.clarion.seat.get_pointer().unwrap();
        let location = pointer.current_location();
        let mut under = match self.clarion.pointer_visibility {
            PointerVisibility::Disabled => PointContents::default(),
            _ => self.clarion.contents_under(location),
        };

        // We're not changing the global cursor location here, so if the contents did not change,
        // then nothing changed.
        if self.clarion.pointer_contents == under {
            return false;
        }

        // Disable the hidden pointer if the contents underneath have changed.
        if !self.clarion.pointer_visibility.is_visible() {
            self.clarion.pointer_visibility = PointerVisibility::Disabled;

            // When setting PointerVisibility::Hidden together with pointer contents changing,
            // we can change straight to nothing to avoid one frame of hover. Notably, this can
            // be triggered through warp-mouse-to-focus combined with hide-when-typing.
            under = PointContents::default();
            if self.clarion.pointer_contents == under {
                return false;
            }
        }

        self.clarion.pointer_contents.clone_from(&under);

        pointer.motion(
            self,
            under.surface,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: get_monotonic_time().as_millis() as u32,
            },
        );

        self.clarion.maybe_activate_pointer_constraint();

        true
    }

    pub fn move_cursor_to_output(&mut self, output: &Output) {
        let geo = self.clarion.global_space.output_geometry(output).unwrap();
        self.move_cursor(center(geo).to_f64());
    }

    pub fn refresh_popup_grab(&mut self) {
        if let Some(grab) = &mut self.clarion.popup_grab {
            if grab.grab.has_ended() {
                self.clarion.popup_grab = None;
            }
        }
    }

    pub fn update_keyboard_focus(&mut self) {
        // This entry point is used both by refresh and immediate input boundaries.
        // Consume the core's latest focus request before dispatching the next key
        // or button; lock surfaces, layer grabs and shell UI still arbitrate here.
        let mut operations = mem::take(&mut self.clarion.core_protocol_ops);
        self.clarion.layout.drain_core_protocol_ops(&mut operations);
        let current = self.clarion.layout.core_focus_id();
        let requested = operations
            .last()
            .map_or(current, |operation| match operation {
                crate::layout::CoreProtocolOp::ReconcileFocus { previous, current } => {
                    if previous != current {
                        trace!(?previous, ?current, "reconciling core focus request");
                    }
                    *current
                }
            });
        // No platform focus cache can supersede the core. A retired request must
        // never revive an unmapped/minimized window or bypass a protocol grab.
        debug_assert_eq!(requested, current, "stale core focus request");
        let focused_id = if requested == current {
            requested
        } else {
            current
        };
        operations.clear();
        self.clarion.core_protocol_ops = operations;
        self.update_keyboard_focus_for_core(focused_id);
    }

    fn update_keyboard_focus_for_core(&mut self, focused_id: Option<u64>) {
        // Clean up on-demand layer surface focus if necessary.
        if let Some(surface) = &self.clarion.layer_shell_on_demand_focus {
            // Still alive and has on-demand interactivity.
            let mut good = surface.alive()
                && surface.cached_state().keyboard_interactivity
                    == wlr_layer::KeyboardInteractivity::OnDemand;

            if let Some(mapped) = self.clarion.mapped_layer_surfaces.get(surface) {
                // Check if it moved to the overview backdrop.
                if mapped.place_within_backdrop() {
                    good = false;
                }
            } else {
                // The layer surface is alive but it got unmapped.
                good = false;
            }

            if !good {
                self.clarion.layer_shell_on_demand_focus = None;
            }
        }

        // Compute the current focus.
        let focus = if self.clarion.exit_confirm_dialog.is_open() {
            KeyboardFocus::ExitConfirmDialog
        } else if self.clarion.is_locked() {
            KeyboardFocus::LockScreen {
                surface: self.clarion.lock_surface_focus(),
            }
        } else if self.clarion.screenshot_ui.is_open() {
            KeyboardFocus::ScreenshotUi
        } else if self.clarion.window_mru_ui.is_open() {
            KeyboardFocus::Mru
        } else if let Some(output) = self.clarion.layout.active_output() {
            let mon = self.clarion.layout.monitor_for_output(output).unwrap();
            let layers = layer_map_for_output(output);

            // Explicitly check for layer-shell popup grabs here, our keyboard focus will stay on
            // the root layer surface while it has grabs.
            let layer_grab = self.clarion.popup_grab.as_ref().and_then(|g| {
                layers
                    .layer_for_surface(&g.root, WindowSurfaceType::TOPLEVEL)
                    .and_then(|l| l.can_receive_keyboard_focus().then(|| (&g.root, l.layer())))
            });
            let grab_on_layer = |layer: Layer| {
                layer_grab
                    .and_then(move |(s, l)| if l == layer { Some(s.clone()) } else { None })
                    .map(|surface| KeyboardFocus::LayerShell { surface })
            };

            let layout_focus = || {
                focused_id
                    .and_then(|id| self.clarion.layout.core_focus_window(id))
                    .map(|win| win.toplevel().wl_surface().clone())
                    .map(|surface| KeyboardFocus::Layout {
                        surface: Some(surface),
                    })
            };

            let excl_focus_on_layer = |layer| {
                layers.layers_on(layer).find_map(|surface| {
                    if surface.cached_state().keyboard_interactivity
                        != wlr_layer::KeyboardInteractivity::Exclusive
                    {
                        return None;
                    }

                    let mapped = self.clarion.mapped_layer_surfaces.get(surface)?;
                    if mapped.place_within_backdrop() {
                        return None;
                    }

                    let surface = surface.wl_surface().clone();
                    Some(KeyboardFocus::LayerShell { surface })
                })
            };

            let on_d_focus_on_layer = |layer| {
                layers.layers_on(layer).find_map(|surface| {
                    let is_on_demand_surface =
                        Some(surface) == self.clarion.layer_shell_on_demand_focus.as_ref();
                    is_on_demand_surface
                        .then(|| surface.wl_surface().clone())
                        .map(|surface| KeyboardFocus::LayerShell { surface })
                })
            };

            // Prefer exclusive focus on a layer, then check on-demand focus.
            let focus_on_layer =
                |layer| excl_focus_on_layer(layer).or_else(|| on_d_focus_on_layer(layer));

            let is_overview_open = self.clarion.layout.is_overview_open();

            let mut surface = grab_on_layer(Layer::Overlay);
            // FIXME: we shouldn't prioritize the top layer grabs over regular overlay input or a
            // fullscreen layout window. This will need tracking in grab() to avoid handing it out
            // in the first place. Or a better way to structure this code.
            surface = surface.or_else(|| grab_on_layer(Layer::Top));

            if !is_overview_open {
                surface = surface.or_else(|| grab_on_layer(Layer::Bottom));
                surface = surface.or_else(|| grab_on_layer(Layer::Background));
            }

            surface = surface.or_else(|| focus_on_layer(Layer::Overlay));

            if mon.render_above_top_layer() {
                surface = surface.or_else(layout_focus);
                surface = surface.or_else(|| focus_on_layer(Layer::Top));
                surface = surface.or_else(|| focus_on_layer(Layer::Bottom));
                surface = surface.or_else(|| focus_on_layer(Layer::Background));
            } else {
                surface = surface.or_else(|| focus_on_layer(Layer::Top));

                if is_overview_open {
                    surface = Some(surface.unwrap_or(KeyboardFocus::Overview));
                }

                surface = surface.or_else(|| on_d_focus_on_layer(Layer::Bottom));
                surface = surface.or_else(|| on_d_focus_on_layer(Layer::Background));
                surface = surface.or_else(layout_focus);

                // Bottom and background layers can only receive exclusive focus when there are no
                // layout windows.
                surface = surface.or_else(|| excl_focus_on_layer(Layer::Bottom));
                surface = surface.or_else(|| excl_focus_on_layer(Layer::Background));
            }

            surface.unwrap_or(KeyboardFocus::Layout { surface: None })
        } else {
            KeyboardFocus::Layout { surface: None }
        };

        let keyboard = self.clarion.seat.get_keyboard().unwrap();
        if self.clarion.keyboard_focus != focus {
            trace!(
                "keyboard focus changed from {:?} to {:?}",
                self.clarion.keyboard_focus,
                focus
            );

            // Tell the windows their new focus state for window rule purposes.
            if let KeyboardFocus::Layout {
                surface: Some(surface),
            } = &self.clarion.keyboard_focus
            {
                if let Some((mapped, _)) = self.clarion.layout.find_window_and_output_mut(surface) {
                    mapped.set_is_focused(false);
                }
            }
            if let KeyboardFocus::Layout {
                surface: Some(surface),
            } = &focus
            {
                if let Some((mapped, _)) = self.clarion.layout.find_window_and_output_mut(surface) {
                    mapped.set_is_focused(true);

                    // If `mapped` does not have a focus timestamp, then the window is newly
                    // created/mapped and a timestamp is unconditionally created.
                    //
                    // If `mapped` already has a timestamp only update it after the focus lock-in
                    // period has gone by without the focus having elsewhere.
                    let stamp = get_monotonic_time();

                    let debounce = self.clarion.config.borrow().recent_windows.debounce_ms;
                    let debounce = Duration::from_millis(u64::from(debounce));

                    if mapped.get_focus_timestamp().is_none() || debounce.is_zero() {
                        mapped.set_focus_timestamp(stamp);
                    } else {
                        let timer = Timer::from_duration(debounce);

                        let focus_token = self
                            .clarion
                            .event_loop
                            .insert_source(timer, move |_, _, state| {
                                state.clarion.mru_apply_keyboard_commit();
                                TimeoutAction::Drop
                            })
                            .unwrap();
                        if let Some(PendingMruCommit { token, .. }) =
                            self.clarion.pending_mru_commit.replace(PendingMruCommit {
                                id: mapped.id(),
                                token: focus_token,
                                stamp,
                            })
                        {
                            self.clarion.event_loop.remove(token);
                        }
                    }
                }
            }

            if let Some(grab) = self.clarion.popup_grab.as_mut() {
                if grab.has_keyboard_grab && Some(&grab.root) != focus.surface() {
                    trace!(
                        "grab root {:?} is not the new focus {:?}, ungrabbing",
                        grab.root,
                        focus
                    );

                    grab.grab.ungrab(PopupUngrabStrategy::All);
                    keyboard.unset_grab(self);
                    self.clarion.seat.get_pointer().unwrap().unset_grab(
                        self,
                        SERIAL_COUNTER.next_serial(),
                        get_monotonic_time().as_millis() as u32,
                    );
                    self.clarion.popup_grab = None;
                }
            }

            if self.clarion.config.borrow().input.keyboard.track_layout == TrackLayout::Window {
                let current_layout = keyboard.with_xkb_state(self, |context| {
                    let xkb = context.xkb().lock().unwrap();
                    xkb.active_layout()
                });

                let mut new_layout = current_layout;
                // Store the currently active layout for the surface.
                if let Some(current_focus) = self.clarion.keyboard_focus.surface() {
                    with_states(current_focus, |data| {
                        let cell = data
                            .data_map
                            .get_or_insert::<Cell<KeyboardLayout>, _>(Cell::default);
                        cell.set(current_layout);
                    });
                }

                if let Some(focus) = focus.surface() {
                    new_layout = with_states(focus, |data| {
                        let cell = data.data_map.get_or_insert::<Cell<KeyboardLayout>, _>(|| {
                            // The default layout is effectively the first layout in the
                            // keymap, so use it for new windows.
                            Cell::new(KeyboardLayout::default())
                        });
                        cell.get()
                    });
                }
                if new_layout != current_layout && focus.surface().is_some() {
                    keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
                    keyboard.with_xkb_state(self, |mut context| {
                        context.set_layout(new_layout);
                    });
                }
            }

            self.clarion.keyboard_focus.clone_from(&focus);
            keyboard.set_focus(self, focus.into_surface(), SERIAL_COUNTER.next_serial());

            // FIXME: can be more granular.
            self.clarion.queue_redraw_all();
        }
    }

    /// Retry only work refused by bounded GPU admission. Ordinary idle completion
    /// still flushes client releases without starting an unnecessary repaint loop.
    pub(crate) fn on_native_gpu_completion(&mut self) {
        let retry = self
            .backend
            .with_renderer(|renderer| {
                let crate::render_helpers::renderer::AnyRenderer::Native(native) = renderer else {
                    return false;
                };
                if let Err(error) = native.retire_completed() {
                    warn!("error retiring completed Vulkan work: {error}");
                }
                native.take_backpressure()
            })
            .unwrap_or(false);
        if retry {
            self.clarion.queue_redraw_all();
        }
    }

    /// Compiler completions contain owned source/SPIR-V only. A separate bounded device
    /// worker prepares GPU resources; the owner later validates and publishes its result.
    pub(crate) fn on_native_shader_completion(
        &mut self,
        completion: crate::render_helpers::vulkan::custom_shader::CompileCompletion<
            crate::render_helpers::vulkan::custom_shader::PreparedShader,
        >,
    ) {
        let data = completion.into_data();
        let installed = self
            .backend
            .with_renderer(move |renderer| {
                let crate::render_helpers::renderer::AnyRenderer::Native(native) = renderer else {
                    return false;
                };
                let installed = native.apply_shader_completion(data);
                native.retry_deferred_shader_compiles();
                installed
            })
            .unwrap_or(false);
        if installed {
            self.clarion.update_shaders();
            self.clarion.queue_redraw_all();
        }
    }

    /// Device-only preparation is already finished. Instance/source checks precede publication;
    /// stale payloads retire on the bounded preparation worker, never in this owner callback.
    pub(crate) fn on_native_gpu_preparation_event(
        &mut self,
        event: crate::render_helpers::vulkan::custom_shader::GpuPreparationEvent,
    ) {
        let (published, retry) = self
            .backend
            .with_renderer(move |renderer| {
                let crate::render_helpers::renderer::AnyRenderer::Native(native) = renderer else {
                    return (false, false);
                };
                let published = native.apply_gpu_preparation_event(event);
                native.retry_deferred_shader_compiles();
                (published, native.take_backpressure())
            })
            .unwrap_or((false, false));
        if published {
            self.clarion.update_shaders();
        }
        if published || retry {
            self.clarion.queue_redraw_all();
        }
    }

    /// Compile only on the owner thread: libxkbcommon context ownership stays local.
    fn apply_xkb_text(&mut self, keymap: String) -> anyhow::Result<()> {
        let keyboard = self.clarion.seat.get_keyboard().unwrap();
        let num_lock = keyboard.modifier_state().num_lock;
        keyboard
            .set_keymap_from_string(self, keymap)
            .context("failed to set keymap")?;
        let mut mods_state = keyboard.modifier_state();
        if mods_state.num_lock != num_lock {
            mods_state.num_lock = num_lock;
            keyboard.set_modifier_state(mods_state);
        }
        Ok(())
    }

    /// Reading and UTF-8 validation use owned bytes; stale completions cannot replace a newer map.
    fn queue_xkb_file(&mut self, path: String, fallback: Option<Xkb>) {
        self.clarion.keymap_request = self.clarion.keymap_request.wrapping_add(1);
        let request = self.clarion.keymap_request;
        let task = self.clarion.owned_tasks.submit(move || {
            let path = PathBuf::from(path);
            let path = expand_home(&path)
                .map_err(|e| e.to_string())?
                .unwrap_or(path);
            crate::utils::owned_tasks::read_regular_text(&path, 4 * 1024 * 1024)
                .map_err(|e| format!("failed to read xkb_file: {e}"))
        });
        let event_loop = self.clarion.event_loop.clone();
        let future = async move {
            let result = match task {
                Ok(task) => task.finish().await,
                Err(err) => Err(err),
            };
            event_loop.insert_idle(move |state| {
                #[cfg(test)]
                {
                    state.clarion.keymap_completions += 1;
                }
                if state.clarion.keymap_request != request {
                    return;
                }
                let result = result
                    .map_err(anyhow::Error::msg)
                    .and_then(|text| state.apply_xkb_text(text));
                if let Err(err) = result {
                    warn!("error loading xkb_file: {err:#}");
                    if let Some(xkb) = fallback {
                        state.apply_xkb_fallback(xkb);
                    }
                }
                state.ipc_keyboard_layouts_changed();
            });
        };
        if let Err(err) = self.clarion.scheduler.schedule(future) {
            warn!("error scheduling xkb_file: {err:?}");
        }
    }

    pub(crate) fn begin_session_layout_restore(&mut self) -> Result<SessionRestoreToken, String> {
        if self.clarion.is_locked() {
            return Err("会话锁定时不能恢复布局".into());
        }
        self.clarion.session_restore_request = self.clarion.session_restore_request.wrapping_add(1);
        Ok(SessionRestoreToken {
            request: self.clarion.session_restore_request,
            lock_epoch: self.clarion.capture_scene_epoch.stamp(),
        })
    }

    pub(crate) fn apply_session_layout_restore(
        &mut self,
        token: SessionRestoreToken,
        session: clarion_ipc::management::SessionLayout,
    ) -> Result<(), String> {
        if self.clarion.is_locked() || !token.lock_epoch.is_current() {
            return Err("会话锁定已取消布局恢复".into());
        }
        if token.request != self.clarion.session_restore_request {
            return Err("新的布局恢复请求已取消此请求".into());
        }
        self.clarion.layout.restore_session_layout(session)?;
        self.clarion.queue_redraw_all();
        Ok(())
    }

    pub(crate) fn queue_session_layout_restore(&mut self, json: String) {
        let token = match self.begin_session_layout_restore() {
            Ok(token) => token,
            Err(error) => {
                warn!(%error, "could not restore session layout");
                return;
            }
        };
        let task = self
            .clarion
            .owned_tasks
            .submit(move || crate::ipc::server::parse_session_layout(&json));
        let event_loop = self.clarion.event_loop.clone();
        let future = async move {
            let session = match task {
                Ok(task) => task.finish().await,
                Err(err) => Err(err),
            };
            event_loop.insert_idle(move |state| {
                if let Err(error) =
                    session.and_then(|session| state.apply_session_layout_restore(token, session))
                {
                    warn!(%error, "could not restore session layout");
                }
            });
        };
        if let Err(err) = self.clarion.scheduler.schedule(future) {
            warn!("error scheduling session layout restoration: {err:?}");
        }
    }

    fn apply_xkb_fallback(&mut self, mut xkb: Xkb) {
        xkb.file = None;
        if xkb == Xkb::default() {
            xkb = self.clarion.xkb_from_locale1.clone().unwrap_or_default();
        }
        self.set_xkb_config(xkb.to_xkb_config());
    }

    fn load_xkb_file(&mut self) {
        let path = self.clarion.config.borrow().input.keyboard.xkb.file.clone();
        if let Some(path) = path {
            self.queue_xkb_file(path, None);
        }
    }

    pub fn set_xkb_config(&mut self, xkb: XkbConfig) {
        self.clarion.keymap_request = self.clarion.keymap_request.wrapping_add(1);
        let keyboard = self.clarion.seat.get_keyboard().unwrap();
        let num_lock = keyboard.modifier_state().num_lock;
        if let Err(err) = keyboard.set_xkb_config(self, xkb) {
            warn!("error updating xkb config: {err:?}");
            return;
        }

        // Restore num lock to its previous value.
        let mut mods_state = keyboard.modifier_state();
        if mods_state.num_lock != num_lock {
            mods_state.num_lock = num_lock;
            keyboard.set_modifier_state(mods_state);
        }
    }

    pub fn reload_config(&mut self, config: Result<Config, ()>) {
        self.cancel_output_preview();
        let _span = clarion_tracy_span!("State::reload_config");
        let _latency =
            crate::utils::latency::Span::new(crate::utils::latency::Operation::ConfigReload);

        let mut config = match config {
            Ok(config) => config,
            Err(()) => {
                self.clarion.config_error_notification.show();
                self.clarion.queue_redraw_all();

                #[cfg(feature = "dbus")]
                self.clarion.a11y_announce_config_error();

                return;
            }
        };

        self.clarion.config_error_notification.hide();

        // Find & orphan removed named workspaces.
        let mut removed_workspaces: Vec<String> = vec![];
        for ws in &self.clarion.config.borrow().workspaces {
            if !config.workspaces.iter().any(|w| w.name == ws.name) {
                removed_workspaces.push(ws.name.0.clone());
            }
        }
        for name in removed_workspaces {
            self.clarion.layout.unname_workspace(&name);
        }

        self.clarion.layout.update_config(&config);
        for mapped in self.clarion.mapped_layer_surfaces.values_mut() {
            mapped.update_config(&config);
        }

        // Create new named workspaces.
        for ws_config in &config.workspaces {
            self.clarion.layout.ensure_named_workspace(ws_config);
        }

        let rate = 1.0 / config.animations.slowdown.max(0.001);
        self.clarion.clock.set_rate(rate);
        self.clarion
            .clock
            .set_complete_instantly(config.animations.off);

        *CHILD_ENV.write().unwrap() = mem::take(&mut config.environment);

        let mut reload_xkb = None;
        let mut libinput_config_changed = false;
        let mut output_config_changed = false;
        let mut preserved_output_config = None;
        let mut window_rules_changed = false;
        let mut layer_rules_changed = false;
        let mut shaders_changed = false;
        let mut cursor_inactivity_timeout_changed = false;
        let mut recent_windows_changed = false;
        let mut xwls_changed = false;
        let mut old_config = self.clarion.config.borrow_mut();

        // Reload the cursor.
        if config.cursor != old_config.cursor {
            self.clarion
                .cursor_manager
                .reload(&config.cursor.xcursor_theme, config.cursor.xcursor_size);
            self.clarion.cursor_texture_cache.clear();
        }

        // We need &mut self to reload the xkb config, so just store it here.
        if config.input.keyboard.xkb != old_config.input.keyboard.xkb {
            reload_xkb = Some(config.input.keyboard.xkb.clone());
        }

        // Reload the repeat info.
        if config.input.keyboard.repeat_rate != old_config.input.keyboard.repeat_rate
            || config.input.keyboard.repeat_delay != old_config.input.keyboard.repeat_delay
        {
            let keyboard = self.clarion.seat.get_keyboard().unwrap();
            keyboard.change_repeat_info(
                config.input.keyboard.repeat_rate.into(),
                config.input.keyboard.repeat_delay.into(),
            );
        }

        if config.input.touchpad != old_config.input.touchpad
            || config.input.mouse != old_config.input.mouse
            || config.input.trackball != old_config.input.trackball
            || config.input.trackpoint != old_config.input.trackpoint
            || config.input.tablet != old_config.input.tablet
            || config.input.touch != old_config.input.touch
        {
            libinput_config_changed = true;
        }

        let ignored_nodes_changed =
            config.debug.ignored_drm_devices != old_config.debug.ignored_drm_devices;

        if config.outputs != self.clarion.config_file_output_config {
            output_config_changed = true;
            self.clarion
                .config_file_output_config
                .clone_from(&config.outputs);
        } else {
            // Output config did not change from the last disk load, so we need to preserve the
            // transient changes.
            preserved_output_config = Some(mem::take(&mut old_config.outputs));
        }

        if config.renderer_backend() != old_config.renderer_backend() {
            warn!(
                "renderer backend changes from {:?} to {:?} require a compositor restart",
                old_config.renderer_backend(),
                config.renderer_backend()
            );
        }

        let binds_changed = config.binds != old_config.binds;
        let new_mod_key = self.backend.mod_key(&config);
        if new_mod_key != self.backend.mod_key(&old_config) || binds_changed {
            self.clarion
                .hotkey_overlay
                .on_hotkey_config_updated(new_mod_key);
            self.clarion.mods_with_mouse_binds = mods_with_mouse_binds(new_mod_key, &config.binds);
            self.clarion.mods_with_wheel_binds = mods_with_wheel_binds(new_mod_key, &config.binds);
            self.clarion.mods_with_tablet_stylus_binds =
                mods_with_tablet_stylus_binds(new_mod_key, &config.binds);
            self.clarion.mods_with_finger_scroll_binds =
                mods_with_finger_scroll_binds(new_mod_key, &config.binds);
        }

        if config.window_rules != old_config.window_rules {
            window_rules_changed = true;
        }

        if config.layer_rules != old_config.layer_rules {
            layer_rules_changed = true;
        }

        if config.animations.window_resize.custom_shader
            != old_config.animations.window_resize.custom_shader
        {
            let src = config.animations.window_resize.custom_shader.as_deref();
            crate::with_any_renderer!(self.backend, |renderer| {
                shaders::set_custom_resize_program(renderer, src);
            });
            shaders_changed = true;
        }

        if config.animations.window_close.custom_shader
            != old_config.animations.window_close.custom_shader
            || config.animations.window_close.effect != old_config.animations.window_close.effect
        {
            let src = shaders::resolve_close_shader(
                config.animations.window_close.custom_shader.as_deref(),
                config.animations.window_close.effect,
            );
            info!(
                "loaded window close shader for effect: {:?}",
                config.animations.window_close.effect
            );
            crate::with_any_renderer!(self.backend, |renderer| {
                shaders::set_custom_close_program(renderer, src);
            });
            shaders_changed = true;
        }

        if config.animations.window_open.custom_shader
            != old_config.animations.window_open.custom_shader
        {
            let src = config.animations.window_open.custom_shader.as_deref();
            crate::with_any_renderer!(self.backend, |renderer| {
                shaders::set_custom_open_program(renderer, src);
            });
            shaders_changed = true;
        }

        if config.cursor.hide_after_inactive_ms != old_config.cursor.hide_after_inactive_ms {
            cursor_inactivity_timeout_changed = true;
        }

        if config.debug.keep_laptop_panel_on_when_lid_is_closed
            != old_config.debug.keep_laptop_panel_on_when_lid_is_closed
        {
            output_config_changed = true;
        }

        if config.debug.ignored_drm_devices != old_config.debug.ignored_drm_devices {
            output_config_changed = true;
        }

        // FIXME: move backdrop rendering into layout::Monitor, then this will become unnecessary.
        if config.overview.backdrop_color != old_config.overview.backdrop_color {
            output_config_changed = true;
        }
        if config.layout.background_color != old_config.layout.background_color {
            output_config_changed = true;
        }

        if config.recent_windows != old_config.recent_windows {
            recent_windows_changed = true;
        }

        if config.xwayland_satellite != old_config.xwayland_satellite {
            xwls_changed = true;
        }

        *old_config = config;

        if let Some(outputs) = preserved_output_config {
            old_config.outputs = outputs;
        }

        // Release the borrow.
        drop(old_config);
        self.backend
            .update_config_snapshot(&self.clarion.config.borrow());

        // Parsing a user keymap must not block input/redraw dispatch.
        if let Some(mut xkb) = reload_xkb {
            if let Some(path) = xkb.file.take() {
                self.queue_xkb_file(path, Some(xkb));
            } else {
                self.apply_xkb_fallback(xkb);
                self.ipc_keyboard_layouts_changed();
            }
        }

        if libinput_config_changed {
            let config = self.clarion.config.borrow();
            for mut device in self.clarion.devices.iter().cloned() {
                apply_libinput_settings(&config.input, &mut device);
            }
        }

        if ignored_nodes_changed {
            self.backend.update_ignored_nodes_config(
                &mut crate::platform::device_bridge::DeviceCoordinator::new(&mut self.clarion),
            );
        }

        if output_config_changed {
            self.reload_output_config();
        }

        if window_rules_changed {
            self.clarion.recompute_window_rules();
        }

        if layer_rules_changed {
            self.clarion.recompute_layer_rules();
        }

        if shaders_changed {
            self.clarion.update_shaders();
        }

        if cursor_inactivity_timeout_changed {
            // Force reset due to timeout change.
            self.clarion.pointer_inactivity_timer_got_reset = false;
            self.clarion.reset_pointer_inactivity_timer();
        }

        if binds_changed {
            self.clarion.window_mru_ui.update_binds();
        }

        if recent_windows_changed {
            self.clarion.window_mru_ui.update_config();
        }

        if xwls_changed {
            // If xwl-s was previously working and is now off, we don't try to kill it or stop
            // watching the sockets, for simplicity's sake.
            let was_working = self.clarion.satellite.is_some();

            // Try to start, or restart in case the user corrected the path or something.
            xwayland::satellite::setup(self);

            let config = self.clarion.config.borrow();
            let display_name = (!config.xwayland_satellite.off)
                .then_some(self.clarion.satellite.as_ref())
                .flatten()
                .map(|satellite| satellite.display_name().to_owned());

            if let Some(name) = &display_name {
                if !was_working {
                    info!("listening on X11 socket: {name}");
                }
            }

            // This won't change the systemd environment, but oh well.
            *CHILD_DISPLAY.write().unwrap() = display_name;
        }

        // Can't really update xdg-decoration settings since we have to hide the globals for CSD
        // due to the SDL2 bug... I don't imagine clients are prepared for the xdg-decoration
        // global suddenly appearing? Either way, right now it's live-reloaded in a sense that new
        // clients will use the new xdg-decoration setting.

        self.clarion.queue_redraw_all();
    }

    pub fn reload_output_config(&mut self) {
        self.backend
            .update_config_snapshot(&self.clarion.config.borrow());
        let mut resized_outputs = vec![];
        let mut recolored_outputs = vec![];

        for output in self.clarion.global_space.outputs() {
            let name = output.user_data().get::<OutputName>().unwrap();
            let full_config = self.clarion.config.borrow_mut();
            let config = full_config.outputs.find(name);

            let scale = config
                .and_then(|c| c.scale)
                .map(|s| s.0)
                .unwrap_or_else(|| {
                    let size_mm = output.physical_properties().size;
                    let resolution = output.current_mode().unwrap().size;
                    guess_monitor_scale(size_mm, resolution)
                });
            let scale = closest_representable_scale(scale.clamp(0.1, 10.));

            let requested_transform = panel_orientation(output)
                + config
                    .map(|c| ipc_transform_to_smithay(c.transform))
                    .unwrap_or(Transform::Normal);
            let transform = crate::backend::winit::output_transform(output, requested_transform);

            if output.current_scale().fractional_scale() != scale
                || output.current_transform() != transform
            {
                output.change_current_state(
                    None,
                    Some(transform),
                    Some(output::Scale::Fractional(scale)),
                    None,
                );
                self.clarion.ipc_outputs_changed = true;
                resized_outputs.push(output.clone());
            }

            let mut backdrop_color = config
                .and_then(|c| c.backdrop_color)
                .unwrap_or(full_config.overview.backdrop_color)
                .to_array_unpremul();
            backdrop_color[3] = 1.;
            let backdrop_color = Color32F::from(backdrop_color);

            if let Some(state) = self.clarion.output_state.get_mut(output) {
                if state.backdrop_buffer.color() != backdrop_color {
                    state.backdrop_buffer.set_color(backdrop_color);
                    recolored_outputs.push(output.clone());
                }
            }

            for mon in self.clarion.layout.monitors_mut() {
                if mon.output() != output {
                    continue;
                }

                let mut layout_config = config.and_then(|c| c.layout.clone());
                // Support the deprecated non-layout background-color key.
                if let Some(layout) = &mut layout_config {
                    if layout.background_color.is_none() {
                        layout.background_color = config.and_then(|c| c.background_color);
                    }
                }

                if mon.update_layout_config(layout_config) {
                    // Also redraw these; if anything, the background color could've changed.
                    recolored_outputs.push(output.clone());
                }
                break;
            }
        }

        for output in resized_outputs {
            self.clarion.output_resized(&output);
        }

        for output in recolored_outputs {
            self.clarion.queue_redraw(&output);
        }

        self.backend.on_output_config_changed(
            &mut crate::platform::device_bridge::DeviceCoordinator::new(&mut self.clarion),
        );

        self.clarion.reposition_outputs(None);

        if let Some(touch) = self.clarion.seat.get_touch() {
            // A down event installs a touch grab. With no active sequence there
            // is nothing to cancel, and Smithay warns about a spurious cancel.
            if touch.is_grabbed() {
                touch.cancel(self);
            }
        }

        let config = self.clarion.config.borrow().outputs.clone();
        self.clarion
            .output_management_state
            .on_config_changed(config);
    }

    pub fn modify_output_config<F>(&mut self, name: &str, fun: F)
    where
        F: FnOnce(&mut clarion_config::Output),
    {
        self.cancel_output_preview();
        // Try hard to find the output config section corresponding to the output set by the
        // user. Since if we add a new section and some existing section also matches the
        // output, then our new section won't do anything.
        let temp;
        let match_name = if let Some(output) = self.clarion.output_by_name_match(name) {
            output.user_data().get::<OutputName>().unwrap()
        } else if let Some(output_name) = self
            .backend
            .tty_checked()
            .and_then(|tty| tty.disconnected_connector_name_by_name_match(name))
        {
            temp = output_name;
            &temp
        } else {
            // Even if name is "make model serial", matching will work fine this way.
            temp = OutputName {
                connector: name.to_owned(),
                make: None,
                model: None,
                serial: None,
            };
            &temp
        };

        let mut config = self.clarion.config.borrow_mut();
        let config = if let Some(config) = config.outputs.find_mut(match_name) {
            config
        } else {
            config.outputs.0.push(clarion_config::Output {
                // Save name as set by the user.
                name: String::from(name),
                ..Default::default()
            });
            config.outputs.0.last_mut().unwrap()
        };

        fun(config);
    }

    pub fn apply_transient_output_config(&mut self, name: &str, action: clarion_ipc::OutputAction) {
        self.modify_output_config(name, move |config| match action {
            clarion_ipc::OutputAction::Off => config.off = true,
            clarion_ipc::OutputAction::On => config.off = false,
            clarion_ipc::OutputAction::Mode { mode } => {
                config.mode = match mode {
                    clarion_ipc::ModeToSet::Automatic => None,
                    clarion_ipc::ModeToSet::Specific(mode) => Some(clarion_config::output::Mode {
                        custom: false,
                        mode,
                    }),
                };
                config.modeline = None;
            }
            clarion_ipc::OutputAction::CustomMode { mode } => {
                config.mode = Some(clarion_config::output::Mode { custom: true, mode });
                config.modeline = None;
            }
            clarion_ipc::OutputAction::Modeline {
                clock,
                hdisplay,
                hsync_start,
                hsync_end,
                htotal,
                vdisplay,
                vsync_start,
                vsync_end,
                vtotal,
                hsync_polarity,
                vsync_polarity,
            } => {
                // Do not reset config.mode to None since it's used as a fallback.
                config.modeline = Some(clarion_config::output::Modeline {
                    clock,
                    hdisplay,
                    hsync_start,
                    hsync_end,
                    htotal,
                    vdisplay,
                    vsync_start,
                    vsync_end,
                    vtotal,
                    hsync_polarity,
                    vsync_polarity,
                })
            }
            clarion_ipc::OutputAction::Scale { scale } => {
                config.scale = match scale {
                    clarion_ipc::ScaleToSet::Automatic => None,
                    clarion_ipc::ScaleToSet::Specific(scale) => Some(FloatOrInt(scale)),
                }
            }
            clarion_ipc::OutputAction::Transform { transform } => config.transform = transform,
            clarion_ipc::OutputAction::Position { position } => {
                config.position = match position {
                    clarion_ipc::PositionToSet::Automatic => None,
                    clarion_ipc::PositionToSet::Specific(position) => {
                        Some(clarion_config::Position {
                            x: position.x,
                            y: position.y,
                        })
                    }
                }
            }
            clarion_ipc::OutputAction::Vrr { vrr } => {
                config.variable_refresh_rate = if vrr.vrr {
                    Some(clarion_config::Vrr {
                        on_demand: vrr.on_demand,
                    })
                } else {
                    None
                }
            }
            clarion_ipc::OutputAction::MaxBpc { max_bpc } => config.max_bpc = Some(MaxBpc(max_bpc)),
            clarion_ipc::OutputAction::Hdr { hdr } => config.hdr = hdr.hdr,
        });

        self.reload_output_config();
    }

    pub fn refresh_ipc_outputs(&mut self) {
        if !self.clarion.ipc_outputs_changed {
            return;
        }
        self.clarion.ipc_outputs_changed = false;

        let _span = clarion_tracy_span!("State::refresh_ipc_outputs");

        for ipc_output in self.backend.ipc_outputs().lock().unwrap().values_mut() {
            let logical = self
                .clarion
                .global_space
                .outputs()
                .find(|output| output.name() == ipc_output.name)
                .map(logical_output);
            ipc_output.logical = logical;
        }

        #[cfg(feature = "dbus")]
        self.clarion.on_ipc_outputs_changed();

        let new_config = self.backend.ipc_outputs().lock().unwrap().clone();
        self.clarion
            .output_management_state
            .notify_changes(new_config);
    }

    pub fn open_screenshot_ui(&mut self, show_pointer: bool, path: Option<String>) {
        if self.clarion.is_locked() || self.clarion.screenshot_ui.is_open() {
            return;
        }

        let default_output = self
            .clarion
            .output_under_cursor()
            .or_else(|| self.clarion.layout.active_output().cloned());
        let Some(default_output) = default_output else {
            return;
        };

        self.clarion.update_render_elements(None);

        // Capture each output with the renderer that can actually consume its KMS swapchain.
        // A TTY may have a native Vulkan output next to a surface that was migrated to GLES;
        // feeding both through the global renderer silently drops the foreign texture type.
        let outputs: Vec<_> = self.clarion.global_space.outputs().cloned().collect();
        let mut screenshots = HashMap::new();
        let mut mixed_panels = HashMap::new();
        let mut saw_gles = false;
        let mut saw_native = false;
        for output in outputs {
            let renderer_output = output.clone();
            let scale = renderer_output.current_scale().fractional_scale();
            let captured =
                self.backend
                    .with_renderer_for_output(Some(&renderer_output), |renderer| match renderer {
                        crate::render_helpers::renderer::AnyRenderer::Gles(gles) => {
                            saw_gles = true;
                            mixed_panels.insert(
                                renderer_output.clone(),
                                ScreenshotUi::panels_gles(gles, scale),
                            );
                            self.clarion
                                .capture_screenshot_for_output(gles, &renderer_output)
                        }
                        crate::render_helpers::renderer::AnyRenderer::Native(native) => {
                            saw_native = true;
                            mixed_panels.insert(
                                renderer_output.clone(),
                                ScreenshotUi::panels_native(native, scale),
                            );
                            self.clarion
                                .capture_screenshot_for_output_native(native, &renderer_output)
                        }
                    });
            if let Some(Some(captured)) = captured {
                screenshots.insert(output, captured);
            }
        }

        if screenshots.is_empty() {
            warn!("cannot open screenshot UI: no outputs were captured");
            return;
        }

        // Now that we captured the screenshots, clear grabs like drag-and-drop, etc.
        let time = get_monotonic_time().as_millis() as u32;
        self.clarion.seat.get_pointer().unwrap().unset_grab(
            self,
            SERIAL_COUNTER.next_serial(),
            time,
        );
        if let Some(touch) = self.clarion.seat.get_touch() {
            touch.unset_grab(self);
        }

        // Can't unset_grab() from with_tools(), will deadlock on tablet seat mutex...
        let mut tools = Vec::new();
        self.clarion.seat.tablet_seat().with_tools(|map| {
            tools = Vec::from_iter(map.values().cloned());
        });
        for tool in tools {
            tool.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
        }

        let opened = if saw_gles && saw_native {
            // Keep each output's help panel in the same renderer domain as its screenshot
            // texture; the UI element wrapper will select the matching variant at draw time.
            Some(self.clarion.screenshot_ui.open_mixed(
                screenshots,
                mixed_panels,
                default_output,
                show_pointer,
                path,
            ))
        } else {
            let renderer_output = screenshots
                .keys()
                .find(|output| **output == default_output)
                .cloned()
                .or_else(|| screenshots.keys().next().cloned());
            let Some(renderer_output) = renderer_output else {
                return;
            };
            self.backend
                .with_renderer_for_output(Some(&renderer_output), |renderer| match renderer {
                    crate::render_helpers::renderer::AnyRenderer::Gles(gles) => self
                        .clarion
                        .screenshot_ui
                        .open(gles, screenshots, default_output, show_pointer, path),
                    crate::render_helpers::renderer::AnyRenderer::Native(native) => self
                        .clarion
                        .screenshot_ui
                        .open_native(native, screenshots, default_output, show_pointer, path),
                })
        };

        if opened != Some(true) {
            warn!("could not open screenshot UI");
            return;
        }

        self.clarion
            .cursor_manager
            .set_cursor_image(CursorImageStatus::Named(CursorIcon::Crosshair));
        self.clarion.queue_redraw_all();
    }

    pub fn handle_pick_color(
        &mut self,
        tx: async_channel::Sender<Option<clarion_ipc::PickedColor>>,
    ) {
        let pointer = self.clarion.seat.get_pointer().unwrap();
        let start_data = PointerGrabStartData {
            focus: None,
            button: 0,
            location: pointer.current_location(),
        };
        let grab = PickColorGrab::new(start_data);
        pointer.set_grab(self, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
        self.clarion.pick_color = Some(tx);
        self.clarion
            .cursor_manager
            .set_cursor_image(CursorImageStatus::Named(CursorIcon::Crosshair));
        self.clarion.queue_redraw_all();
    }

    pub fn confirm_screenshot(&mut self, write_to_disk: bool) {
        let ScreenshotUi::Open { path, .. } = &self.clarion.screenshot_ui else {
            return;
        };
        // Reserve capacity before rendering or submitting GPU work. Keep the
        // selection and explicit path available when preparation cannot start.
        let options = self
            .clarion
            .screenshot_save_options(write_to_disk, path.clone());
        let permit = match self.clarion.screenshot_tasks.acquire() {
            Ok(permit) => permit,
            Err(error) => {
                warn!("cannot start screenshot: {error:#}");
                return;
            }
        };
        let selected_output = self.clarion.screenshot_ui.selection_output().cloned();
        let selected_is_native = self.clarion.screenshot_ui.selection_uses_native();
        let result = selected_output
            .as_ref()
            .and_then(|output| {
                self.backend
                    .with_renderer_for_output(Some(output), |renderer| {
                        match (renderer, selected_is_native) {
                            (
                                crate::render_helpers::renderer::AnyRenderer::Gles(gles),
                                Some(false),
                            ) => {
                                let (size, texture, region) =
                                    self.clarion.screenshot_ui.prepare_capture(gles)?;
                                use crate::platform::pixel_readback::DeferredCaptureRenderer;
                                let job =
                                    gles.enqueue_pixels(&texture, region, Fourcc::Abgr8888)?;
                                self.clarion.enqueue_screenshot(
                                    output.clone(),
                                    job,
                                    size,
                                    options,
                                    permit,
                                )
                            }
                            (
                                crate::render_helpers::renderer::AnyRenderer::Native(native),
                                Some(true),
                            ) => {
                                let (size, texture, region) =
                                    self.clarion.screenshot_ui.prepare_capture_native(native)?;
                                let job =
                                    native.enqueue_readback(&texture, region, Fourcc::Abgr8888)?;
                                self.clarion.enqueue_screenshot(
                                    output.clone(),
                                    crate::platform::pixel_readback::PendingPixels::Native(job),
                                    size,
                                    options,
                                    permit,
                                )
                            }
                            _ => Err(anyhow::anyhow!(
                                "screenshot renderer changed while the selection was open"
                            )),
                        }
                    })
            })
            .unwrap_or_else(|| Err(anyhow::anyhow!("screenshot selection output disappeared")));
        if let Err(error) = result {
            warn!("error preparing screenshot: {error:#}");
            self.clarion.queue_redraw_all();
            return;
        }
        self.clarion.screenshot_ui.close();
        self.clarion
            .cursor_manager
            .set_cursor_image(CursorImageStatus::default_named());
        self.clarion.queue_redraw_all();
    }

    pub fn store_unmap_snapshot(&mut self, window: &Window, output: Option<&Output>) {
        if self.clarion.shutting_down {
            return;
        }
        // The unmapping tile may have an xray background, in which case we will render xray
        // elements, so they need to be updated.
        self.clarion.update_xray_render_elements(output);

        self.backend
            .with_renderer_for_output(output, |mut renderer| {
                let (has_blocked_out, xray) = if let Some(output) = output {
                    let has_blocked_out = match &mut renderer {
                        crate::render_helpers::renderer::AnyRenderer::Gles(gles) => {
                            let mut ctx = RenderCtx {
                                target: RenderTarget::Output,
                                renderer: &mut **gles,
                                xray: None,
                            };

                            self.clarion.fill_xray_elements(ctx.r(), output);

                            // If any background layer has block_out_from, also fill the Screencast
                            // xray buffer so the unmap snapshot can
                            // render a buffer with blocked-out
                            // background.
                            //
                            // This will be used in Tile::render_snapshot().
                            let has_blocked_out =
                                self.clarion.has_blocked_out_background_layers(output);
                            if has_blocked_out {
                                let screencast_ctx = RenderCtx {
                                    target: RenderTarget::Screencast,
                                    ..ctx.r()
                                };
                                self.clarion.fill_xray_elements(screencast_ctx, output);
                            }
                            has_blocked_out
                        }
                        crate::render_helpers::renderer::AnyRenderer::Native(native) => {
                            for target in [
                                RenderTarget::Output,
                                RenderTarget::Screencast,
                                RenderTarget::ScreenCapture,
                            ] {
                                self.clarion.fill_xray_elements_native(
                                    RenderCtx {
                                        renderer: &mut **native,
                                        target,
                                        xray: None,
                                    },
                                    output,
                                );
                            }
                            true
                        }
                    };

                    let state = self.clarion.output_state.get_mut(output).unwrap();
                    (has_blocked_out, Some(&mut state.xray))
                } else {
                    (false, None)
                };

                self.clarion.layout.store_unmap_snapshot_any(
                    &mut renderer,
                    xray,
                    has_blocked_out,
                    window,
                );

                if let Some(output) = output {
                    self.clarion.clear_xray_elements(output);
                }
            });
    }

    #[cfg(not(feature = "xdp-gnome-screencast"))]
    pub fn set_dynamic_cast_target(&mut self, _target: CastTarget) {}

    #[cfg(feature = "dbus")]
    pub fn on_screen_shot_msg(&mut self, msg: ScreenshotToClarion) {
        match msg {
            ScreenshotToClarion::TakeScreenshot {
                include_cursor,
                reply,
            } => {
                self.handle_take_screenshot(&reply, include_cursor);
            }
            ScreenshotToClarion::PickColor(tx) => {
                self.handle_pick_color(tx);
            }
        }
    }

    #[cfg(feature = "dbus")]
    fn handle_take_screenshot(
        &mut self,
        to_screenshot: &async_channel::Sender<ClarionToScreenshot>,
        include_cursor: bool,
    ) {
        let reply = {
            let sender = to_screenshot.clone();
            crate::platform::capture::ScreenshotCompletion::new(move |path| {
                if let Err(error) = sender.try_send(ClarionToScreenshot::ScreenshotResult(path)) {
                    warn!("error sending screenshot portal result: {error}");
                }
            })
        };
        // The existing portal supports a single output. Preserve its explicit
        // limitation while sharing the deferred readback and bounded encoder.
        let mut outputs = self.clarion.global_space.outputs();
        let output = outputs.next().cloned();
        let single = output.is_some() && outputs.next().is_none();
        drop(outputs);
        if !single {
            warn!("portal screenshot requires exactly one output");
            return;
        }
        let output = output.unwrap();
        let path = make_screenshot_path(&self.clarion.config.borrow())
            .ok()
            .flatten()
            .unwrap_or_else(|| env::temp_dir().join("screenshot.png"));
        let options = crate::platform::capture::ScreenshotSaveOptions {
            destination: Some((path, true)),
            publish_clipboard: false,
            completion: reply,
        };
        let result = self
            .backend
            .with_renderer_for_output(Some(&output), |renderer| {
                let permit = self.clarion.screenshot_tasks.acquire()?;
                match renderer {
                    crate::render_helpers::renderer::AnyRenderer::Gles(gles) => {
                        let (size, texture) =
                            self.clarion
                                .prepare_screenshot(gles, &output, include_cursor)?;
                        let region = Rectangle::from_size(texture.size());
                        use crate::platform::pixel_readback::DeferredCaptureRenderer;
                        let job = gles.enqueue_pixels(&texture, region, Fourcc::Abgr8888)?;
                        self.clarion
                            .enqueue_screenshot(output.clone(), job, size, options, permit)
                    }
                    crate::render_helpers::renderer::AnyRenderer::Native(native) => {
                        let (size, texture) =
                            self.clarion
                                .prepare_screenshot(native, &output, include_cursor)?;
                        let job = native.enqueue_readback(
                            &texture,
                            Rectangle::from_size(texture.size()),
                            Fourcc::Abgr8888,
                        )?;
                        self.clarion.enqueue_screenshot(
                            output.clone(),
                            crate::platform::pixel_readback::PendingPixels::Native(job),
                            size,
                            options,
                            permit,
                        )
                    }
                }
            });
        if let Some(Err(error)) = result {
            warn!("error taking portal screenshot: {error:#}");
        }
    }

    #[cfg(feature = "dbus")]
    pub fn on_introspect_msg(
        &mut self,
        to_introspect: &async_channel::Sender<ClarionToIntrospect>,
        msg: IntrospectToClarion,
    ) {
        use crate::utils::with_toplevel_role;

        let IntrospectToClarion::GetWindows = msg;
        let _span = clarion_tracy_span!("GetWindows");

        let mut windows = HashMap::new();

        #[cfg(feature = "xdp-gnome-screencast")]
        windows.insert(
            self.clarion.casting.dynamic_cast_id_for_portal.get(),
            gnome_shell_introspect::WindowProperties {
                title: String::from("clarion Dynamic Cast Target"),
                app_id: String::from("rs.bxt.clarion.desktop"),
            },
        );

        self.clarion.layout.with_windows(|mapped, _, _, _| {
            let id = mapped.id().get();
            let props = with_toplevel_role(mapped.toplevel(), |role| {
                gnome_shell_introspect::WindowProperties {
                    title: role.title.clone().unwrap_or_default(),
                    app_id: role
                        .app_id
                        .as_ref()
                        // We don't do proper .desktop file tracking (it's quite involved), and
                        // Wayland windows can set any app id they want. However, this seems to
                        // work well enough in practice.
                        .map(|app_id| format!("{app_id}.desktop"))
                        .unwrap_or_default(),
                }
            });

            windows.insert(id, props);
        });

        let msg = ClarionToIntrospect::Windows(windows);
        if let Err(err) = to_introspect.send_blocking(msg) {
            warn!("error sending windows to introspect: {err:?}");
        }
    }

    #[cfg(feature = "dbus")]
    pub fn on_login1_msg(&mut self, msg: Login1ToClarion) {
        let Login1ToClarion::LidClosedChanged(is_closed) = msg;

        trace!("login1 lid {}", if is_closed { "closed" } else { "opened" });
        self.set_lid_closed(is_closed);
    }

    #[cfg(feature = "dbus")]
    pub fn on_locale1_msg(&mut self, msg: Locale1ToClarion) {
        let Locale1ToClarion::XkbChanged(xkb) = msg;

        trace!("locale1 xkb settings changed: {xkb:?}");
        let xkb = self.clarion.xkb_from_locale1.insert(xkb);

        {
            let config = self.clarion.config.borrow();
            if config.input.keyboard.xkb != Xkb::default() {
                trace!("ignoring locale1 xkb change because clarion config has xkb settings");
                return;
            }
        }

        let xkb = xkb.clone();
        self.set_xkb_config(xkb.to_xkb_config());
        self.ipc_keyboard_layouts_changed();
    }
}

impl Clarion {
    /// Expose explicit synchronization only when the primary DRM device can
    /// wake surface commit blockers from timeline points.
    pub fn enable_drm_syncobj(&mut self, device: DrmDeviceFd) {
        if !supports_syncobj_eventfd(&device) {
            debug!("primary DRM device does not support syncobj eventfd");
            return;
        }
        if let Some(state) = &mut self.drm_syncobj_state {
            state.update_device(device);
        } else {
            self.drm_syncobj_state =
                Some(DrmSyncobjState::new::<State>(&self.display_handle, device));
        }
    }

    pub fn disable_drm_syncobj(&mut self) {
        if let Some(state) = self.drm_syncobj_state.take() {
            let global = state.into_global();
            self.display_handle.disable_global::<State>(global.clone());
            let display = self.display_handle.clone();
            if let Err(err) = self.event_loop.insert_source(
                Timer::from_duration(Duration::from_secs(10)),
                move |_, _, _| {
                    display.remove_global::<State>(global.clone());
                    TimeoutAction::Drop
                },
            ) {
                warn!(?err, "failed to schedule syncobj global removal");
            }
        }
    }

    pub fn new(
        config: Rc<RefCell<Config>>,
        event_loop: LoopHandle<'static, State>,
        stop_signal: LoopSignal,
        display: Display<State>,
        backend: &Backend,
        create_wayland_socket: bool,
        is_session_instance: bool,
    ) -> Self {
        let _span = clarion_tracy_span!("Clarion::new");

        let (executor, scheduler) = calloop::futures::executor().unwrap();
        let runtime_executor = crate::platform::event_sources::ScopedSource::insert(
            &event_loop,
            executor,
            |_, _, _| (),
        )
        .unwrap();

        let display_handle = display.handle();
        let config_ = config.borrow();
        let config_file_output_config = config_.outputs.clone();

        let mut animation_clock = Clock::default();

        let rate = 1.0 / config_.animations.slowdown.max(0.001);
        animation_clock.set_rate(rate);
        animation_clock.set_complete_instantly(config_.animations.off);

        let scene_resources = crate::scene::resources::SceneResources::new();
        let layout = Layout::new_with_resources(
            animation_clock.clone(),
            &config_,
            scene_resources.allocator(),
        );

        let (blocker_cleared_tx, blocker_cleared_rx) = mpsc::channel();

        fn client_is_unrestricted(client: &Client) -> bool {
            !client.get_data::<ClientState>().unwrap().restricted
        }

        let compositor_state = CompositorState::new_v6::<State>(&display_handle);
        let xdg_shell_state = XdgShellState::new_with_capabilities::<State>(
            &display_handle,
            [WmCapabilities::Fullscreen, WmCapabilities::Maximize],
        );
        let xdg_decoration_state =
            XdgDecorationState::new_with_filter::<State, _>(&display_handle, |client| {
                client
                    .get_data::<ClientState>()
                    .unwrap()
                    .can_view_decoration_globals
            });
        let kde_decoration_state = KdeDecorationState::new_with_filter::<State, _>(
            &display_handle,
            // If we want CSD we will hide the global.
            KdeDecorationsMode::Server,
            |client| {
                client
                    .get_data::<ClientState>()
                    .unwrap()
                    .can_view_decoration_globals
            },
        );
        let layer_shell_state = WlrLayerShellState::new_with_filter::<State, _>(
            &display_handle,
            client_is_unrestricted,
        );
        let session_lock_state =
            SessionLockManagerState::new::<State, _>(&display_handle, client_is_unrestricted);
        let shm_state = ShmState::new::<State>(
            &display_handle,
            vec![wl_shm::Format::Xbgr8888, wl_shm::Format::Abgr8888],
        );
        let output_manager_state =
            OutputManagerState::new_with_xdg_output::<State>(&display_handle);
        let dmabuf_state = DmabufState::new();
        let fractional_scale_manager_state =
            FractionalScaleManagerState::new::<State>(&display_handle);
        let content_type_state = ContentTypeState::new::<State>(&display_handle);
        let commit_timing_manager_state =
            crate::commit_timing::CommitTimingManagerState::new(&display_handle);
        let fifo_manager_state = crate::fifo::FifoManagerState::new(&display_handle);
        let mut seat_state = SeatState::new();
        let tablet_state = TabletManagerState::new::<State>(&display_handle);
        let pointer_gestures_state = PointerGesturesState::new::<State>(&display_handle);
        let relative_pointer_state = RelativePointerManagerState::new::<State>(&display_handle);
        let pointer_constraints_state = PointerConstraintsState::new::<State>(&display_handle);
        let idle_notifier_state = IdleNotifierState::new(&display_handle, event_loop.clone());
        let idle_inhibit_manager_state = IdleInhibitManagerState::new::<State>(&display_handle);
        let data_device_state = DataDeviceState::new::<State>(&display_handle);
        let primary_selection_state =
            PrimarySelectionState::new_with_filter::<State, _>(&display_handle, |client| {
                !client
                    .get_data::<ClientState>()
                    .unwrap()
                    .primary_selection_disabled
            });
        let wlr_data_control_state = WlrDataControlState::new::<State, _>(
            &display_handle,
            Some(&primary_selection_state),
            client_is_unrestricted,
        );
        let ext_data_control_state = ExtDataControlState::new::<State, _>(
            &display_handle,
            Some(&primary_selection_state),
            client_is_unrestricted,
        );
        let presentation_state =
            PresentationState::new::<State>(&display_handle, Monotonic::ID as u32);
        let security_context_state =
            SecurityContextState::new::<State, _>(&display_handle, client_is_unrestricted);

        let text_input_state = TextInputManagerState::new::<State>(&display_handle);
        let input_method_state =
            InputMethodManagerState::new::<State, _>(&display_handle, client_is_unrestricted);
        let keyboard_shortcuts_inhibit_state =
            KeyboardShortcutsInhibitState::new::<State>(&display_handle);
        let virtual_keyboard_state =
            VirtualKeyboardManagerState::new::<State, _>(&display_handle, client_is_unrestricted);
        let virtual_pointer_state =
            VirtualPointerManagerState::new::<State, _>(&display_handle, client_is_unrestricted);
        let foreign_toplevel_state =
            ForeignToplevelManagerState::new::<State, _>(&display_handle, client_is_unrestricted);
        let ext_workspace_state =
            ExtWorkspaceManagerState::new::<State, _>(&display_handle, client_is_unrestricted);
        let mut output_management_state =
            OutputManagementManagerState::new::<State, _>(&display_handle, client_is_unrestricted);
        output_management_state.on_config_changed(config_.outputs.clone());
        let screencopy_state =
            ScreencopyManagerState::new::<State, _>(&display_handle, client_is_unrestricted);
        let viewporter_state = ViewporterState::new::<State>(&display_handle);
        let background_effect_state = BackgroundEffectState::new::<State>(&display_handle);
        let gtk_shell_state = crate::handlers::gtk_shell::GtkShellState::new(&display_handle);
        let qt_app_menu_state = crate::handlers::qt_app_menu::QtAppMenuState::new(&display_handle);
        let xdg_foreign_state = XdgForeignState::new::<State>(&display_handle);

        let is_tty = matches!(backend, Backend::Tty(_));
        let mut gamma_control_manager_state =
            GammaControlManagerState::new::<State, _>(&display_handle, move |client| {
                is_tty && !client.get_data::<ClientState>().unwrap().restricted
            });
        gamma_control_manager_state
            .install_readback_source::<State>(&event_loop)
            .expect("could not register gamma reader completion source");
        let activation_state = XdgActivationState::new::<State>(&display_handle);
        event_loop
            .insert_source(
                Timer::from_duration(XDG_ACTIVATION_TOKEN_TIMEOUT),
                |_, _, state| {
                    state
                        .clarion
                        .activation_state
                        .retain_tokens(|_, token_data| {
                            token_data.timestamp.elapsed() < XDG_ACTIVATION_TOKEN_TIMEOUT
                        });
                    TimeoutAction::ToDuration(XDG_ACTIVATION_TOKEN_TIMEOUT)
                },
            )
            .unwrap();

        let mutter_x11_interop_state =
            MutterX11InteropManagerState::new::<State, _>(&display_handle, move |_| true);

        #[cfg(test)]
        let single_pixel_buffer_state = SinglePixelBufferState::new::<State>(&display_handle);

        let mut seat: Seat<State> = seat_state.new_wl_seat(&display_handle, backend.seat_name());
        let keyboard = match seat.add_keyboard(
            config_.input.keyboard.xkb.to_xkb_config(),
            config_.input.keyboard.repeat_delay.into(),
            config_.input.keyboard.repeat_rate.into(),
        ) {
            Err(err) => {
                if let smithay::input::keyboard::Error::BadKeymap = err {
                    warn!("error loading the configured xkb keymap, trying default");
                } else {
                    warn!("error adding keyboard: {err:?}");
                }
                seat.add_keyboard(
                    Default::default(),
                    config_.input.keyboard.repeat_delay.into(),
                    config_.input.keyboard.repeat_rate.into(),
                )
                .unwrap()
            }
            Ok(keyboard) => keyboard,
        };
        if config_.input.keyboard.numlock {
            let mut modifier_state = keyboard.modifier_state();
            modifier_state.num_lock = true;
            keyboard.set_modifier_state(modifier_state);
        }
        seat.add_pointer();

        let cursor_shape_manager_state = CursorShapeManagerState::new::<State>(&display_handle);
        let mut cursor_manager =
            CursorManager::new(&config_.cursor.xcursor_theme, config_.cursor.xcursor_size);

        if let Some(source) = cursor_manager.take_completion_source() {
            event_loop
                .insert_source(source, |event, _, state| {
                    if let calloop::channel::Event::Msg(completion) = event {
                        let visible_changed =
                            state.clarion.cursor_manager.apply_completion(completion);
                        state.clarion.cursor_texture_cache.clear();
                        if visible_changed && state.clarion.pointer_visibility.is_visible() {
                            state.clarion.queue_redraw_all();
                        }
                    }
                })
                .expect("could not register cursor loader completion source");
        }

        let mod_key = backend.mod_key(&config.borrow());
        let mods_with_mouse_binds = mods_with_mouse_binds(mod_key, &config_.binds);
        let mods_with_wheel_binds = mods_with_wheel_binds(mod_key, &config_.binds);
        let mods_with_finger_scroll_binds = mods_with_finger_scroll_binds(mod_key, &config_.binds);
        let mods_with_tablet_stylus_binds = mods_with_tablet_stylus_binds(mod_key, &config_.binds);

        let screenshot_ui = ScreenshotUi::new(animation_clock.clone(), config.clone());
        let window_mru_ui = WindowMruUi::new(config.clone());
        let config_error_notification =
            ConfigErrorNotification::new(animation_clock.clone(), config.clone());

        let mut hotkey_overlay = HotkeyOverlay::new(config.clone(), mod_key);
        if !config_.hotkey_overlay.skip_at_startup {
            hotkey_overlay.show();
        }

        let exit_confirm_dialog = ExitConfirmDialog::new(animation_clock.clone(), config.clone());

        #[cfg(feature = "dbus")]
        let a11y = A11y::new(event_loop.clone());

        event_loop
            .insert_source(
                Timer::from_duration(Duration::from_secs(1)),
                |_, _, state| {
                    state.clarion.send_frame_callbacks_on_fallback_timer();
                    state.clarion.poll_idle();
                    TimeoutAction::ToDuration(Duration::from_secs(1))
                },
            )
            .unwrap();

        let socket_name = create_wayland_socket.then(|| {
            let socket_source = ListeningSocketSource::new_auto().unwrap();
            let socket_name = socket_source.socket_name().to_os_string();
            event_loop
                .insert_source(socket_source, move |client, _, state| {
                    state.clarion.insert_client(NewClient {
                        client,
                        restricted: false,
                        credentials_unknown: false,
                    });
                })
                .unwrap();
            socket_name
        });

        let ipc_server = match IpcServer::start(&event_loop, socket_name.as_deref()) {
            Ok(server) => Some(server),
            Err(err) => {
                warn!("error starting IPC server: {err:?}");
                None
            }
        };

        #[cfg(feature = "xdp-gnome-screencast")]
        let screencasting = Screencasting::new(&event_loop);

        let display_source = Generic::new(display, Interest::READ, Mode::Level);
        let runtime_display = crate::platform::event_sources::ScopedSource::insert(
            &event_loop,
            display_source,
            |_, display, state| {
                let _latency = crate::utils::latency::Span::new(
                    crate::utils::latency::Operation::ClientDispatch,
                );
                // SAFETY: we don't drop the display.
                unsafe {
                    display.get_mut().dispatch_clients(state).unwrap();
                }
                Ok(PostAction::Continue)
            },
        )
        .unwrap();

        event_loop
            .insert_source(
                Timer::from_duration(Duration::from_secs(60)),
                |_, _, state| {
                    let _span = clarion_tracy_span!("startup timeout");
                    state.clarion.is_at_startup = false;
                    state.clarion.recompute_window_rules();
                    state.clarion.recompute_layer_rules();
                    TimeoutAction::Drop
                },
            )
            .unwrap();

        drop(config_);
        // Constructed before the literal because the literal moves `config`.
        let idle_policy = crate::idle::IdlePolicy::new(&config.borrow().idle, Instant::now());
        let mut clarion = Self {
            config,
            config_file_output_config,
            output_preview: None,
            confirmed_output_preview: None,
            last_cancelled_output_preview: None,
            config_file_watcher: None,

            event_loop,
            scheduler,
            runtime_executor: Some(runtime_executor),
            runtime_display: Some(runtime_display),
            shutting_down: false,
            clients: Default::default(),
            owned_tasks: crate::utils::owned_tasks::TaskPool::new(),
            keymap_request: 0,
            #[cfg(test)]
            keymap_completions: 0,
            session_restore_request: 0,
            stop_signal,
            socket_name,
            display_handle,
            is_session_instance,
            start_time: Instant::now(),
            is_at_startup: true,
            clock: animation_clock,

            layout,
            scene_resources,
            core_scene_outputs: Vec::new(),
            core_protocol_ops: Vec::new(),
            screenshot_tasks: Default::default(),
            screencopy_tasks: Default::default(),
            capture_scene_epoch: Default::default(),
            global_space: Space::default(),
            sorted_outputs: Vec::default(),
            output_state: HashMap::new(),
            unmapped_windows: HashMap::new(),
            unmapped_layer_surfaces: HashSet::new(),
            mapped_layer_surfaces: HashMap::new(),
            root_surface: HashMap::new(),
            dmabuf_pre_commit_hook: HashMap::new(),
            blocker_cleared_tx,
            blocker_cleared_rx,
            monitors_active: true,
            is_lid_closed: false,

            devices: HashSet::new(),
            tablets: HashMap::new(),
            touch: HashSet::new(),

            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            kde_decoration_state,
            layer_shell_state,
            session_lock_state,
            foreign_toplevel_state,
            ext_workspace_state,
            output_management_state,
            color_management_state: Default::default(),
            screencopy_state,
            viewporter_state,
            background_effect_state,
            gtk_shell_state,
            qt_app_menu_state,
            xdg_foreign_state,
            text_input_state,
            input_method_state,
            keyboard_shortcuts_inhibit_state,
            virtual_keyboard_state,
            virtual_pointer_state,
            shm_state,
            output_manager_state,
            dmabuf_state,
            drm_syncobj_state: None,
            fractional_scale_manager_state,
            content_type_state,
            commit_timing_manager_state,
            commit_timing: Default::default(),
            fifo_manager_state,
            fifo: Default::default(),
            seat_state,
            tablet_state,
            pointer_gestures_state,
            relative_pointer_state,
            pointer_constraints_state,
            idle_notifier_state,
            idle_inhibit_manager_state,
            data_device_state,
            primary_selection_state,
            wlr_data_control_state,
            ext_data_control_state,
            popups: PopupManager::default(),
            popup_grab: None,
            suppressed_keys: HashSet::new(),
            suppressed_buttons: HashSet::new(),
            bind_cooldown_timers: HashMap::new(),
            bind_repeat_timer: Option::default(),
            presentation_state,
            security_context_state,
            gamma_control_manager_state,
            activation_state,
            mutter_x11_interop_state,
            #[cfg(test)]
            single_pixel_buffer_state,

            seat,
            keyboard_focus: KeyboardFocus::Layout { surface: None },
            layer_shell_on_demand_focus: None,
            idle_inhibiting_surfaces: HashSet::new(),
            is_fdo_idle_inhibited: Arc::new(AtomicBool::new(false)),
            keyboard_shortcuts_inhibiting_surfaces: HashMap::new(),
            xkb_from_locale1: None,
            cursor_manager,
            cursor_texture_cache: Default::default(),
            cursor_shape_manager_state,
            dnd_icon: None,
            pointer_contents: PointContents::default(),
            pointer_visibility: PointerVisibility::Visible,
            pointer_inactivity_timer: None,
            pointer_inactivity_timer_got_reset: false,
            notified_activity_this_iteration: false,
            last_user_activity: Instant::now(),
            idle_policy,
            pending_dpms_blank: None,
            hot_corners: Default::default(),
            pointer_constraint_position_hint: None,
            tablet_cursor_location: None,
            gesture_swipe_3f_cumulative: None,
            launcher_pinch: None,
            overview_scroll_swipe_gesture: ScrollSwipeGesture::new(),
            vertical_wheel_tracker: ScrollTracker::new(120),
            horizontal_wheel_tracker: ScrollTracker::new(120),
            mods_with_mouse_binds,
            mods_with_wheel_binds,
            mods_with_tablet_stylus_binds,

            // 10 is copied from Clutter: DISCRETE_SCROLL_STEP.
            vertical_finger_scroll_tracker: ScrollTracker::new(10),
            horizontal_finger_scroll_tracker: ScrollTracker::new(10),
            mods_with_finger_scroll_binds,

            lock_state: LockState::Unlocked,
            locked_hint: None,

            screenshot_ui,
            config_error_notification,
            hotkey_overlay,
            exit_confirm_dialog,

            window_mru_ui,
            pending_mru_commit: None,

            pick_window: None,
            pick_color: None,

            debug_draw_opaque_regions: false,
            debug_draw_damage: false,

            #[cfg(feature = "dbus")]
            dbus: None,
            #[cfg(feature = "dbus")]
            a11y_manager: None,
            #[cfg(feature = "dbus")]
            a11y,
            #[cfg(feature = "dbus")]
            inhibit_power_key_fd: None,

            ipc_server,
            ipc_outputs_changed: false,

            satellite: None,
            satellite_probe_request: 0,

            #[cfg(feature = "xdp-gnome-screencast")]
            casting: screencasting,
        };

        clarion.reset_pointer_inactivity_timer();

        clarion
    }

    pub fn insert_client(&mut self, client: NewClient) {
        if self.shutting_down {
            return;
        }
        let NewClient {
            client,
            restricted,
            credentials_unknown,
        } = client;

        let config = self.config.borrow();
        let data = Arc::new(ClientState {
            compositor_state: Default::default(),
            can_view_decoration_globals: config.prefer_no_csd,
            primary_selection_disabled: config.clipboard.disable_primary,
            restricted,
            credentials_unknown,
            registration: self.clients.registration(),
        });

        match self.display_handle.insert_client(client, data.clone()) {
            Ok(client) => data.registration.connected(client.id()),
            Err(err) => warn!("error inserting client: {err}"),
        }
    }

    #[cfg(feature = "dbus")]
    pub fn inhibit_power_key(&mut self) -> anyhow::Result<()> {
        use smithay::reexports::rustix::io::{fcntl_setfd, FdFlags};

        let conn = zbus::blocking::Connection::system()?;

        let message = conn.call_method(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            Some("org.freedesktop.login1.Manager"),
            "Inhibit",
            &("handle-power-key", "clarion", "Power key handling", "block"),
        )?;

        let fd: zbus::zvariant::OwnedFd = message.body().deserialize()?;

        // Don't leak the fd to child processes.
        if let Err(err) = fcntl_setfd(&fd, FdFlags::CLOEXEC) {
            warn!("error setting CLOEXEC on inhibit fd: {err:?}");
        };

        self.inhibit_power_key_fd = Some(fd);

        Ok(())
    }

    /// Repositions all outputs, optionally adding a new output.
    pub fn reposition_outputs(&mut self, new_output: Option<&Output>) {
        let _span = clarion_tracy_span!("Clarion::reposition_outputs");

        #[derive(Debug)]
        struct Data {
            output: Output,
            name: OutputName,
            position: Option<Point<i32, Logical>>,
            config: Option<clarion_config::Position>,
        }

        let config = self.config.borrow();
        let mut outputs = vec![];
        for output in self.global_space.outputs().chain(new_output) {
            let name = output.user_data().get::<OutputName>().unwrap();
            let position = self.global_space.output_geometry(output).map(|geo| geo.loc);
            let config = config.outputs.find(name).and_then(|c| c.position);

            outputs.push(Data {
                output: output.clone(),
                name: name.clone(),
                position,
                config,
            });
        }
        drop(config);

        for Data { output, .. } in &outputs {
            self.global_space.unmap_output(output);
        }

        // Connectors can appear in udev in any order. If we sort by name then we get output
        // positioning that does not depend on the order they appeared.
        //
        // This sorting first compares by make/model/serial so that it is stable regardless of the
        // connector name. However, if make/model/serial is equal or unknown, then it does fall
        // back to comparing the connector name, which should always be unique.
        outputs.sort_unstable_by(|a, b| a.name.compare(&b.name));

        // Place all outputs with explicitly configured position first, then the unconfigured ones.
        outputs.sort_by_key(|d| d.config.is_none());

        trace!(
            "placing outputs in order: {:?}",
            outputs.iter().map(|d| &d.name.connector)
        );

        self.sorted_outputs = outputs
            .iter()
            .map(|Data { output, .. }| output.clone())
            .collect();

        for data in outputs.into_iter() {
            let Data {
                output,
                name,
                position,
                config,
            } = data;

            let size = output_size(&output).to_i32_round();

            let new_position = config
                .map(|pos| Point::from((pos.x, pos.y)))
                .filter(|pos| {
                    // Ensure that the requested position does not overlap any existing output.
                    let target_geom = Rectangle::new(*pos, size);

                    let overlap = self
                        .global_space
                        .outputs()
                        .map(|output| self.global_space.output_geometry(output).unwrap())
                        .find(|geom| geom.overlaps(target_geom));

                    if let Some(overlap) = overlap {
                        warn!(
                            "output {} at x={} y={} sized {}x{} \
                             overlaps an existing output at x={} y={} sized {}x{}, \
                             falling back to automatic placement",
                            name.connector,
                            pos.x,
                            pos.y,
                            size.w,
                            size.h,
                            overlap.loc.x,
                            overlap.loc.y,
                            overlap.size.w,
                            overlap.size.h,
                        );

                        false
                    } else {
                        true
                    }
                })
                .unwrap_or_else(|| {
                    let x = self
                        .global_space
                        .outputs()
                        .map(|output| self.global_space.output_geometry(output).unwrap())
                        .map(|geom| geom.loc.x + geom.size.w)
                        .max()
                        .unwrap_or(0);

                    Point::from((x, 0))
                });

            self.global_space.map_output(&output, new_position);

            // By passing new_output as an Option, rather than mapping it into a bogus location
            // in global_space, we ensure that this branch always runs for it.
            if Some(new_position) != position {
                debug!(
                    "putting output {} at x={} y={}",
                    name.connector, new_position.x, new_position.y
                );
                output.change_current_state(None, None, None, Some(new_position));
                self.ipc_outputs_changed = true;
                self.queue_redraw(&output);
            }
        }
    }

    pub fn add_output(
        &mut self,
        output: Output,
        refresh_interval: Option<Duration>,
        vrr: bool,
        kms_latch_enabled: bool,
    ) {
        let global = output.create_global::<State>(&self.display_handle);

        let name = output.user_data().get::<OutputName>().unwrap();

        let config = self.config.borrow();
        let c = config.outputs.find(name);
        let scale = c.and_then(|c| c.scale).map(|s| s.0).unwrap_or_else(|| {
            let size_mm = output.physical_properties().size;
            let resolution = output.current_mode().unwrap().size;
            guess_monitor_scale(size_mm, resolution)
        });
        let scale = closest_representable_scale(scale.clamp(0.1, 10.));

        let requested_transform = panel_orientation(&output)
            + c.map(|c| ipc_transform_to_smithay(c.transform))
                .unwrap_or(Transform::Normal);
        let transform = crate::backend::winit::output_transform(&output, requested_transform);

        let mut backdrop_color = c
            .and_then(|c| c.backdrop_color)
            .unwrap_or(config.overview.backdrop_color)
            .to_array_unpremul();
        backdrop_color[3] = 1.;

        let mut layout_config = c.and_then(|c| c.layout.clone());
        // Support the deprecated non-layout background-color key.
        if let Some(layout) = &mut layout_config {
            if layout.background_color.is_none() {
                layout.background_color = c.and_then(|c| c.background_color);
            }
        }
        drop(config);

        // Set scale and transform before adding to the layout since that will read the output size.
        output.change_current_state(
            None,
            Some(transform),
            Some(output::Scale::Fractional(scale)),
            None,
        );

        self.layout.add_output(output.clone(), layout_config);

        let lock_render_state = if self.is_locked() {
            // We haven't rendered anything yet so it's as good as locked.
            LockRenderState::Locked
        } else {
            LockRenderState::Unlocked
        };

        let size = output_size(&output);
        let state = OutputState {
            global,
            render_geometry: OutputRenderGeometry::new(&output),
            schedule: OutputSchedule::new(refresh_interval, vrr, kms_latch_enabled),
            on_demand_vrr_enabled: false,
            vblank_throttle: VBlankThrottle::new(self.event_loop.clone(), name.connector.clone()),
            backdrop_buffer: SolidColorBuffer::new(size, backdrop_color),
            overview_shade_buffer: SolidColorBuffer::new(size, [0., 0., 0., 1.]),
            xray: Xray::new(),
            lock_render_state,
            lock_surface: None,
            lock_color_buffer: SolidColorBuffer::new(size, CLEAR_COLOR_LOCKED),
            screen_transition: None,
            debug_damage_tracker: OutputDamageTracker::from_output(&output),
        };
        let rv = self.output_state.insert(output.clone(), state);
        assert!(rv.is_none(), "output was already tracked");
        self.color_management_state.output_added(&output);

        // Must be last since it will call queue_redraw(output) which needs things to be filled-in.
        self.reposition_outputs(Some(&output));
    }

    pub fn output_exists(&self, output: &Output) -> bool {
        self.output_state.contains_key(output)
    }

    /// Converts a `WlOutput` to a corresponding `Output` if it exists.
    ///
    /// Compared to raw `Output::from_resource`, this method also verifies that the output still
    /// exists in clarion. Right after the output global is disabled, but before it is removed for
    /// good, `Output::from_resource` will succeed, but since clarion already forgot the output,
    /// accessing it can cause logic bugs.
    pub fn output_from_resource(&self, wl_output: &WlOutput) -> Option<Output> {
        Output::from_resource(wl_output).filter(|output| self.output_exists(output))
    }

    pub fn remove_output(&mut self, output: &Output) {
        self.hot_corners.reset_output(output);
        // Output removal can be requested more than once when a connector is
        // disconnected while its deferred teardown callback is still queued.
        // In that case the layout and output state have already been cleaned up;
        // attempting the normal path would panic in `HashMap::remove(...).unwrap()`
        // (and, earlier, in `Layout::remove_output`).
        if !self.output_state.contains_key(output) {
            return;
        }

        self.release_fifo_after_estimated_vblank(output);
        for layer in layer_map_for_output(output).layers() {
            layer.layer_surface().send_close();
        }

        self.layout.remove_output(output);
        self.global_space.unmap_output(output);
        self.reposition_outputs(None);
        self.gamma_control_manager_state.output_removed(output);
        self.color_management_state.output_removed(output);

        let mut state = self.output_state.remove(output).unwrap();
        state.schedule.cancel_timers(&self.event_loop);

        self.stop_casts_for_target(CastTarget::output(output));
        self.screencopy_state.remove_output(output);

        // Disable the output global and remove some time later to give the clients some time to
        // process it.
        let global = state.global;
        self.display_handle.disable_global::<State>(global.clone());
        self.event_loop
            .insert_source(
                Timer::from_duration(Duration::from_secs(10)),
                move |_, _, state| {
                    state
                        .clarion
                        .display_handle
                        .remove_global::<State>(global.clone());
                    TimeoutAction::Drop
                },
            )
            .unwrap();

        match mem::take(&mut self.lock_state) {
            LockState::Locking(confirmation) => {
                // We're locking and an output was removed, check if the requirements are now met.
                let all_locked = self
                    .output_state
                    .values()
                    .all(|state| state.lock_render_state == LockRenderState::Locked);

                if all_locked {
                    let lock = confirmation.ext_session_lock().clone();
                    confirmation.lock();
                    self.lock_state = LockState::Locked(lock);
                } else {
                    // Still waiting.
                    self.lock_state = LockState::Locking(confirmation);
                }
            }
            lock_state => {
                self.lock_state = lock_state;
                self.maybe_continue_to_locking();
            }
        }

        if self.screenshot_ui.close() {
            self.cursor_manager
                .set_cursor_image(CursorImageStatus::default_named());
            self.queue_redraw_all();
        }

        if self.window_mru_ui.remove_output(output) {
            // The owner has gone away; clear its backdrop on every remaining output as well.
            self.queue_redraw_all();
        }
    }

    pub fn output_resized(&mut self, output: &Output) {
        // Layer-shell commits also call this to update the working area. A
        // panel or animated wallpaper repaint does not change output coordinates
        // and must not rearm a stationary hot corner or discard a mode handoff.
        let geometry = OutputRenderGeometry::new(output);
        let geometry_changed = self.output_state.get_mut(output).is_some_and(|state| {
            if state.render_geometry == geometry {
                return false;
            }
            state.render_geometry = geometry;
            if state
                .screen_transition
                .as_ref()
                .is_some_and(ScreenTransition::is_overview)
            {
                state.screen_transition = None;
            }
            true
        });
        if geometry_changed {
            self.hot_corners.reset_output(output);
        }
        let output_size = output_size(output);
        let scale = output.current_scale();
        let transform = output.current_transform();

        {
            let mut layer_map = layer_map_for_output(output);
            for layer in layer_map.layers() {
                layer.with_surfaces(|surface, data| {
                    send_scale_transform(surface, data, scale, transform);
                });

                if let Some(mapped) = self.mapped_layer_surfaces.get_mut(layer) {
                    mapped.update_sizes(output_size, scale.fractional_scale());
                }
            }
            layer_map.arrange();
        }

        self.layout.update_output_size(output);

        if let Some(state) = self.output_state.get_mut(output) {
            state.backdrop_buffer.resize(output_size);
            state.overview_shade_buffer.resize(output_size);

            state.lock_color_buffer.resize(output_size);
            if let Some(lock_surface) = &state.lock_surface {
                configure_lock_surface(lock_surface, output);
            }
        }

        // If the output size changed with an open screenshot UI, close the screenshot UI.
        if let Some((old_size, old_scale, old_transform)) = self.screenshot_ui.output_size(output) {
            let output_mode = output.current_mode().unwrap();
            let size = transform.transform_size(output_mode.size);
            let scale = output.current_scale().fractional_scale();
            // FIXME: scale changes and transform flips shouldn't matter but they currently do since
            // I haven't quite figured out how to draw the screenshot textures in
            // physical coordinates.
            if old_size != size || old_scale != scale || old_transform != transform {
                self.screenshot_ui.close();
                self.cursor_manager
                    .set_cursor_image(CursorImageStatus::default_named());
                self.queue_redraw_all();
                return;
            }
        }

        self.queue_redraw(output);
    }

    pub fn deactivate_monitors(&mut self, backend: &mut Backend) {
        if !self.monitors_active {
            return;
        }

        self.monitors_active = false;
        backend.set_monitors_active(false);
    }

    pub fn activate_monitors(&mut self, backend: &mut Backend) {
        if self.monitors_active {
            return;
        }

        self.monitors_active = true;
        backend.set_monitors_active(true);

        self.queue_redraw_all();
    }

    pub fn output_under(&self, pos: Point<f64, Logical>) -> Option<(&Output, Point<f64, Logical>)> {
        let output = self.global_space.output_under(pos).next()?;
        let pos_within_output = pos
            - self
                .global_space
                .output_geometry(output)
                .unwrap()
                .loc
                .to_f64();

        Some((output, pos_within_output))
    }

    fn is_inside_hot_corner(
        &self,
        output: &Output,
        pos: Point<f64, Logical>,
    ) -> Option<HotCornerKind> {
        let config = self.config.borrow();
        let hot_corners = output
            .user_data()
            .get::<OutputName>()
            .and_then(|name| config.outputs.find(name))
            .and_then(|c| c.hot_corners)
            .unwrap_or(config.gestures.hot_corners);

        if hot_corners.off {
            return None;
        }

        // Use size from the ceiled output geometry, since that's what we currently use for pointer
        // motion clamping.
        let geom = self.global_space.output_geometry(output).unwrap();
        let size = geom.size.to_f64();

        let contains = move |corner: Point<f64, Logical>| {
            Rectangle::new(corner, Size::new(8., 8.)).contains(pos)
        };

        // Clarion: Top-right hot corner activates the Wayfire-style 2D Expo grid wall!
        if hot_corners.top_right && contains(Point::new(size.w - 8., 0.)) {
            return Some(HotCornerKind::TopRight);
        }

        // Clarion: Bottom-left hot corner activates Clarion GUI Settings!
        if hot_corners.bottom_left && contains(Point::new(0., size.h - 8.)) {
            return Some(HotCornerKind::BottomLeft);
        }

        if hot_corners.bottom_right && contains(Point::new(size.w - 8., size.h - 8.)) {
            return Some(HotCornerKind::BottomRight);
        }

        // Top-left follows the configured Stage/Strip overview style.
        if hot_corners.top_left && contains(Point::new(0., 0.)) {
            return Some(HotCornerKind::TopLeft);
        }

        None
    }

    /// Retain the entered corner until the pointer leaves its four-pixel halo.
    /// Track its output and kind so absolute pointer jumps can enter another corner.
    pub(crate) fn hot_corner_motion_entered(
        &mut self,
        pos: Point<f64, Logical>,
        candidate: Option<(Output, HotCornerKind)>,
    ) -> bool {
        self.hot_corners.motion(pos, candidate, |output| {
            self.global_space.output_geometry(output)
        })
    }

    pub(crate) fn reset_hot_corner(&mut self) {
        self.hot_corners.reset();
    }

    #[cfg(test)]
    pub(crate) fn hot_corner_latch(&self) -> Option<(Output, HotCornerKind)> {
        self.hot_corners.entered()
    }

    pub fn is_sticky_obscured_under(
        &self,
        output: &Output,
        pos_within_output: Point<f64, Logical>,
    ) -> bool {
        // The ordering here must be consistent with the ordering in render() so that input is
        // consistent with the visuals.

        // Check if some layer-shell surface is on top.
        let layers = layer_map_for_output(output);
        let layer_surface_under = |layer, popup| {
            layers
                .layers_on(layer)
                .rev()
                .find_map(|layer| {
                    let mapped = self.mapped_layer_surfaces.get(layer)?;

                    let mut layer_pos_within_output =
                        layers.layer_geometry(layer).unwrap().loc.to_f64();
                    layer_pos_within_output += mapped.bob_offset();

                    let surface_type = if popup {
                        WindowSurfaceType::POPUP
                    } else {
                        WindowSurfaceType::TOPLEVEL
                    } | WindowSurfaceType::SUBSURFACE;
                    layer.surface_under(pos_within_output - layer_pos_within_output, surface_type)
                })
                .is_some()
        };

        let layer_toplevel_under = |layer| layer_surface_under(layer, false);
        let layer_popup_under = |layer| layer_surface_under(layer, true);

        if layer_popup_under(Layer::Overlay) || layer_toplevel_under(Layer::Overlay) {
            return true;
        }

        let mon = self.layout.monitor_for_output(output).unwrap();
        if mon.render_above_top_layer() {
            return false;
        }

        if self
            .is_inside_hot_corner(output, pos_within_output)
            .is_some()
        {
            return true;
        }

        if layer_popup_under(Layer::Top) || layer_toplevel_under(Layer::Top) {
            return true;
        }

        false
    }

    pub fn is_layout_obscured_under(
        &self,
        output: &Output,
        pos_within_output: Point<f64, Logical>,
    ) -> bool {
        if self.layout.is_overview_open() {
            return false;
        }

        // Check if some layer-shell surface is on top.
        let layers = layer_map_for_output(output);
        let layer_popup_under = |layer| {
            layers
                .layers_on(layer)
                .rev()
                .find_map(|layer_surface| {
                    let mapped = self.mapped_layer_surfaces.get(layer_surface)?;
                    if mapped.place_within_backdrop() {
                        return None;
                    }

                    let mut layer_pos_within_output =
                        layers.layer_geometry(layer_surface).unwrap().loc.to_f64();
                    layer_pos_within_output += mapped.bob_offset();

                    // Background and bottom layers move together with the workspaces.
                    let mon = self.layout.monitor_for_output(output)?;
                    let (_, geo) = mon.workspace_under(pos_within_output)?;
                    layer_pos_within_output += geo.loc;

                    let surface_type = WindowSurfaceType::POPUP | WindowSurfaceType::SUBSURFACE;
                    layer_surface
                        .surface_under(pos_within_output - layer_pos_within_output, surface_type)
                })
                .is_some()
        };

        if layer_popup_under(Layer::Bottom) || layer_popup_under(Layer::Background) {
            return true;
        }

        false
    }

    /// Returns the workspace under the position to be activated.
    ///
    /// The return value is an output and a workspace index on it.
    pub fn workspace_under(
        &self,
        extended_bounds: bool,
        pos: Point<f64, Logical>,
    ) -> Option<(Output, &Workspace<Mapped>)> {
        if self.exit_confirm_dialog.is_open()
            || self.is_locked()
            || self.screenshot_ui.is_open()
            || self.overview_layout_input_blocked()
        {
            return None;
        }

        let (output, pos_within_output) = self.output_under(pos)?;

        if self.is_sticky_obscured_under(output, pos_within_output) {
            return None;
        }

        if self.is_layout_obscured_under(output, pos_within_output) {
            return None;
        }

        let ws = self
            .layout
            .workspace_under(extended_bounds, output, pos_within_output)?;
        Some((output.clone(), ws))
    }

    pub fn workspace_under_cursor(
        &self,
        extended_bounds: bool,
    ) -> Option<(Output, &Workspace<Mapped>)> {
        let pos = self.seat.get_pointer().unwrap().current_location();
        self.workspace_under(extended_bounds, pos)
    }

    /// Returns the window under the position to be activated.
    ///
    /// The cursor may be inside the window's activation region, but not within the window's input
    /// region.
    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<&Mapped> {
        if self.exit_confirm_dialog.is_open()
            || self.is_locked()
            || self.screenshot_ui.is_open()
            || self.window_mru_ui.is_open()
            || self.overview_layout_input_blocked()
        {
            return None;
        }

        let (output, pos_within_output) = self.output_under(pos)?;

        if self.is_sticky_obscured_under(output, pos_within_output) {
            return None;
        }

        if let Some((window, _loc)) = self
            .layout
            .interactive_moved_window_under(output, pos_within_output)
        {
            return Some(window);
        }

        if self.is_layout_obscured_under(output, pos_within_output) {
            return None;
        }

        let (window, _loc) = self.layout.window_under(output, pos_within_output)?;
        Some(window)
    }

    /// Returns the window under the cursor to be activated.
    ///
    /// The cursor may be inside the window's activation region, but not within the window's input
    /// region.
    pub fn window_under_cursor(&self) -> Option<&Mapped> {
        let pos = self.seat.get_pointer().unwrap().current_location();
        self.window_under(pos)
    }

    /// Returns contents under the given point.
    ///
    /// We don't have a proper global space for all windows, so this function converts window
    /// locations to global space according to where they are rendered.
    ///
    /// This function does not take pointer or touch grabs into account.
    pub fn contents_under(&self, pos: Point<f64, Logical>) -> PointContents {
        let mut rv = PointContents::default();

        let Some((output, pos_within_output)) = self.output_under(pos) else {
            return rv;
        };
        rv.output = Some(output.clone());
        let output_pos_in_global_space = self.global_space.output_geometry(output).unwrap().loc;

        // The ordering here must be consistent with the ordering in render() so that input is
        // consistent with the visuals.

        if self.exit_confirm_dialog.is_open() {
            return rv;
        } else if self.is_locked() {
            let Some(state) = self.output_state.get(output) else {
                return rv;
            };
            let Some(surface) = state.lock_surface.as_ref() else {
                return rv;
            };

            rv.surface = under_from_surface_tree(
                surface.wl_surface(),
                pos_within_output,
                // We put lock surfaces at (0, 0).
                (0, 0),
                WindowSurfaceType::ALL,
            )
            .map(|(surface, pos_within_output)| {
                (
                    surface,
                    (pos_within_output + output_pos_in_global_space).to_f64(),
                )
            });

            return rv;
        }

        if self.screenshot_ui.is_open() || self.window_mru_ui.is_open() {
            return rv;
        }

        let layers = layer_map_for_output(output);
        let layer_surface_under = |layer, popup| {
            layers
                .layers_on(layer)
                .rev()
                .find_map(|layer_surface| {
                    let mapped = self.mapped_layer_surfaces.get(layer_surface)?;
                    if mapped.place_within_backdrop() {
                        return None;
                    }

                    let mut layer_pos_within_output =
                        layers.layer_geometry(layer_surface).unwrap().loc.to_f64();
                    layer_pos_within_output += mapped.bob_offset();

                    // Background and bottom layers move together with the workspaces.
                    if matches!(layer, Layer::Background | Layer::Bottom) {
                        let mon = self.layout.monitor_for_output(output)?;
                        let (_, geo) = mon.workspace_under(pos_within_output)?;
                        layer_pos_within_output += geo.loc;
                        // Don't need to deal with zoom here because in the overview background and
                        // bottom layers don't receive input.
                    }

                    let surface_type = if popup {
                        WindowSurfaceType::POPUP
                    } else {
                        WindowSurfaceType::TOPLEVEL
                    } | WindowSurfaceType::SUBSURFACE;

                    layer_surface
                        .surface_under(pos_within_output - layer_pos_within_output, surface_type)
                        .map(|(surface, pos_within_layer)| {
                            (
                                (surface, pos_within_layer.to_f64() + layer_pos_within_output),
                                layer_surface,
                            )
                        })
                })
                .map(|(s, l)| (Some(s), (None, Some(l.clone()))))
        };

        let layer_toplevel_under = |layer| layer_surface_under(layer, false);
        let layer_popup_under = |layer| layer_surface_under(layer, true);

        let mapped_hit_data = |(mapped, hit): (&Mapped, HitType)| {
            // Closing previews still use transformed coordinates. Keep the hit
            // opaque without activating or sending events into the real client.
            if self.overview_layout_input_blocked() {
                return (None, (None, None));
            }
            let window = &mapped.window;
            let surface_and_pos = if let HitType::Input { win_pos } = hit {
                let win_pos_within_output = win_pos;
                window
                    .surface_under(
                        pos_within_output - win_pos_within_output,
                        WindowSurfaceType::ALL,
                    )
                    .map(|(s, pos_within_window)| {
                        (s, pos_within_window.to_f64() + win_pos_within_output)
                    })
            } else {
                None
            };
            (surface_and_pos, (Some((window.clone(), hit)), None))
        };

        let interactive_moved_window_under = || {
            self.layout
                .interactive_moved_window_under(output, pos_within_output)
                .map(mapped_hit_data)
        };
        let window_under = || {
            self.layout
                .window_under(output, pos_within_output)
                .map(mapped_hit_data)
        };

        let Some(mon) = self.layout.monitor_for_output(output) else {
            return rv;
        };

        let mut under =
            layer_popup_under(Layer::Overlay).or_else(|| layer_toplevel_under(Layer::Overlay));

        let is_overview_open =
            self.layout.is_overview_open() || self.layout.is_window_spread_visible();

        // When rendering above the top layer, we put the regular monitor elements first.
        // Otherwise, we will render all layer-shell pop-ups and the top layer on top.
        if mon.render_above_top_layer() {
            under = under
                .or_else(interactive_moved_window_under)
                .or_else(window_under)
                .or_else(|| layer_popup_under(Layer::Top))
                .or_else(|| layer_toplevel_under(Layer::Top))
                .or_else(|| layer_popup_under(Layer::Bottom))
                .or_else(|| layer_popup_under(Layer::Background))
                .or_else(|| layer_toplevel_under(Layer::Bottom))
                .or_else(|| layer_toplevel_under(Layer::Background));
        } else {
            if under.is_none() {
                if let Some(corner) = self.is_inside_hot_corner(output, pos_within_output) {
                    rv.hot_corner = Some(corner);
                    return rv;
                }
            }

            under = under
                .or_else(|| layer_popup_under(Layer::Top))
                .or_else(|| layer_toplevel_under(Layer::Top));

            under = under.or_else(interactive_moved_window_under);

            if !is_overview_open {
                under = under
                    .or_else(|| layer_popup_under(Layer::Bottom))
                    .or_else(|| layer_popup_under(Layer::Background));
            }

            under = under.or_else(window_under);

            if !is_overview_open {
                under = under
                    .or_else(|| layer_toplevel_under(Layer::Bottom))
                    .or_else(|| layer_toplevel_under(Layer::Background));
            }
        }

        let Some((mut surface_and_pos, (window, layer))) = under else {
            return rv;
        };

        if let Some((_, surface_pos)) = &mut surface_and_pos {
            *surface_pos += output_pos_in_global_space.to_f64();
        }

        rv.surface = surface_and_pos;
        rv.window = window;
        rv.layer = layer;
        rv
    }

    pub fn output_under_cursor(&self) -> Option<Output> {
        let pos = self.seat.get_pointer().unwrap().current_location();
        self.global_space.output_under(pos).next().cloned()
    }

    pub fn output_left_of(&self, current: &Output) -> Option<Output> {
        let current_geo = self.global_space.output_geometry(current)?;
        let extended_geo = Rectangle::new(
            Point::from((i32::MIN / 2, current_geo.loc.y)),
            Size::from((i32::MAX, current_geo.size.h)),
        );

        self.global_space
            .outputs()
            .map(|output| (output, self.global_space.output_geometry(output).unwrap()))
            .filter(|(_, geo)| center(*geo).x < center(current_geo).x && geo.overlaps(extended_geo))
            .min_by_key(|(_, geo)| center(current_geo).x - center(*geo).x)
            .map(|(output, _)| output)
            .cloned()
    }

    pub fn output_right_of(&self, current: &Output) -> Option<Output> {
        let current_geo = self.global_space.output_geometry(current)?;
        let extended_geo = Rectangle::new(
            Point::from((i32::MIN / 2, current_geo.loc.y)),
            Size::from((i32::MAX, current_geo.size.h)),
        );

        self.global_space
            .outputs()
            .map(|output| (output, self.global_space.output_geometry(output).unwrap()))
            .filter(|(_, geo)| center(*geo).x > center(current_geo).x && geo.overlaps(extended_geo))
            .min_by_key(|(_, geo)| center(*geo).x - center(current_geo).x)
            .map(|(output, _)| output)
            .cloned()
    }

    pub fn output_up_of(&self, current: &Output) -> Option<Output> {
        let current_geo = self.global_space.output_geometry(current)?;
        let extended_geo = Rectangle::new(
            Point::from((current_geo.loc.x, i32::MIN / 2)),
            Size::from((current_geo.size.w, i32::MAX)),
        );

        self.global_space
            .outputs()
            .map(|output| (output, self.global_space.output_geometry(output).unwrap()))
            .filter(|(_, geo)| center(*geo).y < center(current_geo).y && geo.overlaps(extended_geo))
            .min_by_key(|(_, geo)| center(current_geo).y - center(*geo).y)
            .map(|(output, _)| output)
            .cloned()
    }

    pub fn output_down_of(&self, current: &Output) -> Option<Output> {
        let current_geo = self.global_space.output_geometry(current)?;
        let extended_geo = Rectangle::new(
            Point::from((current_geo.loc.x, i32::MIN / 2)),
            Size::from((current_geo.size.w, i32::MAX)),
        );

        self.global_space
            .outputs()
            .map(|output| (output, self.global_space.output_geometry(output).unwrap()))
            .filter(|(_, geo)| center(*geo).y > center(current_geo).y && geo.overlaps(extended_geo))
            .min_by_key(|(_, geo)| center(*geo).y - center(current_geo).y)
            .map(|(output, _)| output)
            .cloned()
    }

    pub fn output_previous_of(&self, current: &Output) -> Option<Output> {
        self.sorted_outputs
            .iter()
            .rev()
            .skip_while(|&output| output != current)
            .nth(1)
            .or(self.sorted_outputs.last())
            .filter(|&output| output != current)
            .cloned()
    }

    pub fn output_next_of(&self, current: &Output) -> Option<Output> {
        self.sorted_outputs
            .iter()
            .skip_while(|&output| output != current)
            .nth(1)
            .or(self.sorted_outputs.first())
            .filter(|&output| output != current)
            .cloned()
    }

    pub fn output_left(&self) -> Option<Output> {
        let active = self.layout.active_output()?;
        self.output_left_of(active)
    }

    pub fn output_right(&self) -> Option<Output> {
        let active = self.layout.active_output()?;
        self.output_right_of(active)
    }

    pub fn output_up(&self) -> Option<Output> {
        let active = self.layout.active_output()?;
        self.output_up_of(active)
    }

    pub fn output_down(&self) -> Option<Output> {
        let active = self.layout.active_output()?;
        self.output_down_of(active)
    }

    pub fn output_previous(&self) -> Option<Output> {
        let active = self.layout.active_output()?;
        self.output_previous_of(active)
    }

    pub fn output_next(&self) -> Option<Output> {
        let active = self.layout.active_output()?;
        self.output_next_of(active)
    }

    pub fn find_output_and_workspace_index(
        &self,
        workspace_reference: WorkspaceReference,
    ) -> Option<(Option<Output>, usize)> {
        let (target_workspace_index, target_workspace) = match workspace_reference {
            WorkspaceReference::Index(index) => {
                return Some((None, index.saturating_sub(1) as usize));
            }
            WorkspaceReference::Name(name) => self.layout.find_workspace_by_name(&name)?,
            WorkspaceReference::Id(id) => {
                let id = WorkspaceId::specific(id);
                self.layout.find_workspace_by_id(id)?
            }
        };

        let target_output = target_workspace.current_output();
        Some((target_output.cloned(), target_workspace_index))
    }

    pub fn find_window_by_id(&self, id: MappedId) -> Option<Window> {
        self.layout
            .windows()
            .find(|(_, m)| m.id() == id)
            .map(|(_, m)| m.window.clone())
    }

    pub fn output_for_tablet(&self) -> Option<&Output> {
        let config = self.config.borrow();
        if config.input.tablet.map_to_focused_output {
            self.layout.active_output()
        } else {
            let map_to_output = config.input.tablet.map_to_output.as_ref();
            map_to_output.and_then(|name| self.output_by_name_match(name))
        }
    }

    pub fn output_for_touch(&self) -> Option<&Output> {
        let config = self.config.borrow();
        let map_to_output = config.input.touch.map_to_output.as_ref();
        map_to_output
            .and_then(|name| self.output_by_name_match(name))
            .or_else(|| self.global_space.outputs().next())
    }

    pub fn output_by_name_match(&self, target: &str) -> Option<&Output> {
        self.global_space
            .outputs()
            .find(|output| output_matches_name(output, target))
    }

    pub fn output_for_root(&self, root: &WlSurface) -> Option<&Output> {
        // Check the main layout.
        let win_out = self.layout.find_window_and_output(root);
        let layout_output = win_out.map(|(_, output)| output);
        if let Some(output) = layout_output {
            return output;
        }

        // Check layer-shell.
        let has_layer_surface = |o: &&Output| {
            layer_map_for_output(o)
                .layer_for_surface(root, WindowSurfaceType::TOPLEVEL)
                .is_some()
        };
        self.layout.outputs().find(has_layer_surface)
    }

    pub fn lock_surface_focus(&self) -> Option<WlSurface> {
        let output_under_cursor = self.output_under_cursor();
        let output = output_under_cursor
            .as_ref()
            .or_else(|| self.layout.active_output())
            .or_else(|| self.global_space.outputs().next())?;

        let state = self.output_state.get(output)?;
        state.lock_surface.as_ref().map(|s| s.wl_surface()).cloned()
    }

    pub(crate) fn flush_core_scene_invalidations(&mut self) {
        self.layout
            .drain_core_scene_outputs(&mut self.core_scene_outputs);
        for connector in self.core_scene_outputs.drain(..) {
            for (output, state) in &mut self.output_state {
                let matches = output
                    .user_data()
                    .get::<OutputName>()
                    .map(|name| name.connector == connector)
                    .unwrap_or_else(|| output.name() == connector);
                if matches {
                    // Queue an event, never force full-screen damage. The immutable
                    // snapshot and damage tracker preserve previous/current bounds,
                    // clipping, shadows and backdrop effects across animation.
                    state.schedule.queue_redraw();
                    break;
                }
            }
        }
    }

    /// Schedules an immediate redraw on all outputs if one is not already scheduled.
    pub fn queue_redraw_all(&mut self) {
        for state in self.output_state.values_mut() {
            state.schedule.queue_redraw();
        }
    }

    /// Attribute completed device timestamps to their original output owner.
    pub(crate) fn record_gpu_timing(
        &mut self,
        sample: crate::render_helpers::vulkan::timing::GpuTimingSample,
    ) {
        for output in self.output_state.values_mut() {
            if output.schedule.record_gpu_timing(sample) {
                break;
            }
        }
    }

    /// Schedules an immediate redraw if one is not already scheduled.
    pub fn queue_redraw(&mut self, output: &Output) {
        // Completion callbacks may retain an output across hot-unplug. Its
        // resources are retired already, so there is no frame left to request.
        let Some(state) = self.output_state.get_mut(output) else {
            return;
        };
        state.schedule.queue_redraw();
    }

    /// Cancel a latch timer whose deadline was calculated from stale output timing.
    /// The pending damage remains queued so the next redraw uses the new mode or VRR state.
    pub(crate) fn cancel_latch_wait(&mut self, output: &Output) -> bool {
        let token = self
            .output_state
            .get_mut(output)
            .and_then(|state| state.schedule.cancel_latch());

        let Some(token) = token else {
            return false;
        };
        self.event_loop.remove(token);
        true
    }

    pub fn redraw_queued_outputs(&mut self, backend: &mut Backend) {
        let _span = clarion_tracy_span!("Clarion::redraw_queued_outputs");

        loop {
            let now = get_monotonic_time();
            let Some(output) = self.next_queued_output(now) else {
                break;
            };
            if self.defer_kms_submission_retry(&output, now) {
                continue;
            }
            if matches!(backend, Backend::Tty(_))
                && self.defer_redraw_to_latch(&output, get_monotonic_time())
            {
                continue;
            }
            trace!("redrawing output");
            self.redraw(backend, &output);
        }
    }

    /// Coalesce new damage during KMS failure backoff before preparing any render elements.
    /// Keeping an existing estimated-vblank timer also preserves older frame/FIFO deadlines.
    pub(crate) fn defer_kms_submission_retry(&mut self, output: &Output, now: Duration) -> bool {
        let Some(deadline) = self.output_state[output].schedule.retry_deadline(now) else {
            return false;
        };
        crate::backend::tty::queue_retry_after_estimated_vblank(self, output.clone(), deadline);
        true
    }

    /// Hold a physical-output redraw until shortly before its predicted presentation time.
    /// Returns true when a timer was armed and the caller should move to another output.
    pub(crate) fn defer_redraw_to_latch(&mut self, output: &Output, now: Duration) -> bool {
        // Safe minimum delay before deferring to a calloop/timerfd timer.
        // On Linux without hard-realtime kernels, timerfd wakeup jitter typically ranges
        // between 200µs and 1000µs depending on CPU C-states. Arming a timer for < 1500µs
        // risks waking up past the hardware VBlank boundary, causing dropped frames.
        // If the remaining time to latch deadline is below this threshold, proceed to render
        // immediately.
        const MIN_TIMER_DELAY: Duration = Duration::from_micros(1500);

        let Some(latch_time) = self.output_state[output].schedule.latch_deadline(now) else {
            return false;
        };
        let delay = latch_time.saturating_sub(now);
        if delay < MIN_TIMER_DELAY {
            return false;
        }

        let timer_output = output.clone();
        let timer = Timer::from_duration(delay);
        let token = match self.event_loop.insert_source(timer, move |_, _, state| {
            let Some(output_state) = state.clarion.output_state.get_mut(&timer_output) else {
                return TimeoutAction::Drop;
            };
            output_state
                .schedule
                .latch_fired(latch_time, get_monotonic_time());
            TimeoutAction::Drop
        }) {
            Ok(token) => token,
            Err(err) => {
                warn!(
                    output = output.name(),
                    "error scheduling KMS latch timer: {err}"
                );
                return false;
            }
        };

        self.output_state
            .get_mut(output)
            .unwrap()
            .schedule
            .arm_latch(token, delay);
        trace!(
            output = output.name(),
            ?delay,
            "waiting for KMS latch deadline"
        );
        true
    }

    /// Freeze bounded owned data without sorting on the desktop thread.
    /// Querying statistics never schedules a redraw.
    pub(crate) fn freeze_frame_timings(&self) -> Vec<crate::frame_stats::FrozenFrameStats> {
        self.output_state
            .iter()
            .map(|(output, state)| state.schedule.freeze_timings(output.name()))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn frame_timings(&self) -> Vec<clarion_ipc::diagnostics::OutputFrameTimings> {
        let mut timings = self
            .freeze_frame_timings()
            .into_iter()
            .map(crate::frame_stats::FrozenFrameStats::finish)
            .collect::<Vec<_>>();
        timings.sort_unstable_by(|a, b| a.output.cmp(&b.output));
        timings
    }

    /// Render the output with the nearest presentation deadline first. In particular, a slow
    /// secondary output should not go first merely because of HashMap iteration order.
    pub(crate) fn next_queued_output(&self, now: Duration) -> Option<Output> {
        // Space order breaks equal-deadline ties consistently without allocating output names.
        self.global_space
            .outputs()
            .filter_map(|output| {
                let state = self.output_state.get(output)?;
                state
                    .schedule
                    .is_queued()
                    .then_some((output, state.schedule.clock()))
            })
            .min_by_key(|(_, clock)| {
                (
                    clock.next_presentation_time_at(now),
                    clock.refresh_interval(),
                )
            })
            .map(|(output, _)| output.clone())
    }

    pub fn render_pointer<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        output: &Output,
        push: &mut dyn FnMut(PointerRenderElements<R>),
    ) {
        self.pointer_scene(output).render(renderer, output, push);
    }

    /// Checks if the pointer should be included on a window cast or screenshot.
    ///
    /// Returns `(cursor_global_pos, win_pos)` if the pointer should be included, or `None`
    /// otherwise.
    pub fn pointer_pos_for_window_cast(
        &self,
        mapped: &Mapped,
    ) -> Option<(Point<f64, Logical>, Point<f64, Logical>)> {
        // Tablet cursor.
        if let Some(tablet_pos) = self.tablet_cursor_location {
            let contents = self.contents_under(tablet_pos);
            if let Some((w, HitType::Input { win_pos })) = contents.window {
                if w == mapped.window {
                    // Tablet tools don't currently expose current focus, and don't currently
                    // have grabs. When those are implemented, this branch should be adjusted
                    // to look more similar to the branch below.
                    return Some((tablet_pos, win_pos));
                }
            }
        }
        // Regular cursor.
        else if let Some((w, HitType::Input { win_pos })) = &self.pointer_contents.window {
            if w == &mapped.window {
                // Grabs can modify the pointer focus, making it different from
                // pointer_contents. Notably, gestures like Mod+MMB will remove the pointer
                // focus, and ClickGrab will keep pointer focus on the clicked window even
                // while it's moving over a different window.
                //
                // So, double-check that current_focus() (after grabs) also matches the pointer
                // contents.
                let pointer = self.seat.get_pointer().unwrap();

                // The DnD grab is a bit special because it has its own focus (data device)
                // while the pointer focus is cleared. That focus is not currently exposed from
                // Smithay, and showing DnD icons on window screenshots seems useful, so let's
                // just allow it during DnD grabs.
                let is_dnd_grab = pointer
                    .with_grab(|_, grab| State::is_dnd_grab(grab.as_any()))
                    .unwrap_or(false);

                let current_focus_matches = is_dnd_grab
                    || pointer
                        .current_focus()
                        .map(|focused| self.find_root_shell_surface(&focused))
                        .is_some_and(|focused| mapped.is_wl_surface(&focused));
                if current_focus_matches {
                    // We don't check for pointer visibility because it can only be Visible or
                    // Hidden, and never Disabled (then it wouldn't have focus). Even when the
                    // pointer is Hidden, we want to render it, since the user explicitly
                    // requested show_pointer = true, and otherwise there's no easy way to
                    // screenshot a window with pointer with hide-when-typing because pressing
                    // the screenshot bind will hide the pointer.
                    return Some((pointer.current_location(), *win_pos));
                }
            }
        }

        None
    }

    pub fn refresh_pointer_outputs(&mut self) {
        if !self.pointer_visibility.is_visible() {
            return;
        }

        let _span = clarion_tracy_span!("Clarion::refresh_pointer_outputs");

        // Check whether we need to draw the tablet cursor or the regular cursor.
        let pointer_pos = self
            .tablet_cursor_location
            .unwrap_or_else(|| self.seat.get_pointer().unwrap().current_location());

        match self.cursor_manager.cursor_image() {
            CursorImageStatus::Surface(ref surface) => {
                let hotspot = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<CursorImageSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .hotspot
                });

                let surface_pos = pointer_pos.to_i32_round() - hotspot;
                let bbox = bbox_from_surface_tree(surface, surface_pos);

                let dnd = self
                    .dnd_icon
                    .as_ref()
                    .map(|icon| &icon.surface)
                    .map(|surface| (surface, bbox_from_surface_tree(surface, surface_pos)));

                // FIXME we basically need to pick the largest scale factor across the overlapping
                // outputs, this is how it's usually done in clients as well.
                let mut cursor_scale = 1.;
                let mut cursor_transform = Transform::Normal;
                let mut dnd_scale = 1.;
                let mut dnd_transform = Transform::Normal;
                for output in self.global_space.outputs() {
                    let geo = self.global_space.output_geometry(output).unwrap();

                    // Compute pointer surface overlap.
                    if let Some(mut overlap) = geo.intersection(bbox) {
                        overlap.loc -= surface_pos;
                        cursor_scale =
                            f64::max(cursor_scale, output.current_scale().fractional_scale());
                        // FIXME: using the largest overlapping or "primary" output transform would
                        // make more sense here.
                        cursor_transform = output.current_transform();
                        output_update(output, Some(overlap), surface);
                    } else {
                        output_update(output, None, surface);
                    }

                    // Compute DnD icon surface overlap.
                    if let Some((surface, bbox)) = dnd {
                        if let Some(mut overlap) = geo.intersection(bbox) {
                            overlap.loc -= surface_pos;
                            dnd_scale =
                                f64::max(dnd_scale, output.current_scale().fractional_scale());
                            // FIXME: using the largest overlapping or "primary" output transform
                            // would make more sense here.
                            dnd_transform = output.current_transform();
                            output_update(output, Some(overlap), surface);
                        } else {
                            output_update(output, None, surface);
                        }
                    }
                }

                with_states(surface, |data| {
                    send_scale_transform(
                        surface,
                        data,
                        output::Scale::Fractional(cursor_scale),
                        cursor_transform,
                    )
                });
                if let Some((surface, _)) = dnd {
                    with_states(surface, |data| {
                        send_scale_transform(
                            surface,
                            data,
                            output::Scale::Fractional(dnd_scale),
                            dnd_transform,
                        );
                    });
                }
            }
            cursor_image => {
                // There's no cursor surface, but there might be a DnD icon.
                let Some(surface) = self.dnd_icon.as_ref().map(|icon| &icon.surface) else {
                    return;
                };

                let icon = if let CursorImageStatus::Named(icon) = cursor_image {
                    *icon
                } else {
                    Default::default()
                };

                let mut dnd_scale = 1.;
                let mut dnd_transform = Transform::Normal;
                for output in self.global_space.outputs() {
                    let geo = self.global_space.output_geometry(output).unwrap();

                    // The default cursor is rendered at the right scale for each output, which
                    // means that it may have a different hotspot for each output.
                    let output_scale = output.current_scale().integer_scale();
                    let cursor = self
                        .cursor_manager
                        .get_cursor_with_name(icon, output_scale)
                        .unwrap_or_else(|| self.cursor_manager.get_default_cursor(output_scale));

                    // For simplicity, we always use frame 0 for this computation. Let's hope the
                    // hotspot doesn't change between frames.
                    let hotspot = XCursor::hotspot(&cursor.frames()[0]).to_logical(output_scale);

                    let surface_pos = pointer_pos.to_i32_round() - hotspot;
                    let bbox = bbox_from_surface_tree(surface, surface_pos);

                    if let Some(mut overlap) = geo.intersection(bbox) {
                        overlap.loc -= surface_pos;
                        dnd_scale = f64::max(dnd_scale, output.current_scale().fractional_scale());
                        // FIXME: using the largest overlapping or "primary" output transform would
                        // make more sense here.
                        dnd_transform = output.current_transform();
                        output_update(output, Some(overlap), surface);
                    } else {
                        output_update(output, None, surface);
                    }
                }

                with_states(surface, |data| {
                    send_scale_transform(
                        surface,
                        data,
                        output::Scale::Fractional(dnd_scale),
                        dnd_transform,
                    );
                });
            }
        }
    }

    pub fn refresh_layout(&mut self) {
        let layout_is_active = match &self.keyboard_focus {
            KeyboardFocus::Layout { .. } => true,
            KeyboardFocus::LayerShell { .. } => false,

            // Draw layout as active in these cases to reduce unnecessary window animations.
            // There's no confusion because these are both fullscreen modes.
            //
            // FIXME: when going into the screenshot UI from a layer-shell focus, and then back to
            // layer-shell, the layout will briefly draw as active, despite never having focus.
            KeyboardFocus::LockScreen { .. } => true,
            KeyboardFocus::ScreenshotUi => true,
            KeyboardFocus::ExitConfirmDialog => true,
            KeyboardFocus::Overview => true,
            KeyboardFocus::Mru => true,
        };

        self.layout.refresh(layout_is_active);
    }

    pub fn refresh_idle_inhibit(&mut self) {
        let _span = clarion_tracy_span!("Clarion::refresh_idle_inhibit");

        self.idle_inhibiting_surfaces.retain(|s| s.is_alive());

        let is_inhibited = self.is_fdo_idle_inhibited.load(Ordering::SeqCst)
            || self.idle_inhibiting_surfaces.iter().any(|surface| {
                with_states(surface, |states| {
                    surface_primary_scanout_output(surface, states).is_some()
                })
            });
        self.idle_notifier_state.set_is_inhibited(is_inhibited);
    }

    pub fn refresh_window_states(&mut self) {
        let _span = clarion_tracy_span!("Clarion::refresh_window_states");

        let config = self.config.borrow();
        self.layout.with_windows_mut(|mapped, _output| {
            mapped.update_tiled_state(config.prefer_no_csd);
        });
        drop(config);
    }

    pub fn refresh_window_rules(&mut self) {
        let _span = clarion_tracy_span!("Clarion::refresh_window_rules");

        let config = self.config.borrow();
        let window_rules = &config.window_rules;

        let mut windows = vec![];
        let mut outputs = HashSet::new();
        let mut privacy_changed = false;
        self.layout.with_windows_mut(|mapped, output| {
            let previous_privacy = mapped.rules().block_out_from;
            if mapped.recompute_window_rules_if_needed(window_rules, self.is_at_startup) {
                privacy_changed |= previous_privacy != mapped.rules().block_out_from;
                windows.push(mapped.window.clone());

                if let Some(output) = output {
                    outputs.insert(output.clone());
                }

                // Since refresh_window_rules() is called after refresh_layout(), we need to update
                // the tiled state right here, so that it's picked up by the following
                // send_pending_configure().
                mapped.update_tiled_state(config.prefer_no_csd);
            }
        });
        drop(config);

        if privacy_changed {
            self.clear_screen_transition_snapshots();
        }

        for win in windows {
            self.layout.update_window(&win, None);
            win.toplevel()
                .expect("no X11 support")
                .send_pending_configure();
        }
        for output in outputs {
            self.queue_redraw(&output);
        }
    }

    pub fn advance_animations(&mut self) {
        let _span = clarion_tracy_span!("Clarion::advance_animations");

        self.layout.advance_animations();
        self.config_error_notification.advance_animations();
        self.exit_confirm_dialog.advance_animations();
        self.screenshot_ui.advance_animations();
        self.window_mru_ui.advance_animations();

        let locking = !matches!(self.lock_state, LockState::Unlocked);
        let overview_disabled = {
            let config = self.config.borrow();
            config.animations.off || config.animations.overview_open_close.0.off
        };
        for state in self.output_state.values_mut() {
            crate::scene::lifecycle::advance_screen_transition(
                &mut state.screen_transition,
                locking,
                overview_disabled,
            );
        }
    }

    pub fn update_render_elements(&mut self, output: Option<&Output>) {
        self.update_xray_render_elements(output);
        self.layout.update_render_elements(output);
        for (out, state) in &mut self.output_state {
            if output.is_none_or(|output| out == output) {
                crate::scene::lifecycle::update_transition(out, state.screen_transition.as_mut());
                crate::scene::lifecycle::update_layers(out, &mut self.mapped_layer_surfaces, false);
            }
        }
    }

    // Capture/unmap preparation needs only the sources referenced by xray effects.
    pub fn update_xray_render_elements(&mut self, output: Option<&Output>) {
        let blur = BlurOptions::from(self.config.borrow().blur);
        for (out, state) in &mut self.output_state {
            if output.is_none_or(|output| out == output) {
                crate::scene::lifecycle::update_xray(
                    out,
                    self.layout.scene_backgrounds(out),
                    &mut state.xray,
                    state.backdrop_buffer.color(),
                    blur,
                );
                crate::scene::lifecycle::update_layers(out, &mut self.mapped_layer_surfaces, true);
            }
        }
    }

    /// A native renderer replacement starts a new pixel-resource epoch. Immutable
    /// snapshots from the previous context must never be sampled on the new one.
    /// Submitted work owns its own buffers until completion; only scene caches
    /// and effects whose old pixels cannot be recreated are retired here.
    pub(crate) fn on_native_renderer_replaced(&mut self) {
        self.layout.on_native_renderer_replaced();
        self.clear_screen_transition_snapshots();
        self.window_mru_ui.on_native_renderer_replaced();
        self.hotkey_overlay.on_native_renderer_replaced();
        self.config_error_notification.on_native_renderer_replaced();
        self.exit_confirm_dialog.on_native_renderer_replaced();
        if self.screenshot_ui.close() {
            self.cursor_manager
                .set_cursor_image(CursorImageStatus::default_named());
        }
        for state in self.output_state.values() {
            state.xray.invalidate_native_resources();
        }
        self.update_shaders();
        self.queue_redraw_all();
    }

    pub fn update_shaders(&mut self) {
        self.layout.update_shaders();

        for mapped in self.mapped_layer_surfaces.values_mut() {
            mapped.update_shaders();
        }
    }

    /// Project only the state needed to compose this output. Scene code cannot
    /// reach protocol mutation, input dispatch, event-loop or device scheduling.
    pub(crate) fn scene_for_output<'a>(
        &'a self,
        output: &'a Output,
    ) -> crate::scene::OutputScene<'a> {
        let state = &self.output_state[output];
        crate::scene::OutputScene {
            output,
            sources: if self.is_locked() || self.screenshot_ui.is_open() {
                crate::layout::scene_source::OutputSceneSource::empty()
            } else {
                self.layout.scene_source(output)
            },
            windows: &self.layout,
            layers: crate::scene::SceneLayers {
                mapped: &self.mapped_layer_surfaces,
            },
            effects: crate::scene::OutputEffects {
                backdrop_buffer: &state.backdrop_buffer,
                overview_shade_buffer: &state.overview_shade_buffer,
                lock_color_buffer: &state.lock_color_buffer,
                lock_surface: state.lock_surface.as_ref().map(LockSurface::wl_surface),
                screen_transition: state.screen_transition.as_ref(),
                xray: &state.xray,
            },
            ui: crate::scene::SceneUi {
                exit_dialog: &self.exit_confirm_dialog,
                config_notification: &self.config_error_notification,
                screenshot: &self.screenshot_ui,
                hotkeys: &self.hotkey_overlay,
                window_switcher: &self.window_mru_ui,
            },
            pointer: self.pointer_scene(output),
            locked: self.is_locked(),
            transitions_allowed: matches!(self.lock_state, LockState::Unlocked),
            debug_draw_opaque_regions: self.debug_draw_opaque_regions,
            preview_render: self.config.borrow().debug.preview_render,
        }
    }

    fn pointer_scene(&self, output: &Output) -> crate::scene::ScenePointer<'_> {
        let pointer = self
            .tablet_cursor_location
            .unwrap_or_else(|| self.seat.get_pointer().unwrap().current_location());
        let output_pos = self.global_space.output_geometry(output).unwrap().loc;
        crate::scene::ScenePointer {
            visible: self.pointer_visibility.is_visible(),
            position: pointer - output_pos.to_f64(),
            manager: &self.cursor_manager,
            textures: &self.cursor_texture_cache,
            elapsed: self.start_time.elapsed(),
            dnd_icon: self
                .dnd_icon
                .as_ref()
                .map(|icon| crate::scene::SceneDndIcon {
                    surface: &icon.surface,
                    offset: icon.offset,
                }),
        }
    }

    pub fn render_to_vec<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        output: &Output,
        include_pointer: bool,
    ) -> Vec<OutputRenderElements<R>> {
        self.scene_for_output(output)
            .render_to_vec(ctx, include_pointer)
    }

    pub fn render<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        output: &Output,
        include_pointer: bool,
        push: &mut dyn FnMut(OutputRenderElements<R>),
    ) {
        self.scene_for_output(output)
            .render(ctx, include_pointer, push);
    }

    pub fn fill_xray_elements(&self, ctx: RenderCtx<GlesRenderer>, output: &Output) {
        crate::scene::SceneLayers {
            mapped: &self.mapped_layer_surfaces,
        }
        .fill_xray_elements(output, &self.output_state[output].xray, ctx);
    }

    pub fn fill_xray_elements_native(
        &self,
        ctx: RenderCtx<crate::render_helpers::vulkan::native::NativeRenderer>,
        output: &Output,
    ) {
        crate::scene::SceneLayers {
            mapped: &self.mapped_layer_surfaces,
        }
        .fill_xray_elements(output, &self.output_state[output].xray, ctx);
    }

    pub fn clear_xray_elements(&self, output: &Output) {
        crate::scene::clear_xray_elements(&self.output_state[output].xray);
    }

    pub fn has_blocked_out_background_layers(&self, output: &Output) -> bool {
        crate::scene::SceneLayers {
            mapped: &self.mapped_layer_surfaces,
        }
        .has_blocked_background(output)
    }

    /// Idle throttling only suppresses continuous effects; finite UI and layout transitions
    /// must finish even when triggered by clients or configuration changes without user input.
    pub(crate) fn animations_need_redraw(&self, output: &Output) -> bool {
        if self.is_locked() {
            return false;
        }

        let idle = self.last_user_activity.elapsed() > Duration::from_secs(30);
        let layout_animations = if idle {
            self.layout.are_transitions_ongoing(Some(output))
        } else {
            self.layout.are_animations_ongoing(Some(output))
        };
        layout_animations
            || self.config_error_notification.are_animations_ongoing()
            || self.exit_confirm_dialog.are_animations_ongoing()
            || self.screenshot_ui.are_animations_ongoing()
            || self.window_mru_ui.are_animations_ongoing_on(output)
            || self.output_state[output].screen_transition.is_some()
            || (!idle
                && (self.cursor_animation_visible_on_output(output)
                    || layer_map_for_output(output)
                        .layers()
                        .filter_map(|surface| self.mapped_layer_surfaces.get(surface))
                        .any(|mapped| mapped.are_animations_ongoing())))
    }

    fn cursor_animation_visible_on_output(&self, output: &Output) -> bool {
        if !self.pointer_visibility.is_visible() {
            return false;
        }
        let Some(mut bounds) = self
            .cursor_manager
            .current_cursor_animation_bounds(output.current_scale().integer_scale())
        else {
            return false;
        };
        let Some(output_geo) = self.global_space.output_geometry(output) else {
            return false;
        };
        let pointer = self
            .tablet_cursor_location
            .unwrap_or_else(|| self.seat.get_pointer().unwrap().current_location());
        bounds.loc += pointer - output_geo.loc.to_f64();

        let scale = Scale::from(output.current_scale().fractional_scale());
        let mut physical = bounds.to_physical_precise_up(scale);
        // Individual frame origins and sizes are rounded by the renderer. Keep
        // a pixel of slack so fractional scales never strand an edge frame.
        physical.loc -= Point::from((1, 1));
        physical.size += Size::from((2, 2));
        let mode = output.current_mode().unwrap();
        let size = output.current_transform().transform_size(mode.size);
        physical.overlaps(Rectangle::from_size(size))
    }

    fn redraw(&mut self, backend: &mut Backend, output: &Output) {
        let _span = clarion_tracy_span!("Clarion::redraw");
        let now = get_monotonic_time();
        let started = std::time::Instant::now();

        let target_presentation_time = self
            .output_state
            .get_mut(output)
            .unwrap()
            .schedule
            .begin_redraw(now);

        // Input mutations use event time, even if another output has a later
        // presentation forecast. Only scene preparation and rendering project.
        self.layout.flush_pending_move();
        let presentation_projection = self.clock.project_to(target_presentation_time);
        self.update_render_elements(Some(output));

        let is_locked = self.is_locked();
        let mut res = RenderResult::Skipped;
        if self.monitors_active {
            let animations_remain = self.animations_need_redraw(output);
            self.output_state
                .get_mut(output)
                .unwrap()
                .schedule
                .set_animations_remain(animations_remain);

            // Render.
            let mut coordinator = crate::frame_pipeline::FrameCoordinator::new(self, output);
            res = backend.render(&mut coordinator, output, target_presentation_time);
        }
        drop(presentation_projection);

        let state = self.output_state.get_mut(output).unwrap();
        let render_elapsed = started.elapsed();
        state.schedule.finish_redraw(render_elapsed, &res);

        self.finish_lock_frame(output, &res, is_locked, backend.redraw_is_deferred());

        self.finish_fifo_frame(output, &res, matches!(backend, Backend::Winit(_)), started);

        self.refresh_on_demand_vrr(backend, output);

        // Send the frame callbacks.
        //
        // FIXME: The logic here could be a bit smarter. Currently, during an animation, the
        // surfaces that are visible for the very last frame (e.g. because the camera is moving
        // away) will receive frame callbacks, and the surfaces that are invisible but will become
        // visible next frame will not receive frame callbacks (so they will show stale contents for
        // one frame). We could advance the animations for the next frame and send frame callbacks
        // according to the expected new positions.
        //
        // However, this should probably be restricted to sending frame callbacks to more surfaces,
        // to err on the safe side.
        if matches!(backend, Backend::Winit(_)) {
            crate::backend::winit::finish_frame(self, output, &res, started);
        } else {
            self.send_frame_callbacks(output);
        }
        let needs_capture = self.screencopy_state.has_queued_for_output(output);
        #[cfg(feature = "xdp-gnome-screencast")]
        let needs_capture = needs_capture || self.casting.casts.iter().any(|cast| cast.is_active());
        // On a TTY, acquiring the primary GLES renderer can set up its context. Avoid that work
        // and the capture queue traversal on ordinary frames without a queued capture.
        if needs_capture {
            backend.with_renderer_for_output(Some(output), |renderer| {
                // Capture reuses the scene prepared for this target. Its clock
                // projection must end before unrelated event-loop mutations.
                let _capture_projection = self.clock.project_to(target_presentation_time);
                match renderer {
                    crate::render_helpers::renderer::AnyRenderer::Gles(renderer) => {
                        #[cfg(feature = "xdp-gnome-screencast")]
                        {
                            // Render and send to PipeWire screencast streams.
                            self.render_for_screen_cast(renderer, output, target_presentation_time);

                            // FIXME: when a window is hidden, it should probably still receive
                            // frame callbacks and get rendered for
                            // screen cast. This is currently
                            // unimplemented, but happens to work by chance, since output
                            // redrawing is more eager than it should be.
                            self.render_windows_for_screen_cast(
                                renderer,
                                output,
                                target_presentation_time,
                            );
                        }

                        self.render_for_screencopy_with_damage(renderer, output);
                    }
                    crate::render_helpers::renderer::AnyRenderer::Native(renderer) => {
                        #[cfg(feature = "xdp-gnome-screencast")]
                        {
                            self.render_for_screen_cast(renderer, output, target_presentation_time);
                            self.render_windows_for_screen_cast(
                                renderer,
                                output,
                                target_presentation_time,
                            );
                        }
                        self.render_for_screencopy_with_damage(renderer, output);
                    }
                }
            });
        }
        self.finish_scene_effects();
    }

    /// A render failure can retire a previously static effect. Wake the owner
    /// once even when the pre-render animation query returned false; no input
    /// event is required, and ordinary frames perform only one Cell read.
    pub(crate) fn finish_scene_effects(&mut self) {
        if self.scene_resources.take_window_effect_wakeup() {
            // Moving windows and overview previews can have sampled the effect
            // on more than one output. Retire all previous visible bounds.
            self.queue_redraw_all();
        }
    }

    pub(crate) fn finish_lock_frame(
        &mut self,
        output: &Output,
        result: &RenderResult,
        is_locked: bool,
        deferred: bool,
    ) {
        let state = self.output_state.get_mut(output).unwrap();
        // Update the lock render state on successful render, or if monitors are inactive. When
        // monitors are inactive on a TTY, they have no framebuffer attached, so no sensitive data
        // from a last render will be visible.
        if !result.is_unpresented() || !self.monitors_active {
            state.lock_render_state = if is_locked {
                LockRenderState::Locked
            } else {
                LockRenderState::Unlocked
            };
        }

        // If we're in process of locking the session, check if the requirements were met.
        match mem::take(&mut self.lock_state) {
            LockState::Locking(confirmation) => {
                if result.is_unpresented()
                    && (deferred || *result == RenderResult::GpuDeferred)
                    && self.monitors_active
                {
                    // A paced host/GPU wait has not displayed the locked target
                    // yet. Keep the real locker; neither acknowledge nor fail it.
                    self.lock_state = LockState::Locking(confirmation);
                } else if state.lock_render_state == LockRenderState::Unlocked {
                    // We needed to render a locked frame on this output but failed.
                    self.unlock();
                } else {
                    // Check if all outputs are now locked.
                    let all_locked = self
                        .output_state
                        .values()
                        .all(|state| state.lock_render_state == LockRenderState::Locked);

                    if all_locked {
                        // All outputs are locked, report success.
                        let lock = confirmation.ext_session_lock().clone();
                        confirmation.lock();
                        self.lock_state = LockState::Locked(lock);
                    } else {
                        // Still waiting for other outputs.
                        self.lock_state = LockState::Locking(confirmation);
                    }
                }
            }
            lock_state => self.lock_state = lock_state,
        }
    }

    pub fn refresh_on_demand_vrr(&mut self, backend: &mut Backend, output: &Output) {
        let _span = clarion_tracy_span!("Clarion::refresh_on_demand_vrr");

        let name = output.user_data().get::<OutputName>().unwrap();
        let on_demand = self
            .config
            .borrow()
            .outputs
            .find(name)
            .is_some_and(|output| output.is_vrr_on_demand());
        if !on_demand {
            return;
        }

        let current = self.layout.windows_for_output(output).any(|mapped| {
            mapped.rules().variable_refresh_rate == Some(true) && {
                let mut visible = false;
                mapped.window.with_surfaces(|surface, states| {
                    if !visible
                        && surface_primary_scanout_output(surface, states).as_ref() == Some(output)
                    {
                        visible = true;
                    }
                });
                visible
            }
        });

        backend.set_output_on_demand_vrr(
            &mut crate::platform::device_bridge::DeviceCoordinator::new(self),
            output,
            current,
        );
    }

    pub fn update_primary_scanout_output(
        &self,
        output: &Output,
        render_element_states: &RenderElementStates,
    ) {
        // FIXME: potentially tweak the compare function. The default one currently always prefers a
        // higher refresh-rate output, which is not always desirable (i.e. with a very small
        // overlap).
        //
        // While we only have cursors and DnD icons crossing output boundaries though, it doesn't
        // matter all that much.
        if let CursorImageStatus::Surface(surface) = &self.cursor_manager.cursor_image() {
            with_surface_tree_downward(
                surface,
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |surface, states, _| {
                    update_surface_primary_scanout_output(
                        surface,
                        output,
                        states,
                        None,
                        render_element_states,
                        default_primary_scanout_output_compare,
                    );
                },
                |_, _, _| true,
            );
        }

        if let Some(surface) = self.dnd_icon.as_ref().map(|icon| &icon.surface) {
            with_surface_tree_downward(
                surface,
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |surface, states, _| {
                    update_surface_primary_scanout_output(
                        surface,
                        output,
                        states,
                        None,
                        render_element_states,
                        default_primary_scanout_output_compare,
                    );
                },
                |_, _, _| true,
            );
        }

        // MRU previews can show windows from other outputs. Track every window so
        // those surfaces also lose this output when the preview is hidden again.
        // The fade is a sibling of a window's own animation, not another nested
        // offscreen layer: only use its source states if its final texture was shown.
        let mru_data = self.window_mru_ui.offscreen_data(output);
        let mru_data = mru_data
            .as_deref()
            .filter(|data| render_element_states.element_was_presented(data.id.clone()));
        for (monitor, mapped) in self.layout.windows() {
            let owning_output = monitor.map(|monitor| monitor.output());
            let win = &mapped.window;
            let offscreen_data = mapped.offscreen_data();
            let offscreen_data = offscreen_data.as_ref();

            win.with_surfaces(|surface, states| {
                let primary_scanout_output = states
                    .data_map
                    .get_or_insert_threadsafe(Mutex::<PrimaryScanoutOutput>::default);
                let mut primary_scanout_output = primary_scanout_output.lock().unwrap();

                let mut id = Id::from_wayland_resource(surface);
                let mut visible_states = render_element_states;
                if !render_element_states.element_was_presented(id.clone()) {
                    if let Some(data) =
                        mru_data.filter(|data| data.states.element_was_presented(id.clone()))
                    {
                        // Keep the source's thumbnail area, rather than assigning
                        // the whole overlay's area to every surface inside it.
                        visible_states = &data.states;
                    } else if let Some(data) = offscreen_data {
                        if data.states.element_was_presented(id.clone()) {
                            id = data.id.clone();
                        }
                    }
                }

                primary_scanout_output.update_from_render_element_states(
                    id,
                    output,
                    None,
                    visible_states,
                    |current, _, next, _| {
                        // A preview should not steal pacing from a window still
                        // visible on its own output. Otherwise use the new output.
                        if owning_output == Some(current) {
                            current
                        } else {
                            next
                        }
                    },
                );
            });
        }

        let xray = &self.output_state[output].xray;
        let xray_bg = xray.background[RenderTarget::Output as usize].borrow();
        let xray_bd = xray.backdrop[RenderTarget::Output as usize].borrow();

        for layer in layer_map_for_output(output).layers() {
            let surface = layer.wl_surface();
            let is_background = layer.layer() == Layer::Background;

            with_surfaces_surface_tree(surface, |surface, states| {
                let primary_scanout_output = states
                    .data_map
                    .get_or_insert_threadsafe(Mutex::<PrimaryScanoutOutput>::default);
                let mut primary_scanout_output = primary_scanout_output.lock().unwrap();
                let mut id = Id::from_wayland_resource(surface);

                // Background layers may be invisible normally but visible through an xray
                // background effect. Try to find it and use the xray element's id in this case.
                //
                // FIXME: this won't work if there's another layer of offscreen (e.g. window with
                // an xray background during its opening animation). But hopefully with the
                // refactor to draw background effects outside offscreens it won't be a problem.
                if is_background && !render_element_states.element_was_presented(id.clone()) {
                    // A layer may be present either in background or backdrop, never in both.
                    if xray_bg
                        .render_element_states()
                        .is_some_and(|s| s.element_was_presented(id.clone()))
                    {
                        id = xray_bg.id().clone();
                    } else if xray_bd
                        .render_element_states()
                        .is_some_and(|s| s.element_was_presented(id.clone()))
                    {
                        id = xray_bd.id().clone();
                    }
                }

                primary_scanout_output.update_from_render_element_states(
                    id,
                    output,
                    None,
                    render_element_states,
                    // Layer surfaces are shown only on one output at a time.
                    |_, _, output, _| output,
                );
            });

            // Popups never go into xray buffers.
            for (popup, _) in PopupManager::popups_for_surface(surface) {
                let surface = popup.wl_surface();
                with_surfaces_surface_tree(surface, |surface, states| {
                    update_surface_primary_scanout_output(
                        surface,
                        output,
                        states,
                        None,
                        render_element_states,
                        // Layer surfaces are shown only on one output at a time.
                        |_, _, output, _| output,
                    );
                });
            }
        }

        if let Some(surface) = &self.output_state[output].lock_surface {
            with_surface_tree_downward(
                surface.wl_surface(),
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |surface, states, _| {
                    update_surface_primary_scanout_output(
                        surface,
                        output,
                        states,
                        None,
                        render_element_states,
                        default_primary_scanout_output_compare,
                    );
                },
                |_, _, _| true,
            );
        }
    }

    pub fn send_dmabuf_feedbacks(
        &self,
        output: &Output,
        feedback: &SurfaceDmabufFeedback,
        render_element_states: &RenderElementStates,
    ) {
        let _span = clarion_tracy_span!("Clarion::send_dmabuf_feedbacks");

        // We can unconditionally send the current output's feedback to regular and layer-shell
        // surfaces, as they can only be displayed on a single output at a time. Even if a surface
        // is currently invisible, this is the DMABUF feedback that it should know about.
        for mapped in self.layout.windows_for_output(output) {
            mapped.window.send_dmabuf_feedback(
                output,
                |_, _| Some(output.clone()),
                |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        render_element_states,
                        &feedback.render,
                        &feedback.scanout,
                    )
                },
            );
        }

        for surface in layer_map_for_output(output).layers() {
            surface.send_dmabuf_feedback(
                output,
                |_, _| Some(output.clone()),
                |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        render_element_states,
                        &feedback.render,
                        &feedback.scanout,
                    )
                },
            );
        }

        if let Some(surface) = &self.output_state[output].lock_surface {
            send_dmabuf_feedback_surface_tree(
                surface.wl_surface(),
                output,
                |_, _| Some(output.clone()),
                |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        render_element_states,
                        &feedback.render,
                        &feedback.scanout,
                    )
                },
            );
        }

        if let Some(surface) = self.dnd_icon.as_ref().map(|icon| &icon.surface) {
            send_dmabuf_feedback_surface_tree(
                surface,
                output,
                surface_primary_scanout_output,
                |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        render_element_states,
                        &feedback.render,
                        &feedback.scanout,
                    )
                },
            );
        }

        if let CursorImageStatus::Surface(surface) = &self.cursor_manager.cursor_image() {
            send_dmabuf_feedback_surface_tree(
                surface,
                output,
                surface_primary_scanout_output,
                |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        render_element_states,
                        &feedback.render,
                        &feedback.scanout,
                    )
                },
            );
        }
    }

    pub fn send_frame_callbacks(&mut self, output: &Output) {
        let _span = clarion_tracy_span!("Clarion::send_frame_callbacks");

        let state = self.output_state.get(output).unwrap();
        let sequence = state.schedule.callback_sequence();

        let should_send = |surface: &WlSurface, states: &SurfaceData| {
            // Do the standard primary scanout output check. For pointer surfaces it deduplicates
            // the frame callbacks across potentially multiple outputs, and for regular windows and
            // layer-shell surfaces it avoids sending frame callbacks to invisible surfaces.
            let current_primary_output = surface_primary_scanout_output(surface, states);
            if current_primary_output.as_ref() != Some(output) {
                return None;
            }

            // Next, check the throttling status.
            let frame_throttling_state = states
                .data_map
                .get_or_insert(SurfaceFrameThrottlingState::default);
            let mut last_sent_at = frame_throttling_state.last_sent_at.borrow_mut();

            let mut send = true;

            // If we already sent a frame callback to this surface this output refresh
            // cycle, don't send one again to prevent empty-damage commit busy loops.
            if let Some((last_output, last_sequence)) = &*last_sent_at {
                if last_output == output && *last_sequence == sequence {
                    send = false;
                }
            }

            if send {
                *last_sent_at = Some((output.clone(), sequence));
                Some(output.clone())
            } else {
                None
            }
        };

        let frame_callback_time = get_monotonic_time();

        self.layout.with_windows_mut(|mapped, owning_output| {
            if owning_output != Some(output) {
                let mut visible_here = false;
                mapped.window.with_surfaces(|surface, states| {
                    visible_here |=
                        surface_primary_scanout_output(surface, states).as_ref() == Some(output);
                });
                if !visible_here {
                    return;
                }
            }
            mapped.send_frame(
                output,
                frame_callback_time,
                FRAME_CALLBACK_THROTTLE,
                should_send,
            );
        });

        for surface in layer_map_for_output(output).layers() {
            surface.send_frame(
                output,
                frame_callback_time,
                FRAME_CALLBACK_THROTTLE,
                should_send,
            );
        }

        if let Some(surface) = &self.output_state[output].lock_surface {
            send_frames_surface_tree(
                surface.wl_surface(),
                output,
                frame_callback_time,
                FRAME_CALLBACK_THROTTLE,
                should_send,
            );
        }

        if let Some(surface) = self.dnd_icon.as_ref().map(|icon| &icon.surface) {
            send_frames_surface_tree(
                surface,
                output,
                frame_callback_time,
                FRAME_CALLBACK_THROTTLE,
                should_send,
            );
        }

        if let CursorImageStatus::Surface(surface) = self.cursor_manager.cursor_image() {
            send_frames_surface_tree(
                surface,
                output,
                frame_callback_time,
                FRAME_CALLBACK_THROTTLE,
                should_send,
            );
        }
    }

    pub(crate) fn finish_fifo_frame(
        &mut self,
        output: &Output,
        result: &RenderResult,
        nested: bool,
        render_started: Instant,
    ) {
        if result.is_unpresented() {
            return;
        }
        // Submitted frames are paced by KMS or the host compositor. Only empty
        // frames bypass that pacing and need an estimated refresh deadline.
        let defer = *result == RenderResult::NoDamage;
        let deadline = (nested && defer).then(|| {
            let interval = self.output_state[output]
                .schedule
                .clock()
                .refresh_interval()
                .unwrap_or(Duration::from_micros(16_667));
            // Host swap/presentation may already have waited a refresh. Count that time
            // instead of adding another full interval after each submitted frame.
            render_started
                .checked_add(interval)
                .unwrap_or(render_started)
        });
        if self
            .fifo
            .frame_rendered_at(output, defer, deadline, &self.blocker_cleared_tx)
        {
            self.stop_signal.wakeup();
        }
        // A submitted frame can supersede an earlier nested empty frame even
        // when this caller uses physical/headless pacing. Reconcile the timer
        // with the remaining FIFO work, independently of this frame's backend.
        self.schedule_nested_fifo_timer(output);
    }

    fn cancel_nested_fifo_timer(&mut self, output: &Output) {
        if let Some(token) = self
            .output_state
            .get_mut(output)
            .and_then(|state| state.schedule.take_fifo_timer())
        {
            self.event_loop.remove(token);
        }
    }

    fn cancel_unused_fifo_timers(&mut self) {
        // Expiry and surface destruction can remove work from several outputs.
        // Keep timers for every output with a remaining independent deadline.
        for (output, state) in &mut self.output_state {
            if self.fifo.next_estimated_deadline(output).is_none() {
                if let Some(token) = state.schedule.take_fifo_timer() {
                    self.event_loop.remove(token);
                }
            }
        }
    }

    fn schedule_nested_fifo_timer(&mut self, output: &Output) {
        let Some(deadline) = self.fifo.next_estimated_deadline(output) else {
            self.cancel_nested_fifo_timer(output);
            return;
        };
        let Some(output_state) = self.output_state.get_mut(output) else {
            return;
        };
        if output_state.schedule.fifo_timer().is_some() {
            return;
        }
        let output = output.clone();
        let timer = Timer::from_deadline(deadline);
        match self.event_loop.insert_source(timer, move |_, _, state| {
            if let Some(output_state) = state.clarion.output_state.get_mut(&output) {
                output_state.schedule.take_fifo_timer();
                if state.clarion.fifo.estimated_vblank_due(
                    &output,
                    Instant::now(),
                    &state.clarion.blocker_cleared_tx,
                ) {
                    state.clarion.stop_signal.wakeup();
                }
                state.clarion.schedule_nested_fifo_timer(&output);
            }
            TimeoutAction::Drop
        }) {
            Ok(token) => output_state.schedule.arm_fifo_timer(token),
            Err(err) => warn!("error scheduling nested FIFO refresh: {err}"),
        }
    }

    fn commit_timestamp_duration(timestamp: Timestamp) -> Option<Duration> {
        let time: Time<Monotonic> = timestamp.into();
        let timespec: smithay::reexports::rustix::time::Timespec = time.into();
        (timespec.tv_sec >= 0 && (0..1_000_000_000).contains(&timespec.tv_nsec))
            .then(|| Duration::new(timespec.tv_sec as u64, timespec.tv_nsec as u32))
    }

    fn commit_timestamp_schedule_duration(timestamp: Timestamp) -> Option<Duration> {
        let time: Time<Monotonic> = timestamp.into();
        let timespec: smithay::reexports::rustix::time::Timespec = time.into();
        (timespec.tv_sec >= 0 && timespec.tv_nsec >= 0)
            .then(|| {
                Duration::from_secs(timespec.tv_sec as u64)
                    .checked_add(Duration::from_nanos(timespec.tv_nsec as u64))
            })
            .flatten()
    }

    pub(crate) fn schedule_commit_timing(&mut self, surface: &WlSurface, timestamp: Timestamp) {
        // The protocol dispatcher validates wire timestamps. Keep this conversion
        // defensive in case another internal caller supplies a malformed value.
        let deadline = Self::commit_timestamp_duration(timestamp).unwrap_or_else(|| {
            warn!(surface = %surface.id(), "invalid commit-timing timestamp; normalizing safely");
            Self::commit_timestamp_schedule_duration(timestamp).unwrap_or_else(get_monotonic_time)
        });
        self.arm_commit_timing(surface.clone(), deadline, timestamp);
    }

    fn arm_commit_timing(
        &mut self,
        surface: WlSurface,
        deadline: Duration,
        release_until: Timestamp,
    ) {
        if self
            .commit_timing
            .scheduled
            .get(&surface)
            .is_some_and(|scheduled| scheduled.deadline <= deadline)
        {
            return;
        }
        if let Some(scheduled) = self.commit_timing.scheduled.remove(&surface) {
            if let Some(token) = scheduled.token {
                self.event_loop.remove(token);
            }
        }

        let now = get_monotonic_time();
        let Some(instant) = Instant::now().checked_add(deadline.saturating_sub(now)) else {
            // Keep the entry for destruction cleanup and the periodic fallback.
            self.commit_timing.scheduled.insert(
                surface,
                crate::commit_timing::ScheduledCommit {
                    deadline,
                    release_until,
                    token: None,
                },
            );
            return;
        };
        let timer_surface = surface.clone();
        let timer = Timer::from_deadline(instant);
        let token = match self.event_loop.insert_source(timer, move |_, _, state| {
            let is_current = state
                .clarion
                .commit_timing
                .scheduled
                .get(&timer_surface)
                .is_some_and(|scheduled| scheduled.deadline == deadline);
            if is_current {
                state.clarion.commit_timing.scheduled.remove(&timer_surface);
                state
                    .clarion
                    .release_commit_timing(&timer_surface, release_until);
            }
            TimeoutAction::Drop
        }) {
            Ok(token) => Some(token),
            Err(err) => {
                warn!(surface = %surface.id(), "error scheduling commit timing: {err}");
                None
            }
        };
        self.commit_timing.scheduled.insert(
            surface,
            crate::commit_timing::ScheduledCommit {
                deadline,
                release_until,
                token,
            },
        );
    }

    fn release_commit_timing(&mut self, surface: &WlSurface, release_until: Timestamp) {
        let (released, next) = with_states(surface, |states| {
            let Some(timing) = states.data_map.get::<CommitTimerBarrierStateUserData>() else {
                return (false, None);
            };
            let mut timing = timing.lock().unwrap();
            let released = timing.signal_until(release_until);
            let next = timing.next_deadline();
            (released, next)
        });
        if released {
            if let Some(client) = surface.client() {
                let _ = self.blocker_cleared_tx.send(client);
                self.stop_signal.wakeup();
            }
        }
        if let Some(next) = next {
            self.schedule_commit_timing(surface, next);
        }
    }

    fn release_due_commit_timing(&mut self, now: Duration) {
        let due = self
            .commit_timing
            .scheduled
            .iter()
            .filter(|(_, scheduled)| scheduled.deadline <= now)
            .map(|(surface, scheduled)| (surface.clone(), scheduled.release_until))
            .collect::<Vec<_>>();
        for (surface, release_until) in due {
            if let Some(scheduled) = self.commit_timing.scheduled.remove(&surface) {
                if let Some(token) = scheduled.token {
                    self.event_loop.remove(token);
                }
            }
            self.release_commit_timing(&surface, release_until);
        }

        // A timer insertion or an `Instant` conversion can fail. Retry those
        // entries periodically without applying a future constraint early.
        let retry = self
            .commit_timing
            .scheduled
            .iter()
            .filter(|(_, scheduled)| scheduled.token.is_none() && scheduled.deadline > now)
            .map(|(surface, scheduled)| {
                (surface.clone(), scheduled.deadline, scheduled.release_until)
            })
            .collect::<Vec<_>>();
        for (surface, deadline, release_until) in retry {
            self.commit_timing.scheduled.remove(&surface);
            self.arm_commit_timing(surface, deadline, release_until);
        }
    }

    pub(crate) fn destroy_commit_timing(&mut self, surface: &WlSurface) {
        if let Some(scheduled) = self.commit_timing.scheduled.remove(surface) {
            if let Some(token) = scheduled.token {
                self.event_loop.remove(token);
            }
        }
        let released = with_states(surface, |states| {
            let Some(timing) = states.data_map.get::<CommitTimerBarrierStateUserData>() else {
                return false;
            };
            let mut timing = timing.lock().unwrap();
            let mut released = false;
            while let Some(next) = timing.next_deadline() {
                released |= timing.signal_until(next);
            }
            released
        });
        if released {
            if let Some(client) = surface.client() {
                let _ = self.blocker_cleared_tx.send(client);
                self.stop_signal.wakeup();
            }
        }
    }

    pub(crate) fn release_fifo_after_estimated_vblank(&mut self, output: &Output) {
        if self.fifo.estimated_vblank(output, &self.blocker_cleared_tx) {
            self.stop_signal.wakeup();
        }
        self.schedule_nested_fifo_timer(output);
    }

    pub(crate) fn destroy_fifo_surface(&mut self, surface: &WlSurface) {
        self.fifo.destroyed(surface, &self.blocker_cleared_tx);
        self.cancel_unused_fifo_timers();
    }

    pub(crate) fn expire_fifo_barriers(&mut self, now: Duration) {
        self.fifo.expire(now, &self.blocker_cleared_tx);
        self.cancel_unused_fifo_timers();
    }

    pub fn send_frame_callbacks_on_fallback_timer(&mut self) {
        self.release_due_commit_timing(get_monotonic_time());
        self.expire_fifo_barriers(get_monotonic_time());
        let _span = clarion_tracy_span!("Clarion::send_frame_callbacks_on_fallback_timer");

        // Make up a bogus output; we don't care about it here anyway, just the throttling timer.
        let output = Output::new(
            String::new(),
            PhysicalProperties {
                size: Size::from((0, 0)),
                subpixel: Subpixel::Unknown,
                make: String::new(),
                model: String::new(),
                serial_number: String::new(),
            },
        );
        let output = &output;

        let frame_callback_time = get_monotonic_time();

        self.layout.with_windows_mut(|mapped, _| {
            mapped.send_frame(
                output,
                frame_callback_time,
                FRAME_CALLBACK_THROTTLE,
                |_, _| None,
            );
        });

        for (output, state) in self.output_state.iter() {
            for surface in layer_map_for_output(output).layers() {
                surface.send_frame(
                    output,
                    frame_callback_time,
                    FRAME_CALLBACK_THROTTLE,
                    |_, _| None,
                );
            }

            if let Some(surface) = &state.lock_surface {
                send_frames_surface_tree(
                    surface.wl_surface(),
                    output,
                    frame_callback_time,
                    FRAME_CALLBACK_THROTTLE,
                    |_, _| None,
                );
            }
        }

        if let Some(surface) = &self.dnd_icon.as_ref().map(|icon| &icon.surface) {
            send_frames_surface_tree(
                surface,
                output,
                frame_callback_time,
                FRAME_CALLBACK_THROTTLE,
                |_, _| None,
            );
        }

        if let CursorImageStatus::Surface(surface) = self.cursor_manager.cursor_image() {
            send_frames_surface_tree(
                surface,
                output,
                frame_callback_time,
                FRAME_CALLBACK_THROTTLE,
                |_, _| None,
            );
        }
    }

    pub fn take_presentation_feedbacks(
        &mut self,
        output: &Output,
        render_element_states: &RenderElementStates,
    ) -> OutputPresentationFeedback {
        let mut feedback = OutputPresentationFeedback::new(output);

        if let CursorImageStatus::Surface(surface) = &self.cursor_manager.cursor_image() {
            take_presentation_feedback_surface_tree(
                surface,
                &mut feedback,
                surface_primary_scanout_output,
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(
                        surface,
                        None,
                        render_element_states,
                    )
                },
            );
        }

        if let Some(surface) = self.dnd_icon.as_ref().map(|icon| &icon.surface) {
            take_presentation_feedback_surface_tree(
                surface,
                &mut feedback,
                surface_primary_scanout_output,
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(
                        surface,
                        None,
                        render_element_states,
                    )
                },
            );
        }

        for (_, mapped) in self.layout.windows() {
            mapped.window.take_presentation_feedback(
                &mut feedback,
                surface_primary_scanout_output,
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(
                        surface,
                        None,
                        render_element_states,
                    )
                },
            )
        }

        for surface in layer_map_for_output(output).layers() {
            surface.take_presentation_feedback(
                &mut feedback,
                surface_primary_scanout_output,
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(
                        surface,
                        None,
                        render_element_states,
                    )
                },
            );
        }

        if let Some(surface) = &self.output_state[output].lock_surface {
            take_presentation_feedback_surface_tree(
                surface.wl_surface(),
                &mut feedback,
                surface_primary_scanout_output,
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(
                        surface,
                        None,
                        render_element_states,
                    )
                },
            );
        }

        feedback
    }

    pub(crate) fn render_for_screencopy_with_damage<R>(&mut self, renderer: &mut R, output: &Output)
    where
        R: crate::platform::pixel_readback::DeferredCaptureRenderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>,
        OutputRenderElements<R>: RenderElement<R>,
        for<'a> R::Framebuffer<'a>: Texture,
    {
        let _span = clarion_tracy_span!("Clarion::render_for_screencopy_with_damage");

        let mut screencopy_state = mem::take(&mut self.screencopy_state);

        screencopy_state.with_queues_mut(|queue| {
            let (damage_tracker, screencopy) = queue.split();
            if let Some(screencopy) = screencopy {
                if screencopy.output() == output {
                    let ctx = RenderCtx {
                        renderer,
                        target: RenderTarget::ScreenCapture,
                        xray: None,
                    };
                    let offset = screencopy.region_loc().upscale(-1);
                    let mut elements = Vec::new();
                    self.render(ctx, output, screencopy.overlay_cursor(), &mut |elem| {
                        let elem =
                            RelocateRenderElement::from_element(elem, offset, Relocate::Relative);
                        elements.push(elem);
                    });

                    let (damages, states) = Self::damage_screencopy_internal(
                        output,
                        &elements,
                        damage_tracker,
                        screencopy,
                    );
                    if let Some(damages) = damages {
                        // Convert from Physical coordinates back to Buffer coordinates.
                        let transform = output.current_transform();
                        let physical_size = transform.transform_size(screencopy.buffer_size());
                        let damages = damages.iter().map(|dmg| {
                            dmg.to_logical(1).to_buffer(
                                1,
                                transform.invert(),
                                &physical_size.to_logical(1),
                            )
                        });

                        screencopy.damage(damages);

                        let render_result = Self::render_for_screencopy_internal(
                            renderer,
                            damage_tracker,
                            &elements,
                            states,
                            screencopy,
                            &self.screencopy_tasks,
                        );
                        match render_result {
                            Ok(sync) => {
                                if let Err(error) = self.submit_screencopy(queue.pop(), sync) {
                                    // Submitting can fail after rendering advanced damage
                                    // history. The next request must see those pixels again.
                                    let (damage_tracker, _) = queue.split();
                                    *damage_tracker =
                                        OutputDamageTracker::new((0, 0), 1.0, Transform::Normal);
                                    self.queue_redraw(output);
                                    warn!("error submitting screencopy: {error:#}");
                                }
                            }
                            Err(err) => {
                                // Recreate damage tracker to report full damage next check.
                                *damage_tracker =
                                    OutputDamageTracker::new((0, 0), 1.0, Transform::Normal);
                                queue.pop();
                                warn!("error rendering for screencopy: {err:?}");
                            }
                        }
                    } else {
                        trace!("no damage found, waiting till next redraw");
                    }
                };
            }
        });

        self.screencopy_state = screencopy_state;
    }

    pub(crate) fn render_for_screencopy_without_damage<R>(
        &mut self,
        renderer: &mut R,
        manager: &ZwlrScreencopyManagerV1,
        screencopy: Screencopy,
    ) -> anyhow::Result<()>
    where
        R: crate::platform::pixel_readback::DeferredCaptureRenderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>,
        OutputRenderElements<R>: RenderElement<R>,
        for<'a> R::Framebuffer<'a>: Texture,
    {
        let _span = clarion_tracy_span!("Clarion::render_for_screencopy");

        let output = screencopy.output();
        ensure!(
            self.output_state.contains_key(output),
            "screencopy output missing"
        );

        self.update_render_elements(Some(output));

        let ctx = RenderCtx {
            renderer,
            target: RenderTarget::ScreenCapture,
            xray: None,
        };
        let offset = screencopy.region_loc().upscale(-1);
        let mut elements = Vec::new();
        self.render(ctx, output, screencopy.overlay_cursor(), &mut |elem| {
            let elem = RelocateRenderElement::from_element(elem, offset, Relocate::Relative);
            elements.push(elem);
        });

        let Some(damage_tracker) = self.screencopy_state.damage_tracker(manager) else {
            error!("screencopy queue must not be deleted as long as frames exist");
            bail!("screencopy queue missing");
        };

        let (_damages, states) =
            Self::damage_screencopy_internal(output, &elements, damage_tracker, &screencopy);
        let res = Self::render_for_screencopy_internal(
            renderer,
            damage_tracker,
            &elements,
            states,
            &screencopy,
            &self.screencopy_tasks,
        );
        if res.is_err() {
            *damage_tracker = OutputDamageTracker::new((0, 0), 1.0, Transform::Normal);
        }
        let result = res.and_then(|submission| self.submit_screencopy(screencopy, submission));
        if result.is_err() {
            self.screencopy_state.invalidate_damage(manager);
        }
        result
    }

    fn damage_screencopy_internal<'a>(
        output: &Output,
        elements: &[impl Element],
        damage_tracker: &'a mut OutputDamageTracker,
        screencopy: &Screencopy,
    ) -> (
        Option<&'a Vec<Rectangle<i32, Physical>>>,
        RenderElementStates,
    ) {
        let OutputModeSource::Static {
            size: last_size,
            scale: last_scale,
            transform: last_transform,
        } = damage_tracker.mode().clone()
        else {
            unreachable!("damage tracker must have static mode");
        };

        let size = screencopy.buffer_size();
        let scale: Scale<f64> = output.current_scale().fractional_scale().into();
        let transform = output.current_transform();

        if size != last_size || scale != last_scale || transform != last_transform {
            *damage_tracker = OutputDamageTracker::new(size, scale, transform);
        }

        // Just checked damage tracker has static mode
        damage_tracker.damage_output(1, elements).unwrap()
    }

    #[allow(clippy::type_complexity)]
    fn render_for_screencopy_internal<R>(
        renderer: &mut R,
        damage_tracker: &mut OutputDamageTracker,
        elements: &[impl RenderElement<R>],
        states: RenderElementStates,
        screencopy: &Screencopy,
        slots: &crate::platform::capture::CaptureSlots,
    ) -> anyhow::Result<ScreencopySubmission>
    where
        R: crate::platform::pixel_readback::DeferredCaptureRenderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>,
        for<'a> R::Framebuffer<'a>: Texture,
    {
        let sync = match screencopy.buffer() {
            ScreencopyBuffer::Dmabuf(dmabuf) => {
                let sync =
                    render_to_dmabuf(renderer, damage_tracker, dmabuf.clone(), elements, states)
                        .context("error rendering to screencopy dmabuf")?;
                ScreencopySubmission::Dmabuf(sync)
            }
            ScreencopyBuffer::Shm(wl_buffer) => {
                let permit = slots.acquire()?;
                let texture = crate::render_helpers::prepare_shm_texture(
                    renderer,
                    damage_tracker,
                    wl_buffer,
                    elements,
                    states,
                )
                .context("error rendering to screencopy SHM texture")?;
                let job = renderer.enqueue_pixels(
                    &texture,
                    Rectangle::from_size(texture.size()),
                    Fourcc::Xrgb8888,
                )?;
                ScreencopySubmission::Shm(job, permit)
            }
        };

        Ok(sync)
    }

    fn submit_screencopy(
        &self,
        screencopy: Screencopy,
        submission: ScreencopySubmission,
    ) -> anyhow::Result<()> {
        match submission {
            ScreencopySubmission::Dmabuf(sync) => {
                screencopy.submit_after_sync(false, Some(sync), &self.event_loop)
            }
            ScreencopySubmission::Shm(job, permit) => {
                let output = screencopy.output().clone();
                let size = screencopy.buffer_size();
                let ScreencopyBuffer::Shm(buffer) = screencopy.buffer() else {
                    unreachable!();
                };
                let buffer = buffer.clone();
                let manager = screencopy.manager().clone();
                self.enqueue_pixel_readback(Some(output.clone()), job, move |state, result| {
                    let _permit = permit;
                    let result = result.and_then(|bytes| {
                        if !screencopy.is_alive() {
                            return Err("screencopy request or buffer was destroyed".into());
                        }
                        crate::render_helpers::write_shm_pixels(&buffer, size, &bytes)
                            .map_err(|error| error.to_string())
                    });
                    match result {
                        Ok(()) => {
                            screencopy.submit_after_sync(false, None, &state.clarion.event_loop)
                        }
                        Err(error) => {
                            state.clarion.screencopy_state.invalidate_damage(&manager);
                            state.clarion.queue_redraw(&output);
                            warn!("error completing SHM screencopy: {error}");
                        }
                    }
                })?;
            }
        }
        Ok(())
    }

    #[cfg(not(feature = "xdp-gnome-screencast"))]
    pub fn stop_casts_for_target(&mut self, _target: CastTarget) {}

    #[cfg(not(feature = "xdp-gnome-screencast"))]
    pub fn stop_cast(&mut self, _session_id: crate::utils::CastSessionId) {}

    pub fn debug_toggle_damage(&mut self) {
        self.debug_draw_damage = !self.debug_draw_damage;

        if self.debug_draw_damage {
            for (output, state) in &mut self.output_state {
                state.debug_damage_tracker = OutputDamageTracker::from_output(output);
            }
        }

        self.queue_redraw_all();
    }

    pub fn capture_screenshots<'a>(
        &'a self,
        renderer: &'a mut GlesRenderer,
    ) -> impl Iterator<Item = (Output, [OutputScreenshot; 3])> + 'a {
        self.global_space
            .outputs()
            .cloned()
            .filter_map(move |output| {
                self.capture_screenshot_for_output(renderer, &output)
                    .map(|screenshot| (output, screenshot))
            })
    }

    /// Capture all screenshot-ui layers for one output with the supplied GLES renderer.
    ///
    /// Keeping this operation output-local is important on TTY sessions: a KMS output can
    /// migrate to a GLES swapchain while another output continues to use native Vulkan.
    pub fn capture_screenshot_for_output(
        &self,
        renderer: &mut GlesRenderer,
        output: &Output,
    ) -> Option<[OutputScreenshot; 3]> {
        let size = output.current_mode()?.size;
        let transform = output.current_transform();
        let size = transform.transform_size(size);

        let scale = Scale::from(output.current_scale().fractional_scale());
        let targets = [
            RenderTarget::Output,
            RenderTarget::Screencast,
            RenderTarget::ScreenCapture,
        ];
        let screenshot = targets.map(|target| {
            let ctx = RenderCtx {
                renderer,
                target,
                xray: None,
            };
            let elements = self.render_to_vec(ctx, output, false);
            let elements = elements.iter().rev();

            let res = render_to_texture(
                renderer,
                size,
                scale,
                Transform::Normal,
                Fourcc::Abgr8888,
                elements,
            );
            if let Err(err) = &res {
                warn!("error rendering output {}: {err:?}", output.name());
            }
            let res_output = res.ok();

            let mut pointer = Vec::new();

            // We check the pointer visibility for Disabled (and not .is_visible()) in order to
            // show the pointer even when it's hidden through cursor {} options. The user can
            // then toggle it in the screenshot UI as needed.
            if self.pointer_visibility != PointerVisibility::Disabled {
                self.render_pointer(renderer, output, &mut |elem| pointer.push(elem));
            }

            let res_pointer = if pointer.is_empty() {
                None
            } else {
                let res = render_to_encompassing_texture(
                    renderer,
                    scale,
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    &pointer,
                );
                if let Err(err) = &res {
                    warn!("error rendering pointer for {}: {err:?}", output.name());
                }
                res.ok()
            };

            res_output.map(|(texture, _)| {
                OutputScreenshot::from_textures(
                    renderer,
                    scale,
                    texture,
                    res_pointer.map(|(texture, _, geo)| (texture, geo)),
                )
            })
        });

        let [output_screenshot, screencast_screenshot, capture_screenshot] = screenshot;
        Some([
            output_screenshot?,
            screencast_screenshot?,
            capture_screenshot?,
        ])
    }

    pub fn capture_screenshots_native<'a>(
        &'a self,
        renderer: &'a mut crate::render_helpers::vulkan::native::NativeRenderer,
    ) -> impl Iterator<Item = (Output, [OutputScreenshot; 3])> + 'a {
        self.global_space
            .outputs()
            .cloned()
            .filter_map(move |output| {
                self.capture_screenshot_for_output_native(renderer, &output)
                    .map(|screenshot| (output, screenshot))
            })
    }

    /// Capture all screenshot-ui layers for one output with the native Vulkan renderer.
    pub fn capture_screenshot_for_output_native(
        &self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        output: &Output,
    ) -> Option<[OutputScreenshot; 3]> {
        let size = output.current_mode()?.size;
        let transform = output.current_transform();
        let size = transform.transform_size(size);

        let scale = Scale::from(output.current_scale().fractional_scale());
        let targets = [
            RenderTarget::Output,
            RenderTarget::Screencast,
            RenderTarget::ScreenCapture,
        ];
        let screenshot = targets.map(|target| {
            let ctx = RenderCtx {
                renderer: &mut *renderer,
                target,
                xray: None,
            };
            let elements = self.render_to_vec(ctx, output, false);
            let elements = elements.iter().rev();

            let res = render_to_texture(
                renderer,
                size,
                scale,
                Transform::Normal,
                Fourcc::Abgr8888,
                elements,
            );
            if let Err(err) = &res {
                warn!("error rendering output {}: {err:?}", output.name());
            }
            let res_output = res.ok();

            let mut pointer = Vec::new();

            if self.pointer_visibility != PointerVisibility::Disabled {
                self.render_pointer(renderer, output, &mut |elem| pointer.push(elem));
            }

            let res_pointer = if pointer.is_empty() {
                None
            } else {
                let res = render_to_encompassing_texture(
                    renderer,
                    scale,
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    &pointer,
                );
                if let Err(err) = &res {
                    warn!("error rendering pointer for {}: {err:?}", output.name());
                }
                res.ok()
            };

            res_output.map(|(texture, _)| {
                OutputScreenshot::from_native_textures(
                    renderer,
                    scale,
                    texture,
                    res_pointer.map(|(texture, _, geo)| (texture, geo)),
                )
            })
        });

        let [output_screenshot, screencast_screenshot, capture_screenshot] = screenshot;
        Some([
            output_screenshot?,
            screencast_screenshot?,
            capture_screenshot?,
        ])
    }

    fn prepare_screenshot<R>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        include_pointer: bool,
    ) -> anyhow::Result<(Size<i32, Physical>, <R as RendererSuper>::TextureId)>
    where
        R: ClarionRenderer
            + ExportMem
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>,
        R::Error: Send + Sync + 'static,
        OutputRenderElements<R>: RenderElement<R>,
        for<'a> R::Framebuffer<'a>: Texture,
    {
        let _span = clarion_tracy_span!("Clarion::screenshot");

        self.update_render_elements(Some(output));

        let size = output.current_mode().unwrap().size;
        let transform = output.current_transform();
        let size = transform.transform_size(size);

        let scale = Scale::from(output.current_scale().fractional_scale());
        let ctx = RenderCtx {
            renderer,
            target: RenderTarget::ScreenCapture,
            xray: None,
        };
        let elements = self.render_to_vec(ctx, output, include_pointer);
        let elements = elements.iter().rev();
        let (texture, _sync) = render_to_texture(
            renderer,
            size,
            scale,
            Transform::Normal,
            Fourcc::Abgr8888,
            elements,
        )?;

        Ok((size, texture))
    }

    pub(crate) fn screenshot<R>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        write_to_disk: bool,
        include_pointer: bool,
        path: Option<String>,
    ) -> anyhow::Result<()>
    where
        R: crate::platform::pixel_readback::DeferredCaptureRenderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>,
        OutputRenderElements<R>: RenderElement<R>,
        for<'a> R::Framebuffer<'a>: Texture,
    {
        let permit = self.screenshot_tasks.acquire()?;
        let options = self.screenshot_save_options(write_to_disk, path);
        let (size, texture) = self.prepare_screenshot(renderer, output, include_pointer)?;
        let job = renderer.enqueue_pixels(
            &texture,
            Rectangle::from_size(texture.size()),
            Fourcc::Abgr8888,
        )?;
        self.enqueue_screenshot(output.clone(), job, size, options, permit)
    }

    fn prepare_window_screenshot<R>(
        &self,
        renderer: &mut R,
        output: &Output,
        mapped: &Mapped,
        show_pointer: bool,
    ) -> anyhow::Result<(Size<i32, Physical>, <R as RendererSuper>::TextureId)>
    where
        R: ClarionRenderer
            + ExportMem
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>,
        R::Error: Send + Sync + 'static,
        WindowScreenshotRenderElement<R>: RenderElement<R>,
        for<'a> R::Framebuffer<'a>: Texture,
    {
        if let Some(native) = renderer.as_native_renderer() {
            native.set_completion_export_required(false);
        }
        let _span = clarion_tracy_span!("Clarion::screenshot_window");

        let scale = Scale::from(output.current_scale().fractional_scale());
        let alpha =
            if mapped.sizing_mode().is_fullscreen() || mapped.is_ignoring_opacity_window_rule() {
                1.
            } else {
                mapped.rules().opacity.unwrap_or(1.).clamp(0., 1.)
            };

        let mut elements: Vec<WindowScreenshotRenderElement<R>> = Vec::new();

        // Add pointer if requested and it's over this window.
        if show_pointer {
            if let Some((_, win_pos)) = self.pointer_pos_for_window_cast(mapped) {
                // Pointer elements are at output-local physical coords.
                // Relocate by -win_pos to make them window-relative.
                let pos = win_pos.to_physical_precise_round(scale).upscale(-1);
                self.render_pointer(renderer, output, &mut |elem| {
                    let elem = RelocateRenderElement::from_element(elem, pos, Relocate::Relative);
                    elements.push(elem.into());
                });
            }
        }
        let pointer_count = elements.len();

        let ctx = RenderCtx {
            renderer,
            target: RenderTarget::ScreenCapture,
            xray: None,
        };
        mapped.render(
            ctx,
            mapped.window.geometry().loc.to_f64(),
            scale,
            alpha,
            XrayPos::default(),
            &mut |elem| elements.push(elem.into()),
        );

        // The pointer is not included in encompassing_geo because we don't want it to expand the
        // screenshot size.
        let geo = encompassing_geo(scale, elements.iter().skip(pointer_count));
        let elements = elements.iter().rev().map(|elem| {
            RelocateRenderElement::from_element(elem, geo.loc.upscale(-1), Relocate::Relative)
        });
        let (texture, _sync) = render_to_texture(
            renderer,
            geo.size,
            scale,
            Transform::Normal,
            Fourcc::Abgr8888,
            elements,
        )?;

        Ok((geo.size, texture))
    }

    pub(crate) fn screenshot_window<R>(
        &self,
        renderer: &mut R,
        output: &Output,
        mapped: &Mapped,
        write_to_disk: bool,
        show_pointer: bool,
        path: Option<String>,
    ) -> anyhow::Result<()>
    where
        R: crate::platform::pixel_readback::DeferredCaptureRenderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>,
        WindowScreenshotRenderElement<R>: RenderElement<R>,
        for<'a> R::Framebuffer<'a>: Texture,
    {
        let permit = self.screenshot_tasks.acquire()?;
        let options = self.screenshot_save_options(write_to_disk, path);
        let (size, texture) =
            self.prepare_window_screenshot(renderer, output, mapped, show_pointer)?;
        let job = renderer.enqueue_pixels(
            &texture,
            Rectangle::from_size(texture.size()),
            Fourcc::Abgr8888,
        )?;
        self.enqueue_screenshot(output.clone(), job, size, options, permit)
    }

    fn enqueue_screenshot(
        &self,
        output: Output,
        job: crate::platform::pixel_readback::PendingPixels,
        size: Size<i32, Physical>,
        options: crate::platform::capture::ScreenshotSaveOptions,
        permit: crate::platform::capture::CapturePermit,
    ) -> anyhow::Result<()> {
        self.enqueue_pixel_readback(Some(output), job, move |state, result| {
            if state.clarion.is_locked() {
                return;
            }
            match result {
                Ok(pixels) => {
                    if let Err(error) = state
                        .clarion
                        .save_screenshot_prepared(size, pixels, options, permit)
                    {
                        warn!("error saving screenshot: {error:#}");
                    }
                }
                Err(error) => warn!("error reading screenshot: {error}"),
            }
        })
    }

    pub(crate) fn capture_permit(&self) -> anyhow::Result<crate::platform::capture::CapturePermit> {
        self.screenshot_tasks.acquire()
    }

    pub(crate) fn enqueue_pixel_readback(
        &self,
        output: Option<Output>,
        job: crate::platform::pixel_readback::PendingPixels,
        on_done: impl FnOnce(&mut State, Result<Vec<u8>, String>) + 'static,
    ) -> anyhow::Result<()> {
        crate::platform::pixel_readback::enqueue(
            &self.event_loop,
            self.capture_scene_epoch.stamp(),
            output,
            job,
            on_done,
        )
    }

    fn screenshot_save_options(
        &self,
        write_to_disk: bool,
        path_arg: Option<String>,
    ) -> crate::platform::capture::ScreenshotSaveOptions {
        let path = write_to_disk
            .then(|| {
                // When given an explicit path, don't try to strftime it or create parents.
                path_arg.map(|p| (PathBuf::from(p), false)).or_else(|| {
                    match make_screenshot_path(&self.config.borrow()) {
                        Ok(path) => path.map(|p| (p, true)),
                        Err(err) => {
                            warn!("error making screenshot path: {err:?}");
                            None
                        }
                    }
                })
            })
            .flatten();

        crate::platform::capture::ScreenshotSaveOptions {
            destination: path,
            publish_clipboard: true,
            completion: Default::default(),
        }
    }

    fn save_screenshot_prepared(
        &self,
        size: Size<i32, Physical>,
        pixels: Vec<u8>,
        options: crate::platform::capture::ScreenshotSaveOptions,
        permit: crate::platform::capture::CapturePermit,
    ) -> anyhow::Result<()> {
        let scene_epoch = self.capture_scene_epoch.stamp();
        let path = options.destination;
        let reply = options.completion;
        let publish_clipboard = options.publish_clipboard;
        let tx = if publish_clipboard {
            let scene_epoch = scene_epoch.clone();
            let (tx, rx) = calloop::channel::sync_channel::<Arc<[u8]>>(1);
            self.event_loop
                .insert_source(rx, move |event, _, state| {
                    if let calloop::channel::Event::Msg(buf) = event {
                        if scene_epoch.is_current() && !state.clarion.is_locked() {
                            set_data_device_selection(
                                &state.clarion.display_handle,
                                &state.clarion.seat,
                                vec![String::from("image/png")],
                                buf,
                            );
                        }
                    }
                })
                .map_err(|error| {
                    anyhow::anyhow!("error registering screenshot clipboard: {}", error.error)
                })?;
            Some(tx)
        } else {
            None
        };
        let event_tx = if publish_clipboard {
            let scene_epoch = scene_epoch.clone();
            let (tx, rx) = calloop::channel::sync_channel::<Option<String>>(1);
            self.event_loop
                .insert_source(rx, move |event, _, state| {
                    if let calloop::channel::Event::Msg(path) = event {
                        if scene_epoch.is_current() && !state.clarion.is_locked() {
                            state.ipc_screenshot_taken(path);
                        }
                    }
                })
                .map_err(|error| {
                    anyhow::anyhow!("error registering screenshot notification: {}", error.error)
                })?;
            Some(tx)
        } else {
            None
        };
        // Both transfer and encoding hold the same capacity permit. No file or
        // PNG work accesses desktop, renderer or protocol state from this task.
        thread::Builder::new()
            .name("screenshot-encode".into())
            .spawn(move || {
                let _permit = permit;
                if !scene_epoch.is_current() {
                    return;
                }
                let mut buf = vec![];
                let w = std::io::Cursor::new(&mut buf);
                if let Err(error) = write_png_rgba8(w, size.w as u32, size.h as u32, &pixels) {
                    warn!("error encoding screenshot image: {error:#}");
                    return;
                }
                if !scene_epoch.is_current() {
                    return;
                }
                let buf: Arc<[u8]> = Arc::from(buf.into_boxed_slice());
                if let Some(tx) = tx {
                    let _ = tx.send(buf.clone());
                }
                let mut image_path = None;
                if let Some((path, create_parent)) = path {
                    if create_parent {
                        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                            if let Err(error) = std::fs::create_dir_all(parent) {
                                warn!("error creating screenshot directory: {error:#}");
                            }
                        }
                    }
                    match std::fs::write(&path, buf) {
                        Ok(()) => image_path = Some(path),
                        Err(error) => warn!("error saving screenshot image: {error:#}"),
                    }
                }
                drop(_permit);
                #[cfg(all(feature = "dbus", not(test)))]
                if publish_clipboard && scene_epoch.is_current() {
                    if let Err(error) =
                        crate::utils::show_screenshot_notification(image_path.as_deref())
                    {
                        warn!("error showing screenshot notification: {error:#}");
                    }
                }
                if let Some(tx) = event_tx {
                    let path_string = image_path
                        .as_ref()
                        .and_then(|p| p.to_str())
                        .map(str::to_owned);
                    let _ = tx.send(path_string);
                }
                reply.complete(image_path);
            })
            .context("error starting screenshot encoder")?;
        Ok(())
    }

    pub fn is_locked(&self) -> bool {
        match self.lock_state {
            LockState::Unlocked | LockState::WaitingForSurfaces { .. } => false,
            LockState::Locking(_) | LockState::Locked(_) => true,
        }
    }

    pub fn lock(&mut self, confirmation: SessionLocker) {
        // Check if another client is in the process of locking.
        if matches!(
            self.lock_state,
            LockState::WaitingForSurfaces { .. } | LockState::Locking(_)
        ) {
            info!("refusing lock as another client is currently locking");
            return;
        }

        self.layout.suspend_minimize_animations(true);
        // Never retain an unlocked screen image above a session-lock surface.
        self.reset_hot_corner();
        for state in self.output_state.values_mut() {
            state.screen_transition = None;
        }

        // Check if we're already locked with an active client.
        if let LockState::Locked(lock) = &self.lock_state {
            if lock.is_alive() {
                info!("refusing lock as already locked with an active client");
                return;
            }

            // If the client had died, continue with the new lock.
            info!("locking session (replacing existing dead lock)");

            // Since the session was already locked, we know that the outputs are blanked, and
            // can lock right away.
            let lock = confirmation.ext_session_lock().clone();
            confirmation.lock();
            self.lock_state = LockState::Locked(lock);

            return;
        }

        self.capture_scene_epoch.invalidate();
        #[cfg(feature = "xdp-gnome-screencast")]
        for cast in &mut self.casting.casts {
            // Completed metadata pixels need the same lock cancellation as
            // in-flight captures; retain pending permits until GPU retirement.
            cast.invalidate_cursor_bitmap();
        }
        info!("locking session");

        if self.output_state.is_empty() {
            // There are no outputs, lock the session right away.
            self.screenshot_ui.close();
            self.cursor_manager
                .set_cursor_image(CursorImageStatus::default_named());

            let lock = confirmation.ext_session_lock().clone();
            confirmation.lock();
            self.lock_state = LockState::Locked(lock);
        } else {
            // There are outputs which we need to redraw before locking. But before we do that,
            // let's wait for the lock surfaces.
            //
            // Give them a second; swaylock can take its time to paint a big enough image.
            let timer = Timer::from_duration(Duration::from_millis(1000));
            let deadline_token = self
                .event_loop
                .insert_source(timer, |_, _, state| {
                    trace!("lock deadline expired, continuing");
                    state.clarion.continue_to_locking();
                    TimeoutAction::Drop
                })
                .unwrap();

            self.lock_state = LockState::WaitingForSurfaces {
                confirmation,
                deadline_token,
            };
        }
    }

    pub fn maybe_continue_to_locking(&mut self) {
        if !matches!(self.lock_state, LockState::WaitingForSurfaces { .. }) {
            // Not waiting.
            return;
        }

        // Check if there are any outputs whose lock surfaces had not had a commit yet.
        for state in self.output_state.values() {
            let Some(surface) = &state.lock_surface else {
                // Surface not created yet.
                return;
            };

            if !is_mapped(surface.wl_surface()) {
                return;
            }
        }

        // All good.
        trace!("lock surfaces are ready, continuing");
        self.continue_to_locking();
    }

    fn continue_to_locking(&mut self) {
        match mem::take(&mut self.lock_state) {
            LockState::WaitingForSurfaces {
                confirmation,
                deadline_token,
            } => {
                self.event_loop.remove(deadline_token);

                self.screenshot_ui.close();
                self.cursor_manager
                    .set_cursor_image(CursorImageStatus::default_named());
                self.cancel_mru();

                if self.output_state.is_empty() {
                    // There are no outputs, lock the session right away.
                    let lock = confirmation.ext_session_lock().clone();
                    confirmation.lock();
                    self.lock_state = LockState::Locked(lock);
                } else {
                    // There are outputs which we need to redraw before locking.
                    self.lock_state = LockState::Locking(confirmation);
                    self.queue_redraw_all();
                }
            }
            other => {
                error!("continue_to_locking() called with wrong lock state: {other:?}",);
                self.lock_state = other;
            }
        }
    }

    pub fn unlock(&mut self) {
        info!("unlocking session");
        self.layout.suspend_minimize_animations(false);

        let prev = mem::take(&mut self.lock_state);
        if let LockState::WaitingForSurfaces { deadline_token, .. } = prev {
            self.event_loop.remove(deadline_token);
        }

        for output_state in self.output_state.values_mut() {
            output_state.lock_surface = None;
        }
        self.queue_redraw_all();
    }

    #[cfg(feature = "dbus")]
    fn update_locked_hint(&mut self) {
        use std::sync::LazyLock;

        if !self.is_session_instance {
            return;
        }

        static XDG_SESSION_ID: LazyLock<Option<String>> = LazyLock::new(|| {
            let id = std::env::var("XDG_SESSION_ID").ok();
            if id.is_none() {
                warn!(
                    "env var 'XDG_SESSION_ID' is unset or invalid; logind LockedHint won't be set"
                );
            }
            id
        });

        let Some(session_id) = &*XDG_SESSION_ID else {
            return;
        };

        fn call(session_id: &str, locked: bool) -> anyhow::Result<()> {
            let conn = zbus::blocking::Connection::system()
                .context("error connecting to the system bus")?;

            let message = conn
                .call_method(
                    Some("org.freedesktop.login1"),
                    "/org/freedesktop/login1",
                    Some("org.freedesktop.login1.Manager"),
                    "GetSession",
                    &(session_id),
                )
                .context("failed to call GetSession")?;

            let message_body = message.body();
            let session_path: zbus::zvariant::ObjectPath = message_body
                .deserialize()
                .context("failed to deserialize GetSession reply")?;

            conn.call_method(
                Some("org.freedesktop.login1"),
                session_path,
                Some("org.freedesktop.login1.Session"),
                "SetLockedHint",
                &(locked),
            )
            .context("failed to call SetLockedHint")?;

            Ok(())
        }

        // Consider only the fully locked state here. When using the locked hint with sleep
        // inhibitor tools, we want to allow sleep only after the screens are fully cleared with
        // the lock screen, which corresponds to the Locked state.
        let locked = matches!(self.lock_state, LockState::Locked(_));

        if self.locked_hint.is_some_and(|h| h == locked) {
            return;
        }

        self.locked_hint = Some(locked);

        let res = thread::Builder::new()
            .name("Logind LockedHint Updater".to_owned())
            .spawn(move || {
                let _span = clarion_tracy_span!("LockedHint");

                if let Err(err) = call(session_id, locked) {
                    warn!("failed to set logind LockedHint: {err:?}");
                }
            });

        if let Err(err) = res {
            warn!("error spawning a thread to set logind LockedHint: {err:?}");
        }
    }

    pub fn new_lock_surface(&mut self, surface: LockSurface, output: &Output) {
        let lock = match &self.lock_state {
            LockState::Unlocked => {
                error!("tried to add a lock surface on an unlocked session");
                return;
            }
            LockState::WaitingForSurfaces { confirmation, .. } => confirmation.ext_session_lock(),
            LockState::Locking(confirmation) => confirmation.ext_session_lock(),
            LockState::Locked(lock) => lock,
        };

        if lock.client() != surface.wl_surface().client() {
            debug!("ignoring lock surface from an unrelated client");
            return;
        }

        if lock != surface.ext_session_lock() {
            debug!("ignoring lock surface from an unrelated lock instance");
            return;
        }

        let Some(output_state) = self.output_state.get_mut(output) else {
            error!("missing output state");
            return;
        };

        output_state.lock_surface = Some(surface);
    }

    /// Activates the pointer constraint if necessary according to the current pointer contents.
    ///
    /// Make sure the pointer location and contents are up to date before calling this.
    pub fn maybe_activate_pointer_constraint(&self) {
        let Some((surface, surface_loc)) = &self.pointer_contents.surface else {
            return;
        };

        let pointer = self.seat.get_pointer().unwrap();
        if Some(surface) != pointer.current_focus().as_ref() {
            return;
        }

        with_pointer_constraint(surface, &pointer, |constraint| {
            let Some(constraint) = constraint else { return };

            if constraint.is_active() {
                return;
            }

            // Constraint does not apply if not within region.
            if let Some(region) = constraint.region() {
                let pointer_pos = pointer.current_location();
                let pos_within_surface = pointer_pos - *surface_loc;
                if !region.contains(pos_within_surface.to_i32_round()) {
                    return;
                }
            }

            constraint.activate();
        });
    }

    pub fn focus_layer_surface_if_on_demand(&mut self, surface: Option<LayerSurface>) {
        if let Some(surface) = surface {
            if surface.cached_state().keyboard_interactivity
                == wlr_layer::KeyboardInteractivity::OnDemand
            {
                if self.layer_shell_on_demand_focus.as_ref() != Some(&surface) {
                    self.layer_shell_on_demand_focus = Some(surface);

                    // FIXME: granular.
                    self.queue_redraw_all();
                }

                return;
            }
            // A desktop context menu lives on a separate exclusive surface.
            // Keep only its own already-focused desktop as the return target;
            // exclusive focus still wins until the interaction surface closes.
            // Names alone cannot let another Wayland client retain that focus.
            if surface.namespace() == "clarion-desktop-interaction"
                && surface.cached_state().keyboard_interactivity
                    == wlr_layer::KeyboardInteractivity::Exclusive
                && self
                    .layer_shell_on_demand_focus
                    .as_ref()
                    .is_some_and(|base| {
                        base.alive()
                            && base.namespace() == "clarion-desktop"
                            && base.cached_state().keyboard_interactivity
                                == wlr_layer::KeyboardInteractivity::OnDemand
                            && base.wl_surface().client().is_some_and(|client| {
                                surface.wl_surface().client().as_ref() == Some(&client)
                            })
                    })
            {
                return;
            }
        }

        // Something else got clicked, clear on-demand layer-shell focus.
        if self.layer_shell_on_demand_focus.is_some() {
            self.layer_shell_on_demand_focus = None;

            // FIXME: granular.
            self.queue_redraw_all();
        }
    }

    /// Tries to find and return the root shell surface for a given surface.
    ///
    /// I.e. for popups, this function will try to find the parent toplevel or layer surface. For
    /// regular subsurfaces, it will find the root surface.
    pub fn find_root_shell_surface(&self, surface: &WlSurface) -> WlSurface {
        let Some(root) = self.root_surface.get(surface) else {
            return surface.clone();
        };

        if let Some(popup) = self.popups.find_popup(root) {
            return find_popup_root_surface(&popup).unwrap_or_else(|_| root.clone());
        }

        root.clone()
    }

    #[cfg(feature = "dbus")]
    pub fn on_ipc_outputs_changed(&self) {
        let _span = clarion_tracy_span!("Clarion::on_ipc_outputs_changed");

        let Some(dbus) = &self.dbus else { return };
        let Some(conn_display_config) = dbus.conn_display_config.clone() else {
            return;
        };

        let res = thread::Builder::new()
            .name("DisplayConfig MonitorsChanged Emitter".to_owned())
            .spawn(move || {
                use crate::dbus::mutter_display_config::DisplayConfig;
                let _span = clarion_tracy_span!("MonitorsChanged");
                let iface = match conn_display_config
                    .object_server()
                    .interface::<_, DisplayConfig>("/org/gnome/Mutter/DisplayConfig")
                {
                    Ok(iface) => iface,
                    Err(err) => {
                        warn!("error getting DisplayConfig interface: {err:?}");
                        return;
                    }
                };

                async_io::block_on(async move {
                    if let Err(err) = DisplayConfig::monitors_changed(iface.signal_emitter()).await
                    {
                        warn!("error emitting MonitorsChanged: {err:?}");
                    }
                });
            });

        if let Err(err) = res {
            warn!("error spawning a thread to send MonitorsChanged: {err:?}");
        }
    }

    pub fn handle_focus_follows_mouse(&mut self, new_focus: &PointContents) {
        // A pointer snapshot from the grid must not retarget its closing scene.
        if self.overview_layout_input_blocked() && new_focus.layer.is_none() {
            return;
        }
        let Some(ffm) = self.config.borrow().input.focus_follows_mouse else {
            return;
        };

        let pointer = &self.seat.get_pointer().unwrap();
        if pointer.is_grabbed() {
            return;
        }

        if self.window_mru_ui.is_open() {
            return;
        }

        // Recompute the current pointer focus because we don't update it during animations.
        let current_focus = self.contents_under(pointer.current_location());

        if let Some(output) = &new_focus.output {
            if current_focus.output.as_ref() != Some(output) {
                self.layout.focus_output(output);
            }
        }

        if let Some(window) = &new_focus.window {
            if !self.layout.is_overview_open() && current_focus.window.as_ref() != Some(window) {
                let (window, hit) = window;

                // Don't trigger focus-follows-mouse over the tab indicator.
                if matches!(
                    hit,
                    HitType::Activate {
                        is_tab_indicator: true
                    }
                ) {
                    return;
                }

                if !self.layout.should_trigger_focus_follows_mouse_on(window) {
                    return;
                }

                if let Some(threshold) = ffm.max_scroll_amount {
                    if self.layout.scroll_amount_to_activate(window) > threshold.0 {
                        return;
                    }
                }

                self.layout.activate_window_without_raising(window);
                self.layer_shell_on_demand_focus = None;
            }
        }

        if let Some(layer) = &new_focus.layer {
            if current_focus.layer.as_ref() != Some(layer) {
                self.layer_shell_on_demand_focus = Some(layer.clone());
            }
        }
    }

    pub fn do_screen_transition<R>(&mut self, renderer: &mut R, delay_ms: Option<u16>)
    where
        R: ClarionRenderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>,
        OutputRenderElements<R>: RenderElement<R>,
        screen_transition::TransitionTexture: From<TextureBuffer<R::TextureId>>,
    {
        let _span = clarion_tracy_span!("Clarion::do_screen_transition");

        let textures = self.capture_transition_textures(renderer, None);

        let delay = delay_ms.map_or(screen_transition::DELAY, |d| {
            Duration::from_millis(u64::from(d))
        });

        for (output, from_texture) in textures {
            let state = self.output_state.get_mut(&output).unwrap();
            state.screen_transition = Some(ScreenTransition::new(
                from_texture,
                delay,
                self.clock.clone(),
            ));
        }

        // We don't actually need to queue a redraw because the point is to freeze the screen for a
        // bit, and even if the delay was zero, we're drawing the same contents anyway.
    }

    pub(crate) fn clear_overview_mode_transitions(&mut self) {
        for state in self.output_state.values_mut() {
            if state
                .screen_transition
                .as_ref()
                .is_some_and(ScreenTransition::is_overview)
            {
                state.screen_transition = None;
            }
        }
    }

    pub(crate) fn clear_screen_transition_snapshots(&mut self) {
        for state in self.output_state.values_mut() {
            state.screen_transition = None;
        }
    }

    pub fn overview_mode_transition_active(&self) -> bool {
        self.output_state.values().any(|state| {
            state
                .screen_transition
                .as_ref()
                .is_some_and(|transition| transition.is_overview() && !transition.is_done())
        })
    }

    fn overview_layout_input_blocked(&self) -> bool {
        (self.layout.is_overview_visible() && !self.layout.is_overview_open())
            || self.overview_mode_transition_active()
    }

    pub(crate) fn do_overview_mode_transition<R>(&mut self, renderer: &mut R, output: &Output)
    where
        R: ClarionRenderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>,
        OutputRenderElements<R>: RenderElement<R>,
        screen_transition::TransitionTexture: From<TextureBuffer<R::TextureId>>,
    {
        let textures = self.capture_transition_textures(renderer, Some(output));
        // Failed captures must switch directly, rather than retaining a stale image.
        self.output_state.get_mut(output).unwrap().screen_transition = None;
        for (output, from_texture) in textures {
            self.output_state
                .get_mut(&output)
                .unwrap()
                .screen_transition = Some(ScreenTransition::new_overview(
                from_texture,
                self.clock.clone(),
                self.config.borrow().animations.overview_open_close.0,
            ));
        }
    }

    fn capture_transition_textures<R>(
        &mut self,
        renderer: &mut R,
        output_filter: Option<&Output>,
    ) -> Vec<(Output, [screen_transition::TransitionTexture; 3])>
    where
        R: ClarionRenderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>,
        OutputRenderElements<R>: RenderElement<R>,
        screen_transition::TransitionTexture: From<TextureBuffer<R::TextureId>>,
    {
        let _latency =
            crate::utils::latency::Span::new(crate::utils::latency::Operation::TransitionCapture);
        self.update_render_elements(output_filter);

        self.output_state
            .keys()
            .filter(|output| output_filter.is_none_or(|selected| selected == *output))
            .cloned()
            .filter_map(|output| {
                let size = output.current_mode()?.size;
                let transform = output.current_transform();

                let scale = Scale::from(output.current_scale().fractional_scale());
                let mut capture = crate::render_helpers::transition_capture::TransitionCapture::new(
                    size, scale, transform,
                );
                // ScreenCapture lies between Output and Screencast in the
                // privacy policy: adjacent equal scenes can share a capture.
                let targets = [
                    RenderTarget::Output,
                    RenderTarget::ScreenCapture,
                    RenderTarget::Screencast,
                ];
                let textures = targets.map(|target| {
                    let ctx = RenderCtx {
                        renderer,
                        target,
                        xray: None,
                    };
                    let elements = self.render_to_vec(ctx, &output, false);
                    let res = capture.capture(renderer, &elements);

                    if let Err(err) = &res {
                        warn!("error rendering output {}: {err:?}", output.name());
                    }

                    res
                });

                if textures.iter().any(|res| res.is_err()) {
                    return None;
                }

                let [output_texture, capture_texture, cast_texture] =
                    textures.map(|res| res.unwrap().into());
                let textures = [output_texture, cast_texture, capture_texture];

                Some((output, textures))
            })
            .collect()
    }

    pub fn recompute_window_rules(&mut self) {
        let _span = clarion_tracy_span!("Clarion::recompute_window_rules");

        let mut privacy_changed = false;
        let changed = {
            let window_rules = &self.config.borrow().window_rules;

            for unmapped in self.unmapped_windows.values_mut() {
                let new_rules = ResolvedWindowRules::compute(
                    window_rules,
                    WindowRef::Unmapped(unmapped),
                    self.is_at_startup,
                );
                if let InitialConfigureState::Configured { rules, .. } = &mut unmapped.state {
                    *rules = new_rules;
                }
            }

            let mut windows = vec![];
            self.layout.with_windows_mut(|mapped, _| {
                let previous_privacy = mapped.rules().block_out_from;
                if mapped.recompute_window_rules(window_rules, self.is_at_startup) {
                    privacy_changed |= previous_privacy != mapped.rules().block_out_from;
                    windows.push(mapped.window.clone());
                }
            });
            let changed = !windows.is_empty();
            for win in windows {
                self.layout.update_window(&win, None);
            }
            changed
        };

        if privacy_changed {
            self.clear_screen_transition_snapshots();
        }

        if changed {
            // FIXME: granular.
            self.queue_redraw_all();
        }
    }

    pub fn recompute_layer_rules(&mut self) {
        let _span = clarion_tracy_span!("Clarion::recompute_layer_rules");

        let mut changed = false;
        let mut privacy_changed = false;
        {
            let config = self.config.borrow();
            let rules = &config.layer_rules;

            for mapped in self.mapped_layer_surfaces.values_mut() {
                let previous_privacy = mapped.rules().block_out_from;
                if mapped.recompute_layer_rules(rules, self.is_at_startup) {
                    privacy_changed |= previous_privacy != mapped.rules().block_out_from;
                    changed = true;
                    mapped.update_config(&config);
                }
            }
        }

        if privacy_changed {
            self.clear_screen_transition_snapshots();
        }

        if changed {
            // FIXME: granular.
            self.queue_redraw_all();
        }
    }

    pub fn reset_pointer_inactivity_timer(&mut self) {
        if self.pointer_inactivity_timer_got_reset {
            return;
        }

        let _span = clarion_tracy_span!("Clarion::reset_pointer_inactivity_timer");

        if let Some(token) = self.pointer_inactivity_timer.take() {
            self.event_loop.remove(token);
        }

        let Some(timeout_ms) = self.config.borrow().cursor.hide_after_inactive_ms else {
            return;
        };

        let duration = Duration::from_millis(timeout_ms as u64);
        let timer = Timer::from_duration(duration);
        let token = self
            .event_loop
            .insert_source(timer, move |_, _, state| {
                state.clarion.pointer_inactivity_timer = None;

                // If the pointer is already invisible, don't reset it back to Hidden causing one
                // frame of hover.
                if state.clarion.pointer_visibility.is_visible() {
                    state.clarion.pointer_visibility = PointerVisibility::Hidden;
                    state.clarion.queue_redraw_all();
                }

                TimeoutAction::Drop
            })
            .unwrap();
        self.pointer_inactivity_timer = Some(token);

        self.pointer_inactivity_timer_got_reset = true;
    }

    pub fn notify_activity(&mut self) {
        if self.notified_activity_this_iteration {
            return;
        }

        let _span = clarion_tracy_span!("Clarion::notify_activity");

        self.last_user_activity = Instant::now();
        if self.idle_policy.activity(self.last_user_activity) {
            // The panels were off: switch them back on and repaint what the
            // blanked frames never presented.
            self.pending_dpms_blank = Some(false);
            for output in self.global_space.outputs().cloned().collect::<Vec<_>>() {
                self.queue_redraw(&output);
            }
        }
        self.idle_notifier_state.notify_activity(&self.seat);

        self.notified_activity_this_iteration = true;
    }

    /// Runs the idle policy. Called from the compositor's one-second tick, so a
    /// timeout is honored within a second of expiring.
    pub fn poll_idle(&mut self) {
        let inhibited = !self.idle_inhibiting_surfaces.is_empty()
            || self
                .is_fdo_idle_inhibited
                .load(std::sync::atomic::Ordering::Relaxed);
        let locked = !matches!(self.lock_state, LockState::Unlocked);
        let Some(action) = self.idle_policy.poll(Instant::now(), inhibited, locked) else {
            return;
        };
        match action {
            crate::idle::IdleAction::Lock => self.request_lock(),
            crate::idle::IdleAction::Suspend => self.request_suspend(),
            crate::idle::IdleAction::Blank => self.pending_dpms_blank = Some(true),
        }
    }

    /// Starts the configured locker. The session lock surface belongs to a
    /// client, so the policy can only ask one to run.
    fn request_lock(&mut self) {
        let Some(command) = self.idle_policy.lock_command() else {
            warn!("idle: lock requested, but idle {{ lock-command }} is not configured");
            return;
        };
        debug!("idle: starting the lock command {command:?}");
        crate::utils::spawning::spawn(command, None);
    }

    fn request_suspend(&mut self) {
        if self.idle_policy.lock_before_suspend() {
            self.request_lock();
        }
        #[cfg(feature = "dbus")]
        if let Some(dbus) = &self.dbus {
            if let Err(err) = dbus.suspend() {
                warn!("idle: suspend request failed: {err:?}");
            }
        }
        #[cfg(not(feature = "dbus"))]
        warn!("idle: suspend requested, but this build has no D-Bus support");
    }

    pub fn close_mru(&mut self, close_request: MruCloseRequest) -> Option<Window> {
        if !self.window_mru_ui.is_open() {
            return None;
        }
        self.queue_redraw_all();

        let id = self.window_mru_ui.close(close_request)?;
        self.find_window_by_id(id)
    }

    pub fn cancel_mru(&mut self) {
        self.close_mru(MruCloseRequest::Cancel);
    }

    /// Apply a pending MRU commit immediately.
    ///
    /// Called for example on keyboard events that reach the active window, which immediately adds
    /// it to the MRU.
    pub fn mru_apply_keyboard_commit(&mut self) {
        let Some(pending) = self.pending_mru_commit.take() else {
            return;
        };
        self.event_loop.remove(pending.token);

        if let Some(window) = self
            .layout
            .workspaces_mut()
            .flat_map(|ws| ws.windows_mut())
            .find(|w| w.id() == pending.id)
        {
            window.set_focus_timestamp(pending.stamp);
        }
    }

    pub fn queue_redraw_mru_output(&mut self) {
        if let Some(output) = self.window_mru_ui.output().cloned() {
            self.queue_redraw(&output);
        }
    }
}

pub struct NewClient {
    pub client: UnixStream,
    pub restricted: bool,
    pub credentials_unknown: bool,
}

pub struct ClientState {
    registration: crate::platform::wayland_shutdown::ClientRegistration,
    pub compositor_state: CompositorClientState,
    pub can_view_decoration_globals: bool,
    pub primary_selection_disabled: bool,
    /// Whether this client is denied from the restricted protocols such as security-context.
    pub restricted: bool,
    /// We cannot retrieve this client's socket credentials.
    pub credentials_unknown: bool,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, client_id: ClientId, _reason: DisconnectReason) {
        self.registration.disconnected(client_id);
    }
}

#[cfg(test)]
#[path = "tests/capture_screenshot.rs"]
mod capture_screenshot_tests;

#[cfg(test)]
#[path = "tests/owned_task_lifecycle.rs"]
mod owned_task_lifecycle_tests;
