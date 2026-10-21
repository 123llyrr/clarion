//! Vulkan Graphics Pipelines & Shaders for Clarion.
//!
//! Manages SPIR-V shader modules, pipeline layouts, descriptor sets,
//! texture samplers, and dynamic rendering graphics pipelines for quads
//! and animated borders.

use std::sync::Arc;

use ash::vk;

use super::device::VulkanContext;
use super::VulkanTexture;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct YuvKey {
    pub format: vk::Format,
    pub model: vk::SamplerYcbcrModelConversion,
    pub chroma: vk::ChromaLocation,
    pub filter: vk::Filter,
}

// Each combined YCbCr sampler may consume several pool slots even though the
// descriptor-set binding contains exactly one sampler2D.
pub(super) const MAX_YUV_DESCRIPTORS_PER_SET: u32 = 4;

#[derive(Clone, Copy)]
pub(super) struct YuvDrawPipelines {
    pub quad: vk::Pipeline,
    pub blur: vk::Pipeline,
    pub mesh: vk::Pipeline,
    pub color: vk::Pipeline,
}

/// Immutable conversion and sampler shared by every imported image with this
/// format/color policy. The image view and descriptor use this same conversion.
pub(crate) struct YuvPipeline {
    ctx: Arc<VulkanContext>,
    pub conversion: vk::SamplerYcbcrConversion,
    sampler: vk::Sampler,
    pub set_layout: vk::DescriptorSetLayout,
    pub layout: vk::PipelineLayout,
    pipelines: std::sync::RwLock<
        std::collections::HashMap<vk::Format, Result<YuvDrawPipelines, vk::Result>>,
    >,
}

impl YuvPipeline {
    fn new(ctx: Arc<VulkanContext>, key: YuvKey) -> Result<Self, vk::Result> {
        let device = &ctx.device;
        let mut guard = BuildGuard::new(device);
        let conversion_info = vk::SamplerYcbcrConversionCreateInfo::default()
            .format(key.format)
            .ycbcr_model(key.model)
            .ycbcr_range(vk::SamplerYcbcrRange::ITU_NARROW)
            .components(vk::ComponentMapping::default())
            .x_chroma_offset(key.chroma)
            .y_chroma_offset(key.chroma)
            .chroma_filter(key.filter);
        let conversion = unsafe { device.create_sampler_ycbcr_conversion(&conversion_info, None) }?;
        guard.conversions.push(conversion);
        let mut conversion_link = vk::SamplerYcbcrConversionInfo::default().conversion(conversion);
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(key.filter)
            .min_filter(key.filter)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .push_next(&mut conversion_link);
        let sampler = unsafe { device.create_sampler(&sampler_info, None) }?;
        guard.samplers.push(sampler);
        let immutable = [sampler];
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .immutable_samplers(&immutable)];
        let set_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        }?;
        guard.set_layouts.push(set_layout);
        let set_layouts = [set_layout];
        let pushes = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .size(std::mem::size_of::<QuadPushConstants>() as u32)];
        let layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(&pushes),
                None,
            )
        }?;
        guard.layouts.push(layout);
        guard.disarmed = true;
        drop(guard);
        Ok(Self {
            ctx,
            conversion,
            sampler,
            set_layout,
            layout,
            pipelines: std::sync::RwLock::new(std::collections::HashMap::new()),
        })
    }

    pub(super) fn prepared_format(
        &self,
        format: vk::Format,
    ) -> Result<Option<YuvDrawPipelines>, vk::Result> {
        self.pipelines
            .read()
            .unwrap()
            .get(&format)
            .copied()
            .transpose()
    }

    // Raw Vulkan callers retain their synchronous compatibility API. Native
    // frames check readiness before submission and never build through this call.
    pub(super) fn get_pipelines(&self, format: vk::Format) -> Result<YuvDrawPipelines, vk::Result> {
        self.prepare_format(format)
    }

    pub(super) fn prepare_format(
        &self,
        format: vk::Format,
    ) -> Result<YuvDrawPipelines, vk::Result> {
        if let Some(result) = self.pipelines.read().unwrap().get(&format).copied() {
            return result;
        }
        // Compile outside cache locks so a known format remains usable while
        // another output's target is being prepared on the device worker.
        let built = self.build_format(format);
        let (result, published) = {
            let mut cache = self.pipelines.write().unwrap();
            match cache.entry(format) {
                std::collections::hash_map::Entry::Occupied(entry) => (*entry.get(), false),
                std::collections::hash_map::Entry::Vacant(entry) => (*entry.insert(built), true),
            }
        };
        if !published {
            if let Ok(unused) = built {
                unsafe {
                    for pipeline in [unused.quad, unused.blur, unused.mesh, unused.color] {
                        self.ctx.device.destroy_pipeline(pipeline, None);
                    }
                }
            }
        }
        result
    }

    fn build_format(&self, format: vk::Format) -> Result<YuvDrawPipelines, vk::Result> {
        let device = &self.ctx.device;
        let mut guard = BuildGuard::new(device);
        let qv = VulkanPipelineManager::create_shader_module(device, QUAD_VERT_SPV)?;
        guard.shaders.push(qv);
        let qf = VulkanPipelineManager::create_shader_module(device, QUAD_FRAG_SPV)?;
        guard.shaders.push(qf);
        let bv = VulkanPipelineManager::create_shader_module(device, BLUR_VERT_SPV)?;
        guard.shaders.push(bv);
        let bf = VulkanPipelineManager::create_shader_module(device, BLUR_FRAG_SPV)?;
        guard.shaders.push(bf);
        let mv = VulkanPipelineManager::create_shader_module(device, MESH_VERT_SPV)?;
        guard.shaders.push(mv);
        let mf = VulkanPipelineManager::create_shader_module(device, MESH_FRAG_SPV)?;
        guard.shaders.push(mf);
        let quad = VulkanPipelineManager::build_pipeline(device, self.layout, qv, qf, format)?;
        guard.pipelines.push(quad);
        let blur = VulkanPipelineManager::build_pipeline(device, self.layout, bv, bf, format)?;
        guard.pipelines.push(blur);
        let mesh = VulkanPipelineManager::build_mesh_pipeline(device, self.layout, mv, mf, format)?;
        guard.pipelines.push(mesh);
        let cv = VulkanPipelineManager::create_shader_module(device, COLOR_VERT_SPV)?;
        guard.shaders.push(cv);
        let cf = VulkanPipelineManager::create_shader_module(device, COLOR_FRAG_SPV)?;
        guard.shaders.push(cf);
        let color = VulkanPipelineManager::build_pipeline_with_blend(
            device,
            self.layout,
            cv,
            cf,
            format,
            vk::ColorComponentFlags::RGBA,
            false,
        )?;
        guard.pipelines.push(color);
        guard.pipelines.clear();
        let pipelines = YuvDrawPipelines {
            quad,
            blur,
            mesh,
            color,
        };
        Ok(pipelines)
    }
}

