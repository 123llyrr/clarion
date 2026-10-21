#version 450
#extension GL_GOOGLE_include_directive : require
layout(push_constant) uniform BorderPushConstants {
    mat4 matrix;
    vec4 color_from;
    vec4 color_to;
    vec4 outer_radius;
    vec2 geo_size;
    vec2 grad_offset;
    vec2 grad_vec;
    float colorspace;
    float hue_interpolation;
    float grad_width;
    float border_width;
    float alpha;
    float scale;
    float rotation_angle;
    float rounding_power;
    vec2 _geo_padding;
    vec4 geo_bounds;
} pc;
layout(location = 0) in vec2 in_uv;
layout(location = 0) out vec4 out_color;
#define CLARION_MASK_SCALE pc.scale
#define CLARION_MASK_POWER pc.rounding_power
#include "../../shaders/rounding_alpha.frag"
#include "../../shaders/gradient_color.glsl"

void main() {
    vec2 p = in_uv * pc.geo_size;
    float angle = radians(pc.rotation_angle);
    vec2 d = p - pc.geo_size * 0.5;
    vec2 gradient_p = mat2(cos(angle), sin(angle), -sin(angle), cos(angle)) * d + pc.geo_size * 0.5;
    float t = clarion_gradient_position(gradient_p, pc.grad_offset, pc.grad_vec, pc.grad_width);
    float mask = clarion_rounding_alpha(p, pc.geo_size, pc.outer_radius);
    if (pc.border_width > 0.0) {
        float hole = clarion_rounding_alpha(p - pc.border_width,
            pc.geo_size - 2.0 * pc.border_width, max(pc.outer_radius - pc.border_width, 0.0));
        mask *= 1.0 - hole;
    }
    out_color = clarion_gradient_mix(pc.color_from, pc.color_to, t, pc.colorspace, pc.hue_interpolation) * (pc.alpha * mask);
}
