#version 450

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

layout(location = 0) out vec2 out_uv;

const vec2 POSITIONS[4] = vec2[](
    vec2(0.0, 0.0),
    vec2(1.0, 0.0),
    vec2(0.0, 1.0),
    vec2(1.0, 1.0)
);

void main() {
    vec2 pos = POSITIONS[gl_VertexIndex];
    out_uv = mix(pc.geo_bounds.xy, pc.geo_bounds.zw, pos);
    gl_Position = pc.matrix * vec4(pos, 0.0, 1.0);
}
