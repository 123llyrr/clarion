//! Parametric Wayland color management for the native color-managed renderer.
//!
//! Descriptions are immutable records. Surface state is committed through
//! Smithay's cache, so synchronized subsurfaces and blocked commits retain the
//! same atomicity as their buffers. ICC and arbitrary primary/TF definitions are
//! intentionally not advertised until their renderer paths exist.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use smithay::output::{Output, WeakOutput};
use smithay::reexports::wayland_protocols::wp::color_management::v1::server::{
    wp_color_management_output_v1::{self, WpColorManagementOutputV1},
    wp_color_management_surface_feedback_v1::{self, WpColorManagementSurfaceFeedbackV1},
    wp_color_management_surface_v1::{self, WpColorManagementSurfaceV1},
    wp_color_manager_v1::{self, WpColorManagerV1},
    wp_image_description_creator_params_v1::{self, WpImageDescriptionCreatorParamsV1},
    wp_image_description_info_v1::WpImageDescriptionInfoV1,
    wp_image_description_reference_v1::WpImageDescriptionReferenceV1,
    wp_image_description_v1::{self, WpImageDescriptionV1},
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum, Weak,
};
use smithay::wayland::compositor::{
    is_sync_subsurface, with_states, with_surface_tree_upward, Cacheable, SurfaceData,
    TraversalAction,
};
use smithay::wayland::{Dispatch2, GlobalDispatch2};

use crate::color::{ColorDescription, Primaries, RenderingIntent, SurfaceColor, TransferFunction};

const VERSION: u32 = 3;
static NEXT_IDENTITY: AtomicU64 = AtomicU64::new(1);

fn next_identity() -> u64 {
    NEXT_IDENTITY
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
            let next = id.checked_add(1)?;
            // Legacy v1 identities have only 32 bits; zero is reserved there
            // too. Skipping it also keeps the v3 64-bit identities unique.
            if next as u32 == 0 {
                next.checked_add(1)
            } else {
                Some(next)
            }
        })
        .expect("color description identity space exhausted")
}

/// State owned by the compositor. This global must only be installed when the
/// backend can honor the advertised descriptions on every output.
#[derive(Default)]
pub struct ColorManagementState {
    outputs: Vec<WpColorManagementOutputV1>,
    feedback: Vec<WpColorManagementSurfaceFeedbackV1>,
    information: Vec<(WpImageDescriptionInfoV1, Arc<ImageRecord>)>,
    removed_outputs: HashSet<WeakOutput>,
}

pub trait ColorManagementHandler {
    fn color_management_state(&mut self) -> &mut ColorManagementState;
    fn color_output_description(&self, output: &Output) -> ColorDescription;
    fn color_surface_preferred_description(&self, surface: &WlSurface) -> ColorDescription;
    /// Called when a new buffer interpretation is published, after preserving
    /// any old resize snapshot and before replacing the renderer's buffers.
    fn color_surface_changed(&mut self, surface: &WlSurface);
}

#[derive(Clone, Copy, Debug)]
pub struct ColorManagerData {
    hdr_supported: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColorSurfaceCachedState {
    pub color: Option<SurfaceColor>,
}

impl Cacheable for ColorSurfaceCachedState {
    fn commit(&mut self, _: &DisplayHandle) -> Self {
        *self
    }

