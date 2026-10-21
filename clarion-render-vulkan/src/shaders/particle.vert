#version 450

layout(push_constant) uniform ParticlePushConstants {
    mat4 matrix;
} pc;

layout(location = 0) in vec2 position;
layout(location = 1) in vec2 local;
layout(location = 2) in vec4 color;
layout(location = 3) in float shape;
layout(location = 4) in float inv_width;
layout(location = 5) in float aspect;

layout(location = 0) out vec2 out_local;
layout(location = 1) out vec4 out_color;
layout(location = 2) out float out_shape;
layout(location = 3) out float out_inv_width;
layout(location = 4) out float out_aspect;

void main() {
    out_local = local;
    out_color = color;
    out_shape = shape;
    out_inv_width = inv_width;
    out_aspect = aspect;
    gl_Position = pc.matrix * vec4(position, 0.0, 1.0);
}
