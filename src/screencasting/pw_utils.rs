use std::cell::RefCell;
use std::cmp::min;
use std::collections::HashMap;
use std::io::Cursor;
use std::iter::zip;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};
use std::time::Duration;
use std::{mem, slice};

use anyhow::Context as _;
use calloop::timer::{TimeoutAction, Timer};
use calloop::RegistrationToken;
use pipewire::context::ContextRc;
use pipewire::core::{CoreRc, PW_ID_CORE};
use pipewire::loop_::Timeout;
use pipewire::main_loop::MainLoopRc;
use pipewire::properties::PropertiesBox;
use pipewire::spa::buffer::DataType;
use pipewire::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pipewire::spa::param::format_utils::parse_format;
use pipewire::spa::param::video::{VideoFormat, VideoInfoRaw};
use pipewire::spa::param::ParamType;
use pipewire::spa::pod::deserialize::PodDeserializer;
use pipewire::spa::pod::serialize::PodSerializer;
use pipewire::spa::pod::{self, ChoiceValue, Pod, PodPropFlags, Property, PropertyFlags};
use pipewire::spa::sys::*;
use pipewire::spa::utils::{
    Choice, ChoiceEnum, ChoiceFlags, Direction, Fraction, Rectangle, SpaTypes,
};
use pipewire::spa::{self};
use pipewire::stream::{Stream, StreamFlags, StreamListener, StreamRc, StreamState};
use pipewire::sys::{pw_buffer, pw_check_library_version, pw_stream_queue_buffer};
use smithay::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::{GbmBuffer, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Format, Fourcc};
use smithay::backend::drm::DrmDeviceFd;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::backend::renderer::element::{Element, RenderElement};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::{Bind, Offscreen, RendererSuper, Texture};
use smithay::output::{Output, OutputModeSource};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};
use smithay::reexports::gbm::Modifier;
use smithay::utils::{Logical, Physical, Point, Scale, Size, Transform};
use zbus::object_server::SignalEmitter;

use crate::compositor::{CastTarget, State};
use crate::dbus::mutter_screen_cast::{self, CursorMode};
use crate::platform::capture::CapturePermit;
use crate::platform::pixel_readback::{DeferredCaptureRenderer, PendingPixels};
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::{clear_dmabuf, encompassing_geo, render_to_dmabuf, render_to_texture};
use crate::screencasting::CastRenderElement;
use crate::utils::{get_monotonic_time, CastSessionId, CastStreamId};

// Give a 0.1 ms allowance for presentation time errors.
const CAST_DELAY_ALLOWANCE: Duration = Duration::from_micros(100);

const CURSOR_FORMAT: spa_video_format = SPA_VIDEO_FORMAT_BGRA;
const CURSOR_BPP: u32 = 4;
const CURSOR_WIDTH: u32 = 384;
const CURSOR_HEIGHT: u32 = 384;
const CURSOR_BITMAP_SIZE: usize = (CURSOR_WIDTH * CURSOR_HEIGHT * CURSOR_BPP) as usize;
const CURSOR_META_SIZE: usize =
    mem::size_of::<spa_meta_cursor>() + mem::size_of::<spa_meta_bitmap>() + CURSOR_BITMAP_SIZE;
const BITMAP_META_OFFSET: usize = mem::size_of::<spa_meta_cursor>();
const BITMAP_DATA_OFFSET: usize = mem::size_of::<spa_meta_bitmap>();

pub struct PipeWire {
    // Unregister callbacks before dropping the core they are attached to.
    _listener: pipewire::core::Listener,
    pub core: CoreRc,
    _context: ContextRc,
    pub token: RegistrationToken,
    event_loop: LoopHandle<'static, State>,
    to_clarion: calloop::channel::Sender<PwToClarion>,
}

pub enum PwToClarion {
    StopCast { session_id: CastSessionId },
    Redraw { stream_id: CastStreamId },
    FatalError,
}

pub struct Cast {
    event_loop: LoopHandle<'static, State>,
    pub session_id: CastSessionId,
    pub stream_id: CastStreamId,
    // Listener is dropped before Stream to prevent a use-after-free.
    _listener: StreamListener<()>,
    pub stream: StreamRc,
    pub target: CastTarget,
    pub dynamic_target: bool,
    formats: FormatSet,
    offer_alpha: bool,
    cursor_mode: CursorMode,
    pub last_frame_time: Duration,
    scheduled_redraw: Option<RegistrationToken>,
    /// Fence listeners keyed by their PipeWire buffer, so removal or cast teardown
    /// releases the fd even when the GPU never signals it.
    fence_sources: Rc<RefCell<HashMap<usize, RegistrationToken>>>,
    // Incremented once per successful frame, stored in buffer meta.
    sequence_counter: u64,
    inner: Rc<RefCell<CastInner>>,
    cursor_bitmap: Rc<RefCell<CursorBitmapCache>>,
    cursor_readback: Option<CursorReadback>,
}

#[derive(Debug, Clone, PartialEq)]
struct CursorBitmapGeometry {
    size: Size<i32, Physical>,
    hotspot: Point<i32, Physical>,
}

#[derive(Debug)]
struct CursorBitmap {
    geometry: CursorBitmapGeometry,
    pixels: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CursorReadbackKey {
    generation: u64,
    domain_epoch: u64,
}

/// One transfer per cast, regardless of cursor animation or input frequency.
/// Completed bytes are reattached to later buffers since PipeWire may drop the
/// particular buffer that first carried a changed cursor bitmap.
#[derive(Debug, Default)]
struct CursorBitmapCache {
    generation: u64,
    domain_epoch: u64,
    pending: Option<CursorReadbackKey>,
    geometry: Option<CursorBitmapGeometry>,
    output: Option<Output>,
    scale: Option<Scale<f64>>,
    bitmap: Option<CursorBitmap>,
    needs_readback: bool,
    delivery_pending: bool,
}

impl CursorBitmapCache {
    fn invalidate(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("cursor generation exhausted");
        self.domain_epoch = self
            .domain_epoch
            .checked_add(1)
            .expect("cursor domain exhausted");
        self.geometry = None;
        self.output = None;
        self.scale = None;
        self.bitmap = None;
        self.needs_readback = false;
        self.delivery_pending = true;
        // An old job still owns its permit. Do not start another until it
        // completes, even when its generation has become obsolete.
    }

    fn observe(
        &mut self,
        output: &Output,
        scale: Scale<f64>,
        geometry: CursorBitmapGeometry,
        damaged: bool,
    ) {
        let visible = cursor_bitmap_len(geometry.size).is_some();
        let was_visible = self
            .geometry
            .as_ref()
            .is_some_and(|g| cursor_bitmap_len(g.size).is_some());
        let domain_changed = self.output.as_ref() != Some(output)
            || self.scale != Some(scale)
            || visible != was_visible;
        let geometry_changed = self.geometry.as_ref() != Some(&geometry);
        if !domain_changed && !geometry_changed && !damaged {
            return;
        }
        self.generation = self
            .generation
            .checked_add(1)
            .expect("cursor generation exhausted");
        self.geometry = Some(geometry.clone());
        self.output = Some(output.clone());
        self.scale = Some(scale);
        if domain_changed {
            self.domain_epoch = self
                .domain_epoch
                .checked_add(1)
                .expect("cursor domain exhausted");
            self.bitmap = None;
            self.delivery_pending = true;
        }
        self.needs_readback = visible;
    }

    fn begin(&mut self) -> Option<(CursorReadbackKey, CursorBitmapGeometry)> {
        if self.pending.is_some() || !self.needs_readback {
            return None;
        }
        let geometry = self.geometry.clone()?;
        let key = CursorReadbackKey {
            generation: self.generation,
            domain_epoch: self.domain_epoch,
        };
        self.pending = Some(key);
        self.needs_readback = false;
        Some((key, geometry))
    }