    fn merge_into(self, into: &mut Self, _: &DisplayHandle) {
        *into = self;
    }
}

pub fn surface_color_description(states: &SurfaceData) -> SurfaceColor {
    states
        .data_map
        .get::<SurfaceAttachment>()
        .map(|attachment| *attachment.current.lock().unwrap())
        .unwrap_or_default()
}

pub fn surface_color_description_for_surface(surface: &WlSurface) -> SurfaceColor {
    with_states(surface, surface_color_description)
}

/// Publish the colors belonging to the renderer's next buffer state. Call this
/// after freezing old resize contents and immediately before Smithay's
/// `on_commit_buffer_handler`. Cached protocol state is already new when the
/// user commit handler runs; publishing it from a post-commit hook would instead
/// reinterpret the old buffer while that resize snapshot is being captured.
///
/// Smithay applies synchronized children before their parent and updates their
/// renderer buffers only with the non-synchronized parent. Keep color publication
/// on exactly that same boundary, including every descendant of that tree.
pub fn commit_surface_tree_color<D: ColorManagementHandler>(state: &mut D, surface: &WlSurface) {
    if is_sync_subsurface(surface) {
        return;
    }
    let mut changed = Vec::new();
    with_surface_tree_upward(
        surface,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |surface, states, _| {
            let Some(attachment) = states.data_map.get::<SurfaceAttachment>() else {
                return;
            };
            let color = states
                .cached_state
                .get::<ColorSurfaceCachedState>()
                .current()
                .color
                .unwrap_or_default();
            let mut previous = attachment.current.lock().unwrap();
            if *previous != color {
                *previous = color;
                changed.push(surface.clone());
            }
        },
        |_, _, _| true,
    );
    for surface in changed {
        state.color_surface_changed(&surface);
    }
}

#[derive(Default)]
struct SurfaceAttachment {
    attached: AtomicBool,
    current: Mutex<SurfaceColor>,
}

#[derive(Debug)]
pub struct ColorSurfaceData {
    surface: Weak<WlSurface>,
    hdr_supported: bool,
}

pub struct ColorOutputData {
    output: Option<WeakOutput>,
    alive: AtomicBool,
    image: Mutex<Arc<ImageRecord>>,
}

pub struct ColorFeedbackData {
    surface: Weak<WlSurface>,
    image: Mutex<Arc<ImageRecord>>,
}

#[derive(Debug)]
pub struct ImageDescriptionData {
    image: Option<Arc<ImageRecord>>,
    allow_information: bool,
}

pub struct ImageReferenceData {
    image: Arc<ImageRecord>,
    allow_information: bool,
}

#[derive(Debug)]
pub struct ImageInformationData;

/// Stores the exact protocol representation separately from the renderer's
/// compact description: the PQ container is 10000 nits even when its mastering
/// metadata says the content peaks at 1000 nits.
#[derive(Clone, Debug, PartialEq)]
struct ImageParameters {
    description: ColorDescription,
    container_min: f32,
    container_max: f32,
    reference: f32,
    target_primaries: [i32; 8],
    target_min: f32,
    target_max: f32,
    max_cll: Option<u32>,
    max_fall: Option<u32>,
}

#[derive(Debug)]
struct ImageRecord {
    identity: u64,
    parameters: ImageParameters,
}

impl ImageRecord {
    fn new(parameters: ImageParameters) -> Arc<Self> {
        Arc::new(Self {
            identity: next_identity(),
            parameters,
        })
    }

    fn from_description(description: ColorDescription) -> Arc<Self> {
        let container_max = if description.transfer == TransferFunction::Pq {
            description.min_luminance + 10000.0
        } else {
            description.min_luminance + description.linear_scale
        };
        Self::new(ImageParameters {
            description,
            container_min: description.min_luminance,
            container_max,
            reference: description.reference_luminance,
            target_primaries: primaries_coordinates(description.primaries),
            target_min: description.min_luminance,
            target_max: description.max_luminance,
            max_cll: Some(luminance_u32(description.max_luminance)),
            max_fall: Some(luminance_u32(description.max_fall)),
        })
    }
}

#[derive(Debug, Default)]
struct ParametricValues {
    transfer: Option<TransferFunction>,
    primaries: Option<Primaries>,
    luminances: Option<(u32, u32, u32)>,
    mastering_primaries: Option<[i32; 8]>,
    mastering_luminances: Option<(u32, u32)>,
    max_cll: Option<u32>,
    max_fall: Option<u32>,
}

#[derive(Debug)]
pub struct ParametricCreatorData {
    hdr_supported: bool,
    values: Mutex<ParametricValues>,
}

impl ColorManagementState {
    pub fn new<D>(display: &DisplayHandle, hdr_supported: bool) -> Self
    where
        D: GlobalDispatch<WpColorManagerV1, ColorManagerData> + 'static,
    {
        display
            .create_global::<D, WpColorManagerV1, _>(VERSION, ColorManagerData { hdr_supported });
        Self::default()
    }