impl Drop for YuvPipeline {
    fn drop(&mut self) {
        unsafe {
            for pipes in self
                .pipelines
                .get_mut()
                .unwrap()
                .values()
                .filter_map(|pipes| pipes.as_ref().ok())
            {
                self.ctx.device.destroy_pipeline(pipes.quad, None);
                self.ctx.device.destroy_pipeline(pipes.blur, None);
                self.ctx.device.destroy_pipeline(pipes.mesh, None);
                self.ctx.device.destroy_pipeline(pipes.color, None);
            }
            self.ctx.device.destroy_pipeline_layout(self.layout, None);
            self.ctx
                .device
                .destroy_descriptor_set_layout(self.set_layout, None);
            self.ctx.device.destroy_sampler(self.sampler, None);
            self.ctx
                .device
                .destroy_sampler_ycbcr_conversion(self.conversion, None);
        }
    }
}

/// Optional rounded geometry mask in sampled texture coordinates.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TextureClip {
    pub matrix: [[f32; 4]; 3],
    pub radius: [f32; 4],
    pub size: [f32; 2],
    pub scale: f32,
    pub enabled: f32,
}

/// Push constants for drawing 2D textured window quads and elements.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct QuadPushConstants {
    pub matrix: [f32; 16],
    pub uv_bounds: [f32; 4],
    pub color_tint: [f32; 4],
    pub alpha: f32,
    pub corner_radius: f32,
    pub size: [f32; 2],
    pub clip: TextureClip,
    pub noise: f32,
    pub saturation: f32,
    pub target_size: [f32; 2],
    pub bg_color: [f32; 4],
    /// Left/right edge of the right-side gradient fade, in texture UV space.
    /// Disabled when `fade_cutoff[0] >= fade_cutoff[1]`.
    pub fade_cutoff: [f32; 2],
    /// Convert the sampled SDR/sRGB scene to PQ HDR10 output when non-zero.
    pub hdr_transfer: f32,
}

/// A color conversion uses the existing texture descriptor and push range.
/// Matrices are row major; all transfer function luminances are cd/m².
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct ColorPushConstants {
    pub matrix: [f32; 16],
    pub uv_bounds: [f32; 4],
    pub gamut: [[f32; 4]; 3],
    pub source: [f32; 4],
    pub target: [f32; 4],
    pub mapping: [f32; 4],
    pub tone_gamut: [[f32; 4]; 3],
}

const _: () =
    assert!(std::mem::size_of::<ColorPushConstants>() <= std::mem::size_of::<QuadPushConstants>());

impl Default for QuadPushConstants {
    fn default() -> Self {
        Self {
            matrix: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            uv_bounds: [0.0, 0.0, 1.0, 1.0],
            color_tint: [1.0, 1.0, 1.0, 1.0],
            alpha: 1.0,
            corner_radius: 0.0,
            size: [0.0, 0.0],
            clip: TextureClip::default(),
            noise: 0.0,
            saturation: 1.0,
            target_size: [0.0, 0.0],
            bg_color: [0.0; 4],
            fade_cutoff: [0.0, 0.0],
            hdr_transfer: 0.0,
        }
    }
}

