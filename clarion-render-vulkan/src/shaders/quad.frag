#version 450
#extension GL_GOOGLE_include_directive : require
#include "../../shaders/liquid_glass_common.glsl"

layout(push_constant) uniform PushConstants {
    mat4 matrix;
    vec4 uv_bounds;
    vec4 color_tint;
    float alpha;
    float corner_radius;
    vec2 size;
    vec4 clip_matrix[3];
    vec4 clip_radius;
    vec2 clip_size;
    float clip_scale;
    float clip_enabled;
    float noise;
    float saturation;
    vec2 target_size;
    vec4 bg_color;
    vec2 fade_cutoff;
    float hdr_transfer;
} pc;

layout(binding = 0) uniform sampler2D tex_sampler;

layout(location = 0) in vec2 in_uv;
layout(location = 0) out vec4 out_color;

#define CLARION_MASK_SCALE pc.clip_scale
#define CLARION_MASK_POWER pc.clip_matrix[0].w
#include "../../shaders/rounding_alpha.frag"

float corner_alpha(vec2 p, vec2 size, float radius) {
    return clarion_shape_alpha(p, size, vec4(radius), 1.0 / 1.4, 2.0);
}

float srgb_to_linear(float value) {
    return value <= 0.04045 ? value / 12.92 : pow((value + 0.055) / 1.055, 2.4);
}

float pq_encode(float luminance_nits) {
    const float m1 = 2610.0 / 16384.0;
    const float m2 = 2523.0 / 32.0;
    const float c1 = 3424.0 / 4096.0;
    const float c2 = 2413.0 / 128.0;
    const float c3 = 2392.0 / 128.0;
    float l = clamp(luminance_nits / 10000.0, 0.0, 1.0);
    if (l == 0.0) return 0.0;
    float lm = pow(l, m1);
    return pow((c1 + c2 * lm) / (1.0 + c3 * lm), m2);
}

vec3 srgb_to_pq(vec3 color) {
    // SDR reference white is 203 nits in HDR10.
    return vec3(
        pq_encode(srgb_to_linear(max(color.r, 0.0)) * 203.0),
        pq_encode(srgb_to_linear(max(color.g, 0.0)) * 203.0),
        pq_encode(srgb_to_linear(max(color.b, 0.0)) * 203.0));
}

#define CLARION_POST_NOISE pc.noise
#define CLARION_POST_SATURATION pc.saturation
#define CLARION_POST_BACKGROUND pc.bg_color
#include "../../shaders/postprocess_kernel.glsl"

void main() {
    vec2 sample_uv = in_uv;
    float glass_light = 0.0;
    if (pc.clip_enabled != 0.0 && pc.clip_matrix[1].w > 0.0) {
        mat3 input_to_geo = mat3(pc.clip_matrix[0].xyz, pc.clip_matrix[1].xyz, pc.clip_matrix[2].xyz);
        vec2 coords = (input_to_geo * vec3(in_uv, 1.0)).xy;
        vec3 lens = clarion_glass_lens(coords, pc.clip_size, pc.clip_radius, pc.clip_matrix[1].w);
        sample_uv = clamp(in_uv + clarion_glass_uv_offset(lens.xy, pc.clip_size, input_to_geo), vec2(0.0), vec2(1.0));
        glass_light = clamp(lens.z * pc.clip_matrix[2].w, 0.0, 1.0);
    }
    vec4 tex_color = texture(tex_sampler, sample_uv);
    tex_color.rgb = mix(tex_color.rgb, vec3(tex_color.a), glass_light);
    vec4 color = tex_color;
    if (pc.saturation != 1.0 || pc.noise > 0.0 || pc.bg_color.a > 0.0) {
        color = postprocess(color);
    }
    if (pc.fade_cutoff.x < pc.fade_cutoff.y) {
        float fade = clamp((pc.fade_cutoff.y - in_uv.x) / (pc.fade_cutoff.y - pc.fade_cutoff.x), 0.0, 1.0);
        color = color * fade;
    }
    float round_a = 1.0;
    if (pc.corner_radius > 0.0 && pc.size.x > 0.0 && pc.size.y > 0.0) {
        round_a = corner_alpha(in_uv * pc.size, pc.size, pc.corner_radius);
    }
    if (pc.clip_enabled != 0.0) {
        mat3 input_to_geo = mat3(pc.clip_matrix[0].xyz, pc.clip_matrix[1].xyz, pc.clip_matrix[2].xyz);
        vec2 coords = (input_to_geo * vec3(in_uv, 1.0)).xy;
        if (any(lessThan(coords, vec2(0.0))) || any(greaterThan(coords, vec2(1.0)))) {
            round_a = 0.0;
        } else {
            round_a *= clarion_rounding_alpha(coords * pc.clip_size, pc.clip_size, pc.clip_radius);
        }
    }
    color = color * pc.color_tint * (pc.alpha * round_a);
    if (pc.hdr_transfer != 0.0) {
        // Preserve premultiplied alpha while converting RGB to PQ.
        float inv_alpha = color.a > 0.0 ? 1.0 / color.a : 0.0;
        color.rgb = srgb_to_pq(color.rgb * inv_alpha) * color.a;
    }
    out_color = color;
}