    /// Update output descriptions and surface preferences after an output color
    /// mode changes or a surface moves to another output. No client references
    /// are retained after their protocol resources are destroyed.
    pub fn refresh<D: ColorManagementHandler>(state: &mut D) {
        let manager = state.color_management_state();
        manager.outputs.retain(Resource::is_alive);
        manager.feedback.retain(Resource::is_alive);
        manager
            .removed_outputs
            .retain(|output| output.upgrade().is_some());
        let outputs = manager.outputs.clone();
        let feedback = manager.feedback.clone();
        let information = std::mem::take(&mut manager.information);
        for (resource, image) in information {
            if resource.is_alive() {
                send_information(&resource, &image);
            }
        }
        for object in outputs {
            let data = object.data::<ColorOutputData>().unwrap();
            let Some(output) = data.output.as_ref().and_then(WeakOutput::upgrade) else {
                continue;
            };
            if !data.alive.load(Ordering::Acquire) {
                continue;
            }
            let description = state.color_output_description(&output);
            let mut image = data.image.lock().unwrap();
            if image.parameters.description != description {
                *image = ImageRecord::from_description(description);
                object.image_description_changed();
                if let Some(client) = object.client() {
                    for resource in output.client_outputs(&client) {
                        if resource.version() >= 2 {
                            resource.done();
                        }
                    }
                }
            }
        }
        for object in feedback {
            let data = object.data::<ColorFeedbackData>().unwrap();
            let Ok(surface) = data.surface.upgrade() else {
                continue;
            };
            let description = state.color_surface_preferred_description(&surface);
            let mut image = data.image.lock().unwrap();
            if image.parameters.description != description {
                *image = ImageRecord::from_description(description);
                send_preferred(&object, image.identity);
            }
        }
    }

    pub fn output_removed(&mut self, output: &Output) {
        self.removed_outputs.insert(output.downgrade());
        for object in &self.outputs {
            let data = object.data::<ColorOutputData>().unwrap();
            if data.output.as_ref().and_then(WeakOutput::upgrade).as_ref() == Some(output) {
                data.alive.store(false, Ordering::Release);
            }
        }
    }

