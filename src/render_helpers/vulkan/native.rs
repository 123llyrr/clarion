//! Smithay renderer adapter backed exclusively by Vulkan commands.
//! Native sync files cover presentation; a completion worker retires GPU owners.
use super::{
    allocate_texture,
    completion::FrameCompletion,
    frame::VulkanFrame,
    pipeline::{MeshGpuVertex, MeshPushConstants, QuadPushConstants},
    VulkanRenderer, VulkanTexture,
};
use ash::vk;
use smithay::backend::allocator::{
    dmabuf::Dmabuf, format::FormatSet, Buffer as _, Format, Fourcc, Modifier,
};
use smithay::backend::renderer::{
    sync::SyncPoint,
    utils::{Buffer as WlBufferOwner, RendererSurfaceStateUserData},
    Bind, Color32F, ContextId, DebugFlags, ExportMem, Frame, ImportDma, ImportDmaWl, ImportMem,
    Offscreen, Renderer, RendererSuper, Texture, TextureFilter, TextureMapping,
};
#[cfg(test)]
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::{
    protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface},
    Resource,
};
use smithay::utils::{Buffer, Physical, Point, Rectangle, Size, Transform};
use smithay::wayland::{
    compositor::{with_states, with_surface_tree_downward, SurfaceData, TraversalAction},
    dmabuf::get_dmabuf,
};
use std::{
    marker::PhantomData,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
};

/// Only hardware regressions may stop the fixed preparer. Dropping this guard
/// always releases it, including while unwinding a failed assertion.
#[cfg(test)]
pub(crate) struct GpuPreparationGateForTest(Option<std::sync::mpsc::Sender<()>>);
#[cfg(test)]
impl Drop for GpuPreparationGateForTest {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}

struct PendingShaderCompile {
    kind: crate::render_helpers::shaders::ProgramType,
    source: String,
    generation: u64,
    cancel: Arc<AtomicBool>,
    prepared: Option<super::custom_shader::PreparedShader>,
    gpu_pending: bool,
}
impl Drop for PendingShaderCompile {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
struct ShaderCompileOwner {
    instance: u64,
    endpoint: Option<super::custom_shader::CompilerEndpoint>,
    completions: Option<
        calloop::channel::Channel<
            super::custom_shader::CompileCompletion<super::custom_shader::PreparedShader>,
        >,
    >,
    generation: u64,
    pending: Vec<PendingShaderCompile>,
    deferred: Vec<(crate::render_helpers::shaders::ProgramType, String)>,
    configured: std::collections::HashMap<u8, String>,
    fallback: std::collections::HashMap<u8, Arc<super::custom_shader::Program>>,
    gpu_endpoint: Option<super::custom_shader::gpu_preparation::Endpoint>,
    gpu_completions: Option<calloop::channel::Channel<super::custom_shader::GpuPreparationEvent>>,
    desired_formats: std::collections::HashSet<vk::Format>,
    format_pending: Option<(u64, Arc<AtomicBool>)>,
    format_error: Option<String>,
    failed_formats: std::collections::HashMap<vk::Format, String>,
    live_programs: Vec<Weak<super::custom_shader::Program>>,
    yuv_targets: Vec<YuvTargetRequest>,
    yuv_pending: Option<(u64, Arc<AtomicBool>)>,
}
struct YuvTargetRequest {
    variant: Weak<super::pipeline::YuvPipeline>,
    formats: std::collections::HashSet<vk::Format>,
}
impl std::fmt::Debug for ShaderCompileOwner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ShaderCompileOwner")
            .field("instance", &self.instance)
            .field("generation", &self.generation)
            .field("pending", &self.pending.len())
            .field("deferred", &self.deferred.len())
            .finish_non_exhaustive()
    }
}
impl ShaderCompileOwner {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let (endpoint, completions) = match super::custom_shader::compiler_endpoint() {
            Ok((endpoint, completions)) => (Some(endpoint), Some(completions)),
            Err(error) => {
                warn!("native shader compiler unavailable: {error}");
                (None, None)
            }
        };
        let (gpu_endpoint, gpu_completions) =
            match super::custom_shader::gpu_preparation::endpoint() {
                Ok((endpoint, completions)) => (Some(endpoint), Some(completions)),
                Err(error) => {
                    warn!("native GPU preparation unavailable: {error}");
                    (None, None)
                }
            };
        Self {
            instance: NEXT.fetch_add(1, Ordering::Relaxed),
            endpoint,
            completions,
            generation: 0,
            pending: Vec::new(),
            deferred: Vec::new(),
            configured: Default::default(),
            fallback: Default::default(),
            gpu_endpoint,
            gpu_completions,
            desired_formats: Default::default(),
            format_pending: None,
            format_error: None,
            failed_formats: Default::default(),
            live_programs: Vec::new(),
            yuv_targets: Vec::new(),
            yuv_pending: None,
        }
    }
}
impl Drop for ShaderCompileOwner {
    fn drop(&mut self) {
        if let Some((_, cancel)) = &self.format_pending {
            cancel.store(true, Ordering::Release);
        }
        if let Some((_, cancel)) = &self.yuv_pending {
            cancel.store(true, Ordering::Release);
        }
        // Pending shader guards cancel executing work without releasing its worker permit.
    }
}

fn register_yuv_target(
    owner: &mut ShaderCompileOwner,
    variant: &Arc<super::pipeline::YuvPipeline>,
    format: vk::Format,
) {
    let weak = Arc::downgrade(variant);
    if let Some(entry) = owner
        .yuv_targets
        .iter_mut()
        .find(|entry| entry.variant.ptr_eq(&weak))
    {
        entry.formats.insert(format);
    } else {
        owner.yuv_targets.push(YuvTargetRequest {
            variant: weak,
            formats: std::collections::HashSet::from([format]),
        });
    }
}

fn request_yuv_warm(owner: &mut ShaderCompileOwner) -> bool {
    use super::custom_shader::gpu_preparation::{Identity, Work, YUV_PREPARATION_KIND};
    if owner.yuv_pending.is_some() || owner.format_error.is_some() {
        return false;
    }
    owner
        .yuv_targets
        .retain(|target| target.variant.strong_count() != 0);
    let mut targets = Vec::new();
    for target in &owner.yuv_targets {
        let Some(variant) = target.variant.upgrade() else {
            continue;
        };
        let mut formats = Vec::new();
        for &format in &target.formats {
            if owner.failed_formats.contains_key(&format) {
                continue;
            }
            match variant.prepared_format(format) {
                Ok(None) => formats.push(format),
                Err(vk::Result::ERROR_DEVICE_LOST) => {
                    owner.format_error = Some("ERROR_DEVICE_LOST".into());
                    return false;
                }
                Ok(Some(_)) | Err(_) => {}
            }
        }
        if !formats.is_empty() {
            formats.sort_unstable_by_key(|format| format.as_raw());
            targets.push((variant, formats));
        }
    }
    if targets.is_empty() {
        return false;
    }
    let Some(endpoint) = &owner.gpu_endpoint else {
        owner.format_error = Some("GPU preparation worker unavailable".into());
        return false;
    };
    owner.generation = owner
        .generation
        .checked_add(1)
        .expect("YUV preparation generation exhausted");
    let generation = owner.generation;
    let cancel = Arc::new(AtomicBool::new(false));
    if let Err(rejected) = endpoint.request(
        Identity {
            renderer: owner.instance,
            kind: YUV_PREPARATION_KIND,
            generation,
            source: String::new(),
        },
        Work::YuvTargets { targets },
        cancel.clone(),
    ) {
        if rejected.unavailable {
            owner.format_error = Some("GPU preparation worker unavailable".into());
        }
        return false;
    }
    owner.yuv_pending = Some((generation, cancel));
    true
}

/// A surface-local reference to Smithay's current buffer. Smithay holds the
/// renderer state lock while calling ImportAll::import_buffer, so the Vulkan
/// importer reads this independent slot instead of trying to lock that state.
#[derive(Default)]
struct SurfaceBufferOwner(Arc<Mutex<Option<WlBufferOwner>>>);

pub(crate) fn record_surface_tree_buffer_owners(surface: &WlSurface) {
    let mut previous = Vec::new();
    // Smithay applies synchronized child commits when the parent commits.
    // Refresh every descendant after on_commit_buffer_handler has updated its
    // renderer state, before an import can observe the new buffer.
    with_surface_tree_downward(
        surface,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |_, states, _| {
            let current = states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|state| state.lock().unwrap().buffer().cloned());
            let slot = states.data_map.get_or_insert(SurfaceBufferOwner::default);
            previous.push(std::mem::replace(&mut *slot.0.lock().unwrap(), current));
        },
        |_, _, _| true,
    );
    // A Buffer's last drop can send wl_buffer.release. Do it outside the
    // SurfaceData and slot locks, including on a surface destruction path.
    drop(previous);
}

pub(crate) fn clear_surface_buffer_owner(surface: &WlSurface) {
    let previous = with_states(surface, |states| {
        states
            .data_map
            .get::<SurfaceBufferOwner>()
            .and_then(|slot| slot.0.lock().unwrap().take())
    });
    drop(previous);
}

#[cfg(test)]
pub(super) fn recorded_surface_buffer_id(surface: &WlSurface) -> Option<ObjectId> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<SurfaceBufferOwner>()
            .and_then(|slot| slot.0.lock().unwrap().as_ref().map(|owner| owner.id()))
    })
}

fn sampled_client_buffers(frame: &VulkanFrame<'_>) -> Option<Vec<WlBufferOwner>> {
    let mut buffers = Vec::new();
    for texture in frame.external_sampled_textures() {
        // A snapshot keeps its original attachment even after replacement or
        // surface destruction. Raw ImportDma has no lease and retains the
        // synchronous compatibility path instead of guessing a Wayland owner.
        buffers.push(texture.client_buffer.as_ref()?.clone());
    }
    Some(buffers)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeBusy {
    Frame,
    Upload,
    Completion,
    OutputFormat,
    YuvPipelines,
}

#[derive(Debug)]
pub enum NativeError {
    Other(String),
    Busy(NativeBusy),
}
impl NativeError {
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Busy(_))
    }

    pub(crate) fn is_backpressure(error: &(dyn std::error::Error + 'static)) -> bool {
        let mut current = Some(error);
        while let Some(error) = current {
            if error.downcast_ref::<Self>().is_some_and(Self::is_busy) {
                return true;
            }
            current = error.source();
        }
        false
    }
}
impl std::fmt::Display for NativeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Other(message) => message.fmt(f),
            Self::Busy(kind) => write!(f, "Vulkan {kind:?} capacity is busy"),
        }
    }
}
impl std::error::Error for NativeError {}
impl From<vk::Result> for NativeError {
    fn from(e: vk::Result) -> Self {
        Self::Other(e.to_string())
    }
}

/// Recovery transfer data. In particular, the white texture has no Wayland
/// attachment lease: only its Send GPU allocation crosses the worker boundary.
pub(crate) struct PreparedNativeDevice {
    device: super::PreparedVulkanDevice,
    resources: NativeDeviceResources,
}

struct NativeDeviceResources {
    completions: super::completion::CompletionQueue,
    white: Arc<super::TextureGpu>,
    sample_formats: FormatSet,
    target_formats: FormatSet,
    gpu_timings: Option<Arc<super::timing::GpuTimings>>,
}

impl NativeDeviceResources {
    fn prepare(renderer: &VulkanRenderer) -> Result<Self, NativeError> {
        let white = renderer.create_solid_texture([255; 4])?.inner;
        let sample_formats = dma_formats(renderer, true);
        let target_formats = dma_formats(renderer, false);
        let gpu_timings = match super::timing::GpuTimings::new(
            renderer.ctx.as_ref().expect("initialized renderer").clone(),
        ) {
            Ok(timings) => timings,
            Err(error) => {
                warn!(
                    ?error,
                    "Vulkan timestamp diagnostics unavailable; continuing without GPU-work samples"
                );
                None
            }
        };
        let completions = super::completion::CompletionQueue::new()
            .map_err(|error| NativeError::Other(error.to_string()))?;
        Ok(Self {
            completions,
            white,
            sample_formats,
            target_formats,
            gpu_timings,
        })
    }
}

impl PreparedNativeDevice {
    pub(crate) fn device_anchor(&self) -> Arc<super::VulkanContext> {
        self.device.ctx.clone()
    }

    /// This method is called only on the bounded recovery worker.
    pub(crate) fn prepare(node: smithay::backend::drm::DrmNode) -> Result<Self, String> {
        let mut renderer = VulkanRenderer::new();
        renderer
            .init_from_drm_node(Some(&node))
            .map_err(|error| error.to_string())?;
        let resources =
            NativeDeviceResources::prepare(&renderer).map_err(|error| error.to_string())?;
        Ok(Self {
            device: renderer.prepared_device(),
            resources,
        })
    }
}

#[derive(Debug)]
pub struct NativeRenderer {
    renderer: VulkanRenderer,
    backpressured: bool,
    scene_deferred: bool,
    id: ContextId<VulkanTexture>,
    downscale: TextureFilter,
    upscale: TextureFilter,
    debug: DebugFlags,
    white: VulkanTexture,
    shm_cache_lifetime: Arc<()>,
    sample_formats: FormatSet,
    target_formats: FormatSet,
    pending: Vec<Arc<FrameCompletion>>,
    gpu_timings: Option<Arc<super::timing::GpuTimings>>,
    output_timing: Option<(super::timing::GpuTimingTag, (u32, u32))>,
    completion_export_required: bool,
    completion_wakeup: Option<smithay::reexports::calloop::ping::Ping>,
    completions: super::completion::CompletionQueue,
    pending_uploads: Vec<Weak<super::memory::PendingUpload>>,
    offscreen_cache: Vec<VulkanTexture>,
    pub(super) shm_cache_registrations:
        std::collections::HashMap<usize, super::shm::ShmCacheRegistration>,
    pub(super) shm_cache_imports: u64,
    // Keep a few previous sources alive while closing windows created before a
    // configuration reload finish their animation.
    custom_programs:
        std::collections::HashMap<u8, Vec<(String, Arc<super::custom_shader::Program>)>>,
    failed_program_sources: std::collections::HashMap<u8, Vec<String>>,
    shader_compiles: ShaderCompileOwner,
    prepared_output_formats: std::collections::HashSet<vk::Format>,
    hdr_output: bool,
}

pub struct NativeFramebuffer<'a> {
    texture: VulkanTexture,
    _buffer: Option<Dmabuf>,
    _borrow: PhantomData<&'a mut ()>,
}
impl std::fmt::Debug for NativeFramebuffer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.texture.fmt(f)
    }
}
impl Texture for NativeFramebuffer<'_> {
    fn width(&self) -> u32 {
        self.texture.width()
    }
    fn height(&self) -> u32 {
        self.texture.height()
    }
    fn format(&self) -> Option<Fourcc> {
        self.texture.format()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Postprocess {
    pub noise: f32,
    pub saturation: f32,
    pub bg_color: [f32; 4],
}

impl Default for Postprocess {
    fn default() -> Self {
        Self {
            noise: 0.0,
            saturation: 1.0,
            bg_color: [0.0; 4],
        }
    }
}

pub struct NativeFrame<'a> {
    clip: super::pipeline::TextureClip,
    postprocess: Postprocess,
    frame: VulkanFrame<'a>,
    target: VulkanTexture,
    white: VulkanTexture,
    id: ContextId<VulkanTexture>,
    size: Size<i32, Physical>,
    transform: Transform,
    downscale: TextureFilter,
    upscale: TextureFilter,
    debug: DebugFlags,
    pending: &'a mut Vec<Arc<FrameCompletion>>,
    completions: &'a super::completion::CompletionQueue,
    completion_wakeup: Option<smithay::reexports::calloop::ping::Ping>,
    completion_permit: Option<super::completion::CompletionPermit>,
    async_export: bool,
    completion_export_required: bool,
    hdr_output: bool,
    hdr_damage: Vec<Rectangle<i32, Physical>>,
    shader_compiles: &'a mut ShaderCompileOwner,
    scene_deferred: &'a mut bool,
}

