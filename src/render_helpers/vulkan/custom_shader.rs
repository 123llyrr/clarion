//! Compatibility renderer for the existing animation GLSL interface.
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use calloop::channel::Channel;

use anyhow::{bail, Context};
use ash::vk;
use smithay::backend::renderer::gles::{Uniform, UniformValue};

use super::device::{StagingBuffer, VulkanContext};
use super::{VulkanPipelineManager, VulkanTexture};

#[path = "custom_shader/compile_worker.rs"]
mod compile_worker;
#[path = "custom_shader/gpu_preparation.rs"]
pub(super) mod gpu_preparation;
#[path = "custom_shader/gpu_provider.rs"]
pub(super) mod gpu_provider;
#[path = "custom_shader/provider.rs"]
mod provider;
pub(crate) use gpu_preparation::Event as GpuPreparationEvent;
pub(crate) use provider::{CompileCompletion, CompileData};
pub(super) type CompilerEndpoint = provider::Endpoint<PreparedShader>;

/// Pure owned translation/SPIR-V, with no Vulkan, renderer, Wayland or desktop handle.
pub(crate) struct PreparedShader {
    interface: Interface,
    vert: Vec<u32>,
    frag: Vec<u32>,
}
impl PreparedShader {
    pub(super) fn embedded(source: &str) -> anyhow::Result<Self> {
        let frag = embedded_fragment(source)
            .context("custom shader must be prepared by the compiler worker")?;
        Ok(Self {
            interface: translate(source)?,
            vert: spirv(include_bytes!("shaders/animation.vert.spv")),
            frag: spirv(frag),
        })
    }
}
pub(super) fn is_embedded(source: &str) -> bool {
    embedded_fragment(source).is_some()
}
fn prepare_custom(source: &str, cancel: &AtomicBool) -> Result<PreparedShader, String> {
    if source.len() > compile_worker::Limits::default().source_bytes {
        return Err("shader source exceeds byte limit".to_owned());
    }
    let interface = translate(source).map_err(|error| error.to_string())?;
    let frag = compile_worker::compile(&interface.source, "frag", cancel)
        .map_err(|error| error.to_string())?;
    Ok(PreparedShader {
        interface,
        vert: spirv(include_bytes!("shaders/animation.vert.spv")),
        frag,
    })
}
pub(super) fn compiler_endpoint(
) -> Result<(CompilerEndpoint, Channel<CompileCompletion<PreparedShader>>), String> {
    static COMPILER: super::lifecycle::ServiceCell<provider::Compiler<PreparedShader>> =
        super::lifecycle::ServiceCell::new();
    COMPILER
        .get_or_try_init(|| provider::Compiler::new(prepare_custom))
        .map(|compiler| compiler.endpoint())
        .map_err(|error| error.to_string())
}

#[derive(Debug)]
pub struct Interface {
    pub source: String,
    uniforms: HashMap<String, (usize, usize)>,
    pub samplers: Vec<String>,
    slots: usize,
    uniform_binding: u32,
}

