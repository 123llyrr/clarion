#version 450
#extension GL_GOOGLE_include_directive : require
layout(push_constant) uniform ShadowPushConstants {
    mat4 matrix;
    vec4 color;
    vec4 corner_radius;
    vec4 window_radius;
    vec4 coords_bounds;
    vec4 geometry;
    vec4 window_geometry;
    vec4 params;
} pc;
layout(location = 0) in vec2 in_coords;
layout(location = 0) out vec4 out_color;
#define CLARION_MASK_SCALE pc.params.y
#define CLARION_MASK_POWER 2.0
#include "../../shaders/rounding_alpha.frag"
#include "../../shaders/shadow_kernel.glsl"

void main() {
    vec2 p = in_coords - pc.geometry.xy;
    float mask = pc.params.x < 0.1 ? clarion_rounding_alpha(p, pc.geometry.zw, pc.corner_radius)
        : clarion_shadow_mass(p, pc.geometry.zw, pc.corner_radius, pc.params.x);
    if (all(greaterThan(pc.window_geometry.zw, vec2(0.0)))) {
        vec2 hole_p = in_coords - pc.window_geometry.xy;
        mask *= 1.0 - clarion_rounding_alpha(hole_p, pc.window_geometry.zw, pc.window_radius);
    }
    out_color = pc.color * (mask * pc.params.z);
}