impl NativeRenderer {
    /// Wake the main event loop after GPU-owned Wayland buffers are released,
    /// so its normal flush sends release events even on a completely idle desktop.
    pub fn install_completion_wakeup<Data: 'static>(
        &mut self,
        event_loop: &smithay::reexports::calloop::LoopHandle<'_, Data>,
    ) -> Result<(), NativeError> {
        self.install_completion_wakeup_with_timings(event_loop, |_, _| ())
    }

    pub(crate) fn install_completion_wakeup_with_timings<Data: 'static>(
        &mut self,
        event_loop: &smithay::reexports::calloop::LoopHandle<'_, Data>,
        on_timing: impl FnMut(super::timing::GpuTimingSample, &mut Data) + 'static,
    ) -> Result<(), NativeError> {
        self.install_completion_wakeup_with_callbacks(event_loop, on_timing, |_| ())
    }

    pub(crate) fn install_completion_wakeup_with_callbacks<Data: 'static>(
        &mut self,
        event_loop: &smithay::reexports::calloop::LoopHandle<'_, Data>,
        mut on_timing: impl FnMut(super::timing::GpuTimingSample, &mut Data) + 'static,
        mut on_completion: impl FnMut(&mut Data) + 'static,
    ) -> Result<(), NativeError> {
        if self.completion_wakeup.is_none() {
            let (ping, source) = smithay::reexports::calloop::ping::make_ping()
                .map_err(|err| NativeError::Other(err.to_string()))?;
            // An idle/stopped loop may outlive the renderer. Keep only completed
            // CPU samples: retaining GpuTimings here would pin its query pool and
            // device. Pending GPU leases can still publish their final sample.
            let timings = self.gpu_timings.as_ref().map(|timings| timings.delivery());
            event_loop
                .insert_source(source, move |(), &mut (), data| {
                    if let Some(timings) = &timings {
                        timings.drain(|sample| on_timing(sample, data));
                    }
                    on_completion(data);
                })
                .map_err(|err| NativeError::Other(err.to_string()))?;
            self.completion_wakeup = Some(ping);
        }
        Ok(())
    }

    /// Arm only the final output composition after scene preparation. The next
    /// Renderer::render consumes this identity; captures/effect targets are
    /// unarmed and cannot silently become output GPU-work samples.
    pub(crate) fn arm_output_timing(
        &mut self,
        tag: super::timing::GpuTimingTag,
        extent: (u32, u32),
    ) {
        self.output_timing = Some((tag, extent));
    }

    /// Clear an arm after direct scanout, no damage or a failed render that did
    /// not construct a frame. Device adapters call this on every render outcome.
    pub(crate) fn clear_output_timing(&mut self) {
        self.output_timing = None;
    }

    /// Device output frames require an exportable display fence. Internal
    /// capture/effect frames use VkFence retirement on the same ordered queue,
    /// avoiding a potentially blocking sync-file export. The default preserves
    /// existing standalone renderer consumers.
    pub(crate) fn set_completion_export_required(&mut self, required: bool) {
        self.completion_export_required = required;
    }

    #[cfg(test)]
    pub(crate) fn drain_gpu_timings(
        &mut self,
        callback: impl FnMut(super::timing::GpuTimingSample),
    ) {
        if let Some(timings) = &self.gpu_timings {
            timings.drain(callback);
        }
    }

    /// Enable the final PQ transfer for the next output frame only.
    pub fn set_hdr_output(&mut self, enabled: bool) {
        self.hdr_output = enabled;
    }

    /// Configured selection is distinct from historical closing-window source lookup.
    pub(crate) fn configure_animation_program(
        &mut self,
        kind: crate::render_helpers::shaders::ProgramType,
        source: &str,
    ) -> anyhow::Result<Arc<super::custom_shader::Program>> {
        self.shader_compiles
            .configured
            .insert(kind as u8, source.to_owned());
        self.animation_program(kind, source)
    }

    fn install_animation_program(
        &mut self,
        key: u8,
        source: String,
        program: Arc<super::custom_shader::Program>,
    ) {
        self.shader_compiles
            .live_programs
            .retain(|program| program.strong_count() != 0);
        if !self
            .shader_compiles
            .live_programs
            .iter()
            .any(|old| old.ptr_eq(&Arc::downgrade(&program)))
        {
            self.shader_compiles
                .live_programs
                .push(Arc::downgrade(&program));
        }
        // Successful sources permit one fresh retry of previous failures, matching legacy cache.
        self.failed_program_sources.remove(&key);
        let promote = self
            .shader_compiles
            .configured
            .get(&key)
            .is_none_or(|configured| configured == &source);
        if promote || !self.shader_compiles.fallback.contains_key(&key) {
            self.shader_compiles.fallback.insert(key, program.clone());
        }
        const MAX_SOURCE_VARIANTS: usize = 4;
        let entries = self.custom_programs.entry(key).or_default();
        if entries.len() >= MAX_SOURCE_VARIANTS {
            entries.remove(0);
        }
        entries.push((source, program));
    }
    fn record_animation_failure(&mut self, key: u8, source: String, error: impl std::fmt::Display) {
        warn!("Vulkan animation shader unavailable; keeping previous valid program: {error}");
        const MAX_FAILED_SOURCES: usize = 4;
        let failed = self.failed_program_sources.entry(key).or_default();
        if failed.len() >= MAX_FAILED_SOURCES {
            failed.remove(0);
        }
        if !failed.contains(&source) {
            failed.push(source);
        }
    }
    fn request_shader_compile(
        &mut self,
        kind: crate::render_helpers::shaders::ProgramType,
        source: &str,
    ) -> bool {
        if self
            .shader_compiles
            .pending
            .iter()
            .any(|pending| pending.kind == kind && pending.source == source)
        {
            return true;
        }
        // Two GPU slots plus two CPU stages. Prepared SPIR-V awaiting admission cannot grow
        // without bound while a driver blocks its one fixed preparation worker.
        if self.shader_compiles.pending.len() >= 4 {
            return false;
        }
        self.shader_compiles.generation = self
            .shader_compiles
            .generation
            .checked_add(1)
            .expect("shader generation exhausted");
        let generation = self.shader_compiles.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        let prepared = if super::custom_shader::is_embedded(source) {
            match super::custom_shader::PreparedShader::embedded(source) {
                Ok(prepared) => Some(prepared),
                Err(error) => {
                    self.record_animation_failure(kind as u8, source.to_owned(), error);
                    return true;
                }
            }
        } else {
            let Some(endpoint) = &self.shader_compiles.endpoint else {
                return false;
            };
            if endpoint
                .request(
                    self.shader_compiles.instance,
                    kind as u8,
                    generation,
                    source.to_owned(),
                    cancel.clone(),
                )
                .is_err()
            {
                return false;
            }
            None
        };
        self.shader_compiles.pending.push(PendingShaderCompile {
            kind,
            source: source.to_owned(),
            generation,
            cancel,
            prepared,
            gpu_pending: false,
        });
        self.retry_gpu_programs();
        true
    }
    fn defer_shader_compile(
        &mut self,
        kind: crate::render_helpers::shaders::ProgramType,
        source: &str,
    ) {
        let deferred = &mut self.shader_compiles.deferred;
        if deferred
            .iter()
            .any(|(pending_kind, pending)| *pending_kind == kind && pending == source)
        {
            return;
        }
        // Bound historical desired variants independently of active/queued compiler slots.
        if deferred
            .iter()
            .filter(|(pending_kind, _)| *pending_kind == kind)
            .count()
            >= 4
        {
            if let Some(index) = deferred
                .iter()
                .position(|(pending_kind, _)| *pending_kind == kind)
            {
                deferred.remove(index);
            }
        }
        deferred.push((kind, source.to_owned()));
    }
    pub(crate) fn retry_deferred_shader_compiles(&mut self) -> bool {
        let mut submitted = false;
        let mut index = 0;
        while index < self.shader_compiles.deferred.len() {
            let (kind, source) = self.shader_compiles.deferred.remove(index);
            let key = kind as u8;
            if self
                .custom_programs
                .get(&key)
                .is_some_and(|entries| entries.iter().any(|(cached, _)| cached == &source))
                || self
                    .failed_program_sources
                    .get(&key)
                    .is_some_and(|failed| failed.contains(&source))
            {
                continue;
            }
            if self.request_shader_compile(kind, &source) {
                submitted = true;
            } else {
                self.shader_compiles.deferred.insert(index, (kind, source));
                index += 1;
            }
        }
        submitted |= self.retry_gpu_programs();
        submitted |= self.request_format_warm();
        submitted |= request_yuv_warm(&mut self.shader_compiles);
        submitted
    }
    fn desired_gpu_formats(&self) -> Vec<vk::Format> {
        let mut formats = self
            .shader_compiles
            .desired_formats
            .iter()
            .copied()
            .filter(|format| !self.shader_compiles.failed_formats.contains_key(format))
            .collect::<Vec<_>>();
        formats.sort_unstable_by_key(|format| format.as_raw());
        formats
    }
    fn retry_gpu_programs(&mut self) -> bool {
        use super::custom_shader::gpu_preparation::{Identity, Work};
        if self.shader_compiles.format_error.is_some() {
            return false;
        }
        let Some(endpoint) = &self.shader_compiles.gpu_endpoint else {
            return false;
        };
        let formats = self.desired_gpu_formats();
        let mut submitted = false;
        for pending in &mut self.shader_compiles.pending {
            let Some(prepared) = pending.prepared.take() else {
                continue;
            };
            let identity = Identity {
                renderer: self.shader_compiles.instance,
                kind: pending.kind as u8,
                generation: pending.generation,
                source: pending.source.clone(),
            };
            let work = Work::Build {
                ctx: self.renderer.ctx.as_ref().unwrap().clone(),
                prepared,
                formats: formats.clone(),
            };
            match endpoint.request(identity, work, pending.cancel.clone()) {
                Ok(()) => {
                    pending.gpu_pending = true;
                    submitted = true;
                }
                Err(rejected) => {
                    let Work::Build { prepared, .. } = rejected.work else {
                        unreachable!()
                    };
                    pending.prepared = Some(prepared);
                    if rejected.unavailable {
                        self.shader_compiles.format_error =
                            Some("GPU preparation worker unavailable".into());
                        break;
                    }
                }
            }
        }
        submitted
    }
    fn target_programs_ready(&self, format: vk::Format) -> bool {
        self.renderer
            .pipeline_mgr
            .as_ref()
            .unwrap()
            .has_prepared_format(format)
            && self
                .shader_compiles
                .live_programs
                .iter()
                .filter_map(Weak::upgrade)
                .all(|program| program.has_prepared_format(format))
    }
    fn request_format_warm(&mut self) -> bool {
        use super::custom_shader::gpu_preparation::{Identity, Work};
        if self.shader_compiles.format_pending.is_some()
            || self.shader_compiles.format_error.is_some()
        {
            return false;
        }
        self.shader_compiles
            .live_programs
            .retain(|program| program.strong_count() != 0);
        let formats = self
            .desired_gpu_formats()
            .into_iter()
            .filter(|format| !self.target_programs_ready(*format))
            .collect::<Vec<_>>();
        if formats.is_empty() {
            return false;
        }
        let Some(endpoint) = &self.shader_compiles.gpu_endpoint else {
            return false;
        };
        self.shader_compiles.generation = self
            .shader_compiles
            .generation
            .checked_add(1)
            .expect("GPU format generation exhausted");
        let generation = self.shader_compiles.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        let work = Work::Formats {
            manager: self.renderer.pipeline_mgr.as_ref().unwrap().clone(),
            programs: self
                .shader_compiles
                .live_programs
                .iter()
                .filter_map(Weak::upgrade)
                .collect(),
            formats,
        };
        if let Err(rejected) = endpoint.request(
            Identity {
                renderer: self.shader_compiles.instance,
                kind: u8::MAX,
                generation,
                source: String::new(),
            },
            work,
            cancel.clone(),
        ) {
            if rejected.unavailable {
                self.shader_compiles.format_error =
                    Some("GPU preparation worker unavailable".into());
            }
            return false;
        }
        self.shader_compiles.format_pending = Some((generation, cancel));
        true
    }
    pub(crate) fn take_shader_completion_source(
        &mut self,
    ) -> Option<
        calloop::channel::Channel<
            super::custom_shader::CompileCompletion<super::custom_shader::PreparedShader>,
        >,
    > {
        self.shader_compiles.completions.take()
    }
    pub(crate) fn take_gpu_preparation_source(
        &mut self,
    ) -> Option<calloop::channel::Channel<super::custom_shader::GpuPreparationEvent>> {
        self.shader_compiles.gpu_completions.take()
    }
    /// CPU translation results are pure data. Device preparation has a separate bounded worker.
    pub(crate) fn apply_shader_completion(
        &mut self,
        completion: super::custom_shader::CompileData<super::custom_shader::PreparedShader>,
    ) -> bool {
        if completion.renderer != self.shader_compiles.instance {
            return false;
        }
        let Some(index) = self.shader_compiles.pending.iter().position(|pending| {
            pending.kind as u8 == completion.kind
                && pending.generation == completion.generation
                && pending.source == completion.source
                && !pending.gpu_pending
                && pending.prepared.is_none()
        }) else {
            return false;
        };
        match completion.result {
            Ok(prepared) => {
                self.shader_compiles.pending[index].prepared = Some(prepared);
                self.retry_gpu_programs();
            }
            Err(error) => {
                self.shader_compiles.pending.remove(index);
                self.record_animation_failure(completion.kind, completion.source, error);
            }
        }
        false
    }
    pub(crate) fn apply_gpu_preparation_event(
        &mut self,
        event: super::custom_shader::GpuPreparationEvent,
    ) -> bool {
        use super::custom_shader::gpu_preparation::{Event, Prepared, Work, YUV_PREPARATION_KIND};
        let completion = match event {
            Event::Ready(completion) => completion,
            Event::CapacityAvailable(wake) => {
                // Release the coalesced wake before retrying: a slot becoming
                // available during this retry must be able to wake us again.
                drop(wake);
                self.retry_deferred_shader_compiles();
                return false;
            }
        };
        let identity = &completion.identity;
        if identity.renderer != self.shader_compiles.instance {
            return false;
        }
        if identity.kind == YUV_PREPARATION_KIND {
            if !self
                .shader_compiles
                .yuv_pending
                .as_ref()
                .is_some_and(|(generation, _)| *generation == identity.generation)
            {
                return false;
            }
            if let Some((_, cancel)) = self.shader_compiles.yuv_pending.take() {
                cancel.store(true, Ordering::Release);
            }
            match completion.consume() {
                Ok(Prepared::YuvTargets { failed }) => {
                    for (format, error) in failed {
                        warn!(
                            ?format,
                            ?error,
                            "Vulkan YUV target preparation unavailable; retaining ready outputs"
                        );
                    }
                }
                Err(error) => {
                    self.shader_compiles.format_error = Some(error);
                }
                Ok(_) => unreachable!("YUV request produced a different preparation kind"),
            }
            self.retry_deferred_shader_compiles();
            return true;
        }
        if identity.kind == u8::MAX {
            if !self
                .shader_compiles
                .format_pending
                .as_ref()
                .is_some_and(|(generation, _)| *generation == identity.generation)
            {
                return false;
            }
            if let Some((_, cancel)) = self.shader_compiles.format_pending.take() {
                cancel.store(true, Ordering::Release);
            }
            match completion.consume() {
                Ok(Prepared::Formats { formats, failed }) => {
                    self.prepared_output_formats.extend(formats);
                    for (format, error) in failed {
                        warn!(?format,"Vulkan attachment format unavailable; keeping prepared outputs: {error}");
                        self.shader_compiles.failed_formats.insert(format, error);
                    }
                }
                Err(error) => {
                    warn!("Vulkan format preparation failed: {error}");
                    self.shader_compiles.format_error = Some(error);
                }
                Ok(Prepared::Program { .. }) => unreachable!("format request produced a shader"),
                Ok(Prepared::YuvTargets { .. }) => {
                    unreachable!("format request produced YUV preparation")
                }
            }
            self.retry_deferred_shader_compiles();
            return true;
        }
        let Some(index) = self.shader_compiles.pending.iter().position(|pending| {
            pending.kind as u8 == identity.kind
                && pending.generation == identity.generation
                && pending.source == identity.source
                && pending.gpu_pending
        }) else {
            return false;
        };
        if let Some(Ok(Prepared::Program { formats, .. })) = &completion.result {
            if self
                .desired_gpu_formats()
                .iter()
                .any(|format| !formats.contains(format))
            {
                let formats = self.desired_gpu_formats();
                if let Err(error) = completion.retry(|prepared| {
                    let Prepared::Program { program, .. } = prepared else {
                        unreachable!()
                    };
                    Work::Rewarm { program, formats }
                }) {
                    let pending = self.shader_compiles.pending.remove(index);
                    self.record_animation_failure(
                        pending.kind as u8,
                        pending.source.clone(),
                        error,
                    );
                }
                return false;
            }
        }
        let kind = identity.kind;
        let source = identity.source.clone();
        self.shader_compiles.pending.remove(index);
        let installed = match completion.consume() {
            Ok(Prepared::Program { program, .. }) => {
                self.install_animation_program(kind, source, program);
                true
            }
            Err(error) => {
                self.record_animation_failure(kind, source, error);
                false
            }
            Ok(Prepared::Formats { .. }) => {
                unreachable!("shader request produced a format acknowledgment")
            }
            Ok(Prepared::YuvTargets { .. }) => {
                unreachable!("shader request produced YUV preparation")
            }
        };
        self.retry_deferred_shader_compiles();
        installed
    }

    pub fn animation_program(
        &mut self,
        kind: crate::render_helpers::shaders::ProgramType,
        source: &str,
    ) -> anyhow::Result<Arc<super::custom_shader::Program>> {
        anyhow::ensure!(
            matches!(
                kind,
                crate::render_helpers::shaders::ProgramType::Resize
                    | crate::render_helpers::shaders::ProgramType::Close
                    | crate::render_helpers::shaders::ProgramType::Open
            ),
            "unsupported native animation program kind"
        );
        let key = kind as u8;
        if let Some(entries) = self.custom_programs.get_mut(&key) {
            if let Some((cached, program)) = entries.last() {
                if cached == source {
                    if self
                        .shader_compiles
                        .configured
                        .get(&key)
                        .is_none_or(|configured| configured == source)
                    {
                        self.shader_compiles.fallback.insert(key, program.clone());
                    }
                    return Ok(program.clone());
                }
            }
            if let Some(index) = entries.iter().position(|(cached, _)| cached == source) {
                // Preserve the previous "active program" fallback when a later
                // shader reload fails, and keep variants used by old windows hot.
                let entry = entries.remove(index);
                let program = entry.1.clone();
                entries.push(entry);
                if self
                    .shader_compiles
                    .configured
                    .get(&key)
                    .is_none_or(|configured| configured == source)
                {
                    self.shader_compiles.fallback.insert(key, program.clone());
                }
                return Ok(program);
            }
        }
        if self
            .failed_program_sources
            .get(&key)
            .is_some_and(|sources| sources.iter().any(|failed| failed == source))
        {
            if let Some(program) = self.shader_compiles.fallback.get(&key) {
                return Ok(program.clone());
            }
        }
        // A renderer's initial embedded fallback is a small bootstrap object. Driver
        // pipelines are never built here, and later source variants use only the worker.
        if !self.shader_compiles.fallback.contains_key(&key) {
            let default = crate::render_helpers::shaders::animation_source(kind, None);
            #[cfg(test)]
            let start = std::time::Instant::now();
            #[cfg(test)]
            let cpu_start = super::custom_shader::gpu_preparation::thread_cpu();
            let program = super::custom_shader::Program::new(
                self.renderer.ctx.as_ref().unwrap().clone(),
                &default,
            )?;
            #[cfg(test)]
            eprintln!("GPU_BOOTSTRAP_OWNER kind={kind:?} scope=embedded_module_sampler_and_layouts_only wall_ms={} thread_cpu_ms={:?}",start.elapsed().as_secs_f64()*1000.,super::custom_shader::gpu_preparation::thread_cpu().zip(cpu_start).and_then(|(end,start)|end.checked_sub(start)).map(|value|value.as_secs_f64()*1000.));
            self.install_animation_program(key, default.clone(), program);
            self.request_format_warm();
            if source == default {
                return Ok(self.shader_compiles.fallback[&key].clone());
            }
        }
        if !self.request_shader_compile(kind, source) {
            self.defer_shader_compile(kind, source);
        }
        Ok(self.shader_compiles.fallback[&key].clone())
    }

    #[cfg(test)]
    pub(crate) fn prepared_animation_program_for_test(
        &self,
        kind: crate::render_helpers::shaders::ProgramType,
        source: &str,
    ) -> Option<Arc<super::custom_shader::Program>> {
        self.custom_programs
            .get(&(kind as u8))
            .and_then(|entries| entries.iter().find(|(cached, _)| cached == source))
            .map(|(_, program)| program.clone())
    }

    #[cfg(test)]
    fn poll_preparation_for_test(&mut self) -> anyhow::Result<()> {
        while let Some(completions) = self.shader_compiles.completions.as_ref() {
            match completions.try_recv() {
                Ok(completion) => {
                    self.apply_shader_completion(completion.into_data());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(error) => return Err(error.into()),
            }
        }
        while let Some(completions) = self.shader_compiles.gpu_completions.as_ref() {
            match completions.try_recv() {
                Ok(event) => {
                    self.apply_gpu_preparation_event(event);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(error) => return Err(error.into()),
            }
        }
        self.retry_deferred_shader_compiles();
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn gate_gpu_preparation_for_test(
        &self,
    ) -> anyhow::Result<GpuPreparationGateForTest> {
        use super::custom_shader::gpu_preparation::{Identity, Work};
        let (entered, observed) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel();
        let gate = GpuPreparationGateForTest(Some(release));
        let endpoint = self
            .shader_compiles
            .gpu_endpoint
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("GPU preparation worker unavailable"))?;
        endpoint
            .request(
                Identity {
                    renderer: self.shader_compiles.instance,
                    kind: u8::MAX - 2,
                    generation: 0,
                    source: String::new(),
                },
                Work::Gate {
                    entered,
                    release: held,
                },
                Arc::new(AtomicBool::new(false)),
            )
            .map_err(|_| anyhow::anyhow!("GPU preparation worker gate admission failed"))?;
        observed.recv_timeout(std::time::Duration::from_secs(5))?;
        Ok(gate)
    }

    /// Waits on real worker results outside the measured owner calls. It does
    /// not populate caches directly and cannot steal an installed owner source.
    #[cfg(test)]
    pub(crate) fn prepare_yuv_texture_for_test(
        &mut self,
        texture: &VulkanTexture,
        format: Fourcc,
    ) -> anyhow::Result<()> {
        let (format, _) = super::dmabuf::fourcc_to_vk_format(format)
            .ok_or_else(|| anyhow::anyhow!("unsupported YUV target test format"))?;
        let variant = texture
            .inner
            .yuv_pipeline
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("test texture is not YUV"))?
            .clone();
        register_yuv_target(&mut self.shader_compiles, &variant, format);
        request_yuv_warm(&mut self.shader_compiles);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            anyhow::ensure!(
                self.shader_compiles.gpu_completions.is_some(),
                "GPU completion source installed; dispatch its owner loop instead of a test wait"
            );
            self.poll_preparation_for_test()?;
            if let Some(error) = &self.shader_compiles.format_error {
                anyhow::bail!("GPU test YUV preparation failed: {error}");
            }
            if self.shader_compiles.yuv_pending.is_none()
                && variant.prepared_format(format)?.is_some()
            {
                return Ok(());
            }
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "explicit GPU test YUV preparation deadline"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[cfg(test)]
    pub(crate) fn prepare_output_format_for_test(&mut self, format: Fourcc) -> anyhow::Result<()> {
        self.prepare_output_format(format)?;
        let (format, _) = super::dmabuf::fourcc_to_vk_format(format).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if self.target_programs_ready(format)
                && self.target_programs_ready(vk::Format::R8G8B8A8_UNORM)
            {
                return Ok(());
            }
            if let Some(error) = &self.shader_compiles.format_error {
                anyhow::bail!("GPU test format preparation failed: {error}");
            }
            if let Some(error) = self.shader_compiles.failed_formats.get(&format) {
                anyhow::bail!("GPU test attachment format rejected: {error}");
            }
            anyhow::ensure!(
                self.shader_compiles.gpu_completions.is_some(),
                "GPU completion source installed; dispatch its owner loop instead of a test wait"
            );
            self.poll_preparation_for_test()?;
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "explicit GPU test format preparation deadline"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    #[cfg(test)]
    pub(crate) fn wait_for_animation_program_for_test(
        &mut self,
        kind: crate::render_helpers::shaders::ProgramType,
        source: &str,
    ) -> anyhow::Result<Arc<super::custom_shader::Program>> {
        let _fallback = self.animation_program(kind, source)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if let Some(program) = self.prepared_animation_program_for_test(kind, source) {
                if self
                    .desired_gpu_formats()
                    .iter()
                    .all(|format| program.has_prepared_format(*format))
                {
                    return Ok(program);
                }
            }
            if self
                .failed_program_sources
                .get(&(kind as u8))
                .is_some_and(|failed| failed.iter().any(|failed| failed == source))
            {
                anyhow::bail!("requested custom source failed preparation");
            }
            anyhow::ensure!(
                self.shader_compiles.gpu_completions.is_some(),
                "GPU completion source installed; dispatch its owner loop instead of a test wait"
            );
            self.poll_preparation_for_test()?;
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "explicit GPU test source preparation deadline"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Enqueue output and offscreen format preparation. Target admission below defers
    /// only an unprepared target while already prepared outputs keep rendering.
    pub fn prepare_output_format(&mut self, format: Fourcc) -> Result<(), NativeError> {
        let (format, _) = super::dmabuf::fourcc_to_vk_format(format)
            .ok_or_else(|| NativeError::Other("unsupported Vulkan output format".into()))?;
        self.shader_compiles
            .desired_formats
            .extend([format, vk::Format::R8G8B8A8_UNORM]);
        for target in &mut self.shader_compiles.yuv_targets {
            target.formats.extend([format, vk::Format::R8G8B8A8_UNORM]);
        }
        if let Some(error) = &self.shader_compiles.format_error {
            return Err(NativeError::Other(error.clone()));
        }
        if self.shader_compiles.gpu_endpoint.is_none() {
            return Err(NativeError::Other(
                "GPU preparation worker unavailable".into(),
            ));
        }
        if let Some(error) = self.shader_compiles.failed_formats.get(&format) {
            return Err(NativeError::Other(error.clone()));
        }
        self.request_format_warm();
        request_yuv_warm(&mut self.shader_compiles);
        if let Some(error) = &self.shader_compiles.format_error {
            return Err(NativeError::Other(error.clone()));
        }
        Ok(())
    }
    pub(crate) fn ensure_target_format_ready(
        &mut self,
        format: vk::Format,
    ) -> Result<(), NativeError> {
        // Every native scene can enqueue RGBA effect/blur intermediates.
        // Their base and retained-program pipelines must be ready before any
        // frame is built, including a first BGRA scene whose base existed at
        // bootstrap. Otherwise blur_pass could synchronously compile RGBA.
        if self.target_programs_ready(format)
            && self.target_programs_ready(vk::Format::R8G8B8A8_UNORM)
        {
            return Ok(());
        }
        self.shader_compiles
            .desired_formats
            .extend([format, vk::Format::R8G8B8A8_UNORM]);
        if let Some(error) = &self.shader_compiles.format_error {
            return Err(NativeError::Other(error.clone()));
        }
        if self.shader_compiles.gpu_endpoint.is_none() {
            return Err(NativeError::Other(
                "GPU preparation worker unavailable".into(),
            ));
        }
        if let Some(error) = self.shader_compiles.failed_formats.get(&format) {
            return Err(NativeError::Other(error.clone()));
        }
        self.request_format_warm();
        if let Some(error) = &self.shader_compiles.format_error {
            return Err(NativeError::Other(error.clone()));
        }
        // A cold format must not set the global frame/upload capacity latch: other
        // outputs' existing pipelines are valid while this format is prepared.
        Err(self.defer_output_format())
    }

    /// Whether an animation shader program of this kind has been compiled for this renderer.
    pub fn has_animation_program(&self, kind: crate::render_helpers::shaders::ProgramType) -> bool {
        self.custom_programs.contains_key(&(kind as u8))
    }

    pub fn vulkan(&self) -> &VulkanRenderer {
        &self.renderer
    }
    pub fn vulkan_mut(&mut self) -> &mut VulkanRenderer {
        &mut self.renderer
    }
    pub(super) fn shm_cache_lifetime(&self) -> std::sync::Weak<()> {
        Arc::downgrade(&self.shm_cache_lifetime)
    }
    fn retire_completed_uploads(&mut self) -> Result<(), NativeError> {
        let mut i = 0;
        while i < self.pending_uploads.len() {
            if self.pending_uploads[i]
                .upgrade()
                .map_or(Ok(true), |upload| upload.status())?
            {
                self.pending_uploads.remove(i);
            } else {
                i += 1;
            }
        }
        Ok(())
    }

    /// Release completed GPU references before deciding whether a cached image is shared.
    pub(crate) fn retire_completed(&mut self) -> Result<(), NativeError> {
        self.retire_completed_uploads()?;
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].retire_completed()? {
                self.pending.remove(i);
            } else {
                i += 1;
            }
        }
        Ok(())
    }

    pub(crate) fn take_backpressure(&mut self) -> bool {
        std::mem::take(&mut self.backpressured)
    }

    pub(crate) fn begin_scene(&mut self) {
        self.scene_deferred = false;
    }

    pub(crate) fn scene_was_deferred(&self) -> bool {
        self.scene_deferred
    }

    fn busy(&mut self, kind: NativeBusy) -> NativeError {
        self.backpressured = true;
        self.scene_deferred = true;
        NativeError::Busy(kind)
    }

    fn defer_output_format(&mut self) -> NativeError {
        self.scene_deferred = true;
        NativeError::Busy(NativeBusy::OutputFormat)
    }

    /// Admission precedes the Vulkan submission so refusal cannot make an
    /// in-flight upload's Drop wait on the desktop thread.
    pub(super) fn reserve_upload(
        &mut self,
    ) -> Result<super::completion::CompletionPermit, NativeError> {
        self.retire_completed_uploads()?;
        if self.pending_uploads.len() >= 8 {
            return Err(self.busy(NativeBusy::Upload));
        }
        self.completions
            .reserve()
            .ok_or_else(|| self.busy(NativeBusy::Completion))
    }

    pub(super) fn retain_shm_upload(
        &mut self,
        upload: super::memory::PendingUpload,
        permit: super::completion::CompletionPermit,
    ) -> Result<(), NativeError> {
        let upload = Arc::new(upload);
        self.pending_uploads.push(Arc::downgrade(&upload));
        self.completions
            .submit_upload(upload, self.completion_wakeup.clone(), permit)
            .map_err(|err| NativeError::Other(err.into()))?;
        Ok(())
    }
    pub fn new(renderer: VulkanRenderer) -> Result<Self, NativeError> {
        if renderer.ctx.is_none() || renderer.pipeline_mgr.is_none() || renderer.importer.is_none()
        {
            return Err(NativeError::Other(
                "Vulkan renderer initialization is incomplete".into(),
            ));
        }
        let resources = NativeDeviceResources::prepare(&renderer)?;
        Ok(Self::assemble(renderer, resources))
    }

    /// No device creation, image allocation, format query, shader compile or GPU
    /// wait is performed here. Only single-owner caches and CPU endpoints are made.
    pub(crate) fn from_prepared(prepared: PreparedNativeDevice) -> Self {
        let mut renderer = VulkanRenderer::new();
        renderer.init_prepared_device(prepared.device);
        Self::assemble(renderer, prepared.resources)
    }

    pub(crate) fn device_anchor(&self) -> Arc<super::VulkanContext> {
        self.renderer
            .ctx
            .as_ref()
            .expect("initialized renderer")
            .clone()
    }

    fn assemble(renderer: VulkanRenderer, resources: NativeDeviceResources) -> Self {
        let NativeDeviceResources {
            completions,
            white,
            sample_formats,
            target_formats,
            gpu_timings,
        } = resources;
        let white = VulkanTexture {
            width: 1,
            height: 1,
            format: vk::Format::R8G8B8A8_UNORM.as_raw() as u32,
            has_alpha: false,
            y_inverted: false,
            inner: white,
            client_buffer: None,
        };
        Self {
            backpressured: false,
            scene_deferred: false,
            pending: Vec::new(),
            gpu_timings,
            output_timing: None,
            completion_export_required: true,
            completion_wakeup: None,
            completions,
            pending_uploads: Vec::new(),
            sample_formats,
            target_formats,
            renderer,
            white,
            shm_cache_lifetime: Arc::new(()),
            id: ContextId::new(),
            downscale: TextureFilter::Linear,
            upscale: TextureFilter::Linear,
            debug: DebugFlags::empty(),
            offscreen_cache: Vec::new(),
            shm_cache_registrations: Default::default(),
            shm_cache_imports: 0,
            custom_programs: Default::default(),
            failed_program_sources: Default::default(),
            shader_compiles: ShaderCompileOwner::new(),
            prepared_output_formats: Default::default(),
            hdr_output: false,
        }
    }
    /// Retain scratch images for later reuse, without ever overwriting a live snapshot.
    /// Limit both count and pixel storage (64 MiB of 32-bit pixels, excluding driver overhead).
    pub fn recycle_offscreen(&mut self, texture: VulkanTexture) {
        const MAX_IMAGES: usize = 8;
        const MAX_PIXELS: u64 = 16 * 1024 * 1024;
        let Some(ctx) = self.renderer.ctx.as_ref() else {
            return;
        };
        let pixels = u64::from(texture.width) * u64::from(texture.height);
        let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT
            | vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_SRC
            | vk::ImageUsageFlags::TRANSFER_DST;
        if texture.inner.external
            || texture.y_inverted
            || !Arc::ptr_eq(ctx, &texture.inner.ctx)
            || !texture.inner.usage.contains(usage)
            || pixels > MAX_PIXELS
            || self
                .offscreen_cache
                .iter()
                .any(|t| Arc::ptr_eq(&t.inner, &texture.inner))
        {
            return;
        }
        while self.offscreen_cache.len() >= MAX_IMAGES
            || self
                .offscreen_cache
                .iter()
                .map(|t| u64::from(t.width) * u64::from(t.height))
                .sum::<u64>()
                + pixels
                > MAX_PIXELS
        {
            // Do not block the compositor waiting for a cache slot to become idle.
            let Some(idle) = self
                .offscreen_cache
                .iter()
                .position(|t| Arc::strong_count(&t.inner) == 1)
            else {
                return;
            };
            self.offscreen_cache.swap_remove(idle);
        }
        self.offscreen_cache.push(texture);
    }
}
impl RendererSuper for NativeRenderer {
    type Error = NativeError;
    type TextureId = VulkanTexture;
    type Framebuffer<'buffer> = NativeFramebuffer<'buffer>;
    type Frame<'frame, 'buffer>
        = NativeFrame<'frame>
    where
        'buffer: 'frame;
}
impl Renderer for NativeRenderer {
    fn context_id(&self) -> ContextId<VulkanTexture> {
        self.id.clone()
    }
    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), NativeError> {
        self.downscale = filter;
        Ok(())
    }
    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), NativeError> {
        self.upscale = filter;
        Ok(())
    }
    fn set_debug_flags(&mut self, flags: DebugFlags) {
        self.debug = flags;
    }
    fn debug_flags(&self) -> DebugFlags {
        self.debug
    }
    fn render<'frame, 'buffer>(
        &'frame mut self,
        framebuffer: &'frame mut NativeFramebuffer<'buffer>,
        output_size: Size<i32, Physical>,
        transform: Transform,
    ) -> Result<NativeFrame<'frame>, NativeError>
    where
        'buffer: 'frame,
    {
        // Bound queue growth without waiting for stalled GPU work. Device
        // adapters retry after the existing completion wakeup releases capacity.
        self.retire_completed()?;
        if self.pending.len() >= 8 {
            return Err(self.busy(NativeBusy::Frame));
        }
        let completion_permit = self
            .completions
            .reserve()
            .ok_or_else(|| self.busy(NativeBusy::Completion))?;
        // Renderer::render receives the untransformed framebuffer size;
        // Frame::output_size reports the transformed drawing coordinate space.
        let size = output_size;
        if size.w <= 0
            || size.h <= 0
            || size.w as u32 != framebuffer.width()
            || size.h as u32 != framebuffer.height()
        {
            return Err(NativeError::Other(
                "framebuffer dimensions do not match output transform".into(),
            ));
        }
        let ctx = self
            .renderer
            .ctx
            .as_ref()
            .cloned()
            .ok_or_else(|| NativeError::Other("Vulkan is not initialized".into()))?;
        let t = &framebuffer.texture;
        if !Arc::ptr_eq(&ctx, &t.inner.ctx) {
            return Err(NativeError::Other("foreign Vulkan framebuffer".into()));
        }
        if !t
            .inner
            .usage
            .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
        {
            return Err(NativeError::Other("texture is not a render target".into()));
        }
        self.ensure_target_format_ready(vk::Format::from_raw(t.format as i32))?;
        let mut frame = VulkanFrame::begin(
            ctx.clone(),
            self.renderer.pipeline_mgr.as_ref().unwrap().clone(),
            self.renderer.scratch_textures.clone(),
            self.renderer.importer.as_ref().unwrap().clone(),
            t.image(),
            t.image_view(),
            vk::Format::from_raw(t.format as i32),
            t.has_alpha,
            size.w as u32,
            size.h as u32,
            None,
            t.current_layout().get(),
            t.inner.external,
        )?;
        // DRM may render a cursor plane before its primary target. Leave the
        // arm in place until the first output-sized target, then consume once.
        if self
            .output_timing
            .is_some_and(|(_, extent)| extent == (t.width(), t.height()))
        {
            let (tag, _) = self.output_timing.take().unwrap();
            frame.measure_gpu_work(
                self.gpu_timings
                    .as_ref()
                    .and_then(|timings| timings.acquire(tag)),
            );
        }
        frame.retain_target(t);
        Ok(NativeFrame {
            completion_permit: Some(completion_permit),
            clip: Default::default(),
            postprocess: Default::default(),
            pending: &mut self.pending,
            completions: &self.completions,
            completion_wakeup: self.completion_wakeup.clone(),
            async_export: !self.completion_export_required || ctx.supports_sync_file_export,
            completion_export_required: self.completion_export_required,
            frame,
            target: t.clone(),
            white: self.white.clone(),
            id: self.id.clone(),
            size: transform.transform_size(output_size),
            transform,
            downscale: self.downscale,
            upscale: self.upscale,
            debug: self.debug,
            hdr_output: self.hdr_output,
            hdr_damage: Vec::new(),
            shader_compiles: &mut self.shader_compiles,
            scene_deferred: &mut self.scene_deferred,
        })
    }
    fn wait(&mut self, sync: &SyncPoint) -> Result<(), NativeError> {
        crate::render_helpers::sync::wait_for_completion(sync)
            .map_err(|e| NativeError::Other(e.to_string()))
    }
}
impl Bind<VulkanTexture> for NativeRenderer {
    fn bind<'a>(
        &mut self,
        target: &'a mut VulkanTexture,
    ) -> Result<NativeFramebuffer<'a>, NativeError> {
        // A texture from another Vulkan device cannot be used as a framebuffer.
        // Reject it here while the ownership boundary is explicit instead of
        // letting `render` fail after allocating command resources (or, worse,
        // handing a foreign image to a later Vulkan command).
        let ctx = self
            .renderer
            .ctx
            .as_ref()
            .ok_or_else(|| NativeError::Other("Vulkan is not initialized".into()))?;
        if !Arc::ptr_eq(ctx, &target.inner.ctx) {
            return Err(NativeError::Other("foreign Vulkan framebuffer".into()));
        }
        if !target
            .inner
            .usage
            .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
        {
            return Err(NativeError::Other("texture is not a render target".into()));
        }
        Ok(NativeFramebuffer {
            texture: target.clone(),
            _buffer: None,
            _borrow: PhantomData,
        })
    }
}
#[derive(Debug, Clone)]
pub struct NativeMapping {
    size: Size<i32, Buffer>,
    format: Fourcc,
    flipped: bool,
    pixels: Vec<u8>,
}
impl Texture for NativeMapping {
    fn width(&self) -> u32 {
        self.size.w as u32
    }
    fn height(&self) -> u32 {
        self.size.h as u32
    }
    fn format(&self) -> Option<Fourcc> {
        Some(self.format)
    }
}
impl TextureMapping for NativeMapping {
    fn flipped(&self) -> bool {
        self.flipped
    }
}
fn packed_rgba_readback_format(format: vk::Format) -> bool {
    matches!(
        format,
        vk::Format::R8G8B8A8_UNORM
            | vk::Format::B8G8R8A8_UNORM
            | vk::Format::A2B10G10R10_UNORM_PACK32
            | vk::Format::A2R10G10B10_UNORM_PACK32
    )
}

