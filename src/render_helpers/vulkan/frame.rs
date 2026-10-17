//! Deferred Vulkan compositing with ordered rendering and capture scopes.
#![allow(clippy::too_many_arguments)]
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use ash::vk;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::utils::Buffer as WlBufferOwner;

use super::device::{StagingBuffer, VulkanContext};
use super::dmabuf::{DmabufImporter, VulkanImportError};
use super::pipeline::{
    BlurPushConstants, BorderPushConstants, MeshGpuVertex, MeshPushConstants,
    ParticlePushConstants, QuadPushConstants, ShadowPushConstants, VulkanPipelineManager,
};
use super::VulkanTexture;
use crate::render_helpers::particles::ParticleVertex;

const MAX_IDLE_SCRATCH_IMAGES: usize = 8;
const MAX_IDLE_SCRATCH_BYTES: u64 = 64 * 1024 * 1024;

fn scratch_bytes(texture: &VulkanTexture) -> u64 {
    // Include driver alignment and tiling padding, not only logical pixels.
    texture.inner.allocation_size
}

fn recycle_scratch(
    pool: &std::sync::Mutex<Vec<VulkanTexture>>,
    textures: impl Iterator<Item = VulkanTexture>,
) {
    let mut discarded = Vec::new();
    let mut idle = pool.lock().unwrap();
    let mut bytes: u64 = idle.iter().map(scratch_bytes).sum();
    for texture in textures {
        let incoming = scratch_bytes(&texture);
        if incoming > MAX_IDLE_SCRATCH_BYTES {
            discarded.push(texture);
            continue;
        }
        while idle.len() >= MAX_IDLE_SCRATCH_IMAGES || bytes + incoming > MAX_IDLE_SCRATCH_BYTES {
            let oldest = idle.remove(0);
            bytes -= scratch_bytes(&oldest);
            discarded.push(oldest);
        }
        bytes += incoming;
        idle.push(texture);
    }
    drop(idle);
    // Image destruction must not hold the shared cache lock.
    drop(discarded);
}

enum Draw {
    Clear([f32; 4], Vec<vk::Rect2D>),
    Clip(Option<vk::Rect2D>),
    Capture(VulkanTexture, CaptureRegion),
    Quad(VulkanTexture, QuadPushConstants, bool),
    Border(BorderPushConstants),
    Shadow(ShadowPushConstants),
    Blur(VulkanTexture, BlurPushConstants),
    BlurPass(
        VulkanTexture,
        VulkanTexture,
        BlurPushConstants,
        Option<vk::Rect2D>,
    ),
    Custom(Arc<super::custom_shader::ShaderDraw>, [f32; 20]),
    Particles(
        ParticlePushConstants,
        Rc<[ParticleVertex]>,
        Vec<smithay::utils::Rectangle<i32, smithay::utils::Physical>>,
    ),
    Mesh(VulkanTexture, MeshPushConstants, Vec<MeshGpuVertex>),
}

#[derive(Default)]
struct DrawBindings {
    pipeline: Option<vk::Pipeline>,
    layout: Option<vk::PipelineLayout>,
    descriptor: Option<vk::DescriptorSet>,
    vertex_offset: Option<vk::DeviceSize>,
}

impl DrawBindings {
    unsafe fn bind(
        &mut self,
        device: &ash::Device,
        command: vk::CommandBuffer,
        pipeline: vk::Pipeline,
        layout: vk::PipelineLayout,
        descriptor: Option<vk::DescriptorSet>,
    ) {
        if self.pipeline != Some(pipeline) {
            unsafe { device.cmd_bind_pipeline(command, vk::PipelineBindPoint::GRAPHICS, pipeline) };
            self.pipeline = Some(pipeline);
        }
        if self.layout != Some(layout) {
            self.layout = Some(layout);
            self.descriptor = None;
        }
        if let Some(descriptor) = descriptor {
            if self.descriptor != Some(descriptor) {
                unsafe {
                    device.cmd_bind_descriptor_sets(
                        command,
                        vk::PipelineBindPoint::GRAPHICS,
                        layout,
                        0,
                        &[descriptor],
                        &[],
                    )
                };
                self.descriptor = Some(descriptor);
            }
        }
    }
}

struct CaptureRegion {
    offset: vk::Offset3D,
    extent: vk::Extent3D,
    scratch: Option<VulkanTexture>,
}

pub struct VulkanFrame<'frame> {
    pub(super) ctx: Arc<VulkanContext>,
    pipeline_mgr: Arc<VulkanPipelineManager>,
    scratch_pool: Arc<std::sync::Mutex<Vec<VulkanTexture>>>,
    importer: DmabufImporter,
    draws: Vec<Draw>,
    snapshot: Option<VulkanTexture>,
    pub command_buffer: vk::CommandBuffer,
    target_image: vk::Image,
    target_view: vk::ImageView,
    target_layout: vk::ImageLayout,
    target_external: bool,
    target_keepalive: Option<VulkanTexture>,
    clear_color: Option<[f32; 4]>,
    pub target_format: vk::Format,
    pub target_has_alpha: bool,
    scratch_textures: Vec<VulkanTexture>,
    pub width: u32,
    pub height: u32,
    fence: Option<vk::Fence>,
    completion_fence: vk::Fence,
    acquire_semaphores: Vec<vk::Semaphore>,
    producer_error: Option<crate::render_helpers::sync::ProducerNotReady>,
    signal_semaphore: Option<vk::Semaphore>,
    damage: Option<vk::Rect2D>,
    vertex_buffers: Vec<StagingBuffer>,
    client_buffers: Vec<WlBufferOwner>,
    gpu_timing: Option<super::timing::QueryLease>,
    #[cfg(test)]
    sync_export_error_for_test: Option<vk::Result>,
    _marker: std::marker::PhantomData<&'frame ()>,
}

pub(super) fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

impl<'frame> VulkanFrame<'frame> {
    /// Retain a known target owner even if a submitted frame fails its fence wait.
    pub(super) fn retain_target(&mut self, target: &VulkanTexture) {
        debug_assert_eq!(target.image(), self.target_image);
        debug_assert!(Arc::ptr_eq(&self.ctx, &target.inner.ctx));
        self.target_keepalive = Some(target.clone());
    }

    /// Copy an in-bounds, untransformed attachment region at this point in draw order.
    /// Each destination must be private to this capture until it is enqueued.
    pub(super) fn capture_into(
        &mut self,
        texture: &VulkanTexture,
        x: i32,
        y: i32,
    ) -> Result<(), vk::Result> {
        self.capture_region(texture, x, y, texture.width, texture.height, false)
    }

    pub(super) fn capture_scaled_into(
        &mut self,
        texture: &VulkanTexture,
        region: smithay::utils::Rectangle<i32, smithay::utils::Buffer>,
    ) -> Result<(), vk::Result> {
        if region.size.w <= 0 || region.size.h <= 0 {
            return Err(vk::Result::ERROR_INITIALIZATION_FAILED);
        }
        self.capture_region(
            texture,
            region.loc.x,
            region.loc.y,
            region.size.w as u32,
            region.size.h as u32,
            true,
        )
    }

