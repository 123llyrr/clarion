#version 100

// Uniform footprint is evaluated at four strip vertices, not for every sample
// in every fragment. Both kernels use this one linked vertex interface.
attribute vec2 vert;
uniform vec2 half_pixel;
uniform float offset;
varying highp vec2 tap_center;
varying highp vec4 tap_lower;
varying highp vec4 tap_upper;
varying highp vec4 tap_horizontal;
varying highp vec4 tap_vertical;

void main() {
    vec2 radius = half_pixel * offset;
    tap_center = vert;
    tap_lower = vert.xyxy + vec4(-radius.x, -radius.y, radius.x, -radius.y);
    tap_upper = vert.xyxy + vec4(-radius.x, radius.y, radius.x, radius.y);
    tap_horizontal = vert.xyxy + vec4(-2.0 * radius.x, 0.0, 2.0 * radius.x, 0.0);
    tap_vertical = vert.xyxy + vec4(0.0, -2.0 * radius.y, 0.0, 2.0 * radius.y);
    gl_Position = vec4(2.0 * vert - vec2(1.0), 1.0, 1.0);
}
