//! Check the exact descriptor layout and std140 allocation against this device
//! before its first Vulkan object is created. Combined image samplers count
//! against both sampler and sampled-image limits, and once as a stage resource.
use ash::vk;

pub(super) fn validate(
    uniform_slots: usize,
    sampler_bindings: u32,
    limits: &vk::PhysicalDeviceLimits,
) -> Result<(), String> {
    let uniform_bytes = uniform_slots
        .max(1)
        .checked_mul(16)
        .ok_or_else(|| "animation uniform byte count overflow".to_owned())?;
    if uniform_bytes > limits.max_uniform_buffer_range as usize {
        return Err(format!("animation uniform buffer needs {uniform_bytes} bytes; device maxUniformBufferRange is {}", limits.max_uniform_buffer_range));
    }
    // The animation pipeline has one UBO and one framebuffer color attachment;
    // both count against the fragment-stage total resource limit.
    let resources = sampler_bindings
        .checked_add(2)
        .ok_or_else(|| "animation stage resource count overflow".to_owned())?;
    for (name, available, needed) in [
        (
            "maxBoundDescriptorSets",
            limits.max_bound_descriptor_sets,
            1,
        ),
        (
            "maxPerStageDescriptorUniformBuffers",
            limits.max_per_stage_descriptor_uniform_buffers,
            1,
        ),
        (
            "maxDescriptorSetUniformBuffers",
            limits.max_descriptor_set_uniform_buffers,
            1,
        ),
        (
            "maxPerStageDescriptorSamplers",
            limits.max_per_stage_descriptor_samplers,
            sampler_bindings,
        ),
        (
            "maxPerStageDescriptorSampledImages",
            limits.max_per_stage_descriptor_sampled_images,
            sampler_bindings,
        ),
        (
            "maxDescriptorSetSamplers",
            limits.max_descriptor_set_samplers,
            sampler_bindings,
        ),
        (
            "maxDescriptorSetSampledImages",
            limits.max_descriptor_set_sampled_images,
            sampler_bindings,
        ),
        (
            "maxPerStageResources",
            limits.max_per_stage_resources,
            resources,
        ),
    ] {
        if needed > available {
            return Err(format!("animation descriptor layout needs {needed} for {name}; device limit is {available}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device() -> vk::PhysicalDeviceLimits {
        vk::PhysicalDeviceLimits {
            max_uniform_buffer_range: 65_536,
            max_bound_descriptor_sets: 4,
            max_per_stage_descriptor_uniform_buffers: 12,
            max_descriptor_set_uniform_buffers: 72,
            max_per_stage_descriptor_samplers: 16,
            max_per_stage_descriptor_sampled_images: 16,
            max_descriptor_set_samplers: 96,
            max_descriptor_set_sampled_images: 96,
            max_per_stage_resources: 128,
            ..Default::default()
        }
    }
    #[test]
    fn exact_uniform_boundary_is_valid_and_one_more_slot_is_rejected() {
        let limits = device();
        assert!(validate(4096, 2, &limits).is_ok());
        let error = validate(4097, 2, &limits).unwrap_err();
        assert!(error.contains("65552") && error.contains("maxUniformBufferRange"));
        let mut narrow = limits;
        narrow.max_uniform_buffer_range = 16_384;
        assert!(validate(1024, 2, &narrow).is_ok());
        assert!(validate(1025, 2, &narrow).is_err());
    }
    #[test]
    fn each_combined_descriptor_limit_is_checked_independently() {
        for which in 0..4 {
            let mut limits = device();
            match which {
                0 => limits.max_per_stage_descriptor_samplers = 2,
                1 => limits.max_per_stage_descriptor_sampled_images = 2,
                2 => limits.max_descriptor_set_samplers = 2,
                _ => limits.max_descriptor_set_sampled_images = 2,
            }
            assert!(validate(1, 2, &limits).is_ok());
            assert!(validate(1, 3, &limits)
                .unwrap_err()
                .contains("device limit is 2"));
        }
    }
    #[test]
    fn uniform_descriptor_and_total_stage_resource_limits_include_the_ubo() {
        let mut limits = device();
        limits.max_per_stage_resources = 3;
        assert!(validate(0, 2, &limits).is_err());
        limits.max_per_stage_resources = 4;
        assert!(validate(0, 2, &limits).is_ok());
        assert!(validate(1, 3, &limits)
            .unwrap_err()
            .contains("maxPerStageResources"));
        for which in 0..3 {
            let mut limits = device();
            match which {
                0 => limits.max_bound_descriptor_sets = 0,
                1 => limits.max_per_stage_descriptor_uniform_buffers = 0,
                _ => limits.max_descriptor_set_uniform_buffers = 0,
            }
            assert!(validate(0, 2, &limits).is_err());
        }
    }
    #[test]
    fn empty_uniforms_still_reserve_one_vec4_and_overflow_is_rejected() {
        let mut limits = device();
        limits.max_uniform_buffer_range = 16;
        assert!(validate(0, 2, &limits).is_ok());
        limits.max_uniform_buffer_range = 15;
        assert!(validate(0, 2, &limits)
            .unwrap_err()
            .contains("needs 16 bytes"));
        assert!(validate(usize::MAX, 2, &device())
            .unwrap_err()
            .contains("byte count overflow"));
        assert!(validate(1, u32::MAX, &device())
            .unwrap_err()
            .contains("resource count overflow"));
    }
}