    fn complete(
        &mut self,
        key: CursorReadbackKey,
        geometry: CursorBitmapGeometry,
        result: Result<Vec<u8>, String>,
    ) -> bool {
        if self.pending != Some(key) {
            return false;
        }
        self.pending = None;
        if key.domain_epoch != self.domain_epoch {
            return self.needs_readback || self.delivery_pending;
        }
        let expected = cursor_bitmap_len(geometry.size);
        match result {
            Ok(pixels) if expected == Some(pixels.len()) => {
                // Content can change faster than a transfer completes. Publish
                // this image with its own captured hotspot, then fetch the
                // newest content. A changed domain must still reject it.
                self.bitmap = Some(CursorBitmap { geometry, pixels });
                self.delivery_pending = true;
                true
            }
            _ => {
                // Keep a coherent previous image and retry on a later frame.
                // Do not create a tight retry loop for a broken GPU context.
                self.needs_readback = true;
                false
            }
        }
    }
}

pub(super) struct CursorReadback {
    pub(super) job: PendingPixels,
    pub(super) completion: CursorReadbackCompletion,
}

pub(super) struct CursorReadbackCompletion {
    cache: Weak<RefCell<CursorBitmapCache>>,
    generation: CursorReadbackKey,
    geometry: CursorBitmapGeometry,
    // Shared quota is held until the transfer has actually completed, including
    // stale jobs. No SPA pointer or borrowed renderer element is retained.
    _permit: CapturePermit,
}

impl CursorReadbackCompletion {
    pub(super) fn complete(self, result: Result<Vec<u8>, String>) -> bool {
        self.cache.upgrade().is_some_and(|cache| {
            cache
                .borrow_mut()
                .complete(self.generation, self.geometry, result)
        })
    }
}

fn cursor_bitmap_len(size: Size<i32, Physical>) -> Option<usize> {
    if size.w <= 0 || size.h <= 0 || size.w > CURSOR_WIDTH as i32 || size.h > CURSOR_HEIGHT as i32 {
        return None;
    }
    (size.w as usize)
        .checked_mul(size.h as usize)?
        .checked_mul(CURSOR_BPP as usize)
}

/// Mutable `Cast` state shared with PipeWire callbacks.
#[derive(Debug)]
struct CastInner {
    is_active: bool,
    node_id: Option<u32>,
    state: CastState,
    refresh: u32,
    min_time_between_frames: Duration,
    dmabufs: HashMap<i64, Dmabuf>,
    /// Buffers dequeued from PipeWire in process of rendering.
    ///
    /// This is an ordered list of buffers that we started rendering to and waiting for the
    /// rendering to complete. The completion can be checked from the `SyncPoint`s. The buffers are
    /// stored in order from oldest to newest, and the same ordering should be preserved when
    /// submitting completed buffers to PipeWire.
    rendering_buffers: Vec<(NonNull<pw_buffer>, SyncPoint)>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
enum CastState {
    ResizePending {
        pending_size: Size<u32, Physical>,
    },
    ConfirmationPending {
        size: Size<u32, Physical>,
        alpha: bool,
        modifier: Modifier,
        plane_count: i32,
    },
    Ready {
        size: Size<u32, Physical>,
        alpha: bool,
        modifier: Modifier,
        plane_count: i32,
        // Lazily-initialized to keep the initialization to a single place.
        damage_tracker: Option<OutputDamageTracker>,
        cursor_damage_tracker: Option<OutputDamageTracker>,
        last_cursor_location: Option<Point<i32, Physical>>,
    },
}

#[derive(PartialEq, Eq)]
pub enum CastSizeChange {
    Ready,
    Pending,
}

/// Data for drawing a cursor either as metadata or embedded.
///
/// The cursor elements are expected to be at the start of the main elements slice. `elem_count` is
/// the count of the pointer elements. This way, the full slice includes both main and cursor
/// elements for embedded mode, and `&elements[elem_count..]` gives just the main elements for
/// metadata mode.
///
/// We have weird borrowed references here in order to support both metadata and embedded cases.
/// The cursor damage tracker needs a slice of impl Element at (0, 0), so we pass it `relocated`
/// (luckily, &impl Element also impls Element). Then, if we need to embed the cursor, we use the
/// full elements slice which starts with non-relocated pointer elements (that we borrow from).
#[derive(Debug)]
pub struct CursorData<'a, E> {
    /// Count of the pointer elements in the slice (index of the first non-pointer element).
    elem_count: usize,
    /// Cursor elements relocated to (0, 0).
    relocated: Vec<RelocateRenderElement<&'a E>>,
    /// Location of the cursor's hotspot in the video buffer.
    location: Point<i32, Physical>,
    /// Location of the cursor's hotspot on the cursor bitmap.
    hotspot: Point<i32, Physical>,
    /// Size of the elements' encompassing geo.
    size: Size<i32, Physical>,
    /// Scale the elements should be rendered at.
    scale: Scale<f64>,
}

impl<'a, E: Element> CursorData<'a, E> {
    pub fn compute(
        elements: &'a [E],
        elem_count: usize,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
    ) -> Self {
        let pointer_elements = &elements[..elem_count];
        let location = location.to_physical_precise_round(scale);

        let geo = encompassing_geo(scale, pointer_elements.iter());
        let relocated = Vec::from_iter(pointer_elements.iter().map(|elem| {
            RelocateRenderElement::from_element(elem, geo.loc.upscale(-1), Relocate::Relative)
        }));

        Self {
            elem_count,
            relocated,
            location,
            hotspot: location - geo.loc,
            size: geo.size,
            scale,
        }
    }
}

macro_rules! make_params {
    ($params:ident, $formats:expr, $size:expr, $refresh:expr, $alpha:expr) => {
        let mut b1 = Vec::new();
        let mut b2 = Vec::new();

        let o1 = make_video_params($formats, $size, $refresh, false);
        let pod1 = make_pod(&mut b1, o1);

        let mut p1;
        let mut p2;
        $params = if $alpha {
            let o2 = make_video_params($formats, $size, $refresh, true);
            p2 = [pod1, make_pod(&mut b2, o2)];
            &mut p2[..]
        } else {
            p1 = [pod1];
            &mut p1[..]
        };
    };
}

impl PipeWire {
    pub fn new(
        event_loop: LoopHandle<'static, State>,
        to_clarion: calloop::channel::Sender<PwToClarion>,
    ) -> anyhow::Result<Self> {
        let main_loop = MainLoopRc::new(None).context("error creating MainLoop")?;
        let context = ContextRc::new(&main_loop, None).context("error creating Context")?;
        let core = context.connect_rc(None).context("error creating Core")?;

        let to_clarion_ = to_clarion.clone();
        let listener = core
            .add_listener_local()
            .error(move |id, seq, res, message| {
                warn!(id, seq, res, message, "pw error");

                // Reset PipeWire on connection errors.
                if id == PW_ID_CORE && res == -32 {
                    if let Err(err) = to_clarion_.send(PwToClarion::FatalError) {
                        warn!("error sending FatalError to clarion: {err:?}");
                    }
                }
            })
            .register();
        struct AsFdWrapper(MainLoopRc);
        impl AsFd for AsFdWrapper {
            fn as_fd(&self) -> BorrowedFd<'_> {
                self.0.loop_().fd()
            }
        }
        let generic = Generic::new(AsFdWrapper(main_loop), Interest::READ, Mode::Level);
        let token = event_loop
            .insert_source(generic, move |_, wrapper, _| {
                let _span = clarion_tracy_span!("pipewire iteration");
                wrapper.0.loop_().iterate(Timeout::None);
                Ok(PostAction::Continue)
            })
            .unwrap();

