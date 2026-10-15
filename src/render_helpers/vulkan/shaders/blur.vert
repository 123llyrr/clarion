#version 450
layout(push_constant) uniform BlurPushConstants { mat4 matrix; vec4 params; } pc;
layout(location=0) out vec2 uv;
const vec2 POSITIONS[4] = vec2[](vec2(0.,0.),vec2(1.,0.),vec2(0.,1.),vec2(1.,1.));
void main() {
    vec2 pos = POSITIONS[gl_VertexIndex];
    uv = vec2(pos.x, pc.params.w != 0.0 ? 1.0 - pos.y : pos.y);
    gl_Position = pc.matrix * vec4(pos, 0., 1.);
}