/// Push constants for drawing the animated rotating OKLCH gradient border.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BorderPushConstants {
    pub matrix: [f32; 16],
    pub color_from: [f32; 4],
    pub color_to: [f32; 4],
    pub outer_radius: [f32; 4],
    pub geo_size: [f32; 2],
    pub grad_offset: [f32; 2],
    pub grad_vec: [f32; 2],
    pub colorspace: f32,
    pub hue_interpolation: f32,
    pub grad_width: f32,
    pub border_width: f32,
    pub alpha: f32,
    pub scale: f32,
    pub rotation_angle: f32,
    pub rounding_power: f32,
    pub _geo_padding: [f32; 2],
    pub geo_bounds: [f32; 4],
}

impl Default for BorderPushConstants {
    fn default() -> Self {
        Self {
            matrix: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            color_from: [1.0, 0.0, 0.5, 1.0],
            color_to: [0.0, 0.9, 1.0, 1.0],
            outer_radius: [12.0, 12.0, 12.0, 12.0],
            geo_size: [800.0, 600.0],
            grad_offset: [0.0, 0.0],
            grad_vec: [1.0, 0.0],
            colorspace: 3.0,        // OKLCh
            hue_interpolation: 1.0, // Longer hue
            grad_width: 800.0,
            border_width: 4.0,
            alpha: 1.0,
            scale: 1.0,
            rotation_angle: 0.0,
            rounding_power: 2.0,
            _geo_padding: [0.; 2],
            geo_bounds: [0., 0., 1., 1.],
        }
    }
}

/// Analytic rounded shadow, including an optional window-interior cutout.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShadowPushConstants {
    pub matrix: [f32; 16],
    pub color: [f32; 4],
    pub corner_radius: [f32; 4],
    pub window_radius: [f32; 4],
    pub coords_bounds: [f32; 4],
    pub geometry: [f32; 4],
    pub window_geometry: [f32; 4],
    pub params: [f32; 4], // sigma, antialiasing scale, alpha, reserved
}

// Both untextured effects use the same pipeline layout and no descriptors.
const _: () = assert!(
    std::mem::size_of::<ShadowPushConstants>() <= std::mem::size_of::<BorderPushConstants>()
);

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlurPushConstants {
    pub matrix: [f32; 16],
    pub params: [f32; 4], // sampling offset x/y, upsample flag, source y-inversion
}

/// Shared transform for a batch of particle triangles. Reuses the border pipeline layout.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ParticlePushConstants {
    pub matrix: [f32; 16],
}

const _: () = assert!(
    std::mem::size_of::<ParticlePushConstants>() <= std::mem::size_of::<BorderPushConstants>()
);

/// Textured triangle mesh used by the workspace cube and elastic windows.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshPushConstants {
    pub matrix: [f32; 16],
    pub tex_scale: [f32; 2],
    pub alpha: f32,
    pub y_inverted: f32,
}

impl Default for MeshPushConstants {
    fn default() -> Self {
        Self {
            matrix: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            tex_scale: [1.0, 1.0],
            alpha: 1.0,
            y_inverted: 0.0,
        }
    }
}

const _: () =
    assert!(std::mem::size_of::<MeshPushConstants>() <= std::mem::size_of::<QuadPushConstants>());

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshGpuVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
}

const QUAD_VERT_SPV: &[u8] = include_bytes!("shaders/quad.vert.spv");
const QUAD_FRAG_SPV: &[u8] = include_bytes!("shaders/quad.frag.spv");
const COLOR_VERT_SPV: &[u8] = include_bytes!("shaders/color.vert.spv");
const COLOR_FRAG_SPV: &[u8] = include_bytes!("shaders/color.frag.spv");
const BORDER_VERT_SPV: &[u8] = include_bytes!("shaders/border.vert.spv");
const BORDER_FRAG_SPV: &[u8] = include_bytes!("shaders/border.frag.spv");
const SHADOW_VERT_SPV: &[u8] = include_bytes!("shaders/shadow.vert.spv");
const SHADOW_FRAG_SPV: &[u8] = include_bytes!("shaders/shadow.frag.spv");
const BLUR_VERT_SPV: &[u8] = include_bytes!("shaders/blur.vert.spv");
const BLUR_FRAG_SPV: &[u8] = include_bytes!("shaders/blur.frag.spv");
const PARTICLE_VERT_SPV: &[u8] = include_bytes!("shaders/particle.vert.spv");
const PARTICLE_FRAG_SPV: &[u8] = include_bytes!("shaders/particle.frag.spv");
const ALPHA_ONE_VERT_SPV: &[u8] = include_bytes!("shaders/alpha_one.vert.spv");
const ALPHA_ONE_FRAG_SPV: &[u8] = include_bytes!("shaders/alpha_one.frag.spv");
const MESH_VERT_SPV: &[u8] = include_bytes!("shaders/mesh.vert.spv");
const MESH_FRAG_SPV: &[u8] = include_bytes!("shaders/mesh.frag.spv");

