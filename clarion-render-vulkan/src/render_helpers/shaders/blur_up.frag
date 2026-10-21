#version 100
precision highp float;
uniform sampler2D tex;
varying highp vec4 tap_lower;
varying highp vec4 tap_upper;
varying highp vec4 tap_horizontal;
varying highp vec4 tap_vertical;

// Axis ring carries 1/3 of total mass, the diagonal ring the other 2/3.
// Grouping by ring leaves no per-fragment coordinate construction.
void main() {
    vec4 axes = texture2D(tex, tap_horizontal.xy) + texture2D(tex, tap_horizontal.zw);
    axes += texture2D(tex, tap_vertical.xy) + texture2D(tex, tap_vertical.zw);
    vec4 corners = texture2D(tex, tap_upper.xy) + texture2D(tex, tap_upper.zw);
    corners += texture2D(tex, tap_lower.xy) + texture2D(tex, tap_lower.zw);
    gl_FragColor = axes * (1.0 / 12.0) + corners * (1.0 / 6.0);
}
