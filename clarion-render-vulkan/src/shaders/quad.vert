#version 450

layout(push_constant) uniform PushConstants {
    mat4 matrix;
    vec4 uv_bounds; // [min_u, min_v, max_u, max_v]
    vec4 color_tint;
    float alpha;
    float corner_radius;
    vec2 size;
    vec4 clip_matrix[3];
    vec4 clip_radius;
    vec2 clip_size;
    float clip_scale;
    float clip_enabled;
} pc;

layout(location = 0) out vec2 out_uv;

const vec2 POSITIONS[4] = vec2[](
    vec2(0.0, 0.0),
    vec2(1.0, 0.0),
    vec2(0.0, 1.0),
    vec2(1.0, 1.0)
);

void main() {
    vec2 pos = POSITIONS[gl_VertexIndex];
    out_uv = mix(pc.uv_bounds.xy, pc.uv_bounds.zw, pos);
    gl_Position = pc.matrix * vec4(pos, 0.0, 1.0);
}
