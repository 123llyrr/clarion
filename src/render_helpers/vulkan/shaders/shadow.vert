#version 450
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
layout(location = 0) out vec2 out_coords;
const vec2 POSITIONS[4] = vec2[](vec2(0.,0.),vec2(1.,0.),vec2(0.,1.),vec2(1.,1.));
void main() {
    vec2 pos = POSITIONS[gl_VertexIndex];
    out_coords = mix(pc.coords_bounds.xy, pc.coords_bounds.zw, pos);
    gl_Position = pc.matrix * vec4(pos, 0., 1.);
}
