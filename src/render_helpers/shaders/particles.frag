#version 100

precision mediump float;

varying vec2 clarion_v_local;
varying vec4 clarion_v_color;
varying float clarion_v_shape;
varying float clarion_v_inv_width;
varying float clarion_v_aspect;

void main() {
    float distance_to_edge;
    if (clarion_v_shape > 0.5) {
        // Denial's circular particles use radius = width * 0.45.
        distance_to_edge = length(clarion_v_local) - 0.45;
    } else {
        // Coordinates are expressed in particle-width units. This exactly keeps the original
        // radius = width * 0.18 for every aspect ratio.
        const float radius = 0.18;
        vec2 box = vec2(0.5 - radius, clarion_v_aspect * 0.5 - radius);
        vec2 q = abs(clarion_v_local) - box;
        distance_to_edge = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    }

    float aa = max(clarion_v_inv_width, 0.0001);
    float coverage = 1.0 - smoothstep(-aa, aa, distance_to_edge);
    gl_FragColor = clarion_v_color * coverage;
}