    fn capture_region(
        &mut self,
        texture: &VulkanTexture,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        scaled: bool,
    ) -> Result<(), vk::Result> {
        let used = self.draws.iter().any(|draw| match draw {
            Draw::Quad(t, ..) | Draw::Blur(t, ..) | Draw::Capture(t, ..) | Draw::Mesh(t, ..) => {
                t.image() == texture.image()
            }
            Draw::Custom(draw, _) => draw.textures.iter().any(|t| t.image() == texture.image()),
            Draw::BlurPass(source, target, _, _) => {
                source.image() == texture.image() || target.image() == texture.image()
            }
            _ => false,
        });
        if used
            || !Arc::ptr_eq(&self.ctx, &texture.inner.ctx)
            || texture.image() == self.target_image
            || texture.inner.external
            || (!scaled && texture.format as i32 != self.target_format.as_raw())
            || !texture
                .inner
                .usage
                .contains(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
            // Opaque captures into an alpha texture go through the alpha_one render pass,
            // which requires the destination to be usable as a color attachment.
            || (!self.target_has_alpha
                && texture.has_alpha
                && !texture
                    .inner
                    .usage
                    .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT))
            || x < 0
            || y < 0
            || x as u64 + width as u64 > self.width as u64
            || y as u64 + height as u64 > self.height as u64
        {
            return Err(vk::Result::ERROR_INITIALIZATION_FAILED);
        }
        let scratch = if scaled {
            let properties = |format| unsafe {
                self.ctx
                    .instance
                    .get_physical_device_format_properties(self.ctx.physical_device, format)
                    .optimal_tiling_features
            };
            if !properties(self.target_format).contains(
                vk::FormatFeatureFlags::BLIT_SRC
                    | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR,
            ) || !properties(vk::Format::from_raw(texture.format as i32))
                .contains(vk::FormatFeatureFlags::BLIT_DST)
            {
                return Err(vk::Result::ERROR_FORMAT_NOT_SUPPORTED);
            }
            let tex = self.acquire_scratch(width, height, true)?;
            self.scratch_textures.push(tex.clone());
            Some(tex)
        } else {
            None
        };
        self.draws.push(Draw::Capture(
            texture.clone(),
            CaptureRegion {
                offset: vk::Offset3D { x, y, z: 0 },
                extent: vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                },
                scratch,
            },
        ));
        Ok(())
    }

    /// Set the physical output scissor for subsequent draws. None restores the
    /// frame damage scissor. An empty/outside rectangle suppresses drawing.
    pub fn set_clip(
        &mut self,
        clip: Option<smithay::utils::Rectangle<i32, smithay::utils::Physical>>,
    ) {
        self.draws.push(Draw::Clip(clip.map(|r| {
            super::damage::clip_rect(
                r.loc.x,
                r.loc.y,
                r.size.w,
                r.size.h,
                self.width,
                self.height,
            )
        })));
    }

    /// Clear selected output regions in command order, including alpha. A clear
    /// replaces pixels rather than blending a transparent quad over old content.
    pub fn clear_regions(
        &mut self,
        color: [f32; 4],
        regions: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
    ) -> Result<(), vk::Result> {
        if color.iter().any(|v| !v.is_finite()) {
            return Err(vk::Result::ERROR_INITIALIZATION_FAILED);
        }
        let regions = regions
            .iter()
            .map(|r| {
                super::damage::clip_rect(
                    r.loc.x,
                    r.loc.y,
                    r.size.w,
                    r.size.h,
                    self.width,
                    self.height,
                )
            })
            .filter(|r| r.extent.width != 0 && r.extent.height != 0)
            .collect::<Vec<_>>();
        if !regions.is_empty() {
            self.draws.push(Draw::Clear(color, regions));
        }
        Ok(())
    }
    pub fn begin(
        ctx: Arc<VulkanContext>,
        pipeline_mgr: Arc<VulkanPipelineManager>,
        scratch_pool: Arc<std::sync::Mutex<Vec<VulkanTexture>>>,
        importer: DmabufImporter,
        target_image: vk::Image,
        target_view: vk::ImageView,
        target_format: vk::Format,
        target_has_alpha: bool,
        width: u32,
        height: u32,
        clear_color: Option<[f32; 4]>,
        target_layout: vk::ImageLayout,
        target_external: bool,
    ) -> Result<Self, vk::Result> {
        if width == 0 || height == 0 {
            return Err(vk::Result::ERROR_INITIALIZATION_FAILED);
        }
        pipeline_mgr.get_pipelines(target_format)?;
        let (command_buffer, completion_fence) = {
            let _lock = ctx.command_lock.lock().unwrap();
            let idle = ctx.idle_commands.lock().unwrap().pop();
            if let Some((cmd, fence)) = idle {
                unsafe {
                    if let Err(err) = ctx
                        .device
                        .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())
                    {
                        ctx.device.free_command_buffers(ctx.command_pool, &[cmd]);
                        ctx.device.destroy_fence(fence, None);
                        return Err(err);
                    }
                }
                (cmd, fence)
            } else {
                let alloc = vk::CommandBufferAllocateInfo::default()
                    .command_pool(ctx.command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1);
                let cmd = unsafe { ctx.device.allocate_command_buffers(&alloc)? }[0];
                let fence = match unsafe {
                    ctx.device
                        .create_fence(&vk::FenceCreateInfo::default(), None)
                } {
                    Ok(fence) => fence,
                    Err(err) => {
                        unsafe {
                            ctx.device.free_command_buffers(ctx.command_pool, &[cmd]);
                        }
                        return Err(err);
                    }
                };
                (cmd, fence)
            }
        };
        Ok(Self {
            ctx,
            pipeline_mgr,
            scratch_pool,
            importer,
            draws: Vec::new(),
            snapshot: None,
            command_buffer,
            target_image,
            target_view,
            target_layout,
            target_external,
            target_keepalive: None,
            clear_color,
            target_format,
            target_has_alpha,
            scratch_textures: Vec::new(),
            width,
            height,
            fence: None,
            completion_fence,
            acquire_semaphores: Vec::new(),
            producer_error: None,
            signal_semaphore: None,
            damage: None,
            vertex_buffers: Vec::new(),
            client_buffers: Vec::new(),
            gpu_timing: None,
            #[cfg(test)]
            sync_export_error_for_test: None,
            _marker: std::marker::PhantomData,
        })
    }

    /// Acquire a producer on the GPU, or defer an unimportable unfinished fence.
    /// No compatibility path may wait for the producer on the desktop thread.
    pub(super) fn wait_for_producer(
        &mut self,
        sync: &smithay::backend::renderer::sync::SyncPoint,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        use std::os::fd::{AsRawFd, IntoRawFd};
        if !sync.contains_fence() {
            return Ok(false);
        }
        if self.ctx.supports_sync_file_import {
            if let Some(fd) = sync.export() {
                let semaphore = unsafe {
                    self.ctx
                        .device
                        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?
                };
                let api = ash::khr::external_semaphore_fd::Device::new(
                    &self.ctx.instance,
                    &self.ctx.device,
                );
                let import = vk::ImportSemaphoreFdInfoKHR::default()
                    .semaphore(semaphore)
                    .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
                    .flags(vk::SemaphoreImportFlags::TEMPORARY)
                    .fd(fd.as_raw_fd());
                match unsafe { api.import_semaphore_fd(&import) } {
                    Ok(()) => {
                        let _ = fd.into_raw_fd();
                        self.acquire_semaphores.push(semaphore);
                        return Ok(true);
                    }
                    Err(err) => {
                        unsafe {
                            self.ctx.device.destroy_semaphore(semaphore, None);
                        }
                        debug!("Vulkan acquire import unavailable ({err}); checking producer readiness");
                    }
                }
            }
        }
        if let Err(pending) = crate::render_helpers::sync::require_ready(sync) {
            // Latch the refusal for this frame: even a caller that ignores the
            // acquisition error cannot submit commands without the dependency.
            if !matches!(
                self.producer_error,
                Some(crate::render_helpers::sync::ProducerNotReady::Failed(_))
            ) {
                self.producer_error = Some(pending);
            }
            return Err(Box::new(pending));
        }
        Ok(false)
    }

    pub(super) fn producer_error(&self) -> Option<crate::render_helpers::sync::ProducerNotReady> {
        self.producer_error
    }

    fn matrix(&self, x: f32, y: f32, w: f32, h: f32) -> [f32; 16] {
        [
            w / self.width as f32 * 2.,
            0.,
            0.,
            0.,
            0.,
            h / self.height as f32 * 2.,
            0.,
            0.,
            0.,
            0.,
            1.,
            0.,
            x / self.width as f32 * 2. - 1.,
            y / self.height as f32 * 2. - 1.,
            0.,
            1.,
        ]
    }
    fn check_texture(&self, texture: &VulkanTexture) -> Result<(), vk::Result> {
        if !Arc::ptr_eq(&self.ctx, &texture.inner.ctx)
            || texture.image() == self.target_image
            || !texture.inner.usage.contains(vk::ImageUsageFlags::SAMPLED)
        {
            return Err(vk::Result::ERROR_INITIALIZATION_FAILED);
        }
        Ok(())
    }
    pub fn draw_texture(
        &mut self,
        texture: &VulkanTexture,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        uv: Option<[f32; 4]>,
        alpha: f32,
        tint: Option<[f32; 4]>,
        radius: f32,
    ) -> Result<(), vk::Result> {
        self.check_texture(texture)?;
        self.draws.push(Draw::Quad(
            texture.clone(),
            QuadPushConstants {
                matrix: self.matrix(x, y, w, h),
                uv_bounds: uv.unwrap_or([0., 0., 1., 1.]),
                color_tint: tint.unwrap_or([1.; 4]),
                alpha,
                corner_radius: radius,
                size: [w, h],
                clip: Default::default(),
                noise: 0.,
                saturation: 1.,
                target_size: [self.width as f32, self.height as f32],
                bg_color: [0.; 4],
                fade_cutoff: [0.; 2],
                hdr_transfer: 0.,
            },
            true,
        ));
        Ok(())
    }

    pub(super) fn draw_texture_hdr(&mut self, texture: &VulkanTexture) -> Result<(), vk::Result> {
        self.check_texture(texture)?;
        self.draws.push(Draw::Quad(
            texture.clone(),
            QuadPushConstants {
                matrix: self.matrix(0., 0., self.width as f32, self.height as f32),
                uv_bounds: [0., 0., 1., 1.],
                color_tint: [1.; 4],
                alpha: 1.,
                corner_radius: 0.,
                size: [self.width as f32, self.height as f32],
                clip: Default::default(),
                noise: 0.,
                saturation: 1.,
                target_size: [self.width as f32, self.height as f32],
                bg_color: [0.; 4],
                fade_cutoff: [0.; 2],
                hdr_transfer: 1.,
            },
            // This is a pixel-for-pixel transfer. Filtering across a damage
            // edge would mix freshly composed SDR with retained PQ pixels.
            false,
        ));
        Ok(())
    }

    fn acquire_scratch(
        &self,
        width: u32,
        height: u32,
        has_alpha: bool,
    ) -> Result<VulkanTexture, vk::Result> {
        let usage = vk::ImageUsageFlags::TRANSFER_SRC
            | vk::ImageUsageFlags::TRANSFER_DST
            | vk::ImageUsageFlags::SAMPLED;
        let mut pool = self.scratch_pool.lock().unwrap();
        let index = pool.iter().position(|texture| {
            Arc::ptr_eq(&self.ctx, &texture.inner.ctx)
                && !texture.inner.external
                && !texture.y_inverted
                && texture.width == width
                && texture.height == height
                && texture.format == self.target_format.as_raw() as u32
                && texture.has_alpha == has_alpha
                && texture.inner.usage.contains(usage)
        });
        if let Some(index) = index {
            return Ok(pool.swap_remove(index));
        }
        drop(pool);
        super::allocate_texture(
            &self.ctx,
            width,
            height,
            self.target_format,
            vk::ImageTiling::OPTIMAL,
            usage,
            vk::ImageLayout::UNDEFINED,
            has_alpha,
        )
    }

    pub(super) fn encode_hdr_output(
        &mut self,
        damage: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
    ) -> Result<(), vk::Result> {
        let damage = crate::render_helpers::damage_region::disjoint_damage(
            damage,
            (self.width as i32, self.height as i32).into(),
        );
        if damage.is_empty() {
            return Ok(());
        }
        let scratch = self.acquire_scratch(self.width, self.height, self.target_has_alpha)?;
        self.capture_into(&scratch, 0, 0)?;
        self.scratch_textures.push(scratch.clone());
        // Retained pixels are already PQ encoded. Convert each newly composed
        // scene pixel exactly once, leaving all undamaged output bytes intact.
        for rect in damage {
            self.set_clip(Some(rect));
            self.draw_texture_hdr(&scratch)?;
        }
        self.set_clip(None);
        Ok(())
    }

    pub(super) fn draw_quad(
        &mut self,
        texture: &VulkanTexture,
        pc: QuadPushConstants,
        linear: bool,
    ) -> Result<(), vk::Result> {
        self.check_texture(texture)?;
        self.draws.push(Draw::Quad(texture.clone(), pc, linear));
        Ok(())
    }
    pub fn draw_border(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        mut pc: BorderPushConstants,
    ) -> Result<(), vk::Result> {
        pc.matrix = self.matrix(x, y, w, h);
        pc.geo_size = [w, h];
        self.draws.push(Draw::Border(pc));
        Ok(())
    }
    pub(super) fn draw_border_constants(&mut self, pc: BorderPushConstants) {
        self.draws.push(Draw::Border(pc));
    }
    pub(super) fn draw_shadow_constants(&mut self, pc: ShadowPushConstants) {
        self.draws.push(Draw::Shadow(pc));
    }
    pub(super) fn draw_blur(
        &mut self,
        texture: &VulkanTexture,
        pc: BlurPushConstants,
    ) -> Result<(), vk::Result> {
        self.check_texture(texture)?;
        self.draws.push(Draw::Blur(texture.clone(), pc));
        Ok(())
    }
    pub(super) fn blur_pass(
        &mut self,
        source: &VulkanTexture,
        target: &VulkanTexture,
        pc: BlurPushConstants,
        region: Option<vk::Rect2D>,
    ) -> Result<(), vk::Result> {
        self.check_texture(source)?;
        self.check_texture(target)?;
        if source.image() == target.image()
            || target.inner.external
            || !target
                .inner
                .usage
                .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
            || self
                .draws
                .iter()
                .any(|draw| matches!(draw, Draw::Capture(t, _) if t.image() == target.image()))
        {
            return Err(vk::Result::ERROR_INITIALIZATION_FAILED);
        }
        // Resolve format-specific pipelines before mutating the deferred frame.
        self.pipeline_mgr
            .get_pipelines(vk::Format::from_raw(target.format as i32))?;
        self.draws
            .push(Draw::BlurPass(source.clone(), target.clone(), pc, region));
        Ok(())
    }

    pub(super) fn draw_particles(
        &mut self,
        pc: ParticlePushConstants,
        vertices: Rc<[ParticleVertex]>,
        clips: Vec<smithay::utils::Rectangle<i32, smithay::utils::Physical>>,
    ) {
        self.draws.push(Draw::Particles(pc, vertices, clips));
    }
    pub(super) fn draw_mesh(
        &mut self,
        texture: &VulkanTexture,
        pc: MeshPushConstants,
        vertices: Vec<MeshGpuVertex>,
    ) -> Result<(), vk::Result> {
        self.check_texture(texture)?;
        if vertices.is_empty() || !vertices.len().is_multiple_of(3) {
            return Ok(());
        }
        self.draws.push(Draw::Mesh(texture.clone(), pc, vertices));
        Ok(())
    }
    pub fn draw_texture_dmabuf(
        &mut self,
        dmabuf: &Dmabuf,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        uv: Option<[f32; 4]>,
        alpha: f32,
        tint: Option<[f32; 4]>,
        radius: f32,
    ) -> Result<(), VulkanImportError> {
        let tex = self.importer.import_dmabuf(dmabuf)?;
        self.draw_texture(
            &tex,
            x,
            y,
            w,
            h,
            uv.or_else(|| dmabuf.y_inverted().then_some([0., 1., 1., 0.])),
            alpha,
            tint,
            radius,
        )?;
        Ok(())
    }
    /// Preserve the fully composited GLES scene through a GPU-only intermediate.
    /// This respects popups, layer surfaces, output transforms and all existing effects.
    pub(super) fn composite_scene(
        &mut self,
        scratch: VulkanTexture,
        damage: Option<vk::Rect2D>,
    ) -> Result<(), vk::Result> {
        self.draw_texture(
            &scratch,
            0.,
            0.,
            self.width as f32,
            self.height as f32,
            None,
            1.,
            None,
            0.,
        )?;
        self.snapshot = Some(scratch);
        self.damage = damage;
        self.clear_color = if damage.is_none() {
            Some([0.; 4])
        } else {
            None
        };
        Ok(())
    }

    fn barrier(
        &self,
        image: vk::Image,
        old: vk::ImageLayout,
        new: vk::ImageLayout,
        src: vk::AccessFlags,
        dst: vk::AccessFlags,
        src_stage: vk::PipelineStageFlags,
        dst_stage: vk::PipelineStageFlags,
        acquire: bool,
        release: bool,
    ) {
        let barrier = vk::ImageMemoryBarrier::default()
            .image(image)
            .old_layout(old)
            .new_layout(new)
            .src_access_mask(src)
            .dst_access_mask(dst)
            .subresource_range(color_range())
            .src_queue_family_index(if acquire {
                vk::QUEUE_FAMILY_FOREIGN_EXT
            } else if release {
                self.ctx.graphics_queue_family
            } else {
                vk::QUEUE_FAMILY_IGNORED
            })
            .dst_queue_family_index(if release {
                vk::QUEUE_FAMILY_FOREIGN_EXT
            } else if acquire {
                self.ctx.graphics_queue_family
            } else {
                vk::QUEUE_FAMILY_IGNORED
            });
        unsafe {
            self.ctx.device.cmd_pipeline_barrier(
                self.command_buffer,
                src_stage,
                dst_stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
    }

    pub(super) fn draw_custom(
        &mut self,
        draw: Arc<super::custom_shader::ShaderDraw>,
        pc: [f32; 20],
    ) {
        self.draws.push(Draw::Custom(draw, pc));
    }

    pub(super) fn custom_draw(
        &self,
        program: &Arc<super::custom_shader::Program>,
        textures: &std::collections::HashMap<String, VulkanTexture>,
        uniforms: &[smithay::backend::renderer::gles::Uniform<'_>],
    ) -> anyhow::Result<Arc<super::custom_shader::ShaderDraw>> {
        for texture in textures.values() {
            self.check_texture(texture)?;
        }
        program.draw(self.target_format, textures, uniforms)
    }

    /// Find client allocations that will be sampled by this submission. Native
    /// rendering uses these handles to retain Smithay buffer owners until the
    /// GPU fence completes; the imported VkImage alone does not own wl_buffer.
    /// Draw::Capture's texture is a destination and capture_region rejects an
    /// external destination, so it cannot add a sampled client allocation.
    pub(super) fn external_sampled_textures(&self) -> Vec<&VulkanTexture> {
        let mut textures = Vec::new();
        for draw in &self.draws {
            match draw {
                Draw::Quad(texture, ..) | Draw::Blur(texture, ..) | Draw::Mesh(texture, ..)
                    if texture.inner.external =>
                {
                    textures.push(texture);
                }
                Draw::BlurPass(source, _, _, _) if source.inner.external => textures.push(source),
                Draw::Custom(draw, _) => textures.extend(
                    draw.textures
                        .iter()
                        .filter(|texture| texture.inner.external),
                ),
                _ => {}
            }
        }
        textures
    }

    pub(super) fn sampled_yuv_targets(
        &self,
    ) -> Vec<(Arc<super::pipeline::YuvPipeline>, vk::Format)> {
        let mut targets = Vec::new();
        for draw in &self.draws {
            let (source, format) = match draw {
                Draw::Quad(source, ..) | Draw::Blur(source, ..) | Draw::Mesh(source, ..) => {
                    (source, self.target_format)
                }
                Draw::BlurPass(source, target, ..) => {
                    (source, vk::Format::from_raw(target.format as i32))
                }
                _ => continue,
            };
            if let Some(variant) = &source.inner.yuv_pipeline {
                targets.push((variant.clone(), format));
            }
        }
        targets
    }

    pub(super) fn retain_client_buffers(&mut self, buffers: Vec<WlBufferOwner>) {
        self.client_buffers = buffers;
    }

    pub(super) fn measure_gpu_work(&mut self, timing: Option<super::timing::QueryLease>) {
        self.gpu_timing = timing;
    }

    fn submit(&mut self, wait: bool) -> Result<(), vk::Result> {
        if let Some(error) = self.producer_error {
            return Err(match error {
                crate::render_helpers::sync::ProducerNotReady::Pending => vk::Result::NOT_READY,
                crate::render_helpers::sync::ProducerNotReady::Failed(error) => error,
            });
        }
        let _command_lock = self.ctx.command_lock.lock().unwrap();
        let pipes = self.pipeline_mgr.get_pipelines(self.target_format)?;
        // Allocate descriptors before recording; failure leaves external buffers untouched.
        let mut textures = Vec::new();
        let mut texture_images = HashSet::new();
        let mut capture_images = HashSet::new();
        let mut descriptors = Vec::with_capacity(self.draws.len());
        for draw in &self.draws {
            match draw {
                Draw::Quad(t, _, _)
                | Draw::Blur(t, _)
                | Draw::BlurPass(t, _, _, _)
                | Draw::Mesh(t, _, _) => {
                    descriptors.push(Some(
                        self.pipeline_mgr.get_or_create_texture_descriptor_set(
                            t,
                            !matches!(draw, Draw::Quad(_, _, false))
                                && self
                                    .snapshot
                                    .as_ref()
                                    .is_none_or(|s| s.image() != t.image()),
                        )?,
                    ));
                    if texture_images.insert(t.image()) {
                        textures.push(t);
                    }
                }
                Draw::Capture(t, _) => {
                    capture_images.insert(t.image());
                    descriptors.push(None);
                    if texture_images.insert(t.image()) {
                        textures.push(t);
                    }
                }
                Draw::Custom(draw, _) => {
                    descriptors.push(Some(draw.set));
                    for texture in &draw.textures {
                        if texture_images.insert(texture.image()) {
                            textures.push(texture);
                        }
                    }
                }
                Draw::Shadow(_)
                | Draw::Border(_)
                | Draw::Particles(..)
                | Draw::Clear(..)
                | Draw::Clip(_) => descriptors.push(None),
            }
        }
        for draw in &self.draws {
            if let Draw::BlurPass(_, t, _, _) = draw {
                if texture_images.insert(t.image()) {
                    textures.push(t);
                }
            }
        }
        let mesh_vertex_count = self.draws.iter().try_fold(0usize, |count, draw| {
            if let Draw::Mesh(_, _, vertices) = draw {
                count
                    .checked_add(vertices.len())
                    .ok_or(vk::Result::ERROR_OUT_OF_HOST_MEMORY)
            } else {
                Ok(count)
            }
        })?;
        if mesh_vertex_count > u32::MAX as usize {
            return Err(vk::Result::ERROR_OUT_OF_HOST_MEMORY);
        }
        let mesh_size = mesh_vertex_count
            .checked_mul(std::mem::size_of::<MeshGpuVertex>())
            .ok_or(vk::Result::ERROR_OUT_OF_HOST_MEMORY)?;
        let particle_vertex_count = self.draws.iter().try_fold(0usize, |count, draw| {
            if let Draw::Particles(_, vertices, _) = draw {
                count
                    .checked_add(vertices.len())
                    .ok_or(vk::Result::ERROR_OUT_OF_HOST_MEMORY)
            } else {
                Ok(count)
            }
        })?;
        if particle_vertex_count > u32::MAX as usize {
            return Err(vk::Result::ERROR_OUT_OF_HOST_MEMORY);
        }
        let vertex_size = particle_vertex_count
            .checked_mul(std::mem::size_of::<ParticleVertex>())
            .and_then(|size| size.checked_add(mesh_size))
            .ok_or(vk::Result::ERROR_OUT_OF_HOST_MEMORY)?;
        let vertex_buffer = if vertex_size != 0 {
            // Mesh and particle batches share one pooled, host-coherent upload per frame.
            // Meshes occupy the first region; particles follow with their own vertex stride.
            let staging = self.ctx.acquire_vertex_buffer(vertex_size)?;
            let mapped = unsafe {
                self.ctx.device.map_memory(
                    staging.memory,
                    0,
                    vertex_size as vk::DeviceSize,
                    vk::MemoryMapFlags::empty(),
                )
            };
            let mapped = match mapped {
                Ok(mapped) => mapped.cast::<u8>(),
                Err(err) => {
                    self.ctx.release_vertex_buffer(staging);
                    return Err(err);
                }
            };
            let meshes = self.draws.iter().filter_map(|draw| match draw {
                Draw::Mesh(_, _, vertices) => Some(bytemuck::cast_slice::<_, u8>(vertices)),
                _ => None,
            });
            let particles = self.draws.iter().filter_map(|draw| match draw {
                Draw::Particles(_, vertices, _) => Some(bytemuck::cast_slice::<_, u8>(vertices)),
                _ => None,
            });
            let mut offset = 0;
            for bytes in meshes.chain(particles) {
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.add(offset), bytes.len());
                }
                offset += bytes.len();
            }
            unsafe { self.ctx.device.unmap_memory(staging.memory) };
            let buffer = staging.buffer;
            self.vertex_buffers.push(staging);
            Some(buffer)
        } else {
            None
        };
        let device = &self.ctx.device;
        unsafe {
            device.begin_command_buffer(
                self.command_buffer,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
        }
        if let Some(timing) = &self.gpu_timing {
            timing.begin(self.command_buffer);
        }
        if let Some(scratch) = &self.snapshot {
            self.barrier(
                self.target_image,
                self.target_layout,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_READ,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                self.target_external,
                false,
            );
            self.barrier(
                scratch.image(),
                scratch.current_layout().get(),
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::AccessFlags::MEMORY_READ,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                false,
                false,
            );
            let layers = vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1);
            let damage = self.damage.unwrap_or(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: vk::Extent2D {
                    width: self.width,
                    height: self.height,
                },
            });
            let offset = vk::Offset3D {
                x: damage.offset.x,
                y: damage.offset.y,
                z: 0,
            };
            let region = vk::ImageCopy::default()
                .src_offset(offset)
                .dst_offset(offset)
                .src_subresource(layers)
                .dst_subresource(layers)
                .extent(vk::Extent3D {
                    width: damage.extent.width,
                    height: damage.extent.height,
                    depth: 1,
                });
            unsafe {
                device.cmd_copy_image(
                    self.command_buffer,
                    self.target_image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    scratch.image(),
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
            }
        }
        // A persistent offscreen target may still be sampled by the previous
        // frame's submission. TOP_OF_PIPE/empty would not order those reads
        // ahead of the new attachment writes, so derive the source scope from
        // the tracked layout instead. External targets keep relying on their
        // acquire-side synchronization, and a fresh image has no prior work.
        let (target_src_stage, target_src_access) = if self.snapshot.is_some() {
            (
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::TRANSFER_READ,
            )
        } else if !self.target_external
            && !matches!(
                self.target_layout,
                vk::ImageLayout::PREINITIALIZED | vk::ImageLayout::UNDEFINED
            )
        {
            (
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::AccessFlags::MEMORY_READ,
            )
        } else {
            (
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::AccessFlags::empty(),
            )
        };
        self.barrier(
            self.target_image,
            if self.snapshot.is_some() {
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL
            } else {
                self.target_layout
            },
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            target_src_access,
            vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            target_src_stage,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            self.target_external && self.snapshot.is_none(),
            false,
        );
        for t in &textures {
            // Capture destinations become readable only at their ordered copy.
            if capture_images.contains(&t.image()) {
                continue;
            }
            let is_snapshot = self
                .snapshot
                .as_ref()
                .is_some_and(|s| s.image() == t.image());
            let old = if is_snapshot {
                vk::ImageLayout::TRANSFER_DST_OPTIMAL
            } else {
                t.current_layout().get()
            };
            let (stage, access) = if is_snapshot {
                (
                    vk::PipelineStageFlags::TRANSFER,
                    vk::AccessFlags::TRANSFER_WRITE,
                )
            } else if old == vk::ImageLayout::PREINITIALIZED {
                (vk::PipelineStageFlags::HOST, vk::AccessFlags::HOST_WRITE)
            } else {
                (
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::AccessFlags::MEMORY_WRITE,
                )
            };
            self.barrier(
                t.image(),
                old,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                access,
                vk::AccessFlags::SHADER_READ,
                stage,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                t.inner.external,
                false,
            );
        }
        let attachment = [vk::RenderingAttachmentInfo::default()
            .image_view(self.target_view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(if self.clear_color.is_some() {
                vk::AttachmentLoadOp::CLEAR
            } else {
                vk::AttachmentLoadOp::LOAD
            })
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: self.clear_color.unwrap_or([0.; 4]),
                },
            })];
        let rect = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: vk::Extent2D {
                width: self.width,
                height: self.height,
            },
        };
        let rendering = vk::RenderingInfo::default()
            .render_area(self.damage.unwrap_or(rect))
            .layer_count(1)
            .color_attachments(&attachment);
        unsafe {
            device.cmd_begin_rendering(self.command_buffer, &rendering);
            device.cmd_set_viewport(
                self.command_buffer,
                0,
                &[vk::Viewport {
                    x: 0.,
                    y: 0.,
                    width: self.width as f32,
                    height: self.height as f32,
                    min_depth: 0.,
                    max_depth: 1.,
                }],
            );
            let draw_rect = self.damage.unwrap_or(rect);
            if self.damage.is_some() {
                // Clear only the redraw area; alpha must not blend over the old scene twice.
                let attachment = vk::ClearAttachment::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .color_attachment(0)
                    .clear_value(vk::ClearValue {
                        color: vk::ClearColorValue { float32: [0.; 4] },
                    });
                device.cmd_clear_attachments(
                    self.command_buffer,
                    &[attachment],
                    &[vk::ClearRect::default().rect(draw_rect).layer_count(1)],
                );
            }
            device.cmd_set_scissor(self.command_buffer, 0, &[draw_rect]);
            let mut current_scissor = draw_rect;
            let mut bindings = DrawBindings::default();
            let mut first_mesh_vertex = 0u32;
            let mut first_particle_vertex = 0u32;
            for (draw, ds) in self.draws.iter().zip(descriptors) {
                let (pipeline, layout, bytes) = match draw {
                    Draw::BlurPass(source, target, pc, region) => {
                        bindings = DrawBindings::default();
                        device.cmd_end_rendering(self.command_buffer);
                        self.barrier(
                            target.image(),
                            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                            vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                            vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                            vk::PipelineStageFlags::ALL_COMMANDS,
                            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                            false,
                            false,
                        );
                        let blur_attachment = [vk::RenderingAttachmentInfo::default()
                            .image_view(target.image_view())
                            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                            .load_op(vk::AttachmentLoadOp::CLEAR)
                            .store_op(vk::AttachmentStoreOp::STORE)
                            .clear_value(vk::ClearValue {
                                color: vk::ClearColorValue { float32: [0.; 4] },
                            })];
                        let blur_rect = region.unwrap_or(vk::Rect2D {
                            offset: vk::Offset2D::default(),
                            extent: vk::Extent2D {
                                width: target.width,
                                height: target.height,
                            },
                        });
                        let blur_rendering = vk::RenderingInfo::default()
                            .render_area(blur_rect)
                            .layer_count(1)
                            .color_attachments(&blur_attachment);
                        device.cmd_begin_rendering(self.command_buffer, &blur_rendering);
                        device.cmd_set_viewport(
                            self.command_buffer,
                            0,
                            &[vk::Viewport {
                                x: 0.,
                                y: 0.,
                                width: target.width as f32,
                                height: target.height as f32,
                                min_depth: 0.,
                                max_depth: 1.,
                            }],
                        );
                        device.cmd_set_scissor(self.command_buffer, 0, &[blur_rect]);
                        let target_format = vk::Format::from_raw(target.format as i32);
                        let (blur_pipeline, layout) = if let Some(yuv) = &source.inner.yuv_pipeline
                        {
                            (yuv.get_pipelines(target_format)?.blur, yuv.layout)
                        } else {
                            (
                                self.pipeline_mgr
                                    .get_pipelines(target_format)?
                                    .blur_pipeline,
                                self.pipeline_mgr.quad_pipeline_layout,
                            )
                        };
                        device.cmd_bind_pipeline(
                            self.command_buffer,
                            vk::PipelineBindPoint::GRAPHICS,
                            blur_pipeline,
                        );
                        device.cmd_bind_descriptor_sets(
                            self.command_buffer,
                            vk::PipelineBindPoint::GRAPHICS,
                            layout,
                            0,
                            &[ds.unwrap()],
                            &[],
                        );
                        device.cmd_push_constants(
                            self.command_buffer,
                            layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            bytemuck::bytes_of(pc),
                        );
                        device.cmd_draw(self.command_buffer, 4, 1, 0, 0);
                        device.cmd_end_rendering(self.command_buffer);
                        self.barrier(
                            target.image(),
                            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                            vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                            vk::AccessFlags::SHADER_READ,
                            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                            vk::PipelineStageFlags::FRAGMENT_SHADER,
                            false,
                            false,
                        );
                        self.barrier(
                            self.target_image,
                            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                            vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                            vk::AccessFlags::COLOR_ATTACHMENT_READ
                                | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                            false,
                            false,
                        );
                        // Preserve the main attachment and restore its viewport and active damage
                        // clip.
                        let resumed_attachment =
                            [attachment[0].load_op(vk::AttachmentLoadOp::LOAD)];
                        let resumed = rendering.color_attachments(&resumed_attachment);
                        device.cmd_begin_rendering(self.command_buffer, &resumed);
                        device.cmd_set_viewport(
                            self.command_buffer,
                            0,
                            &[vk::Viewport {
                                x: 0.,
                                y: 0.,
                                width: self.width as f32,
                                height: self.height as f32,
                                min_depth: 0.,
                                max_depth: 1.,
                            }],
                        );
                        device.cmd_set_scissor(self.command_buffer, 0, &[current_scissor]);
                        continue;
                    }
                    Draw::Capture(texture, capture) => {
                        bindings = DrawBindings::default();
                        device.cmd_end_rendering(self.command_buffer);
                        self.barrier(
                            self.target_image,
                            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                            vk::AccessFlags::TRANSFER_READ,
                            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                            vk::PipelineStageFlags::TRANSFER,
                            false,
                            false,
                        );
                        self.barrier(
                            texture.image(),
                            texture.current_layout().get(),
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                            vk::AccessFlags::TRANSFER_WRITE,
                            vk::PipelineStageFlags::ALL_COMMANDS,
                            vk::PipelineStageFlags::TRANSFER,
                            false,
                            false,
                        );
                        let layers = vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1);
                        if let Some(scratch) = &capture.scratch {
                            self.barrier(
                                scratch.image(),
                                vk::ImageLayout::UNDEFINED,
                                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                                vk::AccessFlags::empty(),
                                vk::AccessFlags::TRANSFER_WRITE,
                                vk::PipelineStageFlags::TOP_OF_PIPE,
                                vk::PipelineStageFlags::TRANSFER,
                                false,
                                false,
                            );
                            let crop = vk::ImageCopy::default()
                                .src_subresource(layers)
                                .dst_subresource(layers)
                                .src_offset(capture.offset)
                                .extent(capture.extent);
                            device.cmd_copy_image(
                                self.command_buffer,
                                self.target_image,
                                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                                scratch.image(),
                                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                                &[crop],
                            );
                            self.barrier(
                                scratch.image(),
                                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                                vk::AccessFlags::TRANSFER_WRITE,
                                vk::AccessFlags::TRANSFER_READ,
                                vk::PipelineStageFlags::TRANSFER,
                                vk::PipelineStageFlags::TRANSFER,
                                false,
                                false,
                            );
                            let region = vk::ImageBlit::default()
                                .src_subresource(layers)
                                .dst_subresource(layers)
                                .src_offsets([
                                    vk::Offset3D::default(),
                                    vk::Offset3D {
                                        x: capture.extent.width as i32,
                                        y: capture.extent.height as i32,
                                        z: 1,
                                    },
                                ])
                                .dst_offsets([
                                    vk::Offset3D::default(),
                                    vk::Offset3D {
                                        x: texture.width as i32,
                                        y: texture.height as i32,
                                        z: 1,
                                    },
                                ]);
                            device.cmd_blit_image(
                                self.command_buffer,
                                scratch.image(),
                                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                                texture.image(),
                                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                                &[region],
                                vk::Filter::LINEAR,
                            );
                        } else {
                            let region = vk::ImageCopy::default()
                                .src_subresource(layers)
                                .dst_subresource(layers)
                                .src_offset(capture.offset)
                                .extent(capture.extent);
                            device.cmd_copy_image(
                                self.command_buffer,
                                self.target_image,
                                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                                texture.image(),
                                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                                &[region],
                            );
                        }
                        if !self.target_has_alpha && texture.has_alpha {
                            // LOAD preserves the captured RGB while the pass fixes alpha.
                            // The layout transition must make those reads visible too.
                            self.barrier(
                                texture.image(),
                                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                                vk::AccessFlags::TRANSFER_WRITE,
                                vk::AccessFlags::COLOR_ATTACHMENT_READ
                                    | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                                vk::PipelineStageFlags::TRANSFER,
                                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                                false,
                                false,
                            );
                            let target_view = texture.image_view();
                            let pass_attachment = [vk::RenderingAttachmentInfo::default()
                                .image_view(target_view)
                                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                                .load_op(vk::AttachmentLoadOp::LOAD)
                                .store_op(vk::AttachmentStoreOp::STORE)];
                            let pass_rendering = vk::RenderingInfo::default()
                                .render_area(vk::Rect2D {
                                    offset: vk::Offset2D { x: 0, y: 0 },
                                    extent: vk::Extent2D {
                                        width: texture.width,
                                        height: texture.height,
                                    },
                                })
                                .layer_count(1)
                                .color_attachments(&pass_attachment);
                            let pipes = self
                                .pipeline_mgr
                                .get_pipelines(vk::Format::from_raw(texture.format as i32))?;
                            device.cmd_begin_rendering(self.command_buffer, &pass_rendering);
                            device.cmd_bind_pipeline(
                                self.command_buffer,
                                vk::PipelineBindPoint::GRAPHICS,
                                pipes.alpha_one_pipeline,
                            );
                            device.cmd_set_viewport(
                                self.command_buffer,
                                0,
                                &[vk::Viewport {
                                    x: 0.,
                                    y: 0.,
                                    width: texture.width as f32,
                                    height: texture.height as f32,
                                    min_depth: 0.,
                                    max_depth: 1.,
                                }],
                            );
                            device.cmd_set_scissor(
                                self.command_buffer,
                                0,
                                &[vk::Rect2D {
                                    offset: vk::Offset2D { x: 0, y: 0 },
                                    extent: vk::Extent2D {
                                        width: texture.width,
                                        height: texture.height,
                                    },
                                }],
                            );
                            device.cmd_draw(self.command_buffer, 4, 1, 0, 0);
                            device.cmd_end_rendering(self.command_buffer);
                            self.barrier(
                                texture.image(),
                                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                                vk::AccessFlags::SHADER_READ,
                                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                                vk::PipelineStageFlags::FRAGMENT_SHADER,
                                false,
                                false,
                            );
                        } else {
                            self.barrier(
                                texture.image(),
                                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                                vk::AccessFlags::TRANSFER_WRITE,
                                vk::AccessFlags::SHADER_READ,
                                vk::PipelineStageFlags::TRANSFER,
                                vk::PipelineStageFlags::FRAGMENT_SHADER,
                                false,
                                false,
                            );
                        }
                        self.barrier(
                            self.target_image,
                            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                            vk::AccessFlags::TRANSFER_READ,
                            vk::AccessFlags::COLOR_ATTACHMENT_READ
                                | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                            vk::PipelineStageFlags::TRANSFER,
                            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                            false,
                            false,
                        );
                        // Resuming must preserve all previous draws even if the original scope
                        // cleared.
                        let resumed_attachment =
                            [attachment[0].load_op(vk::AttachmentLoadOp::LOAD)];
                        let resumed = rendering.color_attachments(&resumed_attachment);
                        device.cmd_begin_rendering(self.command_buffer, &resumed);
                        device.cmd_set_viewport(
                            self.command_buffer,
                            0,
                            &[vk::Viewport {
                                x: 0.,
                                y: 0.,
                                width: self.width as f32,
                                height: self.height as f32,
                                min_depth: 0.,
                                max_depth: 1.,
                            }],
                        );
                        device.cmd_set_scissor(self.command_buffer, 0, &[current_scissor]);
                        continue;
                    }
                    Draw::Clip(clip) => {
                        let clip =
                            clip.map_or(draw_rect, |r| super::damage::intersection(r, draw_rect));
                        current_scissor = clip;
                        device.cmd_set_scissor(self.command_buffer, 0, &[clip]);
                        continue;
                    }
                    Draw::Clear(color, regions) => {
                        let rects = regions
                            .iter()
                            .map(|r| super::damage::intersection(*r, draw_rect))
                            .filter(|r| r.extent.width != 0 && r.extent.height != 0)
                            .map(|r| vk::ClearRect::default().rect(r).layer_count(1))
                            .collect::<Vec<_>>();
                        if !rects.is_empty() {
                            let attachment = vk::ClearAttachment::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .color_attachment(0)
                                .clear_value(vk::ClearValue {
                                    color: vk::ClearColorValue { float32: *color },
                                });
                            device.cmd_clear_attachments(
                                self.command_buffer,
                                &[attachment],
                                &rects,
                            );
                        }
                        continue;
                    }
                    Draw::Quad(t, pc, _) => {
                        if let Some(yuv) = &t.inner.yuv_pipeline {
                            let variant = yuv.get_pipelines(self.target_format)?;
                            (variant.quad, yuv.layout, bytemuck::bytes_of(pc))
                        } else {
                            (
                                pipes.quad_pipeline,
                                self.pipeline_mgr.quad_pipeline_layout,
                                bytemuck::bytes_of(pc),
                            )
                        }
                    }
                    Draw::Border(pc) => (
                        pipes.border_pipeline,
                        self.pipeline_mgr.border_pipeline_layout,
                        bytemuck::bytes_of(pc),
                    ),
                    Draw::Shadow(pc) => (
                        pipes.shadow_pipeline,
                        self.pipeline_mgr.border_pipeline_layout,
                        bytemuck::bytes_of(pc),
                    ),
                    Draw::Blur(t, pc) => {
                        if let Some(yuv) = &t.inner.yuv_pipeline {
                            let variant = yuv.get_pipelines(self.target_format)?;
                            (variant.blur, yuv.layout, bytemuck::bytes_of(pc))
                        } else {
                            (
                                pipes.blur_pipeline,
                                self.pipeline_mgr.quad_pipeline_layout,
                                bytemuck::bytes_of(pc),
                            )
                        }
                    }
                    Draw::Custom(draw, pc) => {
                        (draw.pipeline, draw.program.layout, bytemuck::cast_slice(pc))
                    }
                    Draw::Particles(pc, vertices, clips) => {
                        let count = vertices.len() as u32;
                        let layout = self.pipeline_mgr.border_pipeline_layout;
                        bindings.bind(
                            device,
                            self.command_buffer,
                            pipes.particle_pipeline,
                            layout,
                            None,
                        );
                        if bindings.vertex_offset != Some(mesh_size as vk::DeviceSize) {
                            device.cmd_bind_vertex_buffers(
                                self.command_buffer,
                                0,
                                &[vertex_buffer.unwrap()],
                                &[mesh_size as vk::DeviceSize],
                            );
                            bindings.vertex_offset = Some(mesh_size as vk::DeviceSize);
                        }
                        device.cmd_push_constants(
                            self.command_buffer,
                            layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            bytemuck::bytes_of(pc),
                        );
                        for clip in clips {
                            let clip = super::damage::intersection(
                                vk::Rect2D {
                                    offset: vk::Offset2D {
                                        x: clip.loc.x,
                                        y: clip.loc.y,
                                    },
                                    extent: vk::Extent2D {
                                        width: clip.size.w as u32,
                                        height: clip.size.h as u32,
                                    },
                                },
                                current_scissor,
                            );
                            if clip.extent.width != 0 && clip.extent.height != 0 {
                                device.cmd_set_scissor(self.command_buffer, 0, &[clip]);
                                device.cmd_draw(
                                    self.command_buffer,
                                    count,
                                    1,
                                    first_particle_vertex,
                                    0,
                                );
                            }
                        }
                        device.cmd_set_scissor(self.command_buffer, 0, &[current_scissor]);
                        first_particle_vertex += count;
                        continue;
                    }
                    Draw::Mesh(t, pc, vertices) => {
                        let count = vertices.len() as u32;
                        if count == 0 {
                            continue;
                        }
                        let (layout, pipeline) = if let Some(yuv) = &t.inner.yuv_pipeline {
                            (yuv.layout, yuv.get_pipelines(self.target_format)?.mesh)
                        } else {
                            (self.pipeline_mgr.quad_pipeline_layout, pipes.mesh_pipeline)
                        };
                        bindings.bind(device, self.command_buffer, pipeline, layout, ds);
                        if bindings.vertex_offset != Some(0) {
                            device.cmd_bind_vertex_buffers(
                                self.command_buffer,
                                0,
                                &[vertex_buffer.unwrap()],
                                &[0],
                            );
                            bindings.vertex_offset = Some(0);
                        }
                        device.cmd_push_constants(
                            self.command_buffer,
                            layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            bytemuck::bytes_of(pc),
                        );
                        device.cmd_draw(self.command_buffer, count, 1, first_mesh_vertex, 0);
                        first_mesh_vertex += count;
                        continue;
                    }
                };
                bindings.bind(device, self.command_buffer, pipeline, layout, ds);
                device.cmd_push_constants(
                    self.command_buffer,
                    layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    bytes,
                );
                device.cmd_draw(self.command_buffer, 4, 1, 0, 0);
            }
            device.cmd_end_rendering(self.command_buffer);
        }
        for t in &textures {
            if t.inner.external {
                self.barrier(
                    t.image(),
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::SHADER_READ,
                    vk::AccessFlags::empty(),
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    false,
                    true,
                );
            }
        }
        self.barrier(
            self.target_image,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            vk::ImageLayout::GENERAL,
            vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            vk::AccessFlags::MEMORY_READ,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            vk::PipelineStageFlags::ALL_COMMANDS,
            false,
            self.target_external,
        );
        unsafe {
            if let Some(timing) = &self.gpu_timing {
                timing.end(self.command_buffer);
            }
            device.end_command_buffer(self.command_buffer)?;
            let fence = self.completion_fence;
            device.reset_fences(&[fence])?;
            let buffers = [self.command_buffer];
            let waits = &self.acquire_semaphores;
            // External images require a FOREIGN_EXT queue-family acquire before
            // their layout transition and sampling. Without maintenance8's
            // ownership-transfer stage flag, those acquire operations are not
            // scoped to FRAGMENT_SHADER: Vulkan requires ALL_COMMANDS waits.
            let stages = vec![vk::PipelineStageFlags::ALL_COMMANDS; waits.len()];
            let mut submit = vk::SubmitInfo::default().command_buffers(&buffers);
            if !waits.is_empty() {
                submit = submit.wait_semaphores(waits).wait_dst_stage_mask(&stages);
            }
            let signals: Vec<_> = self.signal_semaphore.into_iter().collect();
            submit = submit.signal_semaphores(&signals);
            device.queue_submit(self.ctx.graphics_queue, &[submit], fence)?;
            self.fence = Some(fence);
            // Track the queued layout as soon as submission succeeds. A CPU wait
            // failure does not cancel these transitions; later work uses this queue.
            for t in textures {
                t.current_layout().set(if t.inner.external {
                    vk::ImageLayout::GENERAL
                } else {
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                });
            }
            if let Some(target) = &self.target_keepalive {
                target.current_layout().set(vk::ImageLayout::GENERAL);
            }
            // Queue submissions and layout metadata must remain ordered, but a
            // CPU fence wait does not need the queue/command-pool lock. Keeping
            // it here stalls submissions for other outputs while this GPU work
            // is pending (and can prevent a producer from unblocking the queue).
            drop(_command_lock);
            if wait {
                super::wait_for_fences(device, &[fence])?;
                self.fence = None;
                for buf in self.vertex_buffers.drain(..) {
                    self.ctx.release_vertex_buffer(buf);
                }
            }
        }
        Ok(())
    }
    /// Submit without a CPU completion wait. The returned owner must stay alive
    /// until completion; dropping its last owner waits before freeing GPU objects.
    pub fn finish_async(self, target: VulkanTexture) -> Result<Arc<GpuCompletion>, vk::Result> {
        self.finish_async_with_export(target, true)
    }

    /// Internal targets use the same command/fence retirement without exporting
    /// a display sync file. Some drivers block fd export behind timeline waits.
    /// FrameCompletion owns the returned fence; consumers must not pretend that
    /// an absent sync file means GPU work has finished.
    #[cfg(test)]
    pub(super) fn finish_async_unexported(
        self,
        target: VulkanTexture,
    ) -> Result<Arc<GpuCompletion>, vk::Result> {
        self.finish_async_with_export(target, false)
    }

    fn finish_async_with_export(
        self,
        target: VulkanTexture,
        export_required: bool,
    ) -> Result<Arc<GpuCompletion>, vk::Result> {
        let (completion, export_error) = self.finish_async_retained(target, export_required)?;
        // Standalone raw Vulkan callers keep their synchronous failure-cleanup
        // contract. NativeRenderer instead queues this retained owner before
        // propagating an export failure, using its pre-reserved bounded slot.
        if let Some(error) = export_error {
            return Err(error);
        }
        Ok(completion)
    }

    #[cfg(test)]
    pub(super) fn fail_sync_export_for_test(&mut self, error: vk::Result) {
        self.sync_export_error_for_test = Some(error);
    }

    /// Submission owns every referenced resource even if exporting its display
    /// fence fails afterward. The caller must retire this owner before returning
    /// the original export error, without dropping a pending frame on its thread.
    pub(super) fn finish_async_retained(
        mut self,
        target: VulkanTexture,
        export_required: bool,
    ) -> Result<(Arc<GpuCompletion>, Option<vk::Result>), vk::Result> {
        use std::os::fd::FromRawFd;
        if (export_required && !self.ctx.supports_sync_file_export)
            || target.image() != self.target_image
            || !Arc::ptr_eq(&self.ctx, &target.inner.ctx)
        {
            return Err(vk::Result::ERROR_INITIALIZATION_FAILED);
        }
        // Keep the target alive across every partial-initialization failure.
        // A submitted frame transfers this owner into its retained completion.
        self.target_keepalive = Some(target.clone());
        if export_required {
            let mut export = vk::ExportSemaphoreCreateInfo::default()
                .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
            self.signal_semaphore = Some(unsafe {
                self.ctx.device.create_semaphore(
                    &vk::SemaphoreCreateInfo::default().push_next(&mut export),
                    None,
                )
            }?);
        }
        self.submit(false)?;
        let exported = if export_required {
            #[cfg(test)]
            let injected_error = self.sync_export_error_for_test.take();
            #[cfg(not(test))]
            let injected_error: Option<vk::Result> = None;
            let result = if let Some(error) = injected_error {
                Err(error)
            } else {
                let api = ash::khr::external_semaphore_fd::Device::new(
                    &self.ctx.instance,
                    &self.ctx.device,
                );
                unsafe {
                    api.get_semaphore_fd(
                        &vk::SemaphoreGetFdInfoKHR::default()
                            .semaphore(self.signal_semaphore.unwrap())
                            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD),
                    )
                }
            };
            result.map(|fd| {
                // -1 is Vulkan's already-signaled sentinel, not an owned fd.
                (fd >= 0).then(|| Arc::new(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) }))
            })
        } else {
            Ok(None)
        };
        let (exported, export_error) = match exported {
            Ok(exported) => (exported, None),
            Err(error) => (None, Some(error)),
        };
        target.current_layout().set(vk::ImageLayout::GENERAL);
        let mut textures = vec![target];
        for draw in &self.draws {
            if let Draw::Quad(texture, ..)
            | Draw::Blur(texture, ..)
            | Draw::Capture(texture, ..)
            | Draw::Mesh(texture, ..) = draw
            {
                textures.push(texture.clone());
            }
        }
        for draw in &self.draws {
            if let Draw::BlurPass(source, target, _, _) = draw {
                textures.extend([source.clone(), target.clone()]);
            }
        }
        for draw in &self.draws {
            if let Draw::Capture(
                _,
                CaptureRegion {
                    scratch: Some(scratch),
                    ..
                },
            ) = draw
            {
                textures.push(scratch.clone());
            }
        }
        if let Some(texture) = &self.snapshot {
            textures.push(texture.clone());
        }
        let mut semaphores = std::mem::take(&mut self.acquire_semaphores);
        if let Some(semaphore) = self.signal_semaphore.take() {
            semaphores.push(semaphore);
        }
        let completion = GpuCompletion {
            ctx: self.ctx.clone(),
            _pipeline: self.pipeline_mgr.clone(),
            scratch_pool: self.scratch_pool.clone(),
            _textures: textures,
            _custom: self
                .draws
                .iter()
                .filter_map(|draw| {
                    if let Draw::Custom(draw, _) = draw {
                        Some(draw.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            scratch: std::mem::take(&mut self.scratch_textures),
            vertex_buffers: std::mem::take(&mut self.vertex_buffers),
            client_buffers: std::sync::Mutex::new(std::mem::take(&mut self.client_buffers)),
            gpu_timing: self.gpu_timing.take(),
            retirement_result: std::sync::OnceLock::new(),
            command: self.command_buffer,
            fence: self.completion_fence,
            semaphores,
            exported,
            // An export failure is pending VkFence work, never the signaled
            // SYNC_FD sentinel. The completion worker uses its real fence.
            unexported_status: (!export_required || export_error.is_some())
                .then(|| Arc::new(std::sync::atomic::AtomicI8::new(0))),
        };
        self.command_buffer = vk::CommandBuffer::null();
        self.completion_fence = vk::Fence::null();
        self.fence = None;
        Ok((Arc::new(completion), export_error))
    }
    pub fn finish_and_wait(mut self) -> Result<(), vk::Result> {
        self.submit(true)?;
        if let Some(timing) = &self.gpu_timing {
            timing.publish_completed();
        }
        Ok(())
    }
}

impl Drop for VulkanFrame<'_> {
    fn drop(&mut self) {
        if self.command_buffer == vk::CommandBuffer::null() {
            return;
        }
        let _latency =
            crate::utils::latency::Span::new(crate::utils::latency::Operation::VulkanCleanup);
        if let Some(fence) = self.fence {
            if let Err(err) = super::wait_for_fences(&self.ctx.device, &[fence]) {
                warn!(?err, "Vulkan fence wait failed; retaining in-flight GPU resources until process exit");
                // Returning from Drop still drops every Rust field. Retain all
                // image, custom shader and pipeline owners before abandoning
                // the raw command, semaphore and vertex-buffer handles.
                super::lifecycle::record_quarantine();
                std::mem::forget(std::mem::take(&mut self.draws));
                super::lifecycle::record_quarantine();
                std::mem::forget(self.snapshot.take());
                super::lifecycle::record_quarantine();
                std::mem::forget(self.target_keepalive.take());
                super::lifecycle::record_quarantine();
                std::mem::forget(std::mem::take(&mut self.scratch_textures));
                super::lifecycle::record_quarantine();
                std::mem::forget(std::mem::take(&mut self.client_buffers));
                super::lifecycle::record_quarantine();
                std::mem::forget(self.pipeline_mgr.clone());
                super::lifecycle::record_quarantine();
                std::mem::forget(self.gpu_timing.take());
                return;
            }
        }
        let _lock = self.ctx.command_lock.lock().unwrap();
        unsafe {
            {
                recycle_scratch(&self.scratch_pool, self.scratch_textures.drain(..));
                for buf in self.vertex_buffers.drain(..) {
                    self.ctx.release_vertex_buffer(buf);
                }
            }
            if let Some(semaphore) = self.signal_semaphore.take() {
                self.ctx.device.destroy_semaphore(semaphore, None);
            }
            for semaphore in self.acquire_semaphores.drain(..) {
                self.ctx.device.destroy_semaphore(semaphore, None);
            }
            let mut idle = self.ctx.idle_commands.lock().unwrap();
            if idle.len() < 8 {
                idle.push((self.command_buffer, self.completion_fence));
            } else {
                self.ctx.device.destroy_fence(self.completion_fence, None);
                self.ctx
                    .device
                    .free_command_buffers(self.ctx.command_pool, &[self.command_buffer]);
            }
        }
    }
}

/// Owns every resource used by an asynchronous submission and its native fence.
pub struct GpuCompletion {
    ctx: Arc<VulkanContext>,
    _pipeline: Arc<VulkanPipelineManager>,
    scratch_pool: Arc<std::sync::Mutex<Vec<VulkanTexture>>>,
    _textures: Vec<VulkanTexture>,
    _custom: Vec<Arc<super::custom_shader::ShaderDraw>>,
    scratch: Vec<VulkanTexture>,
    vertex_buffers: Vec<StagingBuffer>,
    client_buffers: std::sync::Mutex<Vec<WlBufferOwner>>,
    gpu_timing: Option<super::timing::QueryLease>,
    retirement_result: std::sync::OnceLock<Result<(), vk::Result>>,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    semaphores: Vec<vk::Semaphore>,
    exported: Option<Arc<std::os::fd::OwnedFd>>,
    unexported_status: Option<Arc<std::sync::atomic::AtomicI8>>,
}
impl std::fmt::Debug for GpuCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuCompletion")
            .field("fence", &self.fence)
            .finish()
    }
}
impl GpuCompletion {
    /// Unexported internal frames retain this owner while a worker waits. The
    /// cloned owner prevents VkFence reset/reuse; no main retirement mutex or
    /// command lock needs to cover this driver wait.
    pub(super) fn wait_in_worker(&self) -> Result<(), vk::Result> {
        unsafe {
            self.ctx
                .device
                .wait_for_fences(&[self.fence], true, u64::MAX)
        }
    }
    pub(super) fn publish_gpu_timing(&self) {
        if let Some(timing) = &self.gpu_timing {
            timing.publish_completed();
        }
    }
    pub fn status(&self) -> Result<bool, vk::Result> {
        unsafe { self.ctx.device.get_fence_status(self.fence) }
    }