/// The job retains only GPU resources and conversion metadata. NativeRenderer
/// and its Wayland surface/buffer registries never leave the compositor thread.
#[derive(Debug)]
pub(crate) struct PendingNativeReadback {
    transfer: super::memory::PendingReadback,
    input: vk::Format,
    output: vk::Format,
    input_alpha: bool,
    output_alpha: bool,
}

impl PendingNativeReadback {
    /// Match `mapping_to_vec(ExportMem::copy_texture(...))`: return the selected
    /// Buffer-coordinate rows in their existing order. `y_inverted` describes
    /// texture sampling and must not add another screenshot row reversal.
    pub(crate) fn finish(self) -> Result<Vec<u8>, NativeError> {
        let mut pixels = self.transfer.finish()?;
        convert_readback_pixels(
            &mut pixels,
            self.input,
            self.output,
            self.input_alpha,
            self.output_alpha,
        )?;
        Ok(pixels)
    }
}

impl NativeRenderer {
    /// Submit on the ordered graphics queue without a CPU fence wait. Only the
    /// returned job crosses threads; external producers must be synchronized.
    pub(crate) fn enqueue_readback(
        &self,
        texture: &VulkanTexture,
        region: Rectangle<i32, Buffer>,
        format: Fourcc,
    ) -> Result<PendingNativeReadback, NativeError> {
        let (output, output_alpha) = super::dmabuf::fourcc_to_vk_format(format)
            .ok_or_else(|| NativeError::Other("unsupported Vulkan readback format".into()))?;
        let input = vk::Format::from_raw(texture.format as i32);
        if !packed_rgba_readback_format(output) || !packed_rgba_readback_format(input) {
            return Err(NativeError::Other(
                "unsupported Vulkan readback format".into(),
            ));
        }
        Ok(PendingNativeReadback {
            transfer: self.renderer.enqueue_memory_readback(texture, region)?,
            input,
            output,
            input_alpha: texture.has_alpha,
            output_alpha,
        })
    }
}