pub fn translate(source: &str) -> anyhow::Result<Interface> {
    let mut out = String::from(
        "#version 450\nlayout(location=0) in vec2 clarion_v_coords;\nlayout(location=0) out vec4 clarion_out;\n#define gl_FragColor clarion_out\n#define texture2D texture\n",
    );
    let mut uniforms = HashMap::new();
    let mut samplers = Vec::new();
    let mut slots = 0;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("precision ") || trimmed.starts_with("varying ") {
            continue;
        }
        if trimmed.starts_with("uniform ") {
            let mut words = trimmed.trim_end_matches(';').split_whitespace();
            words.next();
            let ty = words.next().context("missing uniform type")?;
            let name = words.next().context("missing uniform name")?;
            if ty == "sampler2D" {
                out.push_str(&format!(
                    "layout(set=0,binding={}) uniform sampler2D {name};\n",
                    samplers.len()
                ));
                samplers.push(name.to_owned());
                continue;
            }
            let (count, expr) = match ty {
                "float" => (1, format!("u.data[{slots}].x")),
                "vec2" => (1, format!("u.data[{slots}].xy")),
                "vec3" => (1, format!("u.data[{slots}].xyz")),
                "vec4" => (1, format!("u.data[{slots}]")),
                "mat3" => (
                    3,
                    format!(
                        "mat3(u.data[{slots}].xyz,u.data[{}].xyz,u.data[{}].xyz)",
                        slots + 1,
                        slots + 2
                    ),
                ),
                _ => bail!("unsupported animation uniform type {ty}"),
            };
            out.push_str(&format!("#define {name} ({expr})\n"));
            uniforms.insert(name.to_owned(), (slots, count));
            slots += count;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    // The block must precede functions that use the uniform macros.
    // Keep the built-in shaders' binding 2 ABI, while custom shaders with
    // additional samplers place their uniform block after the last sampler.
    let uniform_binding = samplers.len().max(2) as u32;
    let header = format!(
        "layout(std140,set=0,binding={uniform_binding}) uniform Data {{ vec4 data[{}]; }} u;\n",
        slots.max(1)
    );
    out.insert_str(out.find('\n').unwrap() + 1, &header);
    Ok(Interface {
        source: out,
        uniforms,
        samplers,
        slots,
        uniform_binding,
    })
}

fn spirv(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect()
}

fn embedded_fragment(source: &str) -> Option<&'static [u8]> {
    use crate::render_helpers::shaders::{
        animation_source, ProgramType, DENIAL_EXPLOSION_SHADER, DENIAL_IMPLODE_SHADER,
    };
    let builtins: [(ProgramType, Option<&str>, &[u8]); 5] = [
        (
            ProgramType::Resize,
            None,
            include_bytes!("shaders/animation_resize.frag.spv"),
        ),
        (
            ProgramType::Open,
            None,
            include_bytes!("shaders/animation_open.frag.spv"),
        ),
        (
            ProgramType::Close,
            None,
            include_bytes!("shaders/animation_close.frag.spv"),
        ),
        (
            ProgramType::Close,
            Some(DENIAL_EXPLOSION_SHADER),
            include_bytes!("shaders/animation_explosion.frag.spv"),
        ),
        (
            ProgramType::Close,
            Some(DENIAL_IMPLODE_SHADER),
            include_bytes!("shaders/animation_implode.frag.spv"),
        ),
    ];
    builtins.into_iter().find_map(|(kind, custom, bytes)| {
        (source == animation_source(kind, custom)).then_some(bytes)
    })
}

pub struct Program {
    ctx: Arc<VulkanContext>,
    interface: Interface,
    vert: vk::ShaderModule,
    frag: vk::ShaderModule,
    descriptor_layout: vk::DescriptorSetLayout,
    sampler: vk::Sampler,
    pub layout: vk::PipelineLayout,
    pipelines: Mutex<HashMap<vk::Format, vk::Pipeline>>,
    idle_descriptors: Mutex<Vec<DescriptorSlot>>,
}

// Each draw owns a distinct slot until its last CPU/GPU owner releases it.
// A small idle cache avoids driver pool allocation on every animation frame.
const MAX_IDLE_DESCRIPTOR_SLOTS: usize = 32;