    pub(super) fn exported_fence(&self) -> Option<Arc<std::os::fd::OwnedFd>> {
        self.exported.clone()
    }

    /// Called only when committing to retirement. Cache errors too: Drop must
    /// retain failed submissions, without performing a second timeout wait.
    pub(super) fn prepare_retirement(&self) -> Result<(), vk::Result> {
        let result = *self
            .retirement_result
            .get_or_init(|| super::wait_for_fences(&self.ctx.device, &[self.fence]));
        self.record_retirement(result);
        result
    }

    pub(super) fn confirm_retirement(&self) -> Result<(), vk::Result> {
        let result = *self.retirement_result.get_or_init(|| Ok(()));
        self.record_retirement(result);
        result
    }

    fn record_retirement(&self, result: Result<(), vk::Result>) {
        if let Some(status) = &self.unexported_status {
            status.store(
                if result.is_ok() { 1 } else { -1 },
                std::sync::atomic::Ordering::Release,
            );
        }
    }

    pub(super) fn take_retired_client_buffers(&self) -> Vec<WlBufferOwner> {
        assert_eq!(self.retirement_result.get(), Some(&Ok(())));
        std::mem::take(&mut *self.client_buffers.lock().unwrap())
    }
}
impl smithay::backend::renderer::sync::Fence for GpuCompletion {
    fn is_signaled(&self) -> bool {
        self.status().unwrap_or(false)
    }
    fn wait(&self) -> Result<(), smithay::backend::renderer::sync::Interrupted> {
        super::wait_for_fences(&self.ctx.device, &[self.fence])
            .map_err(|_| smithay::backend::renderer::sync::Interrupted)
    }
    fn is_exportable(&self) -> bool {
        self.exported.is_some()
    }
    fn export(&self) -> Option<std::os::fd::OwnedFd> {
        self.exported.as_ref()?.try_clone().ok()
    }
}
impl Drop for GpuCompletion {
    fn drop(&mut self) {
        if let Err(err) = self.prepare_retirement() {
            warn!(
                ?err,
                "Vulkan fence wait failed; retaining in-flight GPU resources until process exit"
            );
            super::lifecycle::record_quarantine();
            std::mem::forget(std::mem::take(&mut self._textures));
            super::lifecycle::record_quarantine();
            std::mem::forget(std::mem::take(&mut self._custom));
            super::lifecycle::record_quarantine();
            std::mem::forget(std::mem::take(&mut self.scratch));
            super::lifecycle::record_quarantine();
            std::mem::forget(std::mem::take(self.client_buffers.get_mut().unwrap()));
            super::lifecycle::record_quarantine();
            std::mem::forget(self._pipeline.clone());
            super::lifecycle::record_quarantine();
            std::mem::forget(self.gpu_timing.take());
            return;
        }
        self.publish_gpu_timing();
        let _lock = self.ctx.command_lock.lock().unwrap();
        unsafe {
            {
                recycle_scratch(&self.scratch_pool, self.scratch.drain(..));
                for buf in self.vertex_buffers.drain(..) {
                    self.ctx.release_vertex_buffer(buf);
                }
            }
            for semaphore in self.semaphores.drain(..) {
                self.ctx.device.destroy_semaphore(semaphore, None);
            }
            let mut idle = self.ctx.idle_commands.lock().unwrap();
            if idle.len() < 8 {
                idle.push((self.command, self.fence));
            } else {
                self.ctx
                    .device
                    .free_command_buffers(self.ctx.command_pool, &[self.command]);
                self.ctx.device.destroy_fence(self.fence, None);
            }
        }
    }
}