fn convert_readback_pixels(
    pixels: &mut [u8],
    input: vk::Format,
    output: vk::Format,
    input_alpha: bool,
    output_alpha: bool,
) -> Result<(), NativeError> {
    if !packed_rgba_readback_format(input)
        || !packed_rgba_readback_format(output)
        || !pixels.len().is_multiple_of(4)
    {
        return Err(NativeError::Other(
            "unsupported Vulkan readback format or length".into(),
        ));
    }
    // Same-format alpha-preserving copies require no channel/bit-depth work.
    // Opaque images still enter conversion to force the unused alpha bits full.
    if input == output && input_alpha && output_alpha {
        return Ok(());
    }
    // Packed 8/10-bit RGB share four bytes per pixel. Preserve premultiplied
    // channels; opaque sources and destinations force full output alpha.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let word = u32::from_le_bytes(*pixel);
        let (mut rgba, max) = match input {
            vk::Format::R8G8B8A8_UNORM => (
                [
                    word & 255,
                    (word >> 8) & 255,
                    (word >> 16) & 255,
                    word >> 24,
                ],
                [255; 4],
            ),
            vk::Format::B8G8R8A8_UNORM => (
                [
                    (word >> 16) & 255,
                    (word >> 8) & 255,
                    word & 255,
                    word >> 24,
                ],
                [255; 4],
            ),
            vk::Format::A2B10G10R10_UNORM_PACK32 => (
                [
                    word & 1023,
                    (word >> 10) & 1023,
                    (word >> 20) & 1023,
                    word >> 30,
                ],
                [1023, 1023, 1023, 3],
            ),
            vk::Format::A2R10G10B10_UNORM_PACK32 => (
                [
                    (word >> 20) & 1023,
                    (word >> 10) & 1023,
                    word & 1023,
                    word >> 30,
                ],
                [1023, 1023, 1023, 3],
            ),
            _ => {
                return Err(NativeError::Other(
                    "unsupported Vulkan source readback format".into(),
                ));
            }
        };
        if !input_alpha || !output_alpha {
            rgba[3] = max[3];
        }
        let eight = matches!(
            output,
            vk::Format::R8G8B8A8_UNORM | vk::Format::B8G8R8A8_UNORM
        );
        for i in 0..4 {
            let to = if eight {
                255
            } else if i == 3 {
                3
            } else {
                1023
            };
            rgba[i] = (rgba[i] * to + max[i] / 2) / max[i];
        }
        let [r, g, b, a] = rgba;
        let word = match output {
            vk::Format::R8G8B8A8_UNORM => r | g << 8 | b << 16 | a << 24,
            vk::Format::B8G8R8A8_UNORM => b | g << 8 | r << 16 | a << 24,
            vk::Format::A2B10G10R10_UNORM_PACK32 => r | g << 10 | b << 20 | a << 30,
            vk::Format::A2R10G10B10_UNORM_PACK32 => b | g << 10 | r << 20 | a << 30,
            _ => unreachable!("validated fourcc mapping"),
        };
        pixel.copy_from_slice(&word.to_le_bytes());
    }
    Ok(())
}
impl ExportMem for NativeRenderer {
    type TextureMapping = NativeMapping;
    fn copy_framebuffer(
        &mut self,
        target: &NativeFramebuffer<'_>,
        region: Rectangle<i32, Buffer>,
        format: Fourcc,
    ) -> Result<NativeMapping, NativeError> {
        self.copy_texture(&target.texture, region, format)
    }
    fn copy_texture(
        &mut self,
        texture: &VulkanTexture,
        region: Rectangle<i32, Buffer>,
        format: Fourcc,
    ) -> Result<NativeMapping, NativeError> {
        let pixels = self.enqueue_readback(texture, region, format)?.finish()?;
        Ok(NativeMapping {
            size: region.size,
            format,
            flipped: texture.y_inverted,
            pixels,
        })
    }
    fn can_read_texture(&mut self, texture: &VulkanTexture) -> Result<bool, NativeError> {
        Ok(self
            .renderer
            .ctx
            .as_ref()
            .is_some_and(|ctx| Arc::ptr_eq(ctx, &texture.inner.ctx))
            && packed_rgba_readback_format(vk::Format::from_raw(texture.format as i32))
            && texture
                .inner
                .usage
                .contains(vk::ImageUsageFlags::TRANSFER_SRC))
    }
    fn map_texture<'a>(&mut self, mapping: &'a NativeMapping) -> Result<&'a [u8], NativeError> {
        Ok(&mapping.pixels)
    }
}
impl ImportMem for NativeRenderer {
    fn import_memory(
        &mut self,
        data: &[u8],
        format: Fourcc,
        size: Size<i32, Buffer>,
        flipped: bool,
    ) -> Result<VulkanTexture, NativeError> {
        if size.w <= 0 || size.h <= 0 {
            return Err(NativeError::Other(
                "invalid memory texture dimensions".into(),
            ));
        }
        let stride = (size.w as usize)
            .checked_mul(4)
            .ok_or_else(|| NativeError::Other("memory texture stride overflow".into()))?;
        let permit = self.reserve_upload()?;
        // The upload owns a source snapshot and uses the same ordered graphics
        // queue as subsequent scene draws. Keep its resources with the completion
        // worker instead of making cursor/text import drain preceding GPU work.
        let (mut texture, upload) = self.renderer.import_memory_texture_deferred(
            data,
            format,
            size.w as u32,
            size.h as u32,
            stride,
        )?;
        texture.y_inverted = flipped;
        self.retain_shm_upload(upload, permit)?;
        Ok(texture)
    }
    fn update_memory(
        &mut self,
        texture: &VulkanTexture,
        data: &[u8],
        region: Rectangle<i32, Buffer>,
    ) -> Result<(), NativeError> {
        let stride = (texture.width as usize)
            .checked_mul(4)
            .ok_or_else(|| NativeError::Other("memory texture stride overflow".into()))?;
        let permit = self.reserve_upload()?;
        if let Some(upload) = self
            .renderer
            .update_memory_texture_deferred(texture, data, stride, region)?
        {
            self.retain_shm_upload(upload, permit)?;
        }
        Ok(())
    }
    fn mem_formats(&self) -> Box<dyn Iterator<Item = Fourcc>> {
        let ctx = self.renderer.ctx.as_ref().unwrap();
        let formats: Vec<_> = [
            Fourcc::Argb8888,
            Fourcc::Xrgb8888,
            Fourcc::Abgr8888,
            Fourcc::Xbgr8888,
        ]
        .into_iter()
        .filter(|code| {
            let (format, _) = super::dmabuf::fourcc_to_vk_format(*code).unwrap();
            let props = unsafe {
                ctx.instance
                    .get_physical_device_format_properties(ctx.physical_device, format)
            };
            props.optimal_tiling_features.contains(
                vk::FormatFeatureFlags::SAMPLED_IMAGE
                    | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR
                    | vk::FormatFeatureFlags::TRANSFER_DST
                    | vk::FormatFeatureFlags::TRANSFER_SRC,
            )
        })
        .collect();
        Box::new(formats.into_iter())
    }
}
impl ImportDma for NativeRenderer {
    fn dmabuf_formats(&self) -> FormatSet {
        self.sample_formats.clone()
    }

    fn import_dmabuf(
        &mut self,
        dmabuf: &Dmabuf,
        _damage: Option<&[Rectangle<i32, Buffer>]>,
    ) -> Result<VulkanTexture, NativeError> {
        if !self.sample_formats.contains(&dmabuf.format()) {
            return Err(NativeError::Other(
                "unsupported Vulkan DMA-BUF sampling format".into(),
            ));
        }
        // Imports alias the producer allocation, so damage does not require a copy.
        // The caller must provide producer synchronization before drawing.
        let texture = self
            .renderer
            .import_dmabuf(dmabuf)
            .map_err(|e| NativeError::Other(e.to_string()))?;
        if let Some(variant) = &texture.inner.yuv_pipeline {
            let mut formats = self.desired_gpu_formats();
            if !formats.contains(&vk::Format::R8G8B8A8_UNORM) {
                formats.push(vk::Format::R8G8B8A8_UNORM);
            }
            for format in formats {
                register_yuv_target(&mut self.shader_compiles, variant, format);
            }
            request_yuv_warm(&mut self.shader_compiles);
        }
        Ok(texture)
    }
}
impl ImportDmaWl for NativeRenderer {
    fn import_dma_buffer(
        &mut self,
        buffer: &WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<VulkanTexture, NativeError> {
        let dmabuf = get_dmabuf(buffer)
            .map_err(|_| NativeError::Other("Wayland buffer is not a DMA-BUF".into()))?;
        let mut texture = self.import_dmabuf(dmabuf, Some(damage))?;
        if let Some(slot) = surface.and_then(|s| s.data_map.get::<SurfaceBufferOwner>()) {
            let current = slot.0.lock().unwrap();
            if current
                .as_ref()
                .is_some_and(|owner| owner.id() == buffer.id())
            {
                texture.client_buffer = current.clone();
            }
        }
        Ok(texture)
    }
}
impl Bind<Dmabuf> for NativeRenderer {
    fn supported_formats(&self) -> Option<FormatSet> {
        Some(self.target_formats.clone())
    }

    fn bind<'a>(&mut self, target: &'a mut Dmabuf) -> Result<NativeFramebuffer<'a>, NativeError> {
        let key = self
            .renderer
            .cache_target(target)
            .map_err(|e| NativeError::Other(e.to_string()))?;
        let texture = self.renderer.target_cache[&key].texture.clone();
        Ok(NativeFramebuffer {
            texture,
            _buffer: Some(target.clone()),
            _borrow: PhantomData,
        })
    }
}
impl Offscreen<VulkanTexture> for NativeRenderer {
    fn create_buffer(
        &mut self,
        format: Fourcc,
        size: Size<i32, Buffer>,
    ) -> Result<VulkanTexture, NativeError> {
        let _latency =
            crate::utils::latency::Span::new(crate::utils::latency::Operation::NativeAllocation);
        let (vk_format, alpha) = super::dmabuf::fourcc_to_vk_format(format)
            .ok_or_else(|| NativeError::Other("unsupported Vulkan target format".into()))?;
        if size.w <= 0 || size.h <= 0 {
            return Err(NativeError::Other(
                "unsupported Vulkan target dimensions".into(),
            ));
        }
        self.retire_completed()?;
        if let Some(pos) = self.offscreen_cache.iter().position(|t| {
            Arc::strong_count(&t.inner) == 1
                && t.width == size.w as u32
                && t.height == size.h as u32
                && t.format == vk_format.as_raw() as u32
                && t.has_alpha == alpha
        }) {
            // Keep the actual layout for the next transition. GPU and external
            // owners have released the image before its storage can be reused.
            return Ok(self.offscreen_cache.swap_remove(pos));
        }
        let ctx = self
            .renderer
            .ctx
            .as_ref()
            .ok_or_else(|| NativeError::Other("Vulkan is not initialized".into()))?;
        let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT
            | vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_SRC
            | vk::ImageUsageFlags::TRANSFER_DST;
        let limits = unsafe {
            ctx.instance.get_physical_device_image_format_properties(
                ctx.physical_device,
                vk_format,
                vk::ImageType::TYPE_2D,
                vk::ImageTiling::OPTIMAL,
                usage,
                vk::ImageCreateFlags::empty(),
            )?
        };
        if size.w <= 0
            || size.h <= 0
            || size.w as u32 > limits.max_extent.width
            || size.h as u32 > limits.max_extent.height
        {
            return Err(NativeError::Other(
                "unsupported Vulkan target dimensions".into(),
            ));
        }
        Ok(allocate_texture(
            ctx,
            size.w as u32,
            size.h as u32,
            vk_format,
            vk::ImageTiling::OPTIMAL,
            usage,
            vk::ImageLayout::UNDEFINED,
            alpha,
        )?)
    }
}