/// Graphics pipelines compiled for a specific render target color format.
#[derive(Debug, Clone, Copy)]
pub struct FormatPipelines {
    pub quad_pipeline: vk::Pipeline,
    pub border_pipeline: vk::Pipeline,
    pub shadow_pipeline: vk::Pipeline,
    pub blur_pipeline: vk::Pipeline,
    pub particle_pipeline: vk::Pipeline,
    pub alpha_one_pipeline: vk::Pipeline,
    pub mesh_pipeline: vk::Pipeline,
    pub color_pipeline: vk::Pipeline,
}

pub struct VulkanPipelineManager {
    ctx: Arc<VulkanContext>,
    pub descriptor_set_layout: vk::DescriptorSetLayout,
    pub descriptor_pool: vk::DescriptorPool,
    descriptor_pool_lock: std::sync::Mutex<()>,
    pub quad_pipeline_layout: vk::PipelineLayout,
    pub quad_pipeline: vk::Pipeline,
    pub border_pipeline_layout: vk::PipelineLayout,
    pub border_pipeline: vk::Pipeline,
    pub alpha_one_pipeline_layout: vk::PipelineLayout,
    pub alpha_one_pipeline: vk::Pipeline,
    pub sampler_linear: vk::Sampler,
    pub sampler_nearest: vk::Sampler,
    yuv_pipelines: std::sync::Mutex<std::collections::HashMap<YuvKey, Arc<YuvPipeline>>>,
    pipelines_by_format: std::sync::RwLock<std::collections::HashMap<vk::Format, FormatPipelines>>,
}

impl VulkanPipelineManager {
    pub fn new(ctx: Arc<VulkanContext>, color_format: vk::Format) -> Result<Self, vk::Result> {
        let device = &ctx.device;
        let mut guard = BuildGuard::new(device);

        // 1. Create samplers
        let linear_sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);

        let sampler_linear = unsafe { device.create_sampler(&linear_sampler_info, None) }?;
        guard.samplers.push(sampler_linear);