    pub fn output_added(&mut self, output: &Output) {
        self.removed_outputs.remove(&output.downgrade());
    }
}

fn primaries_coordinates(primaries: Primaries) -> [i32; 8] {
    match primaries {
        Primaries::Srgb => [
            640000, 330000, 300000, 600000, 150000, 60000, 312700, 329000,
        ],
        Primaries::Bt2020 => [
            708000, 292000, 170000, 797000, 131000, 46000, 312700, 329000,
        ],
        Primaries::DisplayP3 => [
            680000, 320000, 265000, 690000, 150000, 60000, 312700, 329000,
        ],
    }
}

fn named_primaries(primaries: Primaries) -> wp_color_manager_v1::Primaries {
    match primaries {
        Primaries::Srgb => wp_color_manager_v1::Primaries::Srgb,
        Primaries::Bt2020 => wp_color_manager_v1::Primaries::Bt2020,
        Primaries::DisplayP3 => wp_color_manager_v1::Primaries::DisplayP3,
    }
}

fn transfer_name(
    transfer: TransferFunction,
    version: u32,
) -> wp_color_manager_v1::TransferFunction {
    match transfer {
        TransferFunction::Srgb if version >= 2 => {
            wp_color_manager_v1::TransferFunction::CompoundPower24
        }
        TransferFunction::Srgb => wp_color_manager_v1::TransferFunction::Srgb,
        TransferFunction::Gamma22 => wp_color_manager_v1::TransferFunction::Gamma22,
        TransferFunction::Linear => wp_color_manager_v1::TransferFunction::ExtLinear,
        TransferFunction::Pq => wp_color_manager_v1::TransferFunction::St2084Pq,
    }
}

fn luminance_u32(luminance: f32) -> u32 {
    luminance.round().clamp(0., u32::MAX as f32) as u32
}

fn send_preferred(resource: &WpColorManagementSurfaceFeedbackV1, identity: u64) {
    if resource.version() >= 2 {
        resource.preferred_changed2((identity >> 32) as u32, identity as u32);
    } else {
        resource.preferred_changed(identity as u32);
    }
}

fn send_ready(resource: &WpImageDescriptionV1, identity: u64) {
    if resource.version() >= 2 {
        resource.ready2((identity >> 32) as u32, identity as u32);
    } else {
        resource.ready(identity as u32);
    }
}

fn initialize_image<D>(
    data_init: &mut DataInit<'_, D>,
    id: New<WpImageDescriptionV1>,
    image: Arc<ImageRecord>,
    allow_information: bool,
) where
    D: Dispatch<WpImageDescriptionV1, ImageDescriptionData> + 'static,
{
    let identity = image.identity;
    let resource = data_init.init(
        id,
        ImageDescriptionData {
            image: Some(image),
            allow_information,
        },
    );
    send_ready(&resource, identity);
}

fn failed_image<D>(
    data_init: &mut DataInit<'_, D>,
    id: New<WpImageDescriptionV1>,
    cause: wp_image_description_v1::Cause,
    message: &str,
) where
    D: Dispatch<WpImageDescriptionV1, ImageDescriptionData> + 'static,
{
    let resource = data_init.init(
        id,
        ImageDescriptionData {
            image: None,
            allow_information: false,
        },
    );
    resource.failed(cause, message.to_owned());
}

impl<D> GlobalDispatch2<WpColorManagerV1, D> for ColorManagerData
where
    D: Dispatch<WpColorManagerV1, ColorManagerData> + 'static,
{
    fn bind(
        &self,
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WpColorManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let resource = data_init.init(resource, *self);
        for intent in [
            wp_color_manager_v1::RenderIntent::Perceptual,
            wp_color_manager_v1::RenderIntent::Relative,
            wp_color_manager_v1::RenderIntent::Absolute,
        ] {
            resource.supported_intent(intent);
        }
        for feature in [
            wp_color_manager_v1::Feature::Parametric,
            wp_color_manager_v1::Feature::SetLuminances,
            wp_color_manager_v1::Feature::SetMasteringDisplayPrimaries,
        ] {
            resource.supported_feature(feature);
        }
        if self.hdr_supported {
            resource.supported_feature(wp_color_manager_v1::Feature::ExtendedTargetVolume);
            resource.supported_feature(wp_color_manager_v1::Feature::WindowsScrgb);
            if resource.version() >= 3 {
                resource.supported_feature(wp_color_manager_v1::Feature::WindowsBt2100);
            }
        }
        for transfer in [TransferFunction::Srgb, TransferFunction::Gamma22] {
            resource.supported_tf_named(transfer_name(transfer, resource.version()));
        }
        for primaries in [Primaries::Srgb, Primaries::Bt2020, Primaries::DisplayP3] {
            resource.supported_primaries_named(named_primaries(primaries));
        }
        if self.hdr_supported {
            for transfer in [TransferFunction::Pq, TransferFunction::Linear] {
                resource.supported_tf_named(transfer_name(transfer, resource.version()));
            }
        }
        resource.done();
    }
}

impl<D> Dispatch2<WpColorManagerV1, D> for ColorManagerData
where
    D: ColorManagementHandler
        + Dispatch<WpColorManagementOutputV1, ColorOutputData>
        + Dispatch<WpColorManagementSurfaceV1, ColorSurfaceData>
        + Dispatch<WpColorManagementSurfaceFeedbackV1, ColorFeedbackData>
        + Dispatch<WpImageDescriptionCreatorParamsV1, ParametricCreatorData>
        + Dispatch<WpImageDescriptionV1, ImageDescriptionData>
        + 'static,
{
    fn request(
        &self,
        state: &mut D,
        _: &Client,
        manager: &WpColorManagerV1,
        request: wp_color_manager_v1::Request,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_color_manager_v1::Request::Destroy => {}
            wp_color_manager_v1::Request::GetOutput { id, output } => {
                let output = Output::from_resource(&output);
                let alive = output.as_ref().is_some_and(|output| {
                    !state
                        .color_management_state()
                        .removed_outputs
                        .contains(&output.downgrade())
                });
                let description = output
                    .as_ref()
                    .map(|output| state.color_output_description(output))
                    .unwrap_or_default();
                let object = data_init.init(
                    id,
                    ColorOutputData {
                        output: output.as_ref().map(Output::downgrade),
                        alive: AtomicBool::new(alive),
                        image: Mutex::new(ImageRecord::from_description(description)),
                    },
                );
                state.color_management_state().outputs.push(object);
            }
            wp_color_manager_v1::Request::GetSurface { id, surface } => {
                let attached = with_states(&surface, |states| {
                    states
                        .data_map
                        .insert_if_missing_threadsafe(SurfaceAttachment::default);
                    let data = states.data_map.get::<SurfaceAttachment>().unwrap();
                    data.attached.swap(true, Ordering::AcqRel)
                });
                if attached {
                    manager.post_error(
                        wp_color_manager_v1::Error::SurfaceExists,
                        "surface already has color management",
                    );
                    return;
                }
                data_init.init(
                    id,
                    ColorSurfaceData {
                        surface: surface.downgrade(),
                        hdr_supported: self.hdr_supported,
                    },
                );
            }
            wp_color_manager_v1::Request::GetSurfaceFeedback { id, surface } => {
                let image = ImageRecord::from_description(
                    state.color_surface_preferred_description(&surface),
                );
                let identity = image.identity;
                let object = data_init.init(
                    id,
                    ColorFeedbackData {
                        surface: surface.downgrade(),
                        image: Mutex::new(image),
                    },
                );
                send_preferred(&object, identity);
                state.color_management_state().feedback.push(object);
            }
            wp_color_manager_v1::Request::CreateParametricCreator { obj } => {
                data_init.init(
                    obj,
                    ParametricCreatorData {
                        hdr_supported: self.hdr_supported,
                        values: Mutex::new(ParametricValues::default()),
                    },
                );
            }
            wp_color_manager_v1::Request::CreateIccCreator { .. } => {
                manager.post_error(
                    wp_color_manager_v1::Error::UnsupportedFeature,
                    "ICC color profiles are not supported",
                );
            }
            wp_color_manager_v1::Request::CreateWindowsScrgb { image_description } => {
                if self.hdr_supported {
                    initialize_image(
                        data_init,
                        image_description,
                        ImageRecord::from_description(ColorDescription::windows_scrgb()),
                        false,
                    );
                } else {
                    manager.post_error(
                        wp_color_manager_v1::Error::UnsupportedFeature,
                        "extended linear rendering is not supported",
                    );
                }
            }
            wp_color_manager_v1::Request::CreateWindowsBt2100 { image_description } => {
                if self.hdr_supported {
                    initialize_image(
                        data_init,
                        image_description,
                        ImageRecord::from_description(ColorDescription::windows_bt2100()),
                        false,
                    );
                } else {
                    manager.post_error(
                        wp_color_manager_v1::Error::UnsupportedFeature,
                        "PQ rendering is not supported",
                    );
                }
            }
            wp_color_manager_v1::Request::GetImageDescription {
                image_description,
                reference,
            } => {
                if let Some(data) = reference.data::<ImageReferenceData>() {
                    initialize_image(
                        data_init,
                        image_description,
                        data.image.clone(),
                        data.allow_information,
                    );
                } else {
                    failed_image(
                        data_init,
                        image_description,
                        wp_image_description_v1::Cause::Unsupported,
                        "unknown image description reference",
                    );
                }
            }
            _ => unreachable!(),
        }
    }
}