impl NativeFrame<'_> {
    /// Capture attachment pixels without submitting or waiting for the frame.
    /// The region uses untransformed framebuffer coordinates. The destination's
    /// size defines the extent; it must have the same format and must not have
    /// been drawn from or captured into earlier in this frame. Later draws may
    /// sample it. The frame's completion also covers the captured image.
    pub fn capture_into(
        &mut self,
        destination: &VulkanTexture,
        origin: Point<i32, Buffer>,
    ) -> Result<(), NativeError> {
        if !self
            .target
            .inner
            .usage
            .contains(vk::ImageUsageFlags::TRANSFER_SRC)
        {
            return Err(NativeError::Other(
                "framebuffer does not support capture".into(),
            ));
        }
        if self.target.has_alpha && !destination.has_alpha {
            return Err(NativeError::Other(
                "capture requires matching alpha semantics".into(),
            ));
        }
        self.frame.capture_into(destination, origin.x, origin.y)?;
        Ok(())
    }

    /// Capture and linearly resize an in-bounds region in attachment coordinates.
    /// Filtering clamps to the captured region, using a temporary cropped image.
    /// Packed RGB format conversion is supported when the device permits blits;
    /// source and destination must have matching alpha semantics.
    pub fn capture_scaled_into(
        &mut self,
        destination: &VulkanTexture,
        region: Rectangle<i32, Buffer>,
    ) -> Result<(), NativeError> {
        if !self
            .target
            .inner
            .usage
            .contains(vk::ImageUsageFlags::TRANSFER_SRC)
        {
            return Err(NativeError::Other(
                "framebuffer does not support capture".into(),
            ));
        }
        if self.target.has_alpha && !destination.has_alpha {
            return Err(NativeError::Other(
                "capture requires matching alpha semantics".into(),
            ));
        }
        self.frame.capture_scaled_into(destination, region)?;
        Ok(())
    }

    pub(super) fn blur_pass(
        &mut self,
        source: &VulkanTexture,
        target: &VulkanTexture,
        offset: f32,
        up: bool,
    ) -> Result<(), NativeError> {
        self.blur_pass_region(
            source,
            target,
            offset,
            up,
            Rectangle::from_size(target.size()),
        )
    }

    pub(super) fn blur_pass_region(
        &mut self,
        source: &VulkanTexture,
        target: &VulkanTexture,
        offset: f32,
        up: bool,
        region: Rectangle<i32, Buffer>,
    ) -> Result<(), NativeError> {
        let Some(region) = region.intersection(Rectangle::from_size(target.size())) else {
            return Ok(());
        };
        let size = if up { source.size() } else { target.size() };
        self.frame.blur_pass(
            source,
            target,
            super::pipeline::BlurPushConstants {
                matrix: [
                    2., 0., 0., 0., 0., 2., 0., 0., 0., 0., 1., 0., -1., -1., 0., 1.,
                ],
                params: [
                    0.5 * offset / size.w as f32,
                    0.5 * offset / size.h as f32,
                    u8::from(up) as f32,
                    u8::from(source.y_inverted) as f32,
                ],
            },
            Some(vk::Rect2D {
                offset: vk::Offset2D {
                    x: region.loc.x,
                    y: region.loc.y,
                },
                extent: vk::Extent2D {
                    width: region.size.w as u32,
                    height: region.size.h as u32,
                },
            }),
        )?;
        Ok(())
    }

    pub(super) fn draw_blur(
        &mut self,
        source: &VulkanTexture,
        offset: [f32; 2],
        up: bool,
    ) -> Result<(), NativeError> {
        self.frame.draw_blur(
            source,
            super::pipeline::BlurPushConstants {
                matrix: self.matrix(Rectangle::from_size(self.size), Transform::Normal),
                params: [
                    offset[0],
                    offset[1],
                    u8::from(up) as f32,
                    u8::from(source.y_inverted) as f32,
                ],
            },
        )?;
        self.record_output_damage(Rectangle::from_size(
            (self.frame.width as i32, self.frame.height as i32).into(),
        ));
        Ok(())
    }

    pub(crate) fn replace_texture_clip(
        &mut self,
        clip: super::pipeline::TextureClip,
    ) -> super::pipeline::TextureClip {
        std::mem::replace(&mut self.clip, clip)
    }

    pub(crate) fn replace_postprocess(&mut self, postprocess: Postprocess) -> Postprocess {
        std::mem::replace(&mut self.postprocess, postprocess)
    }

    pub fn output_size(&self) -> Size<i32, Physical> {
        self.size
    }

    pub fn transformation(&self) -> Transform {
        self.transform
    }

    pub fn context(&self) -> Arc<super::device::VulkanContext> {
        self.frame.ctx.clone()
    }

    pub fn context_id(&self) -> ContextId<VulkanTexture> {
        self.id.clone()
    }

    fn record_output_damage(&mut self, rect: Rectangle<i32, Physical>) {
        if self.hdr_output {
            self.hdr_damage.push(rect);
        }
    }

    pub fn create_buffer(
        &self,
        format: Fourcc,
        size: Size<i32, Buffer>,
    ) -> Result<VulkanTexture, NativeError> {
        let (format, alpha) = super::dmabuf::fourcc_to_vk_format(format)
            .ok_or_else(|| NativeError::Other("unsupported Vulkan target format".into()))?;
        let ctx = &self.frame.ctx;
        let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT
            | vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_SRC
            | vk::ImageUsageFlags::TRANSFER_DST;
        let limits = unsafe {
            ctx.instance.get_physical_device_image_format_properties(
                ctx.physical_device,
                format,
                vk::ImageType::TYPE_2D,
                vk::ImageTiling::OPTIMAL,
                usage,
                vk::ImageCreateFlags::empty(),
            )?
        };
        if size.w <= 0
            || size.h <= 0
            || size.w as u32 > limits.max_extent.width
            || size.h as u32 > limits.max_extent.height
        {
            return Err(NativeError::Other(
                "unsupported Vulkan target dimensions".into(),
            ));
        }
        Ok(super::allocate_texture(
            ctx,
            size.w as u32,
            size.h as u32,
            format,
            vk::ImageTiling::OPTIMAL,
            usage,
            vk::ImageLayout::UNDEFINED,
            alpha,
        )?)
    }

    pub(crate) fn draw_border(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        mut pc: super::pipeline::BorderPushConstants,
    ) -> Result<(), NativeError> {
        if dst.size.is_empty() || damage.is_empty() {
            return Ok(());
        }
        if dst.loc.x.checked_add(dst.size.w).is_none()
            || dst.loc.y.checked_add(dst.size.h).is_none()
        {
            return Err(NativeError::Other(
                "border destination coordinate overflow".into(),
            ));
        }
        pc.matrix = self.matrix(dst, Transform::Normal);
        if bytemuck::cast_slice::<_, f32>(std::slice::from_ref(&pc))
            .iter()
            .any(|v| !v.is_finite())
        {
            return Err(NativeError::Other(
                "non-finite Vulkan border parameters".into(),
            ));
        }
        let disjoint = crate::render_helpers::damage_region::disjoint_damage(damage, dst.size);
        for mut rect in disjoint {
            rect.loc += dst.loc;
            if let Some(rect) = rect.intersection(Rectangle::from_size(self.size)) {
                let rect = self.transform.transform_rect_in(rect, &self.size);
                self.frame.set_clip(Some(rect));
                self.frame.draw_border_constants(pc);
                self.record_output_damage(rect);
            }
        }
        self.frame.set_clip(None);
        Ok(())
    }

    pub(crate) fn draw_shadow(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        mut pc: super::pipeline::ShadowPushConstants,
    ) -> Result<(), NativeError> {
        if dst.size.is_empty() || damage.is_empty() {
            return Ok(());
        }
        if dst.loc.x.checked_add(dst.size.w).is_none()
            || dst.loc.y.checked_add(dst.size.h).is_none()
        {
            return Err(NativeError::Other(
                "shadow destination coordinate overflow".into(),
            ));
        }
        pc.matrix = self.matrix(dst, Transform::Normal);
        if bytemuck::cast_slice::<_, f32>(std::slice::from_ref(&pc))
            .iter()
            .any(|v| !v.is_finite())
        {
            return Err(NativeError::Other(
                "non-finite Vulkan shadow parameters".into(),
            ));
        }
        let disjoint = crate::render_helpers::damage_region::disjoint_damage(damage, dst.size);
        for mut rect in disjoint {
            rect.loc += dst.loc;
            if let Some(rect) = rect.intersection(Rectangle::from_size(self.size)) {
                let rect = self.transform.transform_rect_in(rect, &self.size);
                self.frame.set_clip(Some(rect));
                self.frame.draw_shadow_constants(pc);
                self.record_output_damage(rect);
            }
        }
        self.frame.set_clip(None);
        Ok(())
    }

    pub(crate) fn draw_animation_shader(
        &mut self,
        program: &Arc<super::custom_shader::Program>,
        textures: &std::collections::HashMap<String, VulkanTexture>,
        uniforms: &[smithay::backend::renderer::gles::Uniform<'_>],
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), NativeError> {
        if dst.size.is_empty() || damage.is_empty() {
            return Ok(());
        }
        let draw = self
            .frame
            .custom_draw(program, textures, uniforms)
            .map_err(|err| NativeError::Other(format!("{err:#}")))?;
        let mut pc = [0.; 20];
        pc[..16].copy_from_slice(&self.matrix(dst, Transform::Normal));
        pc[16..].copy_from_slice(&[
            src.loc.x as f32,
            src.loc.y as f32,
            (src.loc.x + src.size.w) as f32,
            (src.loc.y + src.size.h) as f32,
        ]);
        let disjoint = crate::render_helpers::damage_region::disjoint_damage(damage, dst.size);
        for mut rect in disjoint {
            rect.loc += dst.loc;
            if let Some(rect) = rect.intersection(Rectangle::from_size(self.size)) {
                let rect = self.transform.transform_rect_in(rect, &self.size);
                self.frame.set_clip(Some(rect));
                self.frame.draw_custom(draw.clone(), pc);
                self.record_output_damage(rect);
            }
        }
        self.frame.set_clip(None);
        Ok(())
    }

    pub(crate) fn draw_particles(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        vertices: Rc<[crate::render_helpers::particles::ParticleVertex]>,
    ) -> Result<(), NativeError> {
        if dst.size.is_empty() || damage.is_empty() || vertices.is_empty() {
            return Ok(());
        }
        if !vertices.len().is_multiple_of(3)
            || dst.loc.x.checked_add(dst.size.w).is_none()
            || dst.loc.y.checked_add(dst.size.h).is_none()
        {
            return Err(NativeError::Other(
                "invalid Vulkan particle geometry".into(),
            ));
        }
        let pc = super::pipeline::ParticlePushConstants {
            matrix: self.matrix(dst, Transform::Normal),
        };
        if pc.matrix.iter().any(|v| !v.is_finite())
            || bytemuck::cast_slice::<_, f32>(&vertices)
                .iter()
                .any(|v| !v.is_finite())
        {
            return Err(NativeError::Other(
                "non-finite Vulkan particle parameters".into(),
            ));
        }
        // The same damage applies to every particle. Normalize it once instead of allocating
        // and subtracting overlapping regions for each particle in the close animation.
        let disjoint = crate::render_helpers::damage_region::disjoint_damage(damage, dst.size);
        let mut clips = Vec::with_capacity(disjoint.len());
        for mut rect in disjoint {
            rect.loc += dst.loc;
            if let Some(rect) = rect.intersection(Rectangle::from_size(self.size)) {
                clips.push(self.transform.transform_rect_in(rect, &self.size));
            }
        }
        if !clips.is_empty() {
            for &rect in &clips {
                self.record_output_damage(rect);
            }
            self.frame.draw_particles(pc, vertices, clips);
        }
        Ok(())
    }

    pub(crate) fn draw_mesh(
        &mut self,
        texture: &VulkanTexture,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        vertices: Vec<MeshGpuVertex>,
        tex_scale: [f32; 2],
        alpha: f32,
    ) -> Result<(), NativeError> {
        if dst.size.is_empty() || damage.is_empty() || vertices.is_empty() {
            return Ok(());
        }
        if dst.loc.x.checked_add(dst.size.w).is_none()
            || dst.loc.y.checked_add(dst.size.h).is_none()
        {
            return Err(NativeError::Other(
                "mesh destination coordinate overflow".into(),
            ));
        }
        if !alpha.is_finite() || tex_scale.iter().any(|v| !v.is_finite()) {
            return Err(NativeError::Other(
                "non-finite Vulkan mesh parameters".into(),
            ));
        }
        let pc = MeshPushConstants {
            matrix: self.matrix(dst, Transform::Normal),
            tex_scale,
            alpha,
            y_inverted: if texture.y_inverted { 1. } else { 0. },
        };
        if bytemuck::cast_slice::<_, f32>(std::slice::from_ref(&pc))
            .iter()
            .any(|v| !v.is_finite())
        {
            return Err(NativeError::Other(
                "non-finite Vulkan mesh parameters".into(),
            ));
        }
        if let Some(rect) = dst.intersection(Rectangle::from_size(self.size)) {
            self.frame
                .set_clip(Some(self.transform.transform_rect_in(rect, &self.size)));
            self.frame.draw_mesh(texture, pc, vertices)?;
            // MeshRenderElement has already clipped triangles to the exact
            // damage union. The coarse geometry scissor is not the written area.
            for mut rect in crate::render_helpers::damage_region::disjoint_damage(damage, dst.size)
            {
                rect.loc += dst.loc;
                if let Some(rect) = rect.intersection(Rectangle::from_size(self.size)) {
                    self.record_output_damage(self.transform.transform_rect_in(rect, &self.size));
                }
            }
        }
        self.frame.set_clip(None);
        Ok(())
    }

    fn output_transform(&self) -> Transform {
        // Smithay's renderer projection and transform_rect_in agree, but its
        // transform_point_in swaps the two reflected quarter-turns. Correct
        // points only; clip rectangles must retain the original transform.
        match self.transform {
            Transform::Flipped90 => Transform::Flipped270,
            Transform::Flipped270 => Transform::Flipped90,
            other => other,
        }
    }
    fn project(&self, p: Point<f64, Physical>) -> [f32; 2] {
        let p = self
            .output_transform()
            .transform_point_in(p, &self.size.to_f64());
        let size = self.transform.transform_size(self.size);
        [
            (p.x / f64::from(size.w) * 2. - 1.) as f32,
            (p.y / f64::from(size.h) * 2. - 1.) as f32,
        ]
    }
    fn matrix(&self, dst: Rectangle<i32, Physical>, transform: Transform) -> [f32; 16] {
        let point = |x, y| {
            let p = transform
                .transform_point_in(Point::<f64, Physical>::from((x, y)), &(1., 1.).into());
            self.project(
                (
                    f64::from(dst.loc.x) + p.x * f64::from(dst.size.w),
                    f64::from(dst.loc.y) + p.y * f64::from(dst.size.h),
                )
                    .into(),
            )
        };
        let o = point(0., 0.);
        let x = point(1., 0.);
        let y = point(0., 1.);
        [
            x[0] - o[0],
            x[1] - o[1],
            0.,
            0.,
            y[0] - o[0],
            y[1] - o[1],
            0.,
            0.,
            0.,
            0.,
            1.,
            0.,
            o[0],
            o[1],
            0.,
            1.,
        ]
    }
}
impl Frame for NativeFrame<'_> {
    type Error = NativeError;
    type TextureId = VulkanTexture;
    fn context_id(&self) -> ContextId<VulkanTexture> {
        self.id.clone()
    }
    fn transformation(&self) -> Transform {
        self.transform
    }
    fn output_size(&self) -> Size<i32, Physical> {
        self.size
    }
    fn clear(
        &mut self,
        color: Color32F,
        at: &[Rectangle<i32, Physical>],
    ) -> Result<(), NativeError> {
        let rects = at
            .iter()
            .map(|r| {
                super::damage::clip_rect(
                    r.loc.x,
                    r.loc.y,
                    r.size.w,
                    r.size.h,
                    self.size.w as u32,
                    self.size.h as u32,
                )
            })
            .filter(|r| r.extent.width != 0 && r.extent.height != 0)
            .map(|r| {
                Rectangle::new(
                    (r.offset.x, r.offset.y).into(),
                    (r.extent.width as i32, r.extent.height as i32).into(),
                )
            })
            .map(|r| self.transform.transform_rect_in(r, &self.size))
            .collect::<Vec<_>>();
        self.frame.clear_regions(color.components(), &rects)?;
        if self.hdr_output {
            self.hdr_damage.extend_from_slice(&rects);
        }
        Ok(())
    }
    fn draw_solid(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: Color32F,
    ) -> Result<(), NativeError> {
        let white = self.white.clone();
        self.draw(
            &white,
            Rectangle::from_size((1., 1.).into()),
            dst,
            damage,
            Transform::Normal,
            1.,
            color.components(),
        )
    }
    fn render_texture_from_to(
        &mut self,
        texture: &VulkanTexture,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque: &[Rectangle<i32, Physical>],
        transform: Transform,
        alpha: f32,
    ) -> Result<(), NativeError> {
        self.draw(
            texture,
            src,
            dst,
            damage,
            transform,
            alpha,
            if self.debug.contains(DebugFlags::TINT) {
                [1., 0.5, 0.5, 1.]
            } else {
                [1.; 4]
            },
        )
    }
    fn wait(&mut self, sync: &SyncPoint) -> Result<(), NativeError> {
        self.frame
            .wait_for_producer(sync)
            .map(|_| ())
            .map_err(|e| NativeError::Other(e.to_string()))
    }
    fn finish(mut self) -> Result<SyncPoint, NativeError> {
        if self.hdr_output {
            self.frame.encode_hdr_output(&self.hdr_damage)?;
        }
        let mut cold_yuv = false;
        for (variant, format) in self.frame.sampled_yuv_targets() {
            register_yuv_target(self.shader_compiles, &variant, format);
            match variant.prepared_format(format) {
                Ok(Some(_)) => {}
                Ok(None) => cold_yuv = true,
                Err(error) => {
                    return Err(NativeError::Other(format!(
                        "YUV target preparation failed: {error:?}"
                    )))
                }
            }
        }
        if cold_yuv {
            request_yuv_warm(self.shader_compiles);
            if let Some(error) = &self.shader_compiles.format_error {
                return Err(NativeError::Other(error.clone()));
            }
            // No submission has occurred. Only this scene retains its last
            // pixels while the bounded worker prepares its sampled YUV targets.
            *self.scene_deferred = true;
            return Err(NativeError::Busy(NativeBusy::YuvPipelines));
        }
        // Smithay may release a replaced wl_buffer immediately after this
        // method returns. Keep exactly the sampled buffer owners in the GPU
        // completion, including for offscreen targets without a KMS consumer.
        let buffers = sampled_client_buffers(&self.frame);
        if let (true, Some(buffers)) = (self.async_export, buffers) {
            self.frame.retain_client_buffers(buffers);
            let (completion, export_error) = self
                .frame
                .finish_async_retained(self.target, self.completion_export_required)?;
            // Submission has already happened. Retain its exact resources and
            // attachment leases on the bounded worker even when fence export
            // failed, then return that original error to the device adapter.
            let completion = match self.completions.submit_frame(
                completion,
                self.completion_wakeup,
                self.completion_permit.take().unwrap(),
            ) {
                Ok(completion) => completion,
                Err(error) => {
                    // Queue failure retains pending resources with their slot.
                    // Preserve the earlier driver error if export also failed.
                    return Err(export_error
                        .map(NativeError::from)
                        .unwrap_or_else(|| NativeError::Other(error.into())));
                }
            };
            let sync = completion.sync_point();
            self.pending.push(completion);
            if let Some(error) = export_error {
                return Err(error.into());
            }
            return Ok(sync);
        }
        self.frame.finish_and_wait()?;
        self.target.current_layout().set(vk::ImageLayout::GENERAL);
        Ok(SyncPoint::signaled())
    }
}
impl NativeFrame<'_> {
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        texture: &VulkanTexture,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        transform: Transform,
        alpha: f32,
        tint: [f32; 4],
    ) -> Result<(), NativeError> {
        self.draw_inner(texture, src, dst, damage, transform, alpha, tint, [0.; 2])
    }

    /// Draw a texture with a right-edge gradient fade, mirroring the GLES gradient-fade shader.
    ///
    /// `fade_cutoff` holds the left/right edge of the gradient in texture UV space; the fade is
    /// disabled when `fade_cutoff[0] >= fade_cutoff[1]`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_gradient_fade(
        &mut self,
        texture: &VulkanTexture,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        transform: Transform,
        alpha: f32,
        fade_cutoff: [f32; 2],
    ) -> Result<(), NativeError> {
        let tint = if self.debug.contains(DebugFlags::TINT) {
            [1., 0.5, 0.5, 1.]
        } else {
            [1.; 4]
        };
        self.draw_inner(
            texture,
            src,
            dst,
            damage,
            transform,
            alpha,
            tint,
            fade_cutoff,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_inner(
        &mut self,
        texture: &VulkanTexture,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        transform: Transform,
        alpha: f32,
        tint: [f32; 4],
        fade_cutoff: [f32; 2],
    ) -> Result<(), NativeError> {
        if dst.size.is_empty() || src.size.is_empty() || damage.is_empty() {
            return Ok(());
        }
        if dst.loc.x.checked_add(dst.size.w).is_none()
            || dst.loc.y.checked_add(dst.size.h).is_none()
        {
            return Err(NativeError::Other("destination coordinate overflow".into()));
        }
        if ![src.loc.x, src.loc.y, src.size.w, src.size.h]
            .iter()
            .all(|x| x.is_finite() && x.abs() <= f64::from(f32::MAX) / 4.)
            || !alpha.is_finite()
            || tint.iter().any(|x| !x.is_finite())
        {
            return Err(NativeError::Other(
                "non-finite Vulkan draw parameters".into(),
            ));
        }
        let s = transform.transform_size(src.size);
        let linear = if s.w > f64::from(dst.size.w) || s.h > f64::from(dst.size.h) {
            self.downscale
        } else {
            self.upscale
        } == TextureFilter::Linear;
        let uv_y = |y: f64| {
            let v = (y / f64::from(texture.height)) as f32;
            if texture.y_inverted {
                1. - v
            } else {
                v
            }
        };
        let pc = QuadPushConstants {
            matrix: self.matrix(dst, transform),
            uv_bounds: [
                (src.loc.x / f64::from(texture.width)) as f32,
                uv_y(src.loc.y),
                ((src.loc.x + src.size.w) / f64::from(texture.width)) as f32,
                uv_y(src.loc.y + src.size.h),
            ],
            color_tint: tint,
            alpha,
            corner_radius: 0.,
            size: [dst.size.w as f32, dst.size.h as f32],
            clip: self.clip,
            noise: self.postprocess.noise,
            saturation: self.postprocess.saturation,
            target_size: [self.frame.width as f32, self.frame.height as f32],
            bg_color: self.postprocess.bg_color,
            fade_cutoff,
            hdr_transfer: 0.,
        };
        let disjoint = crate::render_helpers::damage_region::disjoint_damage(damage, dst.size);
        for mut rect in disjoint {
            rect.loc += dst.loc;
            if let Some(rect) = rect.intersection(Rectangle::from_size(self.size)) {
                let rect = self.transform.transform_rect_in(rect, &self.size);
                self.frame.set_clip(Some(rect));
                self.frame.draw_quad(texture, pc, linear)?;
                self.record_output_damage(rect);
            }
        }
        self.frame.set_clip(None);
        Ok(())
    }
}

/// Query explicit modifiers directly from Vulkan, independent of EGL feedback.
/// Only packed RGB formats handled by our importer are candidates. Auxiliary
/// Concrete imports validate plane FD binding and modifier layouts again.
fn dma_formats(renderer: &VulkanRenderer, sampled: bool) -> FormatSet {
    let ctx = renderer.ctx.as_ref().expect("initialized renderer");
    if !ctx.supports_dmabuf_import {
        // Keep NativeRenderer usable for SHM and optimal-tiling WSI when the
        // driver has no Linux DMA-BUF interop extensions.  Advertising an
        // empty set makes the TTY backend select its existing GLES/KMS path
        // instead of attempting invalid DRM-modifier queries.
        return FormatSet::default();
    }
    let mut formats = Vec::new();
    let mut codes = vec![
        Fourcc::Argb8888,
        Fourcc::Xrgb8888,
        Fourcc::Abgr8888,
        Fourcc::Xbgr8888,
        Fourcc::Argb2101010,
        Fourcc::Xrgb2101010,
        Fourcc::Abgr2101010,
        Fourcc::Xbgr2101010,
    ];
    // Many video and embedded clients submit packed 16-bit RGB. It can be
    // sampled directly when the driver advertises modifier support, but it
    // need not be usable as a compositor render target.
    if sampled {
        codes.extend([Fourcc::Rgb565, Fourcc::Bgr565]);
        if ctx.supports_ycbcr_conversion {
            codes.extend([Fourcc::Nv12, Fourcc::Yuv420, Fourcc::P010]);
        }
    }
    for code in codes {
        let yuv = super::dmabuf::is_yuv_format(code);
        let usage = if yuv {
            vk::ImageUsageFlags::SAMPLED
        } else if sampled {
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC
        } else {
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC
        };
        let features = if yuv {
            vk::FormatFeatureFlags::SAMPLED_IMAGE | vk::FormatFeatureFlags::MIDPOINT_CHROMA_SAMPLES
        } else if sampled {
            vk::FormatFeatureFlags::TRANSFER_SRC
                | vk::FormatFeatureFlags::SAMPLED_IMAGE
                | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR
        } else {
            vk::FormatFeatureFlags::TRANSFER_SRC
                | vk::FormatFeatureFlags::COLOR_ATTACHMENT
                | vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND
        };
        let (format, _) = super::dmabuf::fourcc_to_vk_format(code).unwrap();
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
        let mut props = vk::FormatProperties2::default().push_next(&mut list);
        unsafe {
            ctx.instance.get_physical_device_format_properties2(
                ctx.physical_device,
                format,
                &mut props,
            );
        }
        let mut modifiers = vec![
            vk::DrmFormatModifierPropertiesEXT::default();
            list.drm_format_modifier_count as usize
        ];
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default()
            .drm_format_modifier_properties(&mut modifiers);
        let mut props = vk::FormatProperties2::default().push_next(&mut list);
        unsafe {
            ctx.instance.get_physical_device_format_properties2(
                ctx.physical_device,
                format,
                &mut props,
            );
        }
        for modifier in modifiers {
            let available = modifier.drm_format_modifier_tiling_features;
            if !(1..=4).contains(&modifier.drm_format_modifier_plane_count)
                || (yuv
                    && modifier.drm_format_modifier_plane_count as usize
                        != super::dmabuf::yuv_plane_count(format))
                || (yuv
                    && !available.intersects(
                        vk::FormatFeatureFlags::MIDPOINT_CHROMA_SAMPLES
                            | vk::FormatFeatureFlags::COSITED_CHROMA_SAMPLES,
                    ))
                || !available.contains(if yuv {
                    vk::FormatFeatureFlags::SAMPLED_IMAGE
                } else {
                    features
                })
            {
                continue;
            }
            let candidate = Format {
                code,
                modifier: Modifier::from(modifier.drm_format_modifier),
            };
            if renderer.supports_dmabuf_usage(candidate, usage) {
                formats.push(candidate);
            }
        }
    }
    formats.into_iter().collect()
}

#[cfg(test)]
mod readback_tests {
    use super::super::hardware_tests::setup;
    use super::super::VulkanContext;
    use super::*;

    #[test]
    fn pending_job_is_send_without_moving_the_renderer() {
        fn assert_send<T: Send>() {}
        assert_send::<PendingNativeReadback>();
    }

    #[test]
    fn recovery_device_bundle_is_send_without_moving_the_renderer() {
        fn assert_send<T: Send>() {}
        assert_send::<PreparedNativeDevice>();
    }

    #[test]
    fn identical_format_fast_path_preserves_every_byte_including_premultiplied_alpha() {
        for format in [
            vk::Format::R8G8B8A8_UNORM,
            vk::Format::B8G8R8A8_UNORM,
            vk::Format::A2B10G10R10_UNORM_PACK32,
            vk::Format::A2R10G10B10_UNORM_PACK32,
        ] {
            let mut pixels: Vec<_> = (0..=255u8).collect();
            let original = pixels.clone();
            convert_readback_pixels(&mut pixels, format, format, true, true).unwrap();
            assert_eq!(pixels, original, "{format:?}");
        }
    }

    #[test]
    fn packed_conversions_keep_channel_order_bit_depth_and_opaque_alpha() {
        let mut bgra = vec![17, 31, 127, 91];
        convert_readback_pixels(
            &mut bgra,
            vk::Format::B8G8R8A8_UNORM,
            vk::Format::R8G8B8A8_UNORM,
            true,
            true,
        )
        .unwrap();
        assert_eq!(bgra, [127, 31, 17, 91]);
        let word = 1023u32 | 512 << 10 | 257 << 20 | 2 << 30;
        let mut ten = word.to_le_bytes().to_vec();
        convert_readback_pixels(
            &mut ten,
            vk::Format::A2B10G10R10_UNORM_PACK32,
            vk::Format::B8G8R8A8_UNORM,
            true,
            true,
        )
        .unwrap();
        assert_eq!(ten, [64, 128, 255, 170]);
        let mut opaque = word.to_le_bytes().to_vec();
        convert_readback_pixels(
            &mut opaque,
            vk::Format::A2B10G10R10_UNORM_PACK32,
            vk::Format::A2B10G10R10_UNORM_PACK32,
            false,
            true,
        )
        .unwrap();
        assert_eq!(
            u32::from_le_bytes(opaque.try_into().unwrap()),
            word | 3 << 30
        );
        let mut eight = vec![17, 31, 127, 91];
        convert_readback_pixels(
            &mut eight,
            vk::Format::B8G8R8A8_UNORM,
            vk::Format::A2R10G10B10_UNORM_PACK32,
            true,
            false,
        )
        .unwrap();
        assert_eq!(
            u32::from_le_bytes(eight.try_into().unwrap()),
            68 | 124 << 10 | 509 << 20 | 3 << 30
        );
        assert!(convert_readback_pixels(
            &mut [0; 3],
            vk::Format::R8G8B8A8_UNORM,
            vk::Format::R8G8B8A8_UNORM,
            true,
            true
        )
        .is_err());
        assert!(convert_readback_pixels(
            &mut [0; 4],
            vk::Format::R5G6B5_UNORM_PACK16,
            vk::Format::R8G8B8A8_UNORM,
            true,
            true
        )
        .is_err());
    }

    pub(super) struct TimelineGate {
        ctx: Arc<VulkanContext>,
        semaphore: vk::Semaphore,
    }
    impl TimelineGate {
        pub(super) fn new(ctx: Arc<VulkanContext>) -> Self {
            assert!(ctx.info.supports_timeline_semaphores);
            let mut timeline =
                vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
            let semaphore = unsafe {
                ctx.device
                    .create_semaphore(
                        &vk::SemaphoreCreateInfo::default().push_next(&mut timeline),
                        None,
                    )
                    .unwrap()
            };
            {
                let _lock = ctx.command_lock.lock().unwrap();
                let mut wait =
                    vk::TimelineSemaphoreSubmitInfo::default().wait_semaphore_values(&[1]);
                unsafe {
                    ctx.device
                        .queue_submit(
                            ctx.graphics_queue,
                            &[vk::SubmitInfo::default()
                                .push_next(&mut wait)
                                .wait_semaphores(&[semaphore])
                                .wait_dst_stage_mask(&[vk::PipelineStageFlags::ALL_COMMANDS])],
                            vk::Fence::null(),
                        )
                        .unwrap();
                }
            }
            Self { ctx, semaphore }
        }
    }
    impl Drop for TimelineGate {
        fn drop(&mut self) {
            // Test cleanup must release the healthy GPU even if an assertion
            // fails. This wait is test teardown, never a capture code path.
            let _lock = self.ctx.command_lock.lock().unwrap();
            unsafe {
                self.ctx
                    .device
                    .signal_semaphore(
                        &vk::SemaphoreSignalInfo::default()
                            .semaphore(self.semaphore)
                            .value(1),
                    )
                    .unwrap();
                self.ctx
                    .device
                    .queue_wait_idle(self.ctx.graphics_queue)
                    .unwrap();
                self.ctx.device.destroy_semaphore(self.semaphore, None);
            }
        }
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan and timeline semaphores"]
    fn queued_readback_returns_before_gpu_and_worker_copies_the_selected_pixels() {
        for (inverted, format) in [false, true].into_iter().flat_map(|inverted| {
            [Fourcc::Abgr8888, Fourcc::Xrgb8888].map(|format| (inverted, format))
        }) {
            let (renderer, _reference, _allocator) = setup();
            let initial: Vec<_> = (0..32u8)
                .flat_map(|i| [i * 3, i * 5, 255 - i, 255 - i])
                .collect();
            let mut texture = renderer
                .import_memory_texture(&initial, Fourcc::Abgr8888, 8, 4, 32)
                .unwrap();
            texture.y_inverted = inverted;
            let ctx = renderer.ctx.as_ref().unwrap().clone();
            let mut native = NativeRenderer::new(renderer).unwrap();
            let region = Rectangle::new((2, 0).into(), (4, 3).into());
            // Sync compatibility retains the historical raw row metadata.
            let mapping = native.copy_texture(&texture, region, format).unwrap();
            assert_eq!(mapping.flipped(), inverted);
            let legacy = crate::render_helpers::mapping_to_vec(&mut native, &mapping).unwrap();
            let gate = TimelineGate::new(ctx);
            let job = native.enqueue_readback(&texture, region, format).unwrap();
            assert_eq!(
                job.transfer.is_ready(),
                Ok(false),
                "enqueue cannot wait for the gated queue"
            );
            // Dropping all desktop-owned source/renderer handles must not destroy
            // resources retained by the worker job.
            drop(texture);
            drop(native);
            let worker = std::thread::spawn(move || job.finish());
            drop(gate);
            let pixels = worker.join().unwrap().unwrap();
            let row_order = 0..3;
            let mut expected: Vec<_> = row_order
                .flat_map(|row| {
                    initial[(row * 8 + 2) * 4..(row * 8 + 6) * 4]
                        .iter()
                        .copied()
                })
                .collect();
            if format == Fourcc::Xrgb8888 {
                for pixel in expected.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                    pixel[3] = 255;
                }
            }
            assert_eq!(pixels, expected, "inverted={inverted}, format={format:?}");
            assert_eq!(pixels, legacy);
        }
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan and timeline semaphores"]
    fn cancelled_readback_and_timeout_retire_off_thread_without_blocking_queue_release() {
        for timeout in [false, true] {
            let (renderer, _reference, _allocator) = setup();
            let texture = renderer
                .import_memory_texture(&[31, 73, 157, 191].repeat(32), Fourcc::Abgr8888, 8, 4, 32)
                .unwrap();
            let ctx = renderer.ctx.as_ref().unwrap().clone();
            let native = NativeRenderer::new(renderer).unwrap();
            let gate = TimelineGate::new(ctx);
            let job = native
                .enqueue_readback(
                    &texture,
                    Rectangle::from_size((8, 4).into()),
                    Fourcc::Abgr8888,
                )
                .unwrap();
            assert_eq!(job.transfer.is_ready(), Ok(false));
            if timeout {
                let result = std::thread::spawn(move || job.finish()).join().unwrap();
                assert_eq!(
                    result.unwrap_err().to_string(),
                    vk::Result::TIMEOUT.to_string()
                );
            } else {
                let before = std::time::Instant::now();
                drop(job);
                assert!(
                    before.elapsed() < std::time::Duration::from_millis(500),
                    "cancellation waited for the GPU on the caller"
                );
            }
            // Retirement waits without the command lock, so the producer can
            // signal and later synchronous compatibility readbacks still work.
            drop(gate);
            let pixels = native
                .enqueue_readback(
                    &texture,
                    Rectangle::from_size((8, 4).into()),
                    Fourcc::Abgr8888,
                )
                .unwrap()
                .finish()
                .unwrap();
            assert_eq!(pixels, [31, 73, 157, 191].repeat(32));
        }
    }
}

#[cfg(test)]
mod backpressure_tests {
    use super::super::hardware_tests::setup;
    use super::readback_tests::TimelineGate;
    use super::*;

    fn clear(
        native: &mut NativeRenderer,
        target: &mut VulkanTexture,
    ) -> Result<SyncPoint, NativeError> {
        let size = (target.width as i32, target.height as i32).into();
        let mut framebuffer = native.bind(target)?;
        let mut frame = native.render(&mut framebuffer, size, Transform::Normal)?;
        frame.clear(
            Color32F::new(0.3, 0.5, 0.7, 1.),
            &[Rectangle::from_size(size)],
        )?;
        frame.finish()
    }

    #[test]
    #[ignore = "requires real DRM render node and Vulkan"]
    fn rejected_format_keeps_supported_output_and_new_program_preparation() {
        use super::super::custom_shader::{gpu_preparation, gpu_provider};
        use crate::render_helpers::shaders::{self, ProgramType};
        // Inject an ordinary format error at the worker/owner boundary, without
        // asking a driver to execute deliberately invalid Vulkan creation calls.
        fn rejected(
            _: gpu_preparation::Work,
            _: &AtomicBool,
        ) -> Result<gpu_preparation::Prepared, String> {
            Ok(gpu_preparation::Prepared::Formats {
                formats: vec![vk::Format::R8G8B8A8_UNORM],
                failed: vec![(
                    vk::Format::UNDEFINED,
                    vk::Result::ERROR_FORMAT_NOT_SUPPORTED.to_string(),
                )],
            })
        }
        let (renderer, _, _) = setup();
        let mut native = NativeRenderer::new(renderer).unwrap();
        native.set_completion_export_required(false);
        let default = shaders::animation_source(ProgramType::Close, None);
        let old = native
            .animation_program(ProgramType::Close, &default)
            .unwrap();
        native
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        let service = gpu_provider::Service::new(rejected).unwrap();
        let (endpoint, channel) = service.endpoint();
        let generation = native.shader_compiles.generation + 1;
        native.shader_compiles.generation = generation;
        let cancel = Arc::new(AtomicBool::new(false));
        native.shader_compiles.format_pending = Some((generation, cancel.clone()));
        assert!(endpoint
            .request(
                gpu_preparation::Identity {
                    renderer: native.shader_compiles.instance,
                    kind: u8::MAX,
                    generation,
                    source: String::new()
                },
                gpu_preparation::Work::Formats {
                    manager: native.renderer.pipeline_mgr.as_ref().unwrap().clone(),
                    programs: vec![old],
                    formats: vec![vk::Format::UNDEFINED]
                },
                cancel
            )
            .is_ok());
        loop {
            let event = channel.recv().unwrap();
            if matches!(&event, gpu_preparation::Event::Ready(_)) {
                assert!(native.apply_gpu_preparation_event(event));
                break;
            }
        }
        assert!(native.shader_compiles.format_error.is_none());
        assert!(native
            .shader_compiles
            .failed_formats
            .contains_key(&vk::Format::UNDEFINED));
        native
            .shader_compiles
            .desired_formats
            .insert(vk::Format::UNDEFINED);
        assert!(!native
            .desired_gpu_formats()
            .contains(&vk::Format::UNDEFINED));
        assert!(matches!(
            native.ensure_target_format_ready(vk::Format::UNDEFINED),
            Err(NativeError::Other(_))
        ));
        native.begin_scene();
        native
            .ensure_target_format_ready(vk::Format::R8G8B8A8_UNORM)
            .unwrap();
        assert!(!native.scene_was_deferred());
        let source =
            shaders::animation_source(ProgramType::Close, Some(shaders::DENIAL_EXPLOSION_SHADER));
        let program = native
            .wait_for_animation_program_for_test(ProgramType::Close, &source)
            .unwrap();
        assert!(program.has_prepared_format(vk::Format::R8G8B8A8_UNORM));
        let mut target = native
            .create_buffer(Fourcc::Abgr8888, (16, 12).into())
            .unwrap();
        native.begin_scene();
        clear(&mut native, &mut target).unwrap().wait().unwrap();
        assert!(!native.scene_was_deferred());
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan and timeline semaphores"]
    fn gated_frame_capacity_refuses_without_waiting_and_completion_retries() {
        let (renderer, _, _) = setup();
        let ctx = renderer.ctx.as_ref().unwrap().clone();
        let mut native = NativeRenderer::new(renderer).unwrap();
        native.set_completion_export_required(false);
        native
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        let mut event_loop = smithay::reexports::calloop::EventLoop::<usize>::try_new().unwrap();
        native
            .install_completion_wakeup_with_callbacks(
                &event_loop.handle(),
                |_, _| (),
                |wakes| *wakes += 1,
            )
            .unwrap();
        let mut target = native
            .create_buffer(Fourcc::Abgr8888, (16, 12).into())
            .unwrap();
        clear(&mut native, &mut target).unwrap().wait().unwrap();
        let mut wakes = 0;
        let drain_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while native.completions.outstanding_count() != 0 {
            assert!(std::time::Instant::now() < drain_deadline);
            event_loop
                .dispatch(std::time::Duration::from_millis(5), &mut wakes)
                .unwrap();
        }
        let gate = TimelineGate::new(ctx);
        let frames: Vec<_> = (0..8)
            .map(|_| clear(&mut native, &mut target).unwrap())
            .collect();
        assert!(frames.iter().all(|sync| !sync.is_reached()));
        let start = std::time::Instant::now();
        let error = clear(&mut native, &mut target).unwrap_err();
        assert!(matches!(error, NativeError::Busy(NativeBusy::Frame)));
        assert!(
            start.elapsed() < std::time::Duration::from_millis(250),
            "admission waited for a deliberately gated GPU"
        );
        assert!(native.scene_was_deferred());
        assert_eq!(native.pending.len(), 8);
        let before = wakes;
        drop(gate);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while wakes == before || native.completions.outstanding_count() != 0 {
            assert!(std::time::Instant::now() < deadline);
            event_loop
                .dispatch(std::time::Duration::from_millis(5), &mut wakes)
                .unwrap();
        }
        native.retire_completed().unwrap();
        assert!(native.take_backpressure());
        assert!(
            !native.take_backpressure(),
            "one completion burst caused repeated idle redraw requests"
        );
        assert!(frames.iter().all(SyncPoint::is_reached));
        clear(&mut native, &mut target).unwrap().wait().unwrap();
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan and timeline semaphores"]
    fn gated_upload_capacity_preserves_old_pixels_and_refuses_before_submission() {
        let (renderer, _, _) = setup();
        let ctx = renderer.ctx.as_ref().unwrap().clone();
        let mut native = NativeRenderer::new(renderer).unwrap();
        let mut event_loop = smithay::reexports::calloop::EventLoop::<usize>::try_new().unwrap();
        native
            .install_completion_wakeup_with_callbacks(
                &event_loop.handle(),
                |_, _| (),
                |wakes| *wakes += 1,
            )
            .unwrap();
        let gate = TimelineGate::new(ctx);
        let old = [31, 73, 157, 191].repeat(32);
        let textures: Vec<_> = (0..8)
            .map(|_| {
                native
                    .import_memory(&old, Fourcc::Abgr8888, (8, 4).into(), false)
                    .unwrap()
            })
            .collect();
        let start = std::time::Instant::now();
        assert!(matches!(
            native.import_memory(&old, Fourcc::Abgr8888, (8, 4).into(), false),
            Err(NativeError::Busy(NativeBusy::Upload))
        ));
        assert!(matches!(
            native.update_memory(
                &textures[0],
                &[255; 128],
                Rectangle::from_size((8, 4).into())
            ),
            Err(NativeError::Busy(NativeBusy::Upload))
        ));
        assert!(
            start.elapsed() < std::time::Duration::from_millis(250),
            "upload refusal waited for the GPU"
        );
        assert_eq!(native.pending_uploads.len(), 8);
        assert!(native.scene_was_deferred());
        drop(gate);
        let mut wakes = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while native.completions.outstanding_count() != 0 {
            assert!(std::time::Instant::now() < deadline);
            event_loop
                .dispatch(std::time::Duration::from_millis(5), &mut wakes)
                .unwrap();
        }
        assert!(wakes > 0);
        native.retire_completed().unwrap();
        assert!(native.take_backpressure());
        let pixels = native
            .enqueue_readback(
                &textures[0],
                Rectangle::from_size((8, 4).into()),
                Fourcc::Abgr8888,
            )
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(pixels, old, "refused update modified the previous texture");
        native
            .update_memory(
                &textures[0],
                &[255; 128],
                Rectangle::from_size((8, 4).into()),
            )
            .unwrap();
        let pixels = native
            .enqueue_readback(
                &textures[0],
                Rectangle::from_size((8, 4).into()),
                Fourcc::Abgr8888,
            )
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(pixels, [255; 128]);
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan sync export and timeline semaphores"]
    fn failed_sync_export_retires_submitted_frames_off_owner_with_bounded_capacity() {
        let (renderer, _, _) = setup();
        let ctx = renderer.ctx.as_ref().unwrap().clone();
        assert!(ctx.supports_sync_file_export);
        let mut native = NativeRenderer::new(renderer).unwrap();
        native
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        let mut event_loop = smithay::reexports::calloop::EventLoop::<usize>::try_new().unwrap();
        native
            .install_completion_wakeup_with_callbacks(
                &event_loop.handle(),
                |_, _| (),
                |wakes| *wakes += 1,
            )
            .unwrap();
        let mut source = native
            .create_buffer(Fourcc::Abgr8888, (8, 4).into())
            .unwrap();
        native.set_completion_export_required(false);
        clear(&mut native, &mut source).unwrap().wait().unwrap();
        let mut wakes = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while native.completions.outstanding_count() != 0 {
            assert!(std::time::Instant::now() < deadline);
            event_loop
                .dispatch(std::time::Duration::from_millis(5), &mut wakes)
                .unwrap();
        }
        native.retire_completed().unwrap();
        assert!(native.pending.is_empty());
        native.set_completion_export_required(true);
        let mut target = native
            .create_buffer(Fourcc::Abgr8888, (16, 12).into())
            .unwrap();
        let source_weak = Arc::downgrade(&source.inner);
        let target_weak = Arc::downgrade(&target.inner);
        // The injected driver result is returned after a real successful
        // submission. No invalid semaphore-export command is sent to Vulkan.
        let gate = TimelineGate::new(ctx);
        let waits = super::super::owner_fence_wait_calls_for_test();
        let started = std::time::Instant::now();
        let mut syncs = Vec::new();
        for index in 0..8 {
            let error = {
                let mut framebuffer = native.bind(&mut target).unwrap();
                let mut frame = native
                    .render(&mut framebuffer, (16, 12).into(), Transform::Normal)
                    .unwrap();
                let all = Rectangle::from_size((16, 12).into());
                frame
                    .render_texture_from_to(
                        &source,
                        Rectangle::from_size((8., 4.).into()),
                        all,
                        &[all],
                        &[],
                        Transform::Normal,
                        1.,
                    )
                    .unwrap();
                frame
                    .frame
                    .fail_sync_export_for_test(vk::Result::ERROR_TOO_MANY_OBJECTS);
                frame.finish().unwrap_err()
            };
            assert_eq!(
                error.to_string(),
                vk::Result::ERROR_TOO_MANY_OBJECTS.to_string(),
                "retirement must preserve the original export error"
            );
            assert!(!error.is_busy());
            assert_eq!(native.pending.len(), index + 1);
            let sync = native.pending.last().unwrap().sync_point();
            assert!(
                !sync.is_reached(),
                "failed export cannot mean an already-signaled sentinel"
            );
            assert!(!sync.is_exportable());
            syncs.push(sync);
        }
        assert!(matches!(
            clear(&mut native, &mut target),
            Err(NativeError::Busy(NativeBusy::Frame))
        ));
        native.retire_completed().unwrap();
        assert_eq!(
            super::super::owner_fence_wait_calls_for_test(),
            waits,
            "submitted export failures waited on their owner's thread"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_millis(250),
            "error cleanup blocked on the deliberately gated GPU"
        );
        assert_eq!(native.completions.outstanding_count(), 8);
        drop(target);
        drop(source);
        assert!(target_weak.upgrade().is_some());
        assert!(source_weak.upgrade().is_some());
        let before_wakes = wakes;
        drop(gate);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while native.completions.outstanding_count() != 0 || wakes == before_wakes {
            assert!(std::time::Instant::now() < deadline);
            event_loop
                .dispatch(std::time::Duration::from_millis(5), &mut wakes)
                .unwrap();
        }
        // No new scene or explicit renderer prune is needed to release images.
        assert!(target_weak.upgrade().is_none());
        assert!(source_weak.upgrade().is_none());
        assert!(syncs.iter().all(SyncPoint::is_reached));
        native.retire_completed().unwrap();
        assert!(native.pending.is_empty());
        assert!(native.take_backpressure());
        native.set_completion_export_required(false);
        let mut recovered = native
            .create_buffer(Fourcc::Abgr8888, (16, 12).into())
            .unwrap();
        clear(&mut native, &mut recovered).unwrap().wait().unwrap();
    }

    #[test]
    #[ignore = "requires real DRM render node and Vulkan output-format preparation"]
    fn first_bgra_scene_defers_until_rgba_intermediate_pipeline_is_prepared() {
        let (renderer, _, _) = setup();
        let mut native = NativeRenderer::new(renderer).unwrap();
        native.set_completion_export_required(false);
        assert!(native
            .renderer
            .pipeline_mgr
            .as_ref()
            .unwrap()
            .has_prepared_format(vk::Format::B8G8R8A8_UNORM));
        assert!(!native
            .renderer
            .pipeline_mgr
            .as_ref()
            .unwrap()
            .has_prepared_format(vk::Format::R8G8B8A8_UNORM));
        let gate = native.gate_gpu_preparation_for_test().unwrap();
        let mut target = native
            .create_buffer(Fourcc::Argb8888, (16, 12).into())
            .unwrap();
        let waits = super::super::owner_fence_wait_calls_for_test();
        let start = std::time::Instant::now();
        native.begin_scene();
        assert!(matches!(
            clear(&mut native, &mut target),
            Err(NativeError::Busy(NativeBusy::OutputFormat))
        ));
        assert!(
            start.elapsed() < std::time::Duration::from_millis(250),
            "first scene compiled an effect pipeline on its owner"
        );
        assert_eq!(super::super::owner_fence_wait_calls_for_test(), waits);
        assert!(native.scene_was_deferred());
        assert!(!native.take_backpressure());
        assert!(!native
            .renderer
            .pipeline_mgr
            .as_ref()
            .unwrap()
            .has_prepared_format(vk::Format::R8G8B8A8_UNORM));
        drop(gate);
        native
            .prepare_output_format_for_test(Fourcc::Argb8888)
            .unwrap();
        native.begin_scene();
        clear(&mut native, &mut target).unwrap().wait().unwrap();
        assert!(!native.scene_was_deferred());
    }

    #[test]
    #[ignore = "requires real DRM render node and Vulkan output-format preparation"]
    fn cold_output_format_defers_only_its_scene_and_ready_output_keeps_rendering() {
        let (renderer, _, _) = setup();
        let mut native = NativeRenderer::new(renderer).unwrap();
        native.set_completion_export_required(false);
        native
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        let mut ready = native
            .create_buffer(Fourcc::Abgr8888, (16, 12).into())
            .unwrap();
        let mut cold = native
            .create_buffer(Fourcc::Abgr2101010, (16, 12).into())
            .unwrap();
        let start = std::time::Instant::now();
        native.begin_scene();
        let error = clear(&mut native, &mut cold).unwrap_err();
        assert!(matches!(error, NativeError::Busy(NativeBusy::OutputFormat)));
        assert!(
            native.scene_was_deferred(),
            "an incomplete effect scene must not be submitted"
        );
        assert!(
            !native.take_backpressure(),
            "a new display format must not stop another ready output"
        );
        let admission_ms = start.elapsed().as_secs_f64() * 1000.;
        native.begin_scene();
        assert!(!native.scene_was_deferred());
        let start = std::time::Instant::now();
        clear(&mut native, &mut ready).unwrap().wait().unwrap();
        let ready_ms = start.elapsed().as_secs_f64() * 1000.;
        eprintln!("FORMAT_ADMISSION_WORK cold_scene_cpu_wall_ms={admission_ms:.6} ready_output_frame_wall_ms={ready_ms:.6}");
        native
            .prepare_output_format_for_test(Fourcc::Abgr2101010)
            .unwrap();
        native.begin_scene();
        clear(&mut native, &mut cold).unwrap().wait().unwrap();
    }
}

#[cfg(test)]
mod timing_tests {
    use super::super::hardware_tests::setup;
    use super::super::timing::{GpuTimingSample, GpuTimingTag};
    use super::readback_tests::TimelineGate;
    use super::*;

    fn clear(renderer: &mut NativeRenderer, target: &mut VulkanTexture) -> SyncPoint {
        let size = (target.width as i32, target.height as i32).into();
        let mut framebuffer = renderer.bind(target).unwrap();
        let mut frame = renderer
            .render(&mut framebuffer, size, Transform::Normal)
            .unwrap();
        frame
            .clear(
                Color32F::new(0.3, 0.5, 0.7, 1.),
                &[Rectangle::from_size(size)],
            )
            .unwrap();
        frame.finish().unwrap()
    }

    fn drain(renderer: &mut NativeRenderer) -> Vec<GpuTimingSample> {
        let mut samples = Vec::new();
        renderer.drain_gpu_timings(|sample| samples.push(sample));
        samples
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan timestamps and timeline semaphores"]
    fn output_gpu_timestamps_skip_cursor_targets_and_do_not_wait_for_gated_queue() {
        let (renderer, _reference, _allocator) = setup();
        let ctx = renderer.ctx.as_ref().unwrap().clone();
        let mut native = NativeRenderer::new(renderer).unwrap();
        native
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        native.set_completion_export_required(false);
        assert!(
            native.gpu_timings.is_some(),
            "selected GPU must support timestamps"
        );
        let tag = GpuTimingTag {
            output_generation: 41,
            redraw_sequence: 7,
        };
        let mut cursor = native
            .create_buffer(Fourcc::Abgr8888, (2, 2).into())
            .unwrap();
        let mut target = native
            .create_buffer(Fourcc::Abgr8888, (16, 12).into())
            .unwrap();
        let gate = TimelineGate::new(ctx);
        native.arm_output_timing(tag, (16, 12));
        let cursor_sync = clear(&mut native, &mut cursor);
        assert!(!cursor_sync.is_reached());
        assert_eq!(native.output_timing, Some((tag, (16, 12))));
        let output_sync = clear(&mut native, &mut target);
        assert!(!output_sync.is_reached());
        assert!(!output_sync.is_exportable());
        assert!(output_sync.export().is_none());
        assert_eq!(native.output_timing, None);
        // Main retirement and mailbox polling must return while this queue is
        // gated. A CPU wall-time substitute would incorrectly report a sample.
        native.retire_completed().unwrap();
        assert!(drain(&mut native).is_empty());
        drop(gate);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let sample = loop {
            native.retire_completed().unwrap();
            let samples = drain(&mut native);
            if !samples.is_empty() {
                assert_eq!(samples.len(), 1);
                break samples[0];
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        assert_eq!(sample.tag, tag);
        assert!(sample.gpu_time > std::time::Duration::ZERO);
        eprintln!("GPU COMMAND TIMING: {:?} {:?}", sample.tag, sample.gpu_time);
        assert!(drain(&mut native).is_empty(), "timing must publish once");
        native.arm_output_timing(
            GpuTimingTag {
                redraw_sequence: 8,
                ..tag
            },
            (16, 12),
        );
        native.clear_output_timing();
        clear(&mut native, &mut target).wait().unwrap();
        native.retire_completed().unwrap();
        assert!(
            drain(&mut native).is_empty(),
            "cleared skipped-output tag leaked to an offscreen frame"
        );
        // Pool exhaustion skips optional diagnostics rather than waiting or
        // sharing queries between incomplete frames.
        let timings = native.gpu_timings.as_ref().unwrap().clone();
        let leases: Vec<_> = (0..32)
            .map(|sequence| {
                timings
                    .acquire(GpuTimingTag {
                        redraw_sequence: sequence,
                        ..tag
                    })
                    .unwrap()
            })
            .collect();
        assert!(timings.acquire(tag).is_none());
        drop(leases);
        assert!(timings.acquire(tag).is_some());
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan timestamps and timeline semaphores"]
    fn completion_wakeup_delivers_gpu_timing_on_idle_output_after_renderer_drop() {
        let (renderer, _reference, _allocator) = setup();
        let ctx = renderer.ctx.as_ref().unwrap().clone();
        let mut native = NativeRenderer::new(renderer).unwrap();
        native
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        native.set_completion_export_required(false);
        assert!(native.gpu_timings.is_some());
        let mut event_loop =
            smithay::reexports::calloop::EventLoop::<Vec<GpuTimingSample>>::try_new().unwrap();
        native
            .install_completion_wakeup_with_timings(&event_loop.handle(), |sample, samples| {
                samples.push(sample)
            })
            .unwrap();
        let mut target = native
            .create_buffer(Fourcc::Abgr8888, (16, 12).into())
            .unwrap();
        let tag = GpuTimingTag {
            output_generation: 99,
            redraw_sequence: 3,
        };
        let gate = TimelineGate::new(ctx);
        native.arm_output_timing(tag, (16, 12));
        let sync = clear(&mut native, &mut target);
        assert!(!sync.is_reached());
        let mut samples = Vec::new();
        event_loop
            .dispatch(std::time::Duration::ZERO, &mut samples)
            .unwrap();
        assert!(samples.is_empty());
        let before = std::time::Instant::now();
        drop(native);
        assert!(before.elapsed() < std::time::Duration::from_millis(500));
        // A healthy pending internal frame may outlast the bounded synchronous
        // wait. The worker must still retire it after the producer eventually
        // signals, without poisoning completion with a premature TIMEOUT.
        std::thread::sleep(std::time::Duration::from_millis(2100));
        assert!(!sync.is_reached());
        drop(gate);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while samples.is_empty() {
            assert!(std::time::Instant::now() < deadline);
            event_loop
                .dispatch(std::time::Duration::from_millis(20), &mut samples)
                .unwrap();
        }
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].tag, tag);
        event_loop
            .dispatch(std::time::Duration::from_millis(5), &mut samples)
            .unwrap();
        assert_eq!(
            samples.len(),
            1,
            "idle completion delivered a duplicate sample"
        );
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan and timeline semaphores"]
    fn unexported_raw_sync_point_stays_pending_and_does_not_pin_retired_images() {
        let (renderer, _reference, _allocator) = setup();
        let ctx = renderer.ctx.as_ref().unwrap().clone();
        let mut native = NativeRenderer::new(renderer).unwrap();
        let target = native
            .create_buffer(Fourcc::Abgr8888, (16, 12).into())
            .unwrap();
        let weak = Arc::downgrade(&target.inner);
        let frame = native
            .renderer
            .render_frame(
                target.image(),
                target.image_view(),
                vk::Format::R8G8B8A8_UNORM,
                true,
                16,
                12,
                Some([0.2, 0.4, 0.6, 1.]),
            )
            .unwrap();
        let gate = TimelineGate::new(ctx);
        let completion = frame.finish_async_unexported(target).unwrap();
        let sync = completion.sync_point();
        assert!(
            !sync.is_reached(),
            "unexported pending work was confused with SYNC_FD -1"
        );
        assert!(!sync.is_exportable());
        assert!(sync.export().is_none());
        let pending = native
            .completions
            .submit_frame(completion, None, native.completions.reserve().unwrap())
            .unwrap();
        assert!(weak.upgrade().is_some());
        drop(gate);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while weak.upgrade().is_some() {
            pending.retire_completed().unwrap();
            assert!(
                std::time::Instant::now() < deadline,
                "live raw sync point retained completed frame images"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(sync.is_reached());
        sync.wait().unwrap();
        assert!(sync.export().is_none());
    }
}

#[cfg(test)]
mod memory_upload_tests {
    use super::super::hardware_tests::{setup, SubmittedGpuWork};
    use super::*;

    #[test]
    #[ignore = "requires real DRM render node and native completion fences"]
    fn native_memory_import_and_update_return_before_gpu_and_copy_caller_pixels() {
        let (vk, _reference, _allocator) = setup();
        let ctx = vk.ctx.as_ref().unwrap().clone();
        assert!(ctx.supports_sync_file_export);
        let mut renderer = NativeRenderer::new(vk).unwrap();
        renderer
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        let size = Size::from((3, 2));
        let region = Rectangle::new((1, 1).into(), (1, 1).into());
        let formats = [
            Fourcc::Abgr8888,
            Fourcc::Argb8888,
            Fourcc::Xbgr8888,
            Fourcc::Xrgb8888,
        ];
        let mut outputs = formats.map(|_| renderer.create_buffer(Fourcc::Abgr8888, size).unwrap());

        // The signal operation is already submitted, making native SYNC_FD
        // export legal. Uploads must enqueue behind this work rather than wait
        // for it on the compositor thread. No wall-time performance threshold.
        let mut work = SubmittedGpuWork::new(ctx);
        let _producer = work.submit();
        let mut completions = Vec::new();
        let mut sources = Vec::new();
        for ((index, format), output) in formats.into_iter().enumerate().zip(&mut outputs) {
            let opaque = matches!(format, Fourcc::Xbgr8888 | Fourcc::Xrgb8888);
            let bgra = matches!(format, Fourcc::Argb8888 | Fourcc::Xrgb8888);
            let alpha = if opaque { 0 } else { 192 };
            let initial = if bgra {
                [0, 0, 128, alpha]
            } else {
                [128, 0, 0, alpha]
            };
            let mut bytes = initial.repeat(6);
            let flipped = !index.is_multiple_of(2);
            let texture = renderer
                .import_memory(&bytes, format, size, flipped)
                .unwrap();
            assert!(
                !work.is_reached(),
                "memory import drained preceding GPU work"
            );
            bytes.fill(0xcc);
            let alias = texture.clone();
            assert_eq!(alias.image(), texture.image());
            assert_eq!(alias.y_inverted, flipped);

            // ImportMem updates use full-image source coordinates. Only (1, 1)
            // is copied, and the caller may overwrite the source immediately.
            let mut update = [0, 128, 0, alpha].repeat(6);
            renderer.update_memory(&alias, &update, region).unwrap();
            assert!(
                !work.is_reached(),
                "memory update drained preceding GPU work"
            );
            update.fill(0xdd);
            let uploads = renderer.pending_uploads.len();
            assert!(renderer.update_memory(&alias, &[0; 4], region).is_err());
            assert!(renderer
                .update_memory(&alias, &bytes, Rectangle::new((3, 0).into(), (1, 1).into()))
                .is_err());
            assert_eq!(
                renderer.pending_uploads.len(),
                uploads,
                "invalid update must not submit"
            );

            let weak = Arc::downgrade(&texture.inner);
            let mut framebuffer = renderer.bind(output).unwrap();
            let mut frame = renderer
                .render(&mut framebuffer, (3, 2).into(), Transform::Normal)
                .unwrap();
            let damage = Rectangle::from_size((3, 2).into());
            frame.clear(Color32F::TRANSPARENT, &[damage]).unwrap();
            frame
                .render_texture_from_to(
                    &alias,
                    Rectangle::from_size(size).to_f64(),
                    damage,
                    &[damage],
                    &[],
                    Transform::Normal,
                    1.,
                )
                .unwrap();
            completions.push(frame.finish().unwrap());
            sources.push((weak, opaque, flipped));
            drop(alias);
            drop(texture);
        }
        assert!(
            !work.is_reached(),
            "queue-ordered consumers must also stay asynchronous"
        );
        assert!(
            sources
                .iter()
                .all(|(texture, _, _)| texture.upgrade().is_some()),
            "pending uploads/draws must retain their images after callers drop them"
        );
        assert_eq!(
            renderer.pending_uploads.len(),
            8,
            "imports and updates share the bounded upload queue"
        );
        work.wait();
        for completion in completions {
            completion.wait().unwrap();
        }
        for (output, (_, opaque, flipped)) in outputs.iter().zip(sources) {
            let mapping = renderer
                .copy_texture(output, Rectangle::from_size(size), Fourcc::Abgr8888)
                .unwrap();
            let pixels = renderer.map_texture(&mapping).unwrap();
            for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
                let updated = index == if flipped { 1 } else { 4 };
                let alpha = if opaque { 255 } else { 192 };
                let expected = if updated {
                    [0, 128, 0, alpha]
                } else {
                    [128, 0, 0, alpha]
                };
                assert_eq!(*pixel, expected, "queued CPU import/update must preserve format, region, flip and source snapshot");
            }
        }
        // Import validation is preserved; rejected inputs cannot become uploads.
        assert!(renderer
            .import_memory(&[0; 4], Fourcc::Abgr8888, (0, 1).into(), false)
            .is_err());
        assert!(renderer
            .import_memory(&[0; 4], Fourcc::Nv12, (1, 1).into(), false)
            .is_err());
        assert!(renderer
            .import_memory(&[0; 3], Fourcc::Abgr8888, (1, 1).into(), false)
            .is_err());
        eprintln!("PASS: Native ImportMem imports/updates return before queued GPU work, retain source snapshots and images, preserve all four packed formats, flips and partial-update pixels");
    }
}

#[cfg(test)]
mod hdr_damage_tests {
    use super::super::hardware_tests::setup;
    use super::*;

    fn paint(
        renderer: &mut NativeRenderer,
        target: &mut VulkanTexture,
        transform: Transform,
        color: [f32; 4],
        damage: &[Rectangle<i32, Physical>],
        clear: &[Rectangle<i32, Physical>],
    ) {
        let output_size = (target.width() as i32, target.height() as i32).into();
        let mut framebuffer = renderer.bind(target).unwrap();
        let mut frame = renderer
            .render(&mut framebuffer, output_size, transform)
            .unwrap();
        frame
            .clear(Color32F::new(color[0], color[1], color[2], color[3]), clear)
            .unwrap();
        let geometry = Rectangle::from_size(frame.output_size());
        frame
            .draw_solid(
                geometry,
                damage,
                Color32F::new(color[0], color[1], color[2], color[3]),
            )
            .unwrap();
        // Two opaque elements can cover the same damaged pixels even though the
        // clear damage was eliminated by Smithay's opaque-region subtraction.
        if damage.len() > 1 {
            frame
                .draw_solid(
                    geometry,
                    &damage[1..],
                    Color32F::new(color[0], color[1], color[2], color[3]),
                )
                .unwrap();
        }
        frame.finish().unwrap().wait().unwrap();
    }

    fn pixels(renderer: &mut NativeRenderer, target: &VulkanTexture) -> Vec<u8> {
        let mapping = renderer
            .copy_texture(
                target,
                Rectangle::from_size(target.size()),
                Fourcc::Abgr2101010,
            )
            .unwrap();
        renderer.map_texture(&mapping).unwrap().to_vec()
    }

    #[test]
    #[ignore = "requires real DRM render node and Vulkan validation"]
    fn hdr_partial_opaque_damage_preserves_untouched_pixels_for_all_transforms() {
        let (vulkan, _, _) = setup();
        let mut renderer = NativeRenderer::new(vulkan).unwrap();
        renderer.set_hdr_output(true);
        renderer
            .prepare_output_format_for_test(Fourcc::Abgr2101010)
            .unwrap();
        for transform in [
            Transform::Normal,
            Transform::_90,
            Transform::_180,
            Transform::_270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ] {
            let mut target = renderer
                .create_buffer(Fourcc::Abgr2101010, (16, 12).into())
                .unwrap();
            let drawing_size = transform.transform_size((16, 12).into());
            let full = Rectangle::from_size(drawing_size);
            paint(
                &mut renderer,
                &mut target,
                transform,
                [0.5, 0.5, 0.5, 1.],
                &[full],
                &[],
            );
            let damage = [
                Rectangle::new((1, 1).into(), (4, 6).into()),
                Rectangle::new((3, 3).into(), (5, 4).into()),
            ];
            let attachment_damage: Vec<_> = damage
                .iter()
                .map(|rect| transform.transform_rect_in(*rect, &drawing_size))
                .collect();
            for (iteration, color) in [
                [0.7, 0.2, 0.1, 1.],
                [0.1, 0.7, 0.2, 1.],
                [0.2, 0.1, 0.7, 1.],
            ]
            .into_iter()
            .enumerate()
            {
                let before = pixels(&mut renderer, &target);
                let mut reference = renderer
                    .create_buffer(Fourcc::Abgr2101010, (16, 12).into())
                    .unwrap();
                paint(
                    &mut renderer,
                    &mut reference,
                    transform,
                    color,
                    &[full],
                    &[],
                );
                let expected = pixels(&mut renderer, &reference);
                let clear = if iteration == 0 {
                    &[][..]
                } else {
                    &damage[..1]
                };
                paint(&mut renderer, &mut target, transform, color, &damage, clear);
                let actual = pixels(&mut renderer, &target);
                for (index, pixel) in actual.as_chunks::<4>().0.iter().enumerate() {
                    let point = Point::from(((index % 16) as i32, (index / 16) as i32));
                    let written = attachment_damage.iter().any(|rect| rect.contains(point));
                    let bytes = index * 4..index * 4 + 4;
                    assert_eq!(
                        pixel,
                        if written {
                            &expected[bytes]
                        } else {
                            &before[bytes]
                        },
                        "{transform:?}, iteration {iteration}, pixel {point:?}"
                    );
                }
            }
        }
    }

    #[test]
    #[ignore = "requires real DRM render node and Vulkan validation"]
    fn hdr_empty_damage_does_not_convert_pixels_or_grow_scratch_cache() {
        let (vulkan, _, _) = setup();
        let mut renderer = NativeRenderer::new(vulkan).unwrap();
        renderer.set_hdr_output(true);
        renderer
            .prepare_output_format_for_test(Fourcc::Abgr2101010)
            .unwrap();
        let mut target = renderer
            .create_buffer(Fourcc::Abgr2101010, (16, 12).into())
            .unwrap();
        paint(
            &mut renderer,
            &mut target,
            Transform::Normal,
            [0.5, 0.25, 0.125, 1.],
            &[Rectangle::from_size((16, 12).into())],
            &[],
        );
        let before = pixels(&mut renderer, &target);
        let before_scratch: Vec<_> = renderer
            .vulkan()
            .scratch_textures
            .lock()
            .unwrap()
            .iter()
            .map(VulkanTexture::image)
            .collect();
        for _ in 0..3 {
            paint(
                &mut renderer,
                &mut target,
                Transform::Normal,
                [1.; 4],
                &[],
                &[],
            );
            assert_eq!(pixels(&mut renderer, &target), before);
        }
        let after_scratch: Vec<_> = renderer
            .vulkan()
            .scratch_textures
            .lock()
            .unwrap()
            .iter()
            .map(VulkanTexture::image)
            .collect();
        assert_eq!(after_scratch, before_scratch);
    }
}