        let nearest_sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);

        let sampler_nearest = unsafe { device.create_sampler(&nearest_sampler_info, None) }?;
        guard.samplers.push(sampler_nearest);

        // 2. Create descriptor set layout
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];

        let set_layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        let descriptor_set_layout =
            unsafe { device.create_descriptor_set_layout(&set_layout_info, None) }?;
        guard.set_layouts.push(descriptor_set_layout);

        // 3. Create descriptor pool
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1024 * MAX_YUV_DESCRIPTORS_PER_SET)];

        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
            .max_sets(1024)
            .pool_sizes(&pool_sizes);

        let descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info, None) }?;
        guard.pools.push(descriptor_pool);

        // 4. Create Quad Pipeline Layout
        let quad_push_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(std::mem::size_of::<QuadPushConstants>() as u32)];

        let set_layouts = [descriptor_set_layout];
        let quad_layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&set_layouts)
            .push_constant_ranges(&quad_push_ranges);
        let quad_pipeline_layout =
            unsafe { device.create_pipeline_layout(&quad_layout_info, None) }?;
        guard.layouts.push(quad_pipeline_layout);

        // 5. Create Border Pipeline Layout (Border does not need a texture sampler)
        let border_push_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(std::mem::size_of::<BorderPushConstants>() as u32)];

        let border_layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&[])
            .push_constant_ranges(&border_push_ranges);
        let border_pipeline_layout =
            unsafe { device.create_pipeline_layout(&border_layout_info, None) }?;
        guard.layouts.push(border_pipeline_layout);

        // 7. Create Alpha-One Pipeline Layout (for resetting alpha to 1.0 on opaque captures)
        let alpha_one_layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&[])
            .push_constant_ranges(&[]);
        let alpha_one_pipeline_layout =
            unsafe { device.create_pipeline_layout(&alpha_one_layout_info, None) }?;
        guard.layouts.push(alpha_one_pipeline_layout);

        // Build default pipelines for color_format
        let default_pipelines = Self::create_pipelines_for_format(
            device,
            quad_pipeline_layout,
            border_pipeline_layout,
            alpha_one_pipeline_layout,
            color_format,
        )?;

        let mut map = std::collections::HashMap::new();
        map.insert(color_format, default_pipelines);

        guard.disarmed = true;
        drop(guard);
        Ok(Self {
            ctx,
            descriptor_set_layout,
            descriptor_pool,
            descriptor_pool_lock: std::sync::Mutex::new(()),
            quad_pipeline_layout,
            quad_pipeline: default_pipelines.quad_pipeline,
            border_pipeline_layout,
            border_pipeline: default_pipelines.border_pipeline,
            alpha_one_pipeline_layout,
            alpha_one_pipeline: default_pipelines.alpha_one_pipeline,
            sampler_linear,
            sampler_nearest,
            yuv_pipelines: std::sync::Mutex::new(std::collections::HashMap::new()),
            pipelines_by_format: std::sync::RwLock::new(map),
        })
    }

    #[cfg(test)]
    pub(super) fn prepared_pipelines(&self, format: vk::Format) -> Option<FormatPipelines> {
        self.pipelines_by_format
            .read()
            .unwrap()
            .get(&format)
            .copied()
    }

    /// Retrieve or build graphics pipelines matching the given attachment format.
    pub fn get_pipelines(&self, format: vk::Format) -> Result<FormatPipelines, vk::Result> {
        {
            let read = self.pipelines_by_format.read().unwrap();
            if let Some(&pipes) = read.get(&format) {
                return Ok(pipes);
            }
        }
        // Driver compilation can take many milliseconds. Readers drawing a known format
        // must remain independent of preparation for another output's new format.
        let pipes = Self::create_pipelines_for_format(
            &self.ctx.device,
            self.quad_pipeline_layout,
            self.border_pipeline_layout,
            self.alpha_one_pipeline_layout,
            format,
        )?;
        let (selected, published) = {
            let mut write = self.pipelines_by_format.write().unwrap();
            match write.entry(format) {
                std::collections::hash_map::Entry::Occupied(entry) => (*entry.get(), false),
                std::collections::hash_map::Entry::Vacant(entry) => (*entry.insert(pipes), true),
            }
        };
        if !published {
            // Another preparer published first. No render submission could have used ours.
            unsafe {
                for pipeline in [
                    pipes.quad_pipeline,
                    pipes.border_pipeline,
                    pipes.shadow_pipeline,
                    pipes.blur_pipeline,
                    pipes.particle_pipeline,
                    pipes.alpha_one_pipeline,
                    pipes.mesh_pipeline,
                    pipes.color_pipeline,
                ] {
                    self.ctx.device.destroy_pipeline(pipeline, None);
                }
            }
        }
        Ok(selected)
    }

    pub(crate) fn has_prepared_format(&self, format: vk::Format) -> bool {
        self.pipelines_by_format
            .read()
            .unwrap()
            .contains_key(&format)
    }

    pub(super) fn yuv_pipeline(&self, key: YuvKey) -> Result<Arc<YuvPipeline>, vk::Result> {
        if !self.ctx.supports_ycbcr_conversion {
            return Err(vk::Result::ERROR_FEATURE_NOT_PRESENT);
        }
        if let Some(variant) = self.yuv_pipelines.lock().unwrap().get(&key) {
            return Ok(variant.clone());
        }
        let variant = Arc::new(YuvPipeline::new(self.ctx.clone(), key)?);
        let selected = self
            .yuv_pipelines
            .lock()
            .unwrap()
            .entry(key)
            .or_insert_with(|| variant.clone())
            .clone();
        // An unused duplicate drops outside the cache mutex.
        drop(variant);
        Ok(selected)
    }

    /// Build a set of quad, border, and alpha_one pipelines for a specific color format.
    pub fn create_pipelines_for_format(
        device: &ash::Device,
        quad_layout: vk::PipelineLayout,
        border_layout: vk::PipelineLayout,
        alpha_one_layout: vk::PipelineLayout,
        color_format: vk::Format,
    ) -> Result<FormatPipelines, vk::Result> {
        let mut guard = BuildGuard::new(device);
        let vert_module = Self::create_shader_module(device, QUAD_VERT_SPV)?;
        guard.shaders.push(vert_module);
        let frag_module = Self::create_shader_module(device, QUAD_FRAG_SPV)?;
        guard.shaders.push(frag_module);
        let border_vert_module = Self::create_shader_module(device, BORDER_VERT_SPV)?;
        guard.shaders.push(border_vert_module);
        let border_module = Self::create_shader_module(device, BORDER_FRAG_SPV)?;
        guard.shaders.push(border_module);
        let shadow_vert_module = Self::create_shader_module(device, SHADOW_VERT_SPV)?;
        guard.shaders.push(shadow_vert_module);
        let shadow_module = Self::create_shader_module(device, SHADOW_FRAG_SPV)?;
        guard.shaders.push(shadow_module);
        let blur_vert_module = Self::create_shader_module(device, BLUR_VERT_SPV)?;
        guard.shaders.push(blur_vert_module);
        let blur_module = Self::create_shader_module(device, BLUR_FRAG_SPV)?;
        guard.shaders.push(blur_module);
        let particle_vert_module = Self::create_shader_module(device, PARTICLE_VERT_SPV)?;
        guard.shaders.push(particle_vert_module);
        let particle_module = Self::create_shader_module(device, PARTICLE_FRAG_SPV)?;
        guard.shaders.push(particle_module);
        let alpha_one_vert_module = Self::create_shader_module(device, ALPHA_ONE_VERT_SPV)?;
        guard.shaders.push(alpha_one_vert_module);
        let alpha_one_frag_module = Self::create_shader_module(device, ALPHA_ONE_FRAG_SPV)?;
        guard.shaders.push(alpha_one_frag_module);
        let mesh_vert_module = Self::create_shader_module(device, MESH_VERT_SPV)?;
        guard.shaders.push(mesh_vert_module);
        let mesh_frag_module = Self::create_shader_module(device, MESH_FRAG_SPV)?;
        guard.shaders.push(mesh_frag_module);

        let quad_pipeline =
            Self::build_pipeline(device, quad_layout, vert_module, frag_module, color_format)?;
        guard.pipelines.push(quad_pipeline);

        let border_pipeline = Self::build_pipeline(
            device,
            border_layout,
            border_vert_module,
            border_module,
            color_format,
        )?;
        guard.pipelines.push(border_pipeline);

        let shadow_pipeline = Self::build_pipeline(
            device,
            border_layout,
            shadow_vert_module,
            shadow_module,
            color_format,
        )?;
        guard.pipelines.push(shadow_pipeline);

        let blur_pipeline = Self::build_pipeline(
            device,
            quad_layout,
            blur_vert_module,
            blur_module,
            color_format,
        )?;
        guard.pipelines.push(blur_pipeline);
        let particle_pipeline = Self::build_particle_pipeline(
            device,
            border_layout,
            particle_vert_module,
            particle_module,
            color_format,
        )?;
        guard.pipelines.push(particle_pipeline);
        let alpha_one_pipeline = Self::build_pipeline_with_blend(
            device,
            alpha_one_layout,
            alpha_one_vert_module,
            alpha_one_frag_module,
            color_format,
            vk::ColorComponentFlags::A,
            false,
        )?;
        guard.pipelines.push(alpha_one_pipeline);

        let mesh_pipeline = Self::build_mesh_pipeline(
            device,
            quad_layout,
            mesh_vert_module,
            mesh_frag_module,
            color_format,
        )?;
        guard.pipelines.push(mesh_pipeline);
        let color_vert = Self::create_shader_module(device, COLOR_VERT_SPV)?;
        guard.shaders.push(color_vert);
        let color_frag = Self::create_shader_module(device, COLOR_FRAG_SPV)?;
        guard.shaders.push(color_frag);
        let color_pipeline = Self::build_pipeline_with_blend(
            device,
            quad_layout,
            color_vert,
            color_frag,
            color_format,
            vk::ColorComponentFlags::RGBA,
            false,
        )?;
        guard.pipelines.push(color_pipeline);

        guard.pipelines.clear(); // Transfer pipeline ownership; modules are temporary.

        Ok(FormatPipelines {
            quad_pipeline,
            border_pipeline,
            shadow_pipeline,
            blur_pipeline,
            particle_pipeline,
            alpha_one_pipeline,
            mesh_pipeline,
            color_pipeline,
        })
    }

    pub(super) fn build_pipeline(
        device: &ash::Device,
        layout: vk::PipelineLayout,
        vert_module: vk::ShaderModule,
        frag_module: vk::ShaderModule,
        color_format: vk::Format,
    ) -> Result<vk::Pipeline, vk::Result> {
        Self::build_pipeline_with_blend(
            device,
            layout,
            vert_module,
            frag_module,
            color_format,
            vk::ColorComponentFlags::RGBA,
            true,
        )
    }

    fn build_pipeline_with_blend(
        device: &ash::Device,
        layout: vk::PipelineLayout,
        vert_module: vk::ShaderModule,
        frag_module: vk::ShaderModule,
        color_format: vk::Format,
        color_write_mask: vk::ColorComponentFlags,
        blend_enable: bool,
    ) -> Result<vk::Pipeline, vk::Result> {
        let entry_name = std::ffi::CString::new("main").unwrap();

        let shader_stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert_module)
                .name(&entry_name),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag_module)
                .name(&entry_name),
        ];

        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_STRIP)
            .primitive_restart_enable(false);

        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);

        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .depth_clamp_enable(false)
            .rasterizer_discard_enable(false)
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE);

        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .sample_shading_enable(false)
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        let color_blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(color_write_mask)
            .blend_enable(blend_enable)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            // Coverage union: Ad + As * (1 - Ad). The equivalent source-
            // first form can round an opaque FP16 destination below one on
            // fixed-function blend hardware, leaking the next scene on replay.
            // This form keeps Ad=1 exactly, without changing RGB blending or
            // treating genuinely translucent near-one samples as opaque.
            .src_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_DST_ALPHA)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE)
            .alpha_blend_op(vk::BlendOp::ADD)];

        let color_blending =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_blend_attachments);

        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

        let color_formats = [color_format];
        let mut rendering_info =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats);

        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisampling)
            .color_blend_state(&color_blending)
            .dynamic_state(&dynamic_state)
            .layout(layout)
            .push_next(&mut rendering_info);

        let pipelines = unsafe {
            device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
                .map_err(|(pipelines, err)| {
                    for pipeline in pipelines {
                        device.destroy_pipeline(pipeline, None);
                    }
                    err
                })?
        };

        Ok(pipelines[0])
    }

    fn build_mesh_pipeline(
        device: &ash::Device,
        layout: vk::PipelineLayout,
        vert_module: vk::ShaderModule,
        frag_module: vk::ShaderModule,
        color_format: vk::Format,
    ) -> Result<vk::Pipeline, vk::Result> {
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<MeshGpuVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attributes = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .binding(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .binding(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(8),
        ];
        Self::build_triangle_pipeline(
            device,
            layout,
            vert_module,
            frag_module,
            color_format,
            &bindings,
            &attributes,
        )
    }

    fn build_particle_pipeline(
        device: &ash::Device,
        layout: vk::PipelineLayout,
        vert_module: vk::ShaderModule,
        frag_module: vk::ShaderModule,
        color_format: vk::Format,
    ) -> Result<vk::Pipeline, vk::Result> {
        use crate::render_helpers::particles::ParticleVertex;
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<ParticleVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attributes = [
            (
                vk::Format::R32G32_SFLOAT,
                std::mem::offset_of!(ParticleVertex, position),
            ),
            (
                vk::Format::R32G32_SFLOAT,
                std::mem::offset_of!(ParticleVertex, local),
            ),
            (
                vk::Format::R32G32B32A32_SFLOAT,
                std::mem::offset_of!(ParticleVertex, color),
            ),
            (
                vk::Format::R32_SFLOAT,
                std::mem::offset_of!(ParticleVertex, shape),
            ),
            (
                vk::Format::R32_SFLOAT,
                std::mem::offset_of!(ParticleVertex, inv_width),
            ),
            (
                vk::Format::R32_SFLOAT,
                std::mem::offset_of!(ParticleVertex, aspect),
            ),
        ]
        .map(|(format, offset)| {
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .format(format)
                .offset(offset as u32)
        });
        let attributes: Vec<_> = attributes
            .into_iter()
            .enumerate()
            .map(|(location, attribute)| attribute.location(location as u32))
            .collect();
        Self::build_triangle_pipeline(
            device,
            layout,
            vert_module,
            frag_module,
            color_format,
            &bindings,
            &attributes,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_triangle_pipeline(
        device: &ash::Device,
        layout: vk::PipelineLayout,
        vert_module: vk::ShaderModule,
        frag_module: vk::ShaderModule,
        color_format: vk::Format,
        bindings: &[vk::VertexInputBindingDescription],
        attributes: &[vk::VertexInputAttributeDescription],
    ) -> Result<vk::Pipeline, vk::Result> {
        let entry_name = c"main";
        let shader_stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert_module)
                .name(entry_name),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag_module)
                .name(entry_name),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(bindings)
            .vertex_attribute_descriptions(attributes);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
            .primitive_restart_enable(false);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .depth_clamp_enable(false)
            .rasterizer_discard_enable(false)
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE);
        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .sample_shading_enable(false)
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let color_blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            // Coverage union: Ad + As * (1 - Ad). The equivalent source-
            // first form can round an opaque FP16 destination below one on
            // fixed-function blend hardware, leaking the next scene on replay.
            // This form keeps Ad=1 exactly, without changing RGB blending or
            // treating genuinely translucent near-one samples as opaque.
            .src_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_DST_ALPHA)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE)
            .alpha_blend_op(vk::BlendOp::ADD)];
        let color_blending =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [color_format];
        let mut rendering_info =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats);
        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisampling)
            .color_blend_state(&color_blending)
            .dynamic_state(&dynamic_state)
            .layout(layout)
            .push_next(&mut rendering_info);
        let pipelines = unsafe {
            device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
                .map_err(|(pipelines, err)| {
                    for pipeline in pipelines {
                        device.destroy_pipeline(pipeline, None);
                    }
                    err
                })?
        };
        Ok(pipelines[0])
    }

    /// Get or allocate and bind a cached descriptor set for a texture.
    pub fn get_or_create_texture_descriptor_set(
        self: &Arc<Self>,
        texture: &VulkanTexture,
        use_linear_filter: bool,
    ) -> Result<vk::DescriptorSet, vk::Result> {
        if !Arc::ptr_eq(&self.ctx, &texture.inner.ctx) {
            return Err(vk::Result::ERROR_INITIALIZATION_FAILED);
        }
        // The immutable YCbCr sampler fixes filtering for this conversion;
        // nearest/linear draw hints need not consume two descriptor sets.
        let linear = texture.inner.yuv_pipeline.is_some() || use_linear_filter;
        let mut descriptors = texture.inner.descriptors.lock().unwrap();
        if let Some(lease) = descriptors
            .iter()
            .find(|d| Arc::ptr_eq(&d.owner, self) && d.linear == linear)
        {
            return Ok(lease.set);
        }

        let set_layouts = [texture
            .inner
            .yuv_pipeline
            .as_ref()
            .map_or(self.descriptor_set_layout, |yuv| yuv.set_layout)];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.descriptor_pool)
            .set_layouts(&set_layouts);

        let descriptor_set = {
            let _pool_lock = self.descriptor_pool_lock.lock().unwrap();
            unsafe { self.ctx.device.allocate_descriptor_sets(&alloc_info) }?[0]
        };

        let sampler = if linear {
            self.sampler_linear
        } else {
            self.sampler_nearest
        };

        let image_info = [vk::DescriptorImageInfo::default()
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image_view(texture.sampled_image_view())
            .sampler(if texture.inner.yuv_pipeline.is_some() {
                vk::Sampler::null() // immutable in the YCbCr descriptor layout
            } else {
                sampler
            })];

        let writes = [vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(0)
            .dst_array_element(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info)];

        unsafe {
            self.ctx.device.update_descriptor_sets(&writes, &[]);
        }

        descriptors.push(TextureDescriptor {
            owner: self.clone(),
            set: descriptor_set,
            linear,
        });
        Ok(descriptor_set)
    }

    /// Backwards-compatible alias for descriptor set acquisition.
    pub fn create_texture_descriptor_set(
        self: &Arc<Self>,
        texture: &VulkanTexture,
        use_linear_filter: bool,
    ) -> Result<vk::DescriptorSet, vk::Result> {
        self.get_or_create_texture_descriptor_set(texture, use_linear_filter)
    }

    fn create_shader_module(
        device: &ash::Device,
        bytes: &[u8],
    ) -> Result<vk::ShaderModule, vk::Result> {
        let mut cursor = std::io::Cursor::new(bytes);
        let code = ash::util::read_spv(&mut cursor)
            .map_err(|_| vk::Result::ERROR_INITIALIZATION_FAILED)?;

        let create_info = vk::ShaderModuleCreateInfo::default().code(&code);
        unsafe { device.create_shader_module(&create_info, None) }
    }
}

