#version 450
layout(push_constant) uniform BlurPushConstants { mat4 matrix; vec4 params; } pc;
layout(binding=0) uniform sampler2D tex;
layout(location=0) in vec2 tap_center;
layout(location=1) in vec4 tap_lower;
layout(location=2) in vec4 tap_upper;
layout(location=3) in vec4 tap_horizontal;
layout(location=4) in vec4 tap_vertical;
layout(location=0) out vec4 out_color;
void main() {
    if (pc.params.z == 0.0) {
        // Center mass1/2, diagonal masses1/8 each. This is a probability
        // measure, independent of the source's alpha or color representation.
        vec4 diagonals = texture(tex,tap_lower.xy) + texture(tex,tap_lower.zw);
        diagonals += texture(tex,tap_upper.xy) + texture(tex,tap_upper.zw);
        out_color = texture(tex,tap_center)*0.5 + diagonals*0.125;
    } else {
        // Axis masses1/12, diagonal masses1/6. Preserve all RGBA channels.
        vec4 axes = texture(tex,tap_horizontal.xy) + texture(tex,tap_horizontal.zw);
        axes += texture(tex,tap_vertical.xy) + texture(tex,tap_vertical.zw);
        vec4 diagonals = texture(tex,tap_upper.xy) + texture(tex,tap_upper.zw);
        diagonals += texture(tex,tap_lower.xy) + texture(tex,tap_lower.zw);
        out_color = axes*(1.0/12.0) + diagonals*(1.0/6.0);
    }
}
