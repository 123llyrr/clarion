#version 450

layout(location = 0) in vec2 in_local;
layout(location = 1) in vec4 in_color;
layout(location = 2) in float in_shape;
layout(location = 3) in float in_inv_width;
layout(location = 4) in float in_aspect;

layout(location = 0) out vec4 out_color;

void main() {
    float distance_to_edge;
    if (in_shape > 0.5) {
        distance_to_edge = length(in_local) - 0.45;
    } else {
        const float radius = 0.18;
        vec2 box = vec2(0.5 - radius, in_aspect * 0.5 - radius);
        vec2 q = abs(in_local) - box;
        distance_to_edge = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    }

    float aa = max(in_inv_width, 0.0001);
    float coverage = 1.0 - smoothstep(-aa, aa, distance_to_edge);
    out_color = in_color * coverage;
}