struct DescriptorSlot {
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
}
impl std::fmt::Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeAnimationProgram")
            .finish_non_exhaustive()
    }
}
impl Program {
    /// Initial embedded fallback modules have no external compiler or format pipelines.
    pub fn new(ctx: Arc<VulkanContext>, source: &str) -> anyhow::Result<Arc<Self>> {
        Self::from_prepared(ctx, PreparedShader::embedded(source)?)
    }
    /// Build device objects on the bounded GPU worker; only initial embedded fallback
    /// module/layout creation is permitted during renderer bootstrap.
    pub(crate) fn from_prepared(
        ctx: Arc<VulkanContext>,
        prepared: PreparedShader,
    ) -> anyhow::Result<Arc<Self>> {
        let PreparedShader {
            interface,
            vert,
            frag,
        } = prepared;
        let mut program = Self {
            ctx,
            interface,
            vert: vk::ShaderModule::null(),
            frag: vk::ShaderModule::null(),
            descriptor_layout: vk::DescriptorSetLayout::null(),
            sampler: vk::Sampler::null(),
            layout: vk::PipelineLayout::null(),
            pipelines: Mutex::new(HashMap::new()),
            idle_descriptors: Mutex::new(Vec::new()),
        };
        unsafe {
            let device = &program.ctx.device;
            program.vert = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&vert), None)?;
            program.frag = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&frag), None)?;
            program.sampler = device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_BORDER)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_BORDER)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_BORDER)
                    .border_color(vk::BorderColor::FLOAT_TRANSPARENT_BLACK),
                None,
            )?;
            let sampler_bindings = program.interface.uniform_binding;
            let mut bindings = (0..sampler_bindings)
                .map(|binding| {
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(binding)
                        .descriptor_count(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                })
                .collect::<Vec<_>>();
            bindings.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(program.interface.uniform_binding)
                    .descriptor_count(1)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            );
            program.descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            program.layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&[program.descriptor_layout])
                    .push_constant_ranges(&[vk::PushConstantRange::default()
                        .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                        .size(80)]),
                None,
            )?;
        }
        Ok(Arc::new(program))
    }

    /// Prepare a format on the device worker before publishing it to the render path.
    /// Drawing only looks up an already prepared pipeline.
    pub(crate) fn prepare_format(&self, format: vk::Format) -> Result<vk::Pipeline, vk::Result> {
        {
            let pipelines = self.pipelines.lock().unwrap();
            if let Some(&pipeline) = pipelines.get(&format) {
                return Ok(pipeline);
            }
        }
        let pipeline = VulkanPipelineManager::build_pipeline(
            &self.ctx.device,
            self.layout,
            self.vert,
            self.frag,
            format,
        )?;
        let (selected, published) = match self.pipelines.lock().unwrap().entry(format) {
            std::collections::hash_map::Entry::Occupied(entry) => (*entry.get(), false),
            std::collections::hash_map::Entry::Vacant(entry) => (*entry.insert(pipeline), true),
        };
        if !published {
            // A concurrent worker won publication; the unused duplicate has no GPU owner.
            unsafe {
                self.ctx.device.destroy_pipeline(pipeline, None);
            }
        }
        Ok(selected)
    }

    pub(crate) fn has_prepared_format(&self, format: vk::Format) -> bool {
        self.pipelines.lock().unwrap().contains_key(&format)
    }

    pub fn draw(
        self: &Arc<Self>,
        format: vk::Format,
        textures: &HashMap<String, VulkanTexture>,
        uniforms: &[Uniform<'_>],
    ) -> anyhow::Result<Arc<ShaderDraw>> {
        // Runtime draw never invokes a driver compiler. The owner publishes only programs
        // prepared for the target; an unprepared standalone caller receives a retryable error.
        let pipeline = self
            .pipelines
            .lock()
            .unwrap()
            .get(&format)
            .copied()
            .ok_or(vk::Result::NOT_READY)?;
        let mut data = vec![[0f32; 4]; self.interface.slots.max(1)];
        for uniform in uniforms {
            let Some(&(slot, count)) = self.interface.uniforms.get(uniform.name.as_ref()) else {
                continue;
            };
            match &uniform.value {
                UniformValue::_1f(a) => data[slot][0] = *a,
                UniformValue::_2f(a, b) => data[slot] = [*a, *b, 0., 0.],
                UniformValue::_3f(a, b, c) => data[slot] = [*a, *b, *c, 0.],
                UniformValue::_4f(a, b, c, d) => data[slot] = [*a, *b, *c, *d],
                UniformValue::Matrix3x3 {
                    matrices,
                    transpose,
                } if count == 3 && matrices.len() == 1 => {
                    for c in 0..3 {
                        for r in 0..3 {
                            data[slot + c][r] =
                                matrices[0][if *transpose { r * 3 + c } else { c * 3 + r }];
                        }
                    }
                }
                _ => bail!("unsupported value for animation uniform {}", uniform.name),
            }
        }
        let textures = self
            .interface
            .samplers
            .iter()
            .map(|name| {
                textures
                    .get(name)
                    .cloned()
                    .with_context(|| format!("missing shader texture {name}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        if textures
            .iter()
            .any(|t| !Arc::ptr_eq(&t.inner.ctx, &self.ctx))
        {
            bail!("shader textures belong to a different Vulkan device");
        }
        if textures.iter().any(|t| t.inner.yuv_pipeline.is_some()) {
            // These shaders use ordinary, mutable samplers. Vulkan requires an
            // immutable sampler matching the YCbCr image view conversion.
            bail!("animation shader cannot bind a YCbCr texture directly");
        }
        let mut draw = ShaderDraw {
            program: self.clone(),
            pipeline,
            textures,
            descriptor: None,
            set: vk::DescriptorSet::null(),
            buffer: None,
        };
        let bytes = bytemuck::cast_slice(&data);
        draw.buffer = Some(self.ctx.acquire_vertex_buffer(bytes.len())?);
        self.ctx
            .write_host_buffer(draw.buffer.as_ref().unwrap(), bytes)?;
        unsafe {
            let device = &self.ctx.device;
            let descriptor = self.acquire_descriptor()?;
            draw.set = descriptor.set;
            draw.descriptor = Some(descriptor);
            for (binding, t) in draw.textures.iter().enumerate() {
                let image = [vk::DescriptorImageInfo::default()
                    .sampler(self.sampler)
                    .image_view(t.sampled_image_view())
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default()
                        .dst_set(draw.set)
                        .dst_binding(binding as u32)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&image)],
                    &[],
                );
            }
            let buffer = [vk::DescriptorBufferInfo::default()
                .buffer(draw.buffer.as_ref().unwrap().buffer)
                .range(bytes.len() as u64)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(draw.set)
                    .dst_binding(self.interface.uniform_binding)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&buffer)],
                &[],
            );
        }
        Ok(Arc::new(draw))
    }

    fn acquire_descriptor(&self) -> Result<DescriptorSlot, vk::Result> {
        if let Some(slot) = self.idle_descriptors.lock().unwrap().pop() {
            return Ok(slot);
        }
        let sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(self.interface.uniform_binding),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1),
        ];
        let device = &self.ctx.device;
        let pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&sizes),
                None,
            )?
        };
        let layouts = [self.descriptor_layout];
        let sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&layouts),
            )
        };
        match sets {
            Ok(sets) => Ok(DescriptorSlot { pool, set: sets[0] }),
            Err(err) => {
                unsafe { device.destroy_descriptor_pool(pool, None) };
                Err(err)
            }
        }
    }

    fn release_descriptor(&self, slot: DescriptorSlot) {
        let mut idle = self.idle_descriptors.lock().unwrap();
        if idle.len() < MAX_IDLE_DESCRIPTOR_SLOTS {
            idle.push(slot);
            return;
        }
        drop(idle);
        unsafe { self.ctx.device.destroy_descriptor_pool(slot.pool, None) };
    }
}
impl Drop for Program {
    fn drop(&mut self) {
        unsafe {
            let d = &self.ctx.device;
            for slot in self.idle_descriptors.get_mut().unwrap().drain(..) {
                d.destroy_descriptor_pool(slot.pool, None);
            }
            for (_, p) in self.pipelines.get_mut().unwrap().drain() {
                d.destroy_pipeline(p, None);
            }
            d.destroy_sampler(self.sampler, None);
            d.destroy_pipeline_layout(self.layout, None);
            d.destroy_descriptor_set_layout(self.descriptor_layout, None);
            d.destroy_shader_module(self.vert, None);
            d.destroy_shader_module(self.frag, None);
        }
    }
}
pub struct ShaderDraw {
    pub program: Arc<Program>,
    pub pipeline: vk::Pipeline,
    pub textures: Vec<VulkanTexture>,
    descriptor: Option<DescriptorSlot>,
    pub set: vk::DescriptorSet,
    buffer: Option<StagingBuffer>,
}
impl Drop for ShaderDraw {
    fn drop(&mut self) {
        if let Some(slot) = self.descriptor.take() {
            self.program.release_descriptor(slot);
        }
        if let Some(buffer) = self.buffer.take() {
            self.program.ctx.release_vertex_buffer(buffer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_shader_places_uniform_block_after_extra_samplers() {
        let interface = translate(
            "uniform sampler2D first;\nuniform sampler2D second;\nuniform sampler2D third;\nuniform float amount;\nvoid main() { gl_FragColor = texture2D(third, clarion_v_coords) * amount; }",
        )
        .unwrap();
        assert_eq!(interface.samplers, ["first", "second", "third"]);
        assert_eq!(interface.uniform_binding, 3);
        assert!(interface.source.contains("layout(std140,set=0,binding=3)"));
        assert!(interface
            .source
            .contains("layout(set=0,binding=2) uniform sampler2D third"));
    }

    const TEST_SHADER: &str = "uniform sampler2D color;\nuniform float amount;\nvoid main() { gl_FragColor = texture2D(color, clarion_v_coords) * amount; }";

    /// Explicit fixture wait for the real CPU and GPU workers; production never calls it.
    fn prepared_test_program(ctx: Arc<VulkanContext>, source: &str) -> Arc<Program> {
        let (cpu, results) = compiler_endpoint().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while cpu
            .request(0, 0, 1, source.to_owned(), cancel.clone())
            .is_err()
        {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let prepared = loop {
            match results.try_recv() {
                Ok(completion) => break completion.into_data().result.unwrap(),
                Err(std::sync::mpsc::TryRecvError::Empty) => (),
                Err(error) => panic!("CPU preparation channel: {error}"),
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        let (gpu, results) = gpu_preparation::endpoint().unwrap();
        let mut work = gpu_preparation::Work::Build {
            ctx,
            prepared,
            formats: vec![vk::Format::R8G8B8A8_UNORM],
        };
        loop {
            match gpu.request(
                gpu_preparation::Identity {
                    renderer: 0,
                    kind: 0,
                    generation: 1,
                    source: source.to_owned(),
                },
                work,
                cancel.clone(),
            ) {
                Ok(()) => break,
                Err(rejected) => work = rejected.work,
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        loop {
            match results.try_recv() {
                Ok(gpu_preparation::Event::Ready(completion)) => {
                    let gpu_preparation::Prepared::Program { program, .. } =
                        completion.consume().unwrap()
                    else {
                        unreachable!()
                    };
                    return program;
                }
                Ok(gpu_preparation::Event::CapacityAvailable(wake)) => drop(wake),
                Err(std::sync::mpsc::TryRecvError::Empty) => (),
                Err(error) => panic!("GPU preparation channel: {error}"),
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan validation and glslc"]
    fn animation_descriptor_cache_retains_live_draws_and_bounds_idle_slots() {
        let (renderer, _, _) = super::super::hardware_tests::setup();
        let program = prepared_test_program(renderer.ctx.as_ref().unwrap().clone(), TEST_SHADER);
        let texture = renderer.create_solid_texture([255; 4]).unwrap();
        let textures = HashMap::from([("color".into(), texture)]);
        let make_draw = || {
            program
                .draw(
                    vk::Format::R8G8B8A8_UNORM,
                    &textures,
                    &[Uniform::new("amount", 1f32)],
                )
                .unwrap()
        };
        let first = make_draw();
        let retained = first.clone();
        let first_set = first.set;
        drop(first);
        assert!(program.idle_descriptors.lock().unwrap().is_empty());
        let second = make_draw();
        assert_ne!(
            first_set, second.set,
            "live draws cannot share a descriptor"
        );
        let second_set = second.set;
        drop(second);
        assert_eq!(
            make_draw().set,
            second_set,
            "idle descriptor must be reused"
        );
        drop(retained);
        let draws: Vec<_> = (0..MAX_IDLE_DESCRIPTOR_SLOTS + 9)
            .map(|_| make_draw())
            .collect();
        let sets: std::collections::HashSet<_> = draws.iter().map(|draw| draw.set).collect();
        assert_eq!(sets.len(), draws.len());
        drop(draws);
        assert_eq!(
            program.idle_descriptors.lock().unwrap().len(),
            MAX_IDLE_DESCRIPTOR_SLOTS
        );
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan validation and glslc"]
    fn animation_descriptor_cache_waits_for_gpu_owners_and_updates_pixels() {
        let (renderer, _, _) = super::super::hardware_tests::setup();
        let ctx = renderer.ctx.as_ref().unwrap();
        let program = prepared_test_program(ctx.clone(), TEST_SHADER);
        let target = super::super::allocate_texture(
            ctx,
            8,
            4,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageTiling::OPTIMAL,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
            vk::ImageLayout::UNDEFINED,
            true,
        )
        .unwrap();
        let colors = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
        // Match source and target pixel centers. A 1x1 texture sampled over
        // unit UVs intentionally filters against the animation's border color.
        let textures = colors.map(|color| {
            renderer
                .import_memory_texture(
                    &color.repeat(8 * 4),
                    smithay::backend::allocator::Fourcc::Abgr8888,
                    8,
                    4,
                    32,
                )
                .unwrap()
        });
        let make_draw = |index: usize, amount: f32| {
            program
                .draw(
                    vk::Format::R8G8B8A8_UNORM,
                    &HashMap::from([("color".into(), textures[index].clone())]),
                    &[Uniform::new("amount", amount)],
                )
                .unwrap()
        };
        let mut constants = [0.; 20];
        constants[..16].copy_from_slice(&[
            2., 0., 0., 0., 0., 2., 0., 0., 0., 0., 1., 0., -1., -1., 0., 1.,
        ]);
        constants[16..].copy_from_slice(&[0., 0., 1., 1.]);
        let first = make_draw(0, 0.5);
        let second = make_draw(1, 1.);
        let sets = [first.set, second.set];
        assert_ne!(sets[0], sets[1]);
        let mut frame = renderer
            .render_frame(
                target.image(),
                target.image_view(),
                vk::Format::R8G8B8A8_UNORM,
                true,
                8,
                4,
                Some([0.; 4]),
            )
            .unwrap();
        frame.set_clip(Some(smithay::utils::Rectangle::from_size((4, 4).into())));
        frame.draw_custom(first, constants);
        frame.set_clip(Some(smithay::utils::Rectangle::new(
            (4, 0).into(),
            (4, 4).into(),
        )));
        frame.draw_custom(second, constants);
        let completion = frame.finish_async(target.clone()).unwrap();
        assert!(
            program.idle_descriptors.lock().unwrap().is_empty(),
            "GPU completion owns the draws"
        );
        let unrelated = make_draw(2, 0.25);
        assert!(!sets.contains(&unrelated.set));
        drop(unrelated);
        completion.prepare_retirement().unwrap();
        drop(completion);
        let pixels = renderer
            .read_memory_texture(&target, smithay::utils::Rectangle::from_size((8, 4).into()))
            .unwrap();
        for (i, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
            let expected = if i % 8 < 4 {
                [128, 0, 0, 128]
            } else {
                [0, 255, 0, 255]
            };
            assert!(
                pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
                "pixel {i}: {pixel:?}"
            );
        }
        assert_eq!(program.idle_descriptors.lock().unwrap().len(), 3);
        let reused = make_draw(2, 0.25);
        assert!(
            sets.contains(&reused.set),
            "retired GPU slot must be reusable"
        );
        let mut frame = renderer
            .render_frame(
                target.image(),
                target.image_view(),
                vk::Format::R8G8B8A8_UNORM,
                true,
                8,
                4,
                Some([0.; 4]),
            )
            .unwrap();
        frame.draw_custom(reused, constants);
        frame.finish_and_wait().unwrap();
        target.current_layout().set(vk::ImageLayout::GENERAL);
        let pixels = renderer
            .read_memory_texture(&target, smithay::utils::Rectangle::from_size((8, 4).into()))
            .unwrap();
        for pixel in pixels.as_chunks::<4>().0 {
            assert!(
                pixel
                    .iter()
                    .zip([0, 0, 64, 64])
                    .all(|(a, b)| a.abs_diff(b) <= 1),
                "reused descriptor pixel: {pixel:?}"
            );
        }
    }
}
