#version 450

layout(binding = 0) uniform sampler2D tex_sampler;

layout(location = 0) in vec2 in_uv;
layout(location = 1) in float in_alpha;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = texture(tex_sampler, in_uv) * in_alpha;
}
