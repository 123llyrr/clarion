#version 450

layout(push_constant) uniform MeshPushConstants {
    mat4 matrix;
    vec2 tex_scale;
    float alpha;
    float y_inverted;
} pc;

layout(location = 0) in vec2 in_position;
layout(location = 1) in vec2 in_uv;

layout(location = 0) out vec2 out_uv;
layout(location = 1) out float out_alpha;

void main() {
    vec2 uv = in_uv * pc.tex_scale;
    if (pc.y_inverted > 0.5) {
        uv.y = 1.0 - uv.y;
    }
    out_uv = uv;
    out_alpha = pc.alpha;
    gl_Position = pc.matrix * vec4(in_position, 0.0, 1.0);
}