impl Drop for VulkanPipelineManager {
    fn drop(&mut self) {
        unsafe {
            let device = &self.ctx.device;
            if let Ok(pipes_map) = self.pipelines_by_format.read() {
                for pipes in pipes_map.values() {
                    device.destroy_pipeline(pipes.quad_pipeline, None);
                    device.destroy_pipeline(pipes.border_pipeline, None);
                    device.destroy_pipeline(pipes.shadow_pipeline, None);
                    device.destroy_pipeline(pipes.blur_pipeline, None);
                    device.destroy_pipeline(pipes.particle_pipeline, None);
                    device.destroy_pipeline(pipes.alpha_one_pipeline, None);
                    device.destroy_pipeline(pipes.mesh_pipeline, None);
                    device.destroy_pipeline(pipes.color_pipeline, None);
                }
            }
            device.destroy_pipeline_layout(self.alpha_one_pipeline_layout, None);
            device.destroy_pipeline_layout(self.border_pipeline_layout, None);
            device.destroy_pipeline_layout(self.quad_pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            device.destroy_sampler(self.sampler_linear, None);
            device.destroy_sampler(self.sampler_nearest, None);
        }
    }
}

// A texture keeps its pool and samplers alive and returns its descriptor slots.
pub(super) struct TextureDescriptor {
    owner: Arc<VulkanPipelineManager>,
    set: vk::DescriptorSet,
    linear: bool,
}
impl Drop for TextureDescriptor {
    fn drop(&mut self) {
        let _pool_lock = self.owner.descriptor_pool_lock.lock().unwrap();
        unsafe {
            let _ = self
                .owner
                .ctx
                .device
                .free_descriptor_sets(self.owner.descriptor_pool, &[self.set]);
        }
    }
}

/// RAII for partial pipeline construction (ash handles do not implement Drop).
struct BuildGuard<'a> {
    device: &'a ash::Device,
    conversions: Vec<vk::SamplerYcbcrConversion>,
    samplers: Vec<vk::Sampler>,
    set_layouts: Vec<vk::DescriptorSetLayout>,
    pools: Vec<vk::DescriptorPool>,
    layouts: Vec<vk::PipelineLayout>,
    shaders: Vec<vk::ShaderModule>,
    pipelines: Vec<vk::Pipeline>,
    disarmed: bool,
}
impl<'a> BuildGuard<'a> {
    fn new(device: &'a ash::Device) -> Self {
        Self {
            device,
            conversions: Vec::new(),
            samplers: Vec::new(),
            set_layouts: Vec::new(),
            pools: Vec::new(),
            layouts: Vec::new(),
            shaders: Vec::new(),
            pipelines: Vec::new(),
            disarmed: false,
        }
    }
}
impl Drop for BuildGuard<'_> {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        unsafe {
            for &v in &self.pipelines {
                self.device.destroy_pipeline(v, None);
            }
            for &v in &self.shaders {
                self.device.destroy_shader_module(v, None);
            }
            for &v in &self.layouts {
                self.device.destroy_pipeline_layout(v, None);
            }
            for &v in &self.pools {
                self.device.destroy_descriptor_pool(v, None);
            }
            for &v in &self.set_layouts {
                self.device.destroy_descriptor_set_layout(v, None);
            }
            for &v in &self.samplers {
                self.device.destroy_sampler(v, None);
            }
            for &v in &self.conversions {
                self.device.destroy_sampler_ycbcr_conversion(v, None);
            }
        }
    }
}