        Ok(Self {
            _listener: listener,
            core,
            _context: context,
            token,
            event_loop,
            to_clarion,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_cast(
        &self,
        gbm: GbmDevice<DrmDeviceFd>,
        formats: FormatSet,
        session_id: CastSessionId,
        stream_id: CastStreamId,
        target: CastTarget,
        size: Size<i32, Physical>,
        refresh: u32,
        alpha: bool,
        mut cursor_mode: CursorMode,
        signal_ctx: SignalEmitter<'static>,
    ) -> anyhow::Result<Cast> {
        let _span = clarion_tracy_span!("PipeWire::start_cast");

        let to_clarion_ = self.to_clarion.clone();
        let stop_cast = move || {
            if let Err(err) = to_clarion_.send(PwToClarion::StopCast { session_id }) {
                warn!(%session_id, "error sending StopCast to clarion: {err:?}");
            }
        };
        let to_clarion_ = self.to_clarion.clone();
        let redraw = move || {
            if let Err(err) = to_clarion_.send(PwToClarion::Redraw { stream_id }) {
                warn!(%stream_id, "error sending Redraw to clarion: {err:?}");
            }
        };
        let redraw_ = redraw.clone();

        let stream = StreamRc::new(
            self.core.clone(),
            "clarion-screen-cast-src",
            PropertiesBox::new(),
        )
        .context("error creating Stream")?;

        if cursor_mode == CursorMode::Metadata && !pw_version_supports_cursor_metadata() {
            debug!(
                "metadata cursor mode requested, but PipeWire is too old (need >= 1.4.8); \
                 switching to embedded cursor"
            );
            cursor_mode = CursorMode::Embedded;
        }

        let pending_size = Size::from((size.w as u32, size.h as u32));
        let fence_sources = Rc::new(RefCell::new(HashMap::new()));

        // Like in good old wayland-rs times...
        let inner = Rc::new(RefCell::new(CastInner {
            is_active: false,
            node_id: None,
            state: CastState::ResizePending { pending_size },
            refresh,
            min_time_between_frames: Duration::ZERO,
            dmabufs: HashMap::new(),
            rendering_buffers: Vec::new(),
        }));

        let listener = stream
            .add_local_listener_with_user_data(())
            .state_changed({
                let inner = inner.clone();
                let stop_cast = stop_cast.clone();
                move |stream, (), old, new| {
                    let _span = debug_span!("state_changed", %stream_id).entered();
                    debug!("{old:?} -> {new:?}");
                    let mut inner = inner.borrow_mut();

                    match new {
                        StreamState::Paused => {
                            if inner.node_id.is_none() {
                                let id = stream.node_id();
                                inner.node_id = Some(id);
                                debug!("sending signal with {id}");

                                let _span = clarion_tracy_span!("sending PipeWireStreamAdded");
                                async_io::block_on(async {
                                    let res = mutter_screen_cast::Stream::pipe_wire_stream_added(
                                        &signal_ctx,
                                        id,
                                    )
                                    .await;

                                    if let Err(err) = res {
                                        warn!("error sending PipeWireStreamAdded: {err:?}");
                                        stop_cast();
                                    }
                                });
                            }

                            inner.is_active = false;
                        }
                        StreamState::Error(_) => {
                            if inner.is_active {
                                inner.is_active = false;
                                stop_cast();
                            }
                        }
                        StreamState::Unconnected => (),
                        StreamState::Connecting => (),
                        StreamState::Streaming => {
                            inner.is_active = true;
                            redraw();
                        }
                    }
                }
            })
            .param_changed({
                let inner = inner.clone();
                let stop_cast = stop_cast.clone();
                let gbm = gbm.clone();
                let formats = formats.clone();
                move |stream, (), id, pod| {
                    let id = ParamType::from_raw(id);
                    trace!(%stream_id, ?id, "param_changed");
                    let mut inner = inner.borrow_mut();
                    let inner = &mut *inner;

                    if id != ParamType::Format {
                        return;
                    }

                    let _span = debug_span!("param_changed", %stream_id).entered();

                    let Some(pod) = pod else { return };

                    let (m_type, m_subtype) = match parse_format(pod) {
                        Ok(x) => x,
                        Err(err) => {
                            warn!("error parsing format: {err:?}");
                            return;
                        }
                    };

                    if m_type != MediaType::Video || m_subtype != MediaSubtype::Raw {
                        return;
                    }

                    let mut format = VideoInfoRaw::new();
                    format.parse(pod).unwrap();
                    debug!("got format = {format:?}");

                    let format_size = Size::from((format.size().width, format.size().height));

                    let state = &mut inner.state;
                    if format_size != state.expected_format_size() {
                        if !matches!(&*state, CastState::ResizePending { .. }) {
                            warn!("wrong size, but we're not resizing");
                            stop_cast();
                            return;
                        }

                        debug!("wrong size, waiting");
                        return;
                    }

                    let format_has_alpha = format.format() == VideoFormat::BGRA;
                    let fourcc = if format_has_alpha {
                        Fourcc::Argb8888
                    } else {
                        Fourcc::Xrgb8888
                    };

                    let max_frame_rate = format.max_framerate();
                    let min_frame_time = Duration::from_micros(
                        1_000_000 * u64::from(max_frame_rate.denom) / u64::from(max_frame_rate.num),
                    );
                    inner.min_time_between_frames = min_frame_time;

                    let object = pod.as_object().unwrap();
                    let Some(prop_modifier) =
                        object.find_prop(spa::utils::Id(FormatProperties::VideoModifier.0))
                    else {
                        warn!("modifier prop missing");
                        stop_cast();
                        return;
                    };

                    if prop_modifier.flags().contains(PodPropFlags::DONT_FIXATE) {
                        debug!("fixating the modifier");

                        let pod_modifier = prop_modifier.value();
                        let Ok((_, modifiers)) = PodDeserializer::deserialize_from::<Choice<i64>>(
                            pod_modifier.as_bytes(),
                        ) else {
                            warn!("wrong modifier property type");
                            stop_cast();
                            return;
                        };

                        let ChoiceEnum::Enum { alternatives, .. } = modifiers.1 else {
                            warn!("wrong modifier choice type");
                            stop_cast();
                            return;
                        };

                        let (modifier, plane_count) = match find_preferred_modifier(
                            &gbm,
                            format_size,
                            fourcc,
                            alternatives,
                        ) {
                            Ok(x) => x,
                            Err(err) => {
                                warn!("couldn't find preferred modifier: {err:?}");
                                stop_cast();
                                return;
                            }
                        };

                        debug!(
                            "allocation successful \
                             (modifier={modifier:?}, plane_count={plane_count}), \
                             moving to confirmation pending"
                        );

                        *state = CastState::ConfirmationPending {
                            size: format_size,
                            alpha: format_has_alpha,
                            modifier,
                            plane_count: plane_count as i32,
                        };

                        let fixated_format = FormatSet::from_iter([Format {
                            code: fourcc,
                            modifier,
                        }]);

                        let mut b1 = Vec::new();
                        let mut b2 = Vec::new();

                        let o1 = make_video_params(
                            &fixated_format,
                            format_size,
                            inner.refresh,
                            format_has_alpha,
                        );
                        let pod1 = make_pod(&mut b1, o1);

                        let o2 = make_video_params(
                            &formats,
                            format_size,
                            inner.refresh,
                            format_has_alpha,
                        );
                        let mut params = [pod1, make_pod(&mut b2, o2)];

                        if let Err(err) = stream.update_params(&mut params) {
                            warn!("error updating stream params: {err:?}");
                            stop_cast();
                        }

                        return;
                    }

                    // Verify that alpha and modifier didn't change.
                    let plane_count = match &*state {
                        CastState::ConfirmationPending {
                            size,
                            alpha,
                            modifier,
                            plane_count,
                        }
                        | CastState::Ready {
                            size,
                            alpha,
                            modifier,
                            plane_count,
                            ..
                        } if *alpha == format_has_alpha
                            && *modifier == Modifier::from(format.modifier()) =>
                        {
                            let size = *size;
                            let alpha = *alpha;
                            let modifier = *modifier;
                            let plane_count = *plane_count;

                            let (damage_tracker, cursor_damage_tracker) =
                                if let CastState::Ready {
                                    damage_tracker,
                                    cursor_damage_tracker,
                                    ..
                                } = &mut *state
                                {
                                    (damage_tracker.take(), cursor_damage_tracker.take())
                                } else {
                                    (None, None)
                                };

                            debug!("moving to ready state");

                            *state = CastState::Ready {
                                size,
                                alpha,
                                modifier,
                                plane_count,
                                damage_tracker,
                                cursor_damage_tracker,
                                last_cursor_location: None,
                            };

                            plane_count
                        }
                        _ => {
                            // We're negotiating a single modifier, or alpha or modifier changed,
                            // so we need to do a test allocation.
                            let (modifier, plane_count) = match find_preferred_modifier(
                                &gbm,
                                format_size,
                                fourcc,
                                vec![format.modifier() as i64],
                            ) {
                                Ok(x) => x,
                                Err(err) => {
                                    warn!("test allocation failed: {err:?}");
                                    stop_cast();
                                    return;
                                }
                            };

                            debug!(
                                "allocation successful \
                                 (modifier={modifier:?}, plane_count={plane_count}), \
                                 moving to ready"
                            );

                            *state = CastState::Ready {
                                size: format_size,
                                alpha: format_has_alpha,
                                modifier,
                                plane_count: plane_count as i32,
                                damage_tracker: None,
                                cursor_damage_tracker: None,
                                last_cursor_location: None,
                            };

                            plane_count as i32
                        }
                    };

                    // const BPP: u32 = 4;
                    // let stride = format.size().width * BPP;
                    // let size = stride * format.size().height;

                    let o1 = pod::object!(
                        SpaTypes::ObjectParamBuffers,
                        ParamType::Buffers,
                        Property::new(
                            SPA_PARAM_BUFFERS_buffers,
                            pod::Value::Choice(ChoiceValue::Int(Choice(
                                ChoiceFlags::empty(),
                                ChoiceEnum::Range {
                                    default: 8,
                                    min: 2,
                                    max: 16
                                }
                            ))),
                        ),
                        Property::new(SPA_PARAM_BUFFERS_blocks, pod::Value::Int(plane_count)),
                        Property::new(
                            SPA_PARAM_BUFFERS_dataType,
                            pod::Value::Choice(ChoiceValue::Int(Choice(
                                ChoiceFlags::empty(),
                                ChoiceEnum::Flags {
                                    default: 1 << DataType::DmaBuf.as_raw(),
                                    flags: vec![1 << DataType::DmaBuf.as_raw()],
                                },
                            ))),
                        ),
                    );

                    let o2 = pod::object!(
                        SpaTypes::ObjectParamMeta,
                        ParamType::Meta,
                        Property::new(
                            SPA_PARAM_META_type,
                            pod::Value::Id(spa::utils::Id(SPA_META_Header))
                        ),
                        Property::new(
                            SPA_PARAM_META_size,
                            pod::Value::Int(size_of::<spa_meta_header>() as i32)
                        ),
                    );
                    let mut b1 = vec![];
                    let mut b2 = vec![];
                    let mut params = vec![make_pod(&mut b1, o1), make_pod(&mut b2, o2)];

                    let mut b_cursor = vec![];
                    if cursor_mode == CursorMode::Metadata {
                        let o_cursor = pod::object!(
                            SpaTypes::ObjectParamMeta,
                            ParamType::Meta,
                            Property::new(
                                SPA_PARAM_META_type,
                                pod::Value::Id(spa::utils::Id(SPA_META_Cursor))
                            ),
                            Property::new(
                                SPA_PARAM_META_size,
                                pod::Value::Int(CURSOR_META_SIZE as i32)
                            ),
                        );
                        params.push(make_pod(&mut b_cursor, o_cursor));
                    }

                    if let Err(err) = stream.update_params(&mut params) {
                        warn!("error updating stream params: {err:?}");
                        stop_cast();
                    }
                }
            })
            .add_buffer({
                let inner = inner.clone();
                let stop_cast = stop_cast.clone();
                move |stream, (), buffer| {
                    let _span = debug_span!("add_buffer", %stream_id).entered();
                    let mut inner = inner.borrow_mut();

                    let (size, alpha, modifier) = if let CastState::Ready {
                        size,
                        alpha,
                        modifier,
                        ..
                    } = &inner.state
                    {
                        (*size, *alpha, *modifier)
                    } else {
                        trace!("add_buffer, but not ready yet");
                        return;
                    };

                    trace!("size={size:?}, alpha={alpha}, modifier={modifier:?}");

                    unsafe {
                        let spa_buffer = (*buffer).buffer;

                        let fourcc = if alpha {
                            Fourcc::Argb8888
                        } else {
                            Fourcc::Xrgb8888
                        };

                        let dmabuf = match allocate_dmabuf(&gbm, size, fourcc, modifier) {
                            Ok(dmabuf) => dmabuf,
                            Err(err) => {
                                warn!("error allocating dmabuf: {err:?}");
                                stop_cast();
                                return;
                            }
                        };

                        let plane_count = dmabuf.num_planes();
                        assert_eq!((*spa_buffer).n_datas as usize, plane_count);

                        for (i, (fd, (stride, offset))) in
                            zip(dmabuf.handles(), zip(dmabuf.strides(), dmabuf.offsets()))
                                .enumerate()
                        {
                            let spa_data = (*spa_buffer).datas.add(i);
                            assert!((*spa_data).type_ & (1 << DataType::DmaBuf.as_raw()) > 0);

                            (*spa_data).type_ = DataType::DmaBuf.as_raw();

                            // With DMA-BUFs, consumers should ignore the maxsize field, and
                            // producers are allowed to set it to 0.
                            //
                            // https://docs.pipewire.org/page_dma_buf.html
                            (*spa_data).maxsize = 1;
                            (*spa_data).fd = fd.as_raw_fd() as i64;
                            (*spa_data).flags = SPA_DATA_FLAG_READWRITE;

                            let chunk = (*spa_data).chunk;
                            (*chunk).stride = stride as i32;
                            (*chunk).offset = offset;

                            trace!(
                                "pw buffer plane: fd={}, stride={stride}, offset={offset}",
                                (*spa_data).fd
                            );
                        }

                        let fd = (*(*spa_buffer).datas).fd;
                        assert!(inner.dmabufs.insert(fd, dmabuf).is_none());
                    }

                    // During size re-negotiation, the stream sometimes just keeps running, in
                    // which case we may need to force a redraw once we got a newly sized buffer.
                    if inner.dmabufs.len() == 1 && stream.state() == StreamState::Streaming {
                        redraw_();
                    }
                }
            })
            .remove_buffer({
                let inner = inner.clone();
                let fence_sources = fence_sources.clone();
                let event_loop = self.event_loop.clone();
                move |_stream, (), buffer| {
                    trace!(%stream_id, "remove_buffer");
                    if let Some(token) = fence_sources.borrow_mut().remove(&(buffer as usize)) {
                        event_loop.remove(token);
                    }
                    let mut inner = inner.borrow_mut();

                    inner
                        .rendering_buffers
                        .retain(|(buf, _)| buf.as_ptr() != buffer);

                    unsafe {
                        let spa_buffer = (*buffer).buffer;
                        let spa_data = (*spa_buffer).datas;
                        assert!((*spa_buffer).n_datas > 0);

                        let fd = (*spa_data).fd;
                        inner.dmabufs.remove(&fd);
                    }
                }
            })
            .register()
            .unwrap();

        trace!(
            %stream_id,
            "starting pw stream with size={pending_size:?}, refresh={refresh:?}"
        );

        let params;
        make_params!(params, &formats, pending_size, refresh, alpha);
        stream
            .connect(
                Direction::Output,
                None,
                StreamFlags::DRIVER | StreamFlags::ALLOC_BUFFERS,
                params,
            )
            .context("error connecting stream")?;

        let cast = Cast {
            event_loop: self.event_loop.clone(),
            session_id,
            stream_id,
            stream,
            _listener: listener,
            target,
            dynamic_target: false,
            formats,
            offer_alpha: alpha,
            cursor_mode,
            last_frame_time: Duration::ZERO,
            scheduled_redraw: None,
            fence_sources,
            sequence_counter: 0,
            inner,
            cursor_bitmap: Rc::new(RefCell::new(CursorBitmapCache::default())),
            cursor_readback: None,
        };
        Ok(cast)
    }
}

impl Cast {
    pub fn is_active(&self) -> bool {
        self.inner.borrow().is_active
    }

    pub fn node_id(&self) -> Option<u32> {
        self.inner.borrow().node_id
    }

    pub fn ensure_size(&self, size: Size<i32, Physical>) -> anyhow::Result<CastSizeChange> {
        let mut inner = self.inner.borrow_mut();

        let new_size = Size::from((size.w as u32, size.h as u32));

        let state = &mut inner.state;
        if matches!(state, CastState::Ready { size, .. } if *size == new_size) {
            return Ok(CastSizeChange::Ready);
        }

        if state.pending_size() == Some(new_size) {
            debug!("stream size still hasn't changed, skipping frame");
            return Ok(CastSizeChange::Pending);
        }

        let _span = clarion_tracy_span!("Cast::ensure_size");
        debug!("cast size changed, updating stream size");

        self.cursor_bitmap.borrow_mut().invalidate();

        *state = CastState::ResizePending {
            pending_size: new_size,
        };

        let params;
        make_params!(
            params,
            &self.formats,
            new_size,
            inner.refresh,
            self.offer_alpha
        );
        self.stream
            .update_params(params)
            .context("error updating stream params")?;

        Ok(CastSizeChange::Pending)
    }

    pub fn set_refresh(&mut self, refresh: u32) -> anyhow::Result<()> {
        let mut inner = self.inner.borrow_mut();

        if inner.refresh == refresh {
            return Ok(());
        }

        let _span = clarion_tracy_span!("Cast::set_refresh");
        debug!("cast FPS changed, updating stream FPS");
        inner.refresh = refresh;

        let size = inner.state.expected_format_size();
        let params;
        make_params!(params, &self.formats, size, refresh, self.offer_alpha);
        self.stream
            .update_params(params)
            .context("error updating stream params")?;

        Ok(())
    }

    fn compute_extra_delay(&self, target_frame_time: Duration) -> Duration {
        let inner = self.inner.borrow();

        let last = self.last_frame_time;
        let min = inner.min_time_between_frames;

        if last.is_zero() {
            trace!(?target_frame_time, ?last, "last is zero, recording");
            return Duration::ZERO;
        }

        if target_frame_time < last {
            // Record frame with a warning; in case it was an overflow this will fix it.
            warn!(
                ?target_frame_time,
                ?last,
                "target frame time is below last, did it overflow or did we mispredict?"
            );
            return Duration::ZERO;
        }

        let diff = target_frame_time - last;
        if diff < min {
            let delay = min - diff;
            trace!(
                ?target_frame_time,
                ?last,
                "frame is too soon: min={min:?}, delay={:?}",
                delay
            );
            return delay;
        } else {
            trace!("overshoot={:?}", diff - min);
        }

        Duration::ZERO
    }

    fn schedule_redraw(&mut self, output: Output, target_time: Duration) {
        if self.scheduled_redraw.is_some() {
            return;
        }

        let now = get_monotonic_time();
        let duration = target_time.saturating_sub(now);
        let timer = Timer::from_duration(duration);
        let token = self
            .event_loop
            .insert_source(timer, move |_, _, state| {
                // Guard against output disconnecting before the timer has a chance to run.
                if state.clarion.output_state.contains_key(&output) {
                    state.clarion.queue_redraw(&output);
                }

                TimeoutAction::Drop
            })
            .unwrap();
        self.scheduled_redraw = Some(token);
    }

    fn remove_scheduled_redraw(&mut self) {
        if let Some(token) = self.scheduled_redraw.take() {
            self.event_loop.remove(token);
        }
    }

    /// Checks whether this frame should be skipped because it's too soon.
    ///
    /// If the frame should be skipped, schedules a redraw and returns `true`. Otherwise, removes a
    /// scheduled redraw, if any, and returns `false`.
    ///
    /// When this method returns `false`, the calling code is assumed to follow up with
    /// [`Cast::dequeue_buffer_and_render()`].
    pub fn check_time_and_schedule(
        &mut self,
        output: &Output,
        target_frame_time: Duration,
    ) -> bool {
        let delay = self.compute_extra_delay(target_frame_time);
        if delay >= CAST_DELAY_ALLOWANCE {
            trace!("delay >= allowance, scheduling redraw");
            self.schedule_redraw(output.clone(), target_frame_time + delay);
            true
        } else {
            self.remove_scheduled_redraw();
            false
        }
    }

    fn dequeue_available_buffer(&mut self) -> Option<NonNull<pw_buffer>> {
        unsafe { NonNull::new(self.stream.dequeue_raw_buffer()) }
    }

    fn queue_completed_buffers(&mut self) {
        let mut inner = self.inner.borrow_mut();

        // We want to queue buffers in order, so find the first still-rendering buffer, and queue
        // everything up to that. Even if there are completed buffers past the first
        // still-rendering buffer, we do not want to queue them, since that would send frames out
        // of order.
        let first_in_progress_idx = inner
            .rendering_buffers
            .iter()
            .position(|(_, sync)| !sync.is_reached())
            .unwrap_or(inner.rendering_buffers.len());

        for (buffer, _) in inner.rendering_buffers.drain(..first_in_progress_idx) {
            trace!("queueing completed buffer");
            unsafe {
                pw_stream_queue_buffer(self.stream.as_raw_ptr(), buffer.as_ptr());
            }
        }
    }

    unsafe fn queue_after_sync(&mut self, pw_buffer: NonNull<pw_buffer>, sync_point: SyncPoint) {
        let _span = clarion_tracy_span!("Cast::queue_after_sync");

        let mut inner = self.inner.borrow_mut();

        let sync_fd = sync_point.export();
        let unexported_sync = sync_fd.is_none().then(|| sync_point.clone());

        inner.rendering_buffers.push((pw_buffer, sync_point));
        drop(inner);

        match sync_fd {
            None => {
                let sync = unexported_sync.unwrap();
                if sync.is_reached() {
                    // Fence-free or already completed work can be delivered, in order.
                    self.queue_completed_buffers();
                } else {
                    // Keep this dequeued buffer and its original fence in the
                    // rendering queue. Neither timeout nor export failure can
                    // return it to PipeWire while the GPU may still write it.
                    let stream_id = self.stream_id;
                    let buffer_key = pw_buffer.as_ptr() as usize;
                    let fence_sources = self.fence_sources.clone();
                    let token = crate::platform::sync_delivery::defer_unexported_sync(
                        &self.event_loop,
                        sync,
                        move |state| {
                            fence_sources.borrow_mut().remove(&buffer_key);
                            for cast in &mut state.clarion.casting.casts {
                                if cast.stream_id == stream_id {
                                    cast.queue_completed_buffers();
                                }
                            }
                        },
                    );
                    assert!(self
                        .fence_sources
                        .borrow_mut()
                        .insert(buffer_key, token)
                        .is_none());
                }
            }
            Some(sync_fd) => {
                trace!("scheduling buffer to queue");
                let stream_id = self.stream_id;
                let buffer_key = pw_buffer.as_ptr() as usize;
                let fence_sources = self.fence_sources.clone();
                let source = Generic::new(sync_fd, Interest::READ, Mode::OneShot);
                let token = self
                    .event_loop
                    .insert_source(source, move |_, _, state| {
                        fence_sources.borrow_mut().remove(&buffer_key);
                        for cast in &mut state.clarion.casting.casts {
                            if cast.stream_id == stream_id {
                                cast.queue_completed_buffers();
                            }
                        }

                        Ok(PostAction::Remove)
                    })
                    .unwrap();
                assert!(self
                    .fence_sources
                    .borrow_mut()
                    .insert(buffer_key, token)
                    .is_none());
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn dequeue_buffer_and_render<R>(
        &mut self,
        renderer: &mut R,
        mut elements: &[CastRenderElement<R>],
        cursor_data: &CursorData<CastRenderElement<R>>,
        size: Size<i32, Physical>,
        scale: Scale<f64>,
        output: &Output,
        acquire_capture: &mut dyn FnMut() -> anyhow::Result<CapturePermit>,
    ) -> bool
    where
        R: DeferredCaptureRenderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>,
        CastRenderElement<R>: RenderElement<R>,
        for<'a> R::Framebuffer<'a>: Texture,
    {
        let mut inner = self.inner.borrow_mut();

        let CastState::Ready {
            damage_tracker,
            cursor_damage_tracker,
            last_cursor_location,
            ..
        } = &mut inner.state
        else {
            error!("cast must be in Ready state to render");
            return false;
        };
        let damage_tracker = damage_tracker
            .get_or_insert_with(|| OutputDamageTracker::new(size, scale, Transform::Normal));
        let cursor_damage_tracker = cursor_damage_tracker.get_or_insert_with(|| {
            OutputDamageTracker::new(
                Size::from((CURSOR_WIDTH as _, CURSOR_HEIGHT as _)),
                scale,
                Transform::Normal,
            )
        });

        // Size change will drop the damage tracker, but scale change won't, so check it here.
        let OutputModeSource::Static { scale: t_scale, .. } = damage_tracker.mode() else {
            unreachable!();
        };
        if *t_scale != scale {
            *damage_tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
            *cursor_damage_tracker = OutputDamageTracker::new(
                Size::from((CURSOR_WIDTH as _, CURSOR_HEIGHT as _)),
                scale,
                Transform::Normal,
            );
        }

        let mut has_cursor_update = false;

        // For embedded cursor, pass the full slice (cursor + main) to the damage tracker.
        // For metadata or hidden cursor, pass only the main elements.
        if self.cursor_mode == CursorMode::Metadata || self.cursor_mode == CursorMode::Hidden {
            elements = &elements[cursor_data.elem_count..];
        }
        let (damage, states) = damage_tracker.damage_output(1, elements).unwrap();
        let has_damage = damage.is_some();

        if self.cursor_mode == CursorMode::Metadata {
            let (damage, _states) = cursor_damage_tracker
                .damage_output(1, &cursor_data.relocated)
                .unwrap();
            let geometry = CursorBitmapGeometry {
                size: Size::new(
                    min(cursor_data.size.w, CURSOR_WIDTH as i32),
                    min(cursor_data.size.h, CURSOR_HEIGHT as i32),
                ),
                hotspot: cursor_data.hotspot,
            };
            self.cursor_bitmap
                .borrow_mut()
                .observe(output, scale, geometry, damage.is_some());
            has_cursor_update = damage.is_some()
                || self.cursor_bitmap.borrow().delivery_pending
                || *last_cursor_location != Some(cursor_data.location);
        }

        drop(inner);
        if self.cursor_mode == CursorMode::Metadata {
            self.prepare_cursor_readback(renderer, cursor_data, output, acquire_capture);
        }

        if !has_damage && !has_cursor_update {
            trace!("no damage, skipping frame");
            return false;
        }
        let Some(pw_buffer) = self.dequeue_available_buffer() else {
            warn!("no available buffer in pw stream, skipping frame");
            return false;
        };
        let buffer = pw_buffer.as_ptr();

        let mut inner = self.inner.borrow_mut();
        let inner_ = &mut *inner;
        let CastState::Ready { damage_tracker, .. } = &mut inner_.state else {
            unreachable!()
        };
        let damage_tracker = damage_tracker.as_mut().unwrap();

        unsafe {
            let spa_buffer = (*buffer).buffer;

            if self.cursor_mode == CursorMode::Metadata {
                add_cursor_metadata(
                    spa_buffer,
                    cursor_data.location,
                    &self.cursor_bitmap.borrow(),
                );
            }

            // FIXME: would be good to skip rendering the full frame if only the pointer changed.
            // Unfortunately, I think the OBS PipeWire code needs to be updated first to cleanly
            // allow for that codepath.
            let fd = (*(*spa_buffer).datas).fd;
            let dmabuf = inner_.dmabufs[&fd].clone();

            let res = render_to_dmabuf(renderer, damage_tracker, dmabuf, elements, states);
            drop(inner);

            match res {
                Ok(sync_point) => {
                    if self.cursor_mode == CursorMode::Metadata {
                        self.cursor_bitmap.borrow_mut().delivery_pending = false;
                        if let CastState::Ready {
                            last_cursor_location,
                            ..
                        } = &mut self.inner.borrow_mut().state
                        {
                            *last_cursor_location = Some(cursor_data.location);
                        }
                    }
                    mark_buffer_as_good(pw_buffer, &mut self.sequence_counter);
                    trace!("queueing buffer with seq={}", self.sequence_counter);
                    self.queue_after_sync(pw_buffer, sync_point);
                    true
                }
                Err(err) => {
                    warn!("error rendering to dmabuf: {err:?}");
                    return_unused_buffer(&self.stream, pw_buffer);
                    false
                }
            }
        }
    }

    fn prepare_cursor_readback<R>(
        &mut self,
        renderer: &mut R,
        cursor: &CursorData<CastRenderElement<R>>,
        output: &Output,
        acquire: &mut dyn FnMut() -> anyhow::Result<CapturePermit>,
    ) where
        R: DeferredCaptureRenderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>,
        CastRenderElement<R>: RenderElement<R>,
    {
        let cache = self.cursor_bitmap.borrow();
        if cache.pending.is_some() || !cache.needs_readback {
            return;
        }
        drop(cache);
        let Ok(permit) = acquire() else {
            // A bounded timer retries the latest shape without spinning or
            // submitting a GPU transfer before obtaining shared capacity.
            self.schedule_redraw(
                output.clone(),
                get_monotonic_time() + Duration::from_millis(16),
            );
            return;
        };
        let Some((generation, geometry)) = self.cursor_bitmap.borrow_mut().begin() else {
            return;
        };
        let result = render_to_texture(
            renderer,
            geometry.size,
            cursor.scale,
            Transform::Normal,
            Fourcc::Argb8888,
            cursor.relocated.iter().rev(),
        )
        .and_then(|(texture, _sync)| {
            renderer.enqueue_pixels(
                &texture,
                smithay::utils::Rectangle::from_size(texture.size()),
                Fourcc::Argb8888,
            )
        });
        let completion = CursorReadbackCompletion {
            cache: Rc::downgrade(&self.cursor_bitmap),
            generation,
            geometry,
            _permit: permit,
        };
        match result {
            Ok(job) => self.cursor_readback = Some(CursorReadback { job, completion }),
            Err(error) => {
                warn!("error submitting cursor bitmap: {error:#}");
                completion.complete(Err(error.to_string()));
            }
        }
    }

    pub(super) fn take_cursor_readback(&mut self) -> Option<CursorReadback> {
        self.cursor_readback.take()
    }

    pub(super) fn cursor_readback_registration_failed(&mut self) {
        let mut cache = self.cursor_bitmap.borrow_mut();
        cache.generation = cache
            .generation
            .checked_add(1)
            .expect("cursor generation exhausted");
        cache.pending = None;
        cache.needs_readback = cache
            .geometry
            .as_ref()
            .is_some_and(|g| cursor_bitmap_len(g.size).is_some());
    }

    pub(crate) fn invalidate_cursor_bitmap(&mut self) {
        self.cursor_bitmap.borrow_mut().invalidate();
    }

    pub fn dequeue_buffer_and_clear<R: ClarionRenderer>(&mut self, renderer: &mut R) -> bool {
        self.invalidate_cursor_bitmap();
        let mut inner = self.inner.borrow_mut();

        // Clear out the damage tracker if we're in Ready state.
        if let CastState::Ready {
            damage_tracker,
            cursor_damage_tracker,
            ..
        } = &mut inner.state
        {
            *damage_tracker = None;
            *cursor_damage_tracker = None;
        };
        drop(inner);

        let Some(pw_buffer) = self.dequeue_available_buffer() else {
            warn!("no available buffer in pw stream, skipping frame");
            return false;
        };
        let buffer = pw_buffer.as_ptr();

        unsafe {
            let spa_buffer = (*buffer).buffer;

            if self.cursor_mode == CursorMode::Metadata {
                add_invisible_cursor(spa_buffer);
            }

            let fd = (*(*spa_buffer).datas).fd;
            let dmabuf = self.inner.borrow().dmabufs[&fd].clone();

            match clear_dmabuf(renderer, dmabuf) {
                Ok(sync_point) => {
                    mark_buffer_as_good(pw_buffer, &mut self.sequence_counter);
                    trace!("queueing clear buffer with seq={}", self.sequence_counter);
                    self.queue_after_sync(pw_buffer, sync_point);
                    true
                }
                Err(err) => {
                    warn!("error clearing dmabuf: {err:?}");
                    return_unused_buffer(&self.stream, pw_buffer);
                    false
                }
            }
        }
    }
}

impl Drop for Cast {
    fn drop(&mut self) {
        self.cursor_bitmap.borrow_mut().invalidate();
        self.remove_scheduled_redraw();
        for (_, token) in self.fence_sources.borrow_mut().drain() {
            self.event_loop.remove(token);
        }
    }
}

impl CastState {
    fn pending_size(&self) -> Option<Size<u32, Physical>> {
        match self {
            CastState::ResizePending { pending_size } => Some(*pending_size),
            CastState::ConfirmationPending { size, .. } => Some(*size),
            CastState::Ready { .. } => None,
        }
    }

    fn expected_format_size(&self) -> Size<u32, Physical> {
        match self {
            CastState::ResizePending { pending_size } => *pending_size,
            CastState::ConfirmationPending { size, .. } => *size,
            CastState::Ready { size, .. } => *size,
        }
    }
}

fn pw_version_supports_cursor_metadata() -> bool {
    // This PipeWire version fixed a critical memory issue with cursor metadata:
    // https://gitlab.freedesktop.org/pipewire/pipewire/-/merge_requests/2538
    unsafe { pw_check_library_version(1, 4, 8) }
}

fn make_video_params(
    formats: &FormatSet,
    size: Size<u32, Physical>,
    refresh: u32,
    alpha: bool,
) -> pod::Object {
    let format = if alpha {
        VideoFormat::BGRA
    } else {
        VideoFormat::BGRx
    };

    let fourcc = if alpha {
        Fourcc::Argb8888
    } else {
        Fourcc::Xrgb8888
    };

    let formats: Vec<_> = formats
        .iter()
        .filter_map(|f| (f.code == fourcc).then_some(u64::from(f.modifier) as i64))
        .collect();

    trace!("offering: {formats:?}");

    let dont_fixate = if formats.len() > 1 {
        PropertyFlags::DONT_FIXATE
    } else {
        PropertyFlags::empty()
    };

    pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pod::property!(FormatProperties::VideoFormat, Id, format),
        Property {
            key: FormatProperties::VideoModifier.as_raw(),
            flags: PropertyFlags::MANDATORY | dont_fixate,
            value: pod::Value::Choice(ChoiceValue::Long(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Enum {
                    default: formats[0],
                    alternatives: formats,
                }
            )))
        },
        pod::property!(
            FormatProperties::VideoSize,
            Rectangle,
            Rectangle {
                width: size.w,
                height: size.h,
            }
        ),
        pod::property!(
            FormatProperties::VideoFramerate,
            Fraction,
            Fraction { num: 0, denom: 1 }
        ),
        pod::property!(
            FormatProperties::VideoMaxFramerate,
            Choice,
            Range,
            Fraction,
            Fraction {
                num: refresh,
                denom: 1000
            },
            Fraction { num: 1, denom: 1 },
            Fraction {
                num: refresh,
                denom: 1000
            }
        ),
    )
}

fn make_pod(buffer: &mut Vec<u8>, object: pod::Object) -> &Pod {
    PodSerializer::serialize(Cursor::new(&mut *buffer), &pod::Value::Object(object)).unwrap();
    Pod::from_bytes(buffer).unwrap()
}

fn find_preferred_modifier(
    gbm: &GbmDevice<DrmDeviceFd>,
    size: Size<u32, Physical>,
    fourcc: Fourcc,
    modifiers: Vec<i64>,
) -> anyhow::Result<(Modifier, usize)> {
    debug!("find_preferred_modifier: size={size:?}, fourcc={fourcc}, modifiers={modifiers:?}");

    let (buffer, modifier) = allocate_buffer(gbm, size, fourcc, &modifiers)?;

    let dmabuf = buffer
        .export()
        .context("error exporting GBM buffer object as dmabuf")?;
    let plane_count = dmabuf.num_planes();

    // FIXME: Ideally this also needs to try binding the dmabuf for rendering.

    Ok((modifier, plane_count))
}

fn allocate_buffer(
    gbm: &GbmDevice<DrmDeviceFd>,
    size: Size<u32, Physical>,
    fourcc: Fourcc,
    modifiers: &[i64],
) -> anyhow::Result<(GbmBuffer, Modifier)> {
    let (w, h) = (size.w, size.h);
    let flags = GbmBufferFlags::RENDERING;

    if modifiers.len() == 1 && Modifier::from(modifiers[0] as u64) == Modifier::Invalid {
        let bo = gbm
            .create_buffer_object::<()>(w, h, fourcc, flags)
            .context("error creating GBM buffer object")?;

        let buffer = GbmBuffer::from_bo(bo, true);
        Ok((buffer, Modifier::Invalid))
    } else {
        let modifiers = modifiers
            .iter()
            .map(|m| Modifier::from(*m as u64))
            .filter(|m| *m != Modifier::Invalid);

        let bo = gbm
            .create_buffer_object_with_modifiers2::<()>(w, h, fourcc, modifiers, flags)
            .context("error creating GBM buffer object")?;

        let modifier = bo.modifier();
        let buffer = GbmBuffer::from_bo(bo, false);
        Ok((buffer, modifier))
    }
}

fn allocate_dmabuf(
    gbm: &GbmDevice<DrmDeviceFd>,
    size: Size<u32, Physical>,
    fourcc: Fourcc,
    modifier: Modifier,
) -> anyhow::Result<Dmabuf> {
    let (buffer, _modifier) = allocate_buffer(gbm, size, fourcc, &[u64::from(modifier) as i64])?;
    let dmabuf = buffer
        .export()
        .context("error exporting GBM buffer object as dmabuf")?;
    Ok(dmabuf)
}

unsafe fn return_unused_buffer(stream: &Stream, pw_buffer: NonNull<pw_buffer>) {
    // pw_stream_return_buffer() requires too new PipeWire (1.4.0). So, mark as
    // corrupted and queue.
    let pw_buffer = pw_buffer.as_ptr();
    let spa_buffer = (*pw_buffer).buffer;
    let chunk = (*(*spa_buffer).datas).chunk;
    // Some (older?) consumers will check for size == 0 instead of the CORRUPTED flag.
    (*chunk).size = 0;
    (*chunk).flags = SPA_CHUNK_FLAG_CORRUPTED as i32;

    if let Some(header) = find_meta_header(spa_buffer) {
        let header = header.as_ptr();
        (*header).flags = SPA_META_HEADER_FLAG_CORRUPTED;
    }

    pw_stream_queue_buffer(stream.as_raw_ptr(), pw_buffer);
}

unsafe fn mark_buffer_as_good(pw_buffer: NonNull<pw_buffer>, sequence: &mut u64) {
    let pw_buffer = pw_buffer.as_ptr();
    let spa_buffer = (*pw_buffer).buffer;
    let chunk = (*(*spa_buffer).datas).chunk;

    // With DMA-BUFs, consumers should ignore the size field, and producers are allowed
    // to set it to 0.
    //
    // https://docs.pipewire.org/page_dma_buf.html
    //
    // However, OBS checks for size != 0 as a workaround for old compositor versions,
    // so we set it to 1.
    (*chunk).size = 1;
    // Clear the corrupted flag we may have set before.
    (*chunk).flags = SPA_CHUNK_FLAG_NONE as i32;

    *sequence = sequence.wrapping_add(1);
    if let Some(header) = find_meta_header(spa_buffer) {
        let header = header.as_ptr();
        // Clear the corrupted flag we may have set before.
        (*header).flags = 0;
        (*header).seq = *sequence;

        // Set buffer timestamp as unknown.
        //
        // FIXME: we could try passing real presentation timestamps for rendered frames here.
        // However, then we must also ensure that the time base never jumps (e.g. when switching a
        // dynamic cast between outputs) as this would mess up the timing downstream.
        (*header).pts = -1;
    }
}

unsafe fn find_meta_header(buffer: *mut spa_buffer) -> Option<NonNull<spa_meta_header>> {
    let p = spa_buffer_find_meta_data(buffer, SPA_META_Header, size_of::<spa_meta_header>()).cast();
    NonNull::new(p)
}

unsafe fn add_invisible_cursor(spa_buffer: *mut spa_buffer) {
    unsafe {
        let cursor_meta_ptr: *mut spa_meta_cursor = spa_buffer_find_meta_data(
            spa_buffer,
            SPA_META_Cursor,
            mem::size_of::<spa_meta_cursor>(),
        )
        .cast();
        let Some(cursor_meta) = cursor_meta_ptr.as_mut() else {
            return;
        };

        // The cursor is present but invisible.
        cursor_meta.id = 1;
        cursor_meta.position.x = 0;
        cursor_meta.position.y = 0;
        cursor_meta.hotspot.x = 0;
        cursor_meta.hotspot.y = 0;
        cursor_meta.bitmap_offset = BITMAP_META_OFFSET as _;

        let bitmap_meta_ptr = cursor_meta_ptr
            .byte_add(BITMAP_META_OFFSET)
            .cast::<spa_meta_bitmap>();
        let bitmap_meta = &mut *bitmap_meta_ptr;

        // HACK: PipeWire docs say offset = 0 means invisible.
        //
        // Unfortunately, OBS doesn't actually check that, instead it checks that size isn't zero:
        // https://github.com/obsproject/obs-studio/blob/f4aaa5f0417c5ec40a3799551e125129fce1e007/plugins/linux-pipewire/pipewire.c#L900
        //
        // Unfortunately, libwebrtc, on top of ignoring offset, also treats size = 0 as "preserve
        // previous cursor":
        // https://webrtc.googlesource.com/src/+/97b46e12582606a238d4f0c8524365cf5bdcb411/modules/desktop_capture/linux/wayland/shared_screencast_stream.cc#765
        //
        // So, send a 1x1 transparent pixel instead...
        bitmap_meta.offset = BITMAP_DATA_OFFSET as _;
        bitmap_meta.size.width = 1;
        bitmap_meta.size.height = 1;
        bitmap_meta.stride = CURSOR_BPP as i32;
        bitmap_meta.format = CURSOR_FORMAT;

        let bitmap_data = bitmap_meta_ptr.cast::<u8>().add(BITMAP_DATA_OFFSET);
        let bitmap_slice = slice::from_raw_parts_mut(bitmap_data, CURSOR_BITMAP_SIZE);
        bitmap_slice[..4].copy_from_slice(&[0, 0, 0, 0]);
    }
}

unsafe fn add_cursor_metadata(
    spa_buffer: *mut spa_buffer,
    location: Point<i32, Physical>,
    cache: &CursorBitmapCache,
) {
    // Request the whole advertised metadata allocation before constructing a
    // slice. A short or absent allocation is left untouched.
    let metadata = unsafe {
        spa_buffer_find_meta_data(spa_buffer, SPA_META_Cursor, CURSOR_META_SIZE).cast::<u8>()
    };
    if metadata.is_null() {
        return;
    }
    let bytes = unsafe { slice::from_raw_parts_mut(metadata, CURSOR_META_SIZE) };
    if let Err(error) = write_cursor_metadata(bytes, location, cache.bitmap.as_ref()) {
        warn!("error writing cached cursor metadata: {error}");
    }
}

fn write_cursor_metadata(
    bytes: &mut [u8],
    location: Point<i32, Physical>,
    bitmap: Option<&CursorBitmap>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        bytes.len() >= CURSOR_META_SIZE,
        "short cursor metadata allocation"
    );
    let (geometry, pixels) = match bitmap {
        Some(bitmap) => {
            anyhow::ensure!(
                cursor_bitmap_len(bitmap.geometry.size) == Some(bitmap.pixels.len()),
                "invalid cursor bitmap length"
            );
            (&bitmap.geometry, bitmap.pixels.as_slice())
        }
        None => (
            &CursorBitmapGeometry {
                size: Size::new(1, 1),
                hotspot: Point::from((0, 0)),
            },
            &[0, 0, 0, 0][..],
        ),
    };
    let mut cursor: spa_meta_cursor = unsafe { mem::zeroed() };
    cursor.id = 1;
    cursor.position.x = location.x;
    cursor.position.y = location.y;
    cursor.hotspot.x = geometry.hotspot.x;
    cursor.hotspot.y = geometry.hotspot.y;
    cursor.bitmap_offset = BITMAP_META_OFFSET as _;
    let mut header: spa_meta_bitmap = unsafe { mem::zeroed() };
    header.offset = BITMAP_DATA_OFFSET as _;
    header.size.width = geometry.size.w as _;
    header.size.height = geometry.size.h as _;
    header.stride = geometry.size.w * CURSOR_BPP as i32;
    header.format = CURSOR_FORMAT;
    // Use unaligned writes so validation/tests do not depend on byte-slice
    // alignment; no pointers escape this owner-thread metadata write.
    unsafe {
        bytes
            .as_mut_ptr()
            .cast::<spa_meta_cursor>()
            .write_unaligned(cursor);
        bytes
            .as_mut_ptr()
            .add(BITMAP_META_OFFSET)
            .cast::<spa_meta_bitmap>()
            .write_unaligned(header);
    }
    let data_offset = BITMAP_META_OFFSET + BITMAP_DATA_OFFSET;
    bytes[data_offset..data_offset + pixels.len()].copy_from_slice(pixels);
    Ok(())
}

#[cfg(test)]
mod cursor_bitmap_tests {
    use smithay::output::{PhysicalProperties, Subpixel};

    use super::*;

    fn output(name: &str) -> Output {
        Output::new(
            name.into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: String::new(),
                model: String::new(),
                serial_number: String::new(),
            },
        )
    }

    fn geometry(width: i32, hotspot: (i32, i32)) -> CursorBitmapGeometry {
        CursorBitmapGeometry {
            size: (width, 2).into(),
            hotspot: hotspot.into(),
        }
    }

    fn complete_next(cache: &mut CursorBitmapCache, fill: u8) {
        let (generation, geometry) = cache.begin().expect("one desired bitmap");
        let pixels = vec![fill; cursor_bitmap_len(geometry.size).unwrap()];
        assert!(cache.complete(generation, geometry, Ok(pixels)));
    }

    #[test]
    fn one_pending_job_coalesces_animated_shapes_without_starving_visibility() {
        let output = output("fast");
        let mut cache = CursorBitmapCache::default();
        cache.observe(&output, 1.0.into(), geometry(2, (1, 1)), true);
        let (old, old_geometry) = cache.begin().unwrap();
        for shape in 3..100 {
            cache.observe(&output, 1.0.into(), geometry(shape, (shape - 1, 0)), true);
            assert!(
                cache.begin().is_none(),
                "pending work is bounded to one job"
            );
        }
        assert!(cache.complete(old, old_geometry, Ok(vec![7; 16])));
        assert_eq!(cache.bitmap.as_ref().unwrap().geometry, geometry(2, (1, 1)));
        assert_eq!(cache.bitmap.as_ref().unwrap().pixels[0], 7);
        assert!(
            cache.needs_readback,
            "latest animated shape remains desired"
        );
        complete_next(&mut cache, 9);
        let bitmap = cache.bitmap.as_ref().unwrap();
        assert_eq!(bitmap.geometry, geometry(99, (98, 0)));
        assert!(bitmap.pixels.iter().all(|p| *p == 9));
        assert!(!cache.complete(old, geometry(2, (1, 1)), Ok(vec![7; 16])));
        assert_eq!(cache.bitmap.as_ref().unwrap().pixels[0], 9);
    }

    #[test]
    fn hidden_cancels_visibility_immediately_without_releasing_inflight_capacity() {
        let output = output("main");
        let mut cache = CursorBitmapCache::default();
        cache.observe(&output, 1.0.into(), geometry(2, (1, 0)), true);
        complete_next(&mut cache, 8);
        cache.observe(&output, 1.0.into(), geometry(3, (2, 0)), true);
        let (pending, pending_geometry) = cache.begin().unwrap();
        cache.observe(
            &output,
            1.0.into(),
            CursorBitmapGeometry {
                size: (0, 0).into(),
                hotspot: (0, 0).into(),
            },
            true,
        );
        assert!(cache.bitmap.is_none());
        assert!(!cache.needs_readback);
        assert_eq!(cache.pending, Some(pending));
        assert!(cache.complete(pending, pending_geometry, Ok(vec![8; 24])));
        assert!(cache.bitmap.is_none());
        assert!(cache.begin().is_none());
        let mut bytes = vec![222; CURSOR_META_SIZE];
        write_cursor_metadata(&mut bytes, (40, 70).into(), cache.bitmap.as_ref()).unwrap();
        let bitmap = unsafe {
            bytes
                .as_ptr()
                .add(BITMAP_META_OFFSET)
                .cast::<spa_meta_bitmap>()
                .read_unaligned()
        };
        assert_eq!(
            (bitmap.size.width, bitmap.size.height, bitmap.stride),
            (1, 1, 4)
        );
        assert_eq!(
            &bytes[BITMAP_META_OFFSET + BITMAP_DATA_OFFSET..][..4],
            &[0, 0, 0, 0]
        );
    }

    #[test]
    fn output_scale_and_resize_invalidation_keep_old_jobs_from_publishing() {
        let first = output("same-name");
        let replacement = output("same-name");
        let mut cache = CursorBitmapCache::default();
        cache.observe(&first, 1.0.into(), geometry(2, (1, 1)), true);
        complete_next(&mut cache, 2);
        cache.observe(&first, 1.0.into(), geometry(3, (2, 1)), true);
        let (pending, pending_geometry) = cache.begin().unwrap();
        cache.observe(&replacement, 2.0.into(), geometry(4, (3, 1)), false);
        assert!(cache.bitmap.is_none());
        assert!(cache.begin().is_none());
        assert!(cache.complete(pending, pending_geometry, Ok(vec![3; 24])));
        complete_next(&mut cache, 4);
        assert_eq!(cache.bitmap.as_ref().unwrap().geometry, geometry(4, (3, 1)));
        cache.observe(&replacement, 2.0.into(), geometry(5, (4, 1)), true);
        let (pending, pending_geometry) = cache.begin().unwrap();
        cache.invalidate();
        assert!(cache.complete(pending, pending_geometry, Ok(vec![5; 40])));
        assert!(cache.pending.is_none());
        assert!(cache.bitmap.is_none());
        assert!(cache.begin().is_none());
    }

    #[test]
    fn failed_transfer_and_busy_capacity_preserve_matching_previous_hotspot() {
        let output = output("main");
        let mut cache = CursorBitmapCache::default();
        cache.observe(&output, 1.0.into(), geometry(2, (1, 0)), true);
        complete_next(&mut cache, 6);
        cache.delivery_pending = false;
        cache.observe(&output, 1.0.into(), geometry(4, (3, 1)), true);
        // Quota exhaustion does not call begin, so the newest desired image
        // remains retriable without erasing a coherent visible bitmap.
        assert!(cache.needs_readback);
        assert!(cache.pending.is_none());
        assert_eq!(
            cache.bitmap.as_ref().unwrap().geometry.hotspot,
            (1, 0).into()
        );
        let (pending, pending_geometry) = cache.begin().unwrap();
        assert!(!cache.complete(
            pending,
            pending_geometry.clone(),
            Err("replaced context".into())
        ));
        assert!(cache.needs_readback);
        assert_eq!(cache.bitmap.as_ref().unwrap().geometry, geometry(2, (1, 0)));
        let (pending, pending_geometry) = cache.begin().unwrap();
        assert!(!cache.complete(pending, pending_geometry, Ok(vec![0; 1])));
        assert!(cache.needs_readback);
        complete_next(&mut cache, 7);
        assert_eq!(cache.bitmap.as_ref().unwrap().geometry, geometry(4, (3, 1)));
    }

    #[test]
    fn metadata_offsets_bounds_hotspot_and_cached_retransmission_are_exact() {
        let bitmap = CursorBitmap {
            geometry: geometry(3, (2, 1)),
            pixels: (0..24).collect(),
        };
        let mut allocation = vec![199; CURSOR_META_SIZE + 16];
        // Deliberately misalign the byte slice and retain prefix/suffix guards.
        write_cursor_metadata(
            &mut allocation[1..1 + CURSOR_META_SIZE],
            (71, 82).into(),
            Some(&bitmap),
        )
        .unwrap();
        assert_eq!(allocation[0], 199);
        assert!(allocation[1 + CURSOR_META_SIZE..].iter().all(|p| *p == 199));
        let data = &allocation[1..1 + CURSOR_META_SIZE];
        let cursor = unsafe { data.as_ptr().cast::<spa_meta_cursor>().read_unaligned() };
        assert_eq!(cursor.id, 1);
        assert_eq!((cursor.position.x, cursor.position.y), (71, 82));
        assert_eq!((cursor.hotspot.x, cursor.hotspot.y), (2, 1));
        assert_eq!(cursor.bitmap_offset as usize, BITMAP_META_OFFSET);
        assert!(cursor.bitmap_offset > 0);
        let header = unsafe {
            data.as_ptr()
                .add(cursor.bitmap_offset as usize)
                .cast::<spa_meta_bitmap>()
                .read_unaligned()
        };
        assert_eq!(header.offset as usize, BITMAP_DATA_OFFSET);
        assert!(header.offset > 0);
        assert_eq!(
            (header.size.width, header.size.height, header.stride),
            (3, 2, 12)
        );
        assert_eq!(header.format, CURSOR_FORMAT);
        let offset = cursor.bitmap_offset as usize + header.offset as usize;
        assert_eq!(&data[offset..offset + 24], bitmap.pixels.as_slice());
        let mut later = vec![55; CURSOR_META_SIZE];
        write_cursor_metadata(&mut later, (72, 83).into(), Some(&bitmap)).unwrap();
        assert_eq!(&later[offset..offset + 24], bitmap.pixels.as_slice());
        let mut short = vec![200; CURSOR_META_SIZE - 1];
        assert!(write_cursor_metadata(&mut short, (0, 0).into(), Some(&bitmap)).is_err());
        assert!(short.iter().all(|p| *p == 200));
        let invalid = CursorBitmap {
            geometry: geometry(385, (0, 0)),
            pixels: vec![0; 385 * 8],
        };
        assert!(write_cursor_metadata(&mut later, (0, 0).into(), Some(&invalid)).is_err());
        assert!(cursor_bitmap_len((i32::MAX, i32::MAX).into()).is_none());
        let mut invalid_size: Size<i32, Physical> = (0, 1).into();
        invalid_size.w = -1;
        assert!(cursor_bitmap_len(invalid_size).is_none());
        assert_eq!(
            cursor_bitmap_len((384, 384).into()),
            Some(CURSOR_BITMAP_SIZE)
        );
    }

    #[test]
    fn dropped_cast_weak_completion_releases_shared_capacity_without_publication() {
        let slots = crate::platform::capture::CaptureSlots::default();
        let cache = Rc::new(RefCell::new(CursorBitmapCache::default()));
        let output = output("main");
        cache
            .borrow_mut()
            .observe(&output, 1.0.into(), geometry(2, (1, 1)), true);
        let (generation, geometry) = cache.borrow_mut().begin().unwrap();
        let completion = CursorReadbackCompletion {
            cache: Rc::downgrade(&cache),
            generation,
            geometry,
            _permit: slots.acquire().unwrap(),
        };
        let other = slots.acquire().unwrap();
        assert!(slots.acquire().is_err());
        drop(cache);
        assert!(!completion.complete(Ok(vec![4; 16])));
        let reused = slots
            .acquire()
            .expect("completed obsolete transfer releases quota");
        drop((other, reused));
    }

    #[test]
    fn lock_invalidation_drops_completed_client_pixels_and_rejects_pending_client_shape() {
        let output = output("main");
        let mut cache = CursorBitmapCache::default();
        cache.observe(&output, 1.0.into(), geometry(2, (1, 1)), true);
        complete_next(&mut cache, 55);
        cache.observe(&output, 1.0.into(), geometry(3, (2, 1)), true);
        let (pending, client_geometry) = cache.begin().unwrap();

        // start_lock invalidates both pending captures and completed cursor
        // caches before any lock scene can expose a pre-lock client bitmap.
        cache.invalidate();
        assert!(cache.bitmap.is_none());
        assert_eq!(cache.pending, Some(pending));
        let mut metadata = vec![17; CURSOR_META_SIZE];
        write_cursor_metadata(&mut metadata, (50, 60).into(), cache.bitmap.as_ref()).unwrap();
        assert_eq!(
            &metadata[BITMAP_META_OFFSET + BITMAP_DATA_OFFSET..][..4],
            &[0; 4]
        );

        cache.observe(&output, 1.0.into(), geometry(2, (0, 0)), true);
        assert!(
            cache.begin().is_none(),
            "old GPU work still owns bounded capacity"
        );
        assert!(cache.complete(pending, client_geometry.clone(), Ok(vec![99; 24])));
        assert!(
            cache.bitmap.is_none(),
            "pre-lock client content cannot publish"
        );
        complete_next(&mut cache, 7);
        assert_eq!(cache.bitmap.as_ref().unwrap().geometry, geometry(2, (0, 0)));
        assert_eq!(cache.bitmap.as_ref().unwrap().pixels, vec![7; 16]);
        assert!(!cache.complete(pending, client_geometry, Ok(vec![99; 24])));
        assert_eq!(cache.bitmap.as_ref().unwrap().pixels, vec![7; 16]);
    }

    #[test]
    #[ignore = "requires a DRM render node and EGL fences"]
    fn deferred_cursor_pixels_match_bgra_channels_rows_and_metadata_geometry() {
        use std::fs::OpenOptions;
        use std::sync::Arc;

        use smithay::backend::allocator::gbm::GbmDevice;
        use smithay::backend::egl::{EGLContext, EGLDisplay};
        use smithay::backend::renderer::element::memory::{
            MemoryRenderBuffer, MemoryRenderBufferRenderElement,
        };
        use smithay::backend::renderer::element::Kind;
        use smithay::backend::renderer::gles::GlesRenderer;

        use crate::platform::gles_readback::GlesReadbackPoll;

        let node = std::env::var("CLARION_GLES_TEST_NODE").expect("set CLARION_GLES_TEST_NODE");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&node)
            .unwrap();
        let display = unsafe { EGLDisplay::new(GbmDevice::new(Arc::new(file)).unwrap()).unwrap() };
        let context = EGLContext::new(&display).unwrap();
        let mut renderer = unsafe { GlesRenderer::new(context).unwrap() };
        let source: Vec<u8> = (0..4u8)
            .flat_map(|y| (0..5u8).flat_map(move |x| [20 * x + 1, 30 * y + 2, x + 5 * y + 3, 255]))
            .collect();
        let buffer = MemoryRenderBuffer::from_slice(
            &source,
            Fourcc::Argb8888,
            (5, 4),
            1,
            Transform::Normal,
            None,
        );
        let element = MemoryRenderBufferRenderElement::from_buffer(
            &mut renderer,
            (0., 0.),
            &buffer,
            None,
            None,
            None,
            Kind::Cursor,
        )
        .unwrap();
        let (texture, _sync) = render_to_texture(
            &mut renderer,
            (5, 4).into(),
            1.0.into(),
            Transform::Normal,
            Fourcc::Argb8888,
            std::iter::once(element),
        )
        .unwrap();
        let mut job = match renderer
            .enqueue_pixels(
                &texture,
                smithay::utils::Rectangle::from_size(texture.size()),
                Fourcc::Argb8888,
            )
            .unwrap()
        {
            PendingPixels::Gles(job) => job,
            _ => panic!("the test requires genuine deferred GLES fencing"),
        };
        let pixels = loop {
            match job.poll(&mut renderer).unwrap() {
                GlesReadbackPoll::Pending => std::thread::sleep(Duration::from_millis(1)),
                GlesReadbackPoll::Ready(pixels) => break pixels,
            }
        };
        verify_cursor_pixels(source, pixels);
    }