impl<D: ColorManagementHandler> Dispatch2<WpColorManagementSurfaceV1, D> for ColorSurfaceData {
    fn request(
        &self,
        _: &mut D,
        _: &Client,
        resource: &WpColorManagementSurfaceV1,
        request: wp_color_management_surface_v1::Request,
        _: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        let Ok(surface) = self.surface.upgrade() else {
            if !matches!(request, wp_color_management_surface_v1::Request::Destroy) {
                resource.post_error(
                    wp_color_management_surface_v1::Error::Inert,
                    "wl_surface was destroyed",
                );
            }
            return;
        };
        let color = match request {
            wp_color_management_surface_v1::Request::Destroy => {
                with_states(&surface, |states| {
                    states
                        .data_map
                        .get::<SurfaceAttachment>()
                        .unwrap()
                        .attached
                        .store(false, Ordering::Release)
                });
                None
            }
            wp_color_management_surface_v1::Request::UnsetImageDescription => None,
            wp_color_management_surface_v1::Request::SetImageDescription {
                image_description,
                render_intent,
            } => {
                let intent = match render_intent {
                    WEnum::Value(wp_color_manager_v1::RenderIntent::Perceptual) => {
                        RenderingIntent::Perceptual
                    }
                    WEnum::Value(wp_color_manager_v1::RenderIntent::Relative) => {
                        RenderingIntent::Relative
                    }
                    WEnum::Value(wp_color_manager_v1::RenderIntent::Absolute) => {
                        RenderingIntent::Absolute
                    }
                    _ => {
                        resource.post_error(
                            wp_color_management_surface_v1::Error::RenderIntent,
                            "rendering intent was not advertised",
                        );
                        return;
                    }
                };
                let Some(image) = image_description
                    .data::<ImageDescriptionData>()
                    .and_then(|data| data.image.as_ref())
                else {
                    resource.post_error(
                        wp_color_management_surface_v1::Error::ImageDescription,
                        "image description is not ready",
                    );
                    return;
                };
                let description = image.parameters.description;
                debug_assert!(
                    self.hdr_supported
                        || !matches!(
                            description.transfer,
                            TransferFunction::Pq | TransferFunction::Linear
                        )
                );
                Some(SurfaceColor {
                    description,
                    intent,
                })
            }
            _ => unreachable!(),
        };
        with_states(&surface, |states| {
            states
                .cached_state
                .get::<ColorSurfaceCachedState>()
                .pending()
                .color = color
        });
    }
}