#[derive(Debug)]
struct CompletionFence {
    exported: Option<Arc<std::os::fd::OwnedFd>>,
    status: std::sync::atomic::AtomicI8,
}

#[derive(Debug)]
struct UnexportedFence {
    frame: std::sync::Weak<GpuCompletion>,
    status: Arc<std::sync::atomic::AtomicI8>,
}

impl smithay::backend::renderer::sync::Fence for UnexportedFence {
    fn is_signaled(&self) -> bool {
        let status = self.status.load(std::sync::atomic::Ordering::Acquire);
        if status != 0 {
            return status > 0;
        }
        self.frame
            .upgrade()
            .is_some_and(|frame| frame.status() == Ok(true))
    }

    fn wait(&self) -> Result<(), smithay::backend::renderer::sync::Interrupted> {
        use smithay::backend::renderer::sync::Interrupted;
        if let Some(frame) = self.frame.upgrade() {
            frame.wait()?;
            frame.confirm_retirement().map_err(|_| Interrupted)?;
            return Ok(());
        }
        // A final owner may be in its bounded destructor wait. Weak expiration
        // alone cannot imply success: only confirmed retirement updates status.
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_nanos(super::FENCE_WAIT_TIMEOUT_NS);
        loop {
            let status = self.status.load(std::sync::atomic::Ordering::Acquire);
            if status != 0 {
                return if status > 0 { Ok(()) } else { Err(Interrupted) };
            }
            if std::time::Instant::now() >= deadline {
                return Err(Interrupted);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn is_exportable(&self) -> bool {
        false
    }
    fn export(&self) -> Option<std::os::fd::OwnedFd> {
        None
    }
}

/// Linux's SYNC_IOC_FILE_INFO ABI. With num_fences == 0 the kernel returns the
/// aggregate fence status without accessing a userspace fence-info array.
#[repr(C)]
#[derive(Default)]
struct SyncFileInfo {
    name: [u8; 32],
    status: i32,
    flags: u32,
    num_fences: u32,
    pad: u32,
    sync_fence_info: u64,
}

impl CompletionFence {
    fn poll(&self, timeout_ms: i32) -> bool {
        use std::os::fd::AsRawFd;
        use std::sync::atomic::Ordering;

        let Some(fd) = &self.exported else {
            // Vulkan's -1 SYNC_FD export sentinel is already signaled.
            return true;
        };
        let status = self.status.load(Ordering::Acquire);
        if status != 0 {
            return status > 0;
        }
        let mut poll = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, timeout_ms) != 1 || poll.revents & libc::POLLIN == 0 }
        {
            return false;
        }
        // poll reports readability for errored sync_files too. Preserve the
        // original Vulkan fence wait's failure semantics on GPU/device loss.
        let mut info = SyncFileInfo::default();
        let request = libc::_IOWR::<SyncFileInfo>(u32::from(b'>'), 4);
        if unsafe { libc::ioctl(fd.as_raw_fd(), request, &mut info) } < 0 {
            return false;
        }
        self.record_status(info.status)
    }

    fn record_status(&self, status: i32) -> bool {
        use std::sync::atomic::Ordering;

        let status = status.signum() as i8;
        self.status.store(status, Ordering::Release);
        status > 0
    }
}
impl smithay::backend::renderer::sync::Fence for CompletionFence {
    fn is_signaled(&self) -> bool {
        self.poll(0)
    }
    fn wait(&self) -> Result<(), smithay::backend::renderer::sync::Interrupted> {
        if self.poll((super::FENCE_WAIT_TIMEOUT_NS / 1_000_000) as i32) {
            Ok(())
        } else {
            Err(smithay::backend::renderer::sync::Interrupted)
        }
    }
    fn is_exportable(&self) -> bool {
        self.exported.is_some()
    }
    fn export(&self) -> Option<std::os::fd::OwnedFd> {
        self.exported.as_ref()?.try_clone().ok()
    }
}
impl GpuCompletion {
    pub fn sync_point(self: &Arc<Self>) -> smithay::backend::renderer::sync::SyncPoint {
        if let Some(status) = &self.unexported_status {
            return smithay::backend::renderer::sync::SyncPoint::from(UnexportedFence {
                frame: Arc::downgrade(self),
                status: status.clone(),
            });
        }
        // Native fence fds own their signal independently of the reusable
        // Vulkan fence and command buffer. A consumer may retain this SyncPoint
        // indefinitely without keeping client buffers or GPU images alive.
        smithay::backend::renderer::sync::SyncPoint::from(CompletionFence {
            exported: self.exported.clone(),
            status: std::sync::atomic::AtomicI8::new(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_helpers::vulkan::allocate_texture;
    use crate::render_helpers::vulkan::hardware_tests::setup;

    #[test]
    fn an_expired_unexported_owner_is_not_an_already_signaled_export_sentinel() {
        use smithay::backend::renderer::sync::Fence;
        let status = Arc::new(std::sync::atomic::AtomicI8::new(0));
        let fence = UnexportedFence {
            frame: std::sync::Weak::new(),
            status: status.clone(),
        };
        assert!(!fence.is_signaled());
        assert!(!fence.is_exportable());
        assert!(fence.export().is_none());
        status.store(-1, std::sync::atomic::Ordering::Release);
        assert!(!fence.is_signaled());
        assert!(fence.wait().is_err());
        status.store(1, std::sync::atomic::Ordering::Release);
        assert!(fence.is_signaled());
        assert!(fence.wait().is_ok());
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan validation and native fence export"]
    fn hdr_scratch_is_reused_only_after_completion_releases_its_owner() {
        let (renderer, _, _) = setup();
        let ctx = renderer.ctx.as_ref().unwrap();
        let targets: Vec<_> = (0..4)
            .map(|_| {
                allocate_texture(
                    ctx,
                    32,
                    16,
                    vk::Format::R8G8B8A8_UNORM,
                    vk::ImageTiling::OPTIMAL,
                    vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
                    vk::ImageLayout::UNDEFINED,
                    true,
                )
                .unwrap()
            })
            .collect();
        let make_frame = |target: &VulkanTexture| {
            let mut frame = renderer
                .render_frame(
                    target.image(),
                    target.image_view(),
                    vk::Format::R8G8B8A8_UNORM,
                    true,
                    32,
                    16,
                    Some([0.5, 0.5, 0.5, 1.]),
                )
                .unwrap();
            frame
                .encode_hdr_output(&[smithay::utils::Rectangle::from_size((32, 16).into())])
                .unwrap();
            frame
        };
        let first = make_frame(&targets[0]);
        let first_image = first.scratch_textures[0].image();
        let first_completion = first.finish_async(targets[0].clone()).unwrap();
        assert!(renderer.scratch_textures.lock().unwrap().is_empty());
        let second = make_frame(&targets[1]);
        let second_image = second.scratch_textures[0].image();
        assert_ne!(first_image, second_image);
        second.finish_and_wait().unwrap();
        let third = make_frame(&targets[2]);
        assert_eq!(third.scratch_textures[0].image(), second_image);
        let third_completion = third.finish_async(targets[2].clone()).unwrap();
        assert!(renderer.scratch_textures.lock().unwrap().is_empty());
        first_completion.prepare_retirement().unwrap();
        drop(first_completion);
        let fourth = make_frame(&targets[3]);
        assert_eq!(fourth.scratch_textures[0].image(), first_image);
        fourth.finish_and_wait().unwrap();
        third_completion.prepare_retirement().unwrap();
        drop(third_completion);
        assert_eq!(renderer.scratch_textures.lock().unwrap().len(), 2);
        let different_size = renderer
            .render_frame(
                targets[0].image(),
                targets[0].image_view(),
                vk::Format::R8G8B8A8_UNORM,
                true,
                32,
                16,
                None,
            )
            .unwrap();
        // Both alpha scratch images are idle here, so this exercises the key
        // rather than succeeding merely because the pool was empty.
        let opaque = different_size.acquire_scratch(32, 16, false).unwrap();
        assert!(![first_image, second_image].contains(&opaque.image()));
        assert_eq!(renderer.scratch_textures.lock().unwrap().len(), 2);
        drop(opaque);
        let different = different_size.acquire_scratch(16, 16, true).unwrap();
        assert!(![first_image, second_image].contains(&different.image()));
    }

    #[test]
    #[ignore = "requires real DRM render node and Vulkan validation"]
    fn scratch_cache_bounds_actual_allocations_and_image_count() {
        let (renderer, _, _) = setup();
        let ctx = renderer.ctx.as_ref().unwrap();
        let pool = std::sync::Mutex::new(Vec::new());
        let make_texture = |width, height| {
            allocate_texture(
                ctx,
                width,
                height,
                vk::Format::R8G8B8A8_UNORM,
                vk::ImageTiling::OPTIMAL,
                vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::TRANSFER_DST
                    | vk::ImageUsageFlags::SAMPLED,
                vk::ImageLayout::UNDEFINED,
                true,
            )
            .unwrap()
        };
        recycle_scratch(&pool, (0..5).map(|_| make_texture(2048, 2048)));
        {
            let cached = pool.lock().unwrap();
            assert!(cached.len() < 5);
            assert!(cached.iter().map(scratch_bytes).sum::<u64>() <= MAX_IDLE_SCRATCH_BYTES);
            for texture in cached.iter() {
                let requirements =
                    unsafe { ctx.device.get_image_memory_requirements(texture.image()) };
                assert_eq!(
                    scratch_bytes(texture),
                    requirements.size,
                    "cache counts driver padding"
                );
            }
        }
        let oversized = make_texture(4097, 4096);
        assert!(scratch_bytes(&oversized) > MAX_IDLE_SCRATCH_BYTES);
        let weak = Arc::downgrade(&oversized.inner);
        recycle_scratch(&pool, std::iter::once(oversized));
        assert!(
            weak.upgrade().is_none(),
            "oversized idle allocation must be released"
        );
        recycle_scratch(&pool, (0..9).map(|_| make_texture(16, 16)));
        let cached = pool.lock().unwrap();
        assert_eq!(cached.len(), MAX_IDLE_SCRATCH_IMAGES);
        assert!(cached.iter().map(scratch_bytes).sum::<u64>() <= MAX_IDLE_SCRATCH_BYTES);
    }

    #[test]
    fn readable_fd_without_successful_sync_status_is_not_completion() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;

        use smithay::backend::renderer::sync::Fence;

        let (read, mut write) = UnixStream::pair().unwrap();
        write.write_all(&[1]).unwrap();
        let fence = CompletionFence {
            exported: Some(Arc::new(read.into())),
            status: std::sync::atomic::AtomicI8::new(0),
        };
        assert!(
            !fence.is_signaled(),
            "readability alone must not prove successful GPU completion"
        );
        assert!(fence.wait().is_err());
        assert_eq!(
            fence.status.load(std::sync::atomic::Ordering::Acquire),
            0,
            "a failed ioctl must remain retryable"
        );
    }

    #[test]
    fn sync_file_status_distinguishes_pending_success_and_device_error() {
        use std::os::unix::net::UnixStream;

        use smithay::backend::renderer::sync::Fence;

        for (status, complete) in [
            (0, false),
            (1, true),
            (-libc::EIO, false),
            (-libc::ENODEV, false),
        ] {
            let (read, _write) = UnixStream::pair().unwrap();
            let fence = CompletionFence {
                exported: Some(Arc::new(read.into())),
                status: std::sync::atomic::AtomicI8::new(0),
            };
            assert_eq!(fence.record_status(status), complete);
            assert_eq!(fence.is_signaled(), complete);
            if status < 0 {
                assert!(fence.wait().is_err());
            }
        }
        let already_signaled = CompletionFence {
            exported: None,
            status: std::sync::atomic::AtomicI8::new(0),
        };
        assert!(already_signaled.is_signaled());
        assert!(already_signaled.wait().is_ok());
        assert!(!already_signaled.is_exportable());
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan and native fence export"]
    fn particle_batch_uses_one_upload_retained_by_async_completion() {
        let (renderer, _gles, _allocator) = setup();
        let ctx = renderer.ctx.as_ref().unwrap().clone();
        let target = allocate_texture(
            &ctx,
            32,
            32,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageTiling::OPTIMAL,
            vk::ImageUsageFlags::COLOR_ATTACHMENT,
            vk::ImageLayout::UNDEFINED,
            true,
        )
        .unwrap();
        let mut frame = renderer
            .render_frame(
                target.image(),
                target.image_view(),
                vk::Format::R8G8B8A8_UNORM,
                true,
                32,
                32,
                Some([0.; 4]),
            )
            .unwrap();
        let vertices: Rc<[ParticleVertex]> = (0..88)
            .flat_map(|_| {
                [
                    [-0.5, -0.5],
                    [-0.5, 0.5],
                    [0.5, 0.5],
                    [-0.5, -0.5],
                    [0.5, 0.5],
                    [0.5, -0.5],
                ]
                .map(|position| ParticleVertex {
                    position,
                    local: position,
                    color: [0.01; 4],
                    shape: 0.,
                    inv_width: 0.05,
                    aspect: 1.,
                })
            })
            .collect::<Vec<_>>()
            .into();
        frame.draw_particles(
            ParticlePushConstants {
                matrix: glam::Mat4::IDENTITY.to_cols_array(),
            },
            vertices,
            vec![smithay::utils::Rectangle::from_size((32, 32).into())],
        );
        assert_eq!(
            frame.draws.len(),
            1,
            "88 particles must be one deferred batch"
        );
        let completion = frame.finish_async(target).unwrap();
        assert_eq!(
            completion.vertex_buffers.len(),
            1,
            "one pooled upload serves the entire batch"
        );
        // The upload stays owned by completion until its GPU fence has finished.
        smithay::backend::renderer::sync::Fence::wait(completion.as_ref()).unwrap();
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan and native fence export"]
    fn fence_timeout_keeps_completion_owners_alive() {
        let (renderer, _gles, _allocator) = setup();
        let ctx = renderer.ctx.as_ref().unwrap().clone();
        let target = allocate_texture(
            &ctx,
            8,
            8,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageTiling::OPTIMAL,
            vk::ImageUsageFlags::COLOR_ATTACHMENT,
            vk::ImageLayout::UNDEFINED,
            true,
        )
        .unwrap();
        let source = renderer.create_solid_texture([255; 4]).unwrap();
        let weak_target = Arc::downgrade(&target.inner);
        let weak_source = Arc::downgrade(&source.inner);
        let weak_pipeline = Arc::downgrade(renderer.pipeline_mgr.as_ref().unwrap());
        let mut frame = renderer
            .render_frame(
                target.image(),
                target.image_view(),
                vk::Format::R8G8B8A8_UNORM,
                true,
                8,
                8,
                Some([0.; 4]),
            )
            .unwrap();
        frame
            .draw_texture(&source, 0., 0., 8., 8., None, 1., None, 0.)
            .unwrap();
        let mut completion = frame.finish_async(target).unwrap();
        smithay::backend::renderer::sync::Fence::wait(completion.as_ref()).unwrap();
        // The actual submission completes first so exporting its SYNC_FD is
        // valid. Inject an unsignaled fence only to exercise the owner's Drop
        // failure path; this does not simulate a still-running original draw.
        let injected = unsafe {
            ctx.device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .unwrap()
        };
        let actual = std::mem::replace(&mut Arc::get_mut(&mut completion).unwrap().fence, injected);
        drop(source);
        drop(renderer);
        drop(completion);
        assert!(weak_target.upgrade().is_some());
        assert!(weak_source.upgrade().is_some());
        assert!(weak_pipeline.upgrade().is_some());
        unsafe {
            ctx.device.destroy_fence(actual, None);
            ctx.device.destroy_fence(injected, None);
        }
    }
}