    fn verify_cursor_pixels(source: Vec<u8>, pixels: Vec<u8>) {
        assert_eq!(pixels, source, "cursor BGRA channels or rows changed");
        let bitmap = CursorBitmap {
            geometry: CursorBitmapGeometry {
                size: (5, 4).into(),
                hotspot: (3, 2).into(),
            },
            pixels,
        };
        let mut bytes = vec![0; CURSOR_META_SIZE];
        write_cursor_metadata(&mut bytes, (70, 90).into(), Some(&bitmap)).unwrap();
        let cursor = unsafe { bytes.as_ptr().cast::<spa_meta_cursor>().read_unaligned() };
        let header = unsafe {
            bytes
                .as_ptr()
                .add(cursor.bitmap_offset as usize)
                .cast::<spa_meta_bitmap>()
                .read_unaligned()
        };
        assert_eq!((cursor.hotspot.x, cursor.hotspot.y), (3, 2));
        assert_eq!(
            (header.size.width, header.size.height, header.stride),
            (5, 4, 20)
        );
        assert_eq!(
            &bytes[BITMAP_META_OFFSET + BITMAP_DATA_OFFSET..][..source.len()],
            source.as_slice()
        );
    }

    #[test]
    #[ignore = "requires a real DRM render node and Vulkan readback"]
    fn native_deferred_cursor_pixels_match_bgra_channels_rows_and_metadata_geometry() {
        use smithay::backend::renderer::element::memory::{
            MemoryRenderBuffer, MemoryRenderBufferRenderElement,
        };
        use smithay::backend::renderer::element::Kind;

        use crate::render_helpers::vulkan::native::NativeRenderer;

        let (vulkan, _reference, _allocator) =
            crate::render_helpers::vulkan::hardware_tests::setup();
        let mut renderer = NativeRenderer::new(vulkan).unwrap();
        renderer.set_completion_export_required(false);
        // Cold-target admission has its own regression. This fixture exercises
        // deferred cursor pixels after the real worker prepares BGRA and RGBA.
        renderer
            .prepare_output_format_for_test(Fourcc::Argb8888)
            .unwrap();
        let source: Vec<u8> = (0..4u8)
            .flat_map(|y| (0..5u8).flat_map(move |x| [20 * x + 1, 30 * y + 2, x + 5 * y + 3, 255]))
            .collect();
        let buffer = MemoryRenderBuffer::from_slice(
            &source,
            Fourcc::Argb8888,
            (5, 4),
            1,
            Transform::Normal,
            None,
        );
        let element = MemoryRenderBufferRenderElement::from_buffer(
            &mut renderer,
            (0., 0.),
            &buffer,
            None,
            None,
            None,
            Kind::Cursor,
        )
        .unwrap();
        let (texture, _sync) = render_to_texture(
            &mut renderer,
            (5, 4).into(),
            1.0.into(),
            Transform::Normal,
            Fourcc::Argb8888,
            std::iter::once(element),
        )
        .unwrap();
        let job = match renderer
            .enqueue_pixels(
                &texture,
                smithay::utils::Rectangle::from_size(texture.size()),
                Fourcc::Argb8888,
            )
            .unwrap()
        {
            PendingPixels::Native(job) => job,
            _ => panic!("native capture must return its asynchronous device readback"),
        };
        let pixels = std::thread::spawn(move || job.finish().unwrap())
            .join()
            .unwrap();
        verify_cursor_pixels(source, pixels);
    }
}
