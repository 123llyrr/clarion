#version 450
layout(push_constant) uniform ColorPushConstants {
    mat4 matrix;
    vec4 uv_bounds;
} pc;
layout(location = 0) out vec2 uv;
const vec2 positions[4] = vec2[](vec2(0, 0), vec2(1, 0), vec2(0, 1), vec2(1, 1));
void main() {
    vec2 position = positions[gl_VertexIndex];
    uv = mix(pc.uv_bounds.xy, pc.uv_bounds.zw, position);
    gl_Position = pc.matrix * vec4(position, 0, 1);
}
