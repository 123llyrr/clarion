#version 100
precision highp float;
uniform sampler2D tex;
varying highp vec2 tap_center;
varying highp vec4 tap_lower;
varying highp vec4 tap_upper;

// The reconstruction kernel is a normalized center mass (1/2) and a square
// ring (four masses of 1/8). Coordinates arrive from the shared vertex stage.
void main() {
    vec4 ring = texture2D(tex, tap_lower.xy) + texture2D(tex, tap_lower.zw);
    ring += texture2D(tex, tap_upper.xy) + texture2D(tex, tap_upper.zw);
    gl_FragColor = texture2D(tex, tap_center) * 0.5 + ring * 0.125;
}