impl<D> Dispatch2<WpColorManagementOutputV1, D> for ColorOutputData
where
    D: Dispatch<WpImageDescriptionV1, ImageDescriptionData> + 'static,
{
    fn request(
        &self,
        _: &mut D,
        _: &Client,
        _: &WpColorManagementOutputV1,
        request: wp_color_management_output_v1::Request,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_color_management_output_v1::Request::Destroy => {}
            wp_color_management_output_v1::Request::GetImageDescription { image_description } => {
                if self.alive.load(Ordering::Acquire)
                    && self.output.as_ref().and_then(WeakOutput::upgrade).is_some()
                {
                    initialize_image(
                        data_init,
                        image_description,
                        self.image.lock().unwrap().clone(),
                        true,
                    );
                } else {
                    failed_image(
                        data_init,
                        image_description,
                        wp_image_description_v1::Cause::NoOutput,
                        "output was removed",
                    );
                }
            }
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch2<WpColorManagementSurfaceFeedbackV1, D> for ColorFeedbackData
where
    D: Dispatch<WpImageDescriptionV1, ImageDescriptionData> + 'static,
{
    fn request(
        &self,
        _: &mut D,
        _: &Client,
        resource: &WpColorManagementSurfaceFeedbackV1,
        request: wp_color_management_surface_feedback_v1::Request,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_color_management_surface_feedback_v1::Request::Destroy => {}
            wp_color_management_surface_feedback_v1::Request::GetPreferred {
                image_description,
            }
            | wp_color_management_surface_feedback_v1::Request::GetPreferredParametric {
                image_description,
            } => {
                if self.surface.upgrade().is_err() {
                    resource.post_error(
                        wp_color_management_surface_feedback_v1::Error::Inert,
                        "wl_surface was destroyed",
                    );
                } else {
                    initialize_image(
                        data_init,
                        image_description,
                        self.image.lock().unwrap().clone(),
                        true,
                    );
                }
            }
            _ => unreachable!(),
        }
    }
}

fn read_transfer(
    tf: WEnum<wp_color_manager_v1::TransferFunction>,
    version: u32,
    hdr_supported: bool,
) -> Option<TransferFunction> {
    match tf {
        WEnum::Value(wp_color_manager_v1::TransferFunction::Srgb) if version == 1 => {
            Some(TransferFunction::Srgb)
        }
        WEnum::Value(wp_color_manager_v1::TransferFunction::CompoundPower24) if version >= 2 => {
            Some(TransferFunction::Srgb)
        }
        WEnum::Value(wp_color_manager_v1::TransferFunction::Gamma22) => {
            Some(TransferFunction::Gamma22)
        }
        WEnum::Value(wp_color_manager_v1::TransferFunction::St2084Pq) if hdr_supported => {
            Some(TransferFunction::Pq)
        }
        WEnum::Value(wp_color_manager_v1::TransferFunction::ExtLinear) if hdr_supported => {
            Some(TransferFunction::Linear)
        }
        _ => None,
    }
}

fn read_primaries(primaries: WEnum<wp_color_manager_v1::Primaries>) -> Option<Primaries> {
    match primaries {
        WEnum::Value(wp_color_manager_v1::Primaries::Srgb) => Some(Primaries::Srgb),
        WEnum::Value(wp_color_manager_v1::Primaries::Bt2020) => Some(Primaries::Bt2020),
        WEnum::Value(wp_color_manager_v1::Primaries::DisplayP3) => Some(Primaries::DisplayP3),
        _ => None,
    }
}

fn set_once<T>(target: &mut Option<T>, value: T, resource: &WpImageDescriptionCreatorParamsV1) {
    if target.is_some() {
        resource.post_error(
            wp_image_description_creator_params_v1::Error::AlreadySet,
            "property was already set",
        );
    } else {
        *target = Some(value);
    }
}

impl ParametricValues {
    fn build(
        &self,
        version: u32,
    ) -> Result<ImageParameters, (wp_image_description_creator_params_v1::Error, &'static str)>
    {
        use wp_image_description_creator_params_v1::Error;
        let (Some(transfer), Some(primaries)) = (self.transfer, self.primaries) else {
            return Err((
                Error::IncompleteSet,
                "primaries and transfer function are required",
            ));
        };
        let (default_min, default_max, default_reference) = if transfer == TransferFunction::Pq {
            (0.005, 10000.005, 203.)
        } else {
            (0.2, 80., 80.)
        };
        let (container_min, container_max, reference) = self
            .luminances
            .map(|(min, max, reference)| {
                let min = min as f32 / 10000.;
                (
                    min,
                    if transfer == TransferFunction::Pq {
                        min + 10000.
                    } else {
                        max as f32
                    },
                    reference as f32,
                )
            })
            .unwrap_or((default_min, default_max, default_reference));
        if container_max <= container_min || reference <= container_min {
            return Err((
                Error::InvalidLuminance,
                "max luminance and reference white must exceed min luminance",
            ));
        }
        let (target_min, target_max) = self
            .mastering_luminances
            .map(|(min, max)| (min as f32 / 10000., max as f32))
            .unwrap_or((container_min, container_max));
        if target_max <= target_min {
            return Err((
                Error::InvalidLuminance,
                "mastering maximum must exceed its minimum",
            ));
        }
        if self
            .max_cll
            .zip(self.max_fall)
            .is_some_and(|(cll, fall)| fall > cll)
        {
            return Err((Error::InvalidLuminance, "maxFALL must not exceed maxCLL"));
        }
        if version == 1
            && [self.max_cll, self.max_fall]
                .into_iter()
                .flatten()
                .any(|value| value as f32 <= target_min || value as f32 > target_max)
        {
            return Err((
                Error::InvalidLuminance,
                "content light levels must fit the mastering range in protocol version 1",
            ));
        }
        let peak = self
            .max_cll
            .filter(|value| *value > 0)
            .map(|value| value as f32)
            .unwrap_or(target_max)
            .min(if transfer == TransferFunction::Pq {
                10000.
            } else {
                f32::MAX
            });
        let max_fall = self
            .max_fall
            .filter(|value| *value > 0)
            .map(|value| value as f32)
            .unwrap_or(peak);
        let description = ColorDescription {
            primaries,
            transfer,
            min_luminance: container_min,
            reference_luminance: reference,
            max_luminance: peak,
            max_fall,
            linear_scale: container_max - container_min,
        };
        Ok(ImageParameters {
            description,
            container_min,
            container_max,
            reference,
            target_primaries: self
                .mastering_primaries
                .unwrap_or_else(|| primaries_coordinates(primaries)),
            target_min,
            target_max,
            max_cll: self.max_cll,
            max_fall: self.max_fall,
        })
    }
}

impl<D> Dispatch2<WpImageDescriptionCreatorParamsV1, D> for ParametricCreatorData
where
    D: Dispatch<WpImageDescriptionV1, ImageDescriptionData> + 'static,
{
    fn request(
        &self,
        _: &mut D,
        _: &Client,
        resource: &WpImageDescriptionCreatorParamsV1,
        request: wp_image_description_creator_params_v1::Request,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use wp_image_description_creator_params_v1::{Error, Request};
        let mut values = self.values.lock().unwrap();
        match request {
            Request::Create { image_description } => match values.build(resource.version()) {
                Ok(parameters) if parameters.description.validate() => initialize_image(
                    data_init,
                    image_description,
                    ImageRecord::new(parameters),
                    false,
                ),
                Ok(_) => failed_image(
                    data_init,
                    image_description,
                    wp_image_description_v1::Cause::Unsupported,
                    "luminance range is outside the renderer's supported range",
                ),
                Err((error, message)) => resource.post_error(error, message),
            },
            Request::SetTfNamed { tf } => {
                if let Some(transfer) = read_transfer(tf, resource.version(), self.hdr_supported) {
                    set_once(&mut values.transfer, transfer, resource);
                } else {
                    resource.post_error(Error::InvalidTf, "transfer function was not advertised");
                }
            }
            Request::SetPrimariesNamed { primaries } => {
                if let Some(primaries) = read_primaries(primaries) {
                    set_once(&mut values.primaries, primaries, resource);
                } else {
                    resource.post_error(
                        Error::InvalidPrimariesNamed,
                        "primaries were not advertised",
                    );
                }
            }
            Request::SetLuminances {
                min_lum,
                max_lum,
                reference_lum,
            } => {
                if reference_lum as f64 <= min_lum as f64 / 10000. {
                    resource.post_error(
                        Error::InvalidLuminance,
                        "reference luminance must exceed minimum",
                    );
                } else {
                    set_once(
                        &mut values.luminances,
                        (min_lum, max_lum, reference_lum),
                        resource,
                    );
                }
            }
            Request::SetMasteringDisplayPrimaries {
                r_x,
                r_y,
                g_x,
                g_y,
                b_x,
                b_y,
                w_x,
                w_y,
            } => {
                set_once(
                    &mut values.mastering_primaries,
                    [r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y],
                    resource,
                );
            }
            Request::SetMasteringLuminance { min_lum, max_lum } => {
                if max_lum as f64 <= min_lum as f64 / 10000. {
                    resource.post_error(
                        Error::InvalidLuminance,
                        "mastering maximum must exceed minimum",
                    );
                } else {
                    set_once(
                        &mut values.mastering_luminances,
                        (min_lum, max_lum),
                        resource,
                    );
                }
            }
            Request::SetMaxCll { max_cll } => set_once(&mut values.max_cll, max_cll, resource),
            Request::SetMaxFall { max_fall } => set_once(&mut values.max_fall, max_fall, resource),
            Request::SetPrimaries { .. } | Request::SetTfPower { .. } => {
                resource.post_error(
                    Error::UnsupportedFeature,
                    "arbitrary primaries and power curves are not supported",
                );
            }
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch2<WpImageDescriptionV1, D> for ImageDescriptionData
where
    D: ColorManagementHandler + Dispatch<WpImageDescriptionInfoV1, ImageInformationData> + 'static,
{
    fn request(
        &self,
        state: &mut D,
        _: &Client,
        resource: &WpImageDescriptionV1,
        request: wp_image_description_v1::Request,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_image_description_v1::Request::Destroy => {}
            wp_image_description_v1::Request::GetInformation { information } => {
                let Some(image) = self.image.as_ref() else {
                    resource.post_error(
                        wp_image_description_v1::Error::NotReady,
                        "image description failed or is not ready",
                    );
                    return;
                };
                if !self.allow_information {
                    resource.post_error(
                        wp_image_description_v1::Error::NoInformation,
                        "client-created descriptions do not permit information requests",
                    );
                    return;
                }
                let info = data_init.init(information, ImageInformationData);
                // libwayland's Rust backend initializes new child userdata only
                // after this request returns. Sending the destructor event here
                // would free that child before initialization (use-after-free).
                state
                    .color_management_state()
                    .information
                    .push((info, image.clone()));
            }
            _ => unreachable!(),
        }
    }
}

fn send_information(info: &WpImageDescriptionInfoV1, image: &ImageRecord) {
    let params = &image.parameters;
    let description = params.description;
    let [r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y] = primaries_coordinates(description.primaries);
    info.primaries(r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y);
    info.primaries_named(named_primaries(description.primaries));
    info.tf_named(transfer_name(description.transfer, info.version()));
    info.luminances(
        luminance_u32(params.container_min * 10000.),
        luminance_u32(params.container_max),
        luminance_u32(params.reference),
    );
    let [r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y] = params.target_primaries;
    info.target_primaries(r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y);
    info.target_luminance(
        luminance_u32(params.target_min * 10000.),
        luminance_u32(params.target_max),
    );
    if let Some(value) = params.max_cll {
        info.target_max_cll(value);
    }
    if let Some(value) = params.max_fall {
        info.target_max_fall(value);
    }
    info.done();
}

impl<D> Dispatch2<WpImageDescriptionInfoV1, D> for ImageInformationData {
    fn request(
        &self,
        _: &mut D,
        _: &Client,
        _: &WpImageDescriptionInfoV1,
        _: <WpImageDescriptionInfoV1 as Resource>::Request,
        _: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        unreachable!()
    }
}

impl<D> Dispatch2<WpImageDescriptionReferenceV1, D> for ImageReferenceData {
    fn request(
        &self,
        _: &mut D,
        _: &Client,
        _: &WpImageDescriptionReferenceV1,
        _: <WpImageDescriptionReferenceV1 as Resource>::Request,
        _: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
    }
}
