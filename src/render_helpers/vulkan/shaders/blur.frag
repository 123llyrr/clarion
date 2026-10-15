#version 450
layout(push_constant) uniform BlurPushConstants { mat4 matrix; vec4 params; } pc;
layout(binding=0) uniform sampler2D tex;
layout(location=0) in vec2 uv;
layout(location=0) out vec4 out_color;
void main() {
if (pc.params.z == 0.0) {

    vec2 o = pc.params.xy;

    vec4 sum = texture(tex, uv) * 4.0;
    sum += texture(tex, uv + vec2(-o.x, -o.y));
    sum += texture(tex, uv + vec2( o.x, -o.y));
    sum += texture(tex, uv + vec2(-o.x,  o.y));
    sum += texture(tex, uv + vec2( o.x,  o.y));

    out_color = sum / 8.0;

} else {

    vec2 o = pc.params.xy;

    vec4 sum = vec4(0.0);

    // Four edge centers
    sum += texture(tex, uv + vec2(-o.x * 2.0, 0.0));
    sum += texture(tex, uv + vec2( o.x * 2.0, 0.0));
    sum += texture(tex, uv + vec2(0.0, -o.y * 2.0));
    sum += texture(tex, uv + vec2(0.0,  o.y * 2.0));

    // Four diagonal corners
    sum += texture(tex, uv + vec2(-o.x,  o.y)) * 2.0;
    sum += texture(tex, uv + vec2( o.x,  o.y)) * 2.0;
    sum += texture(tex, uv + vec2(-o.x, -o.y)) * 2.0;
    sum += texture(tex, uv + vec2( o.x, -o.y)) * 2.0;

    out_color = sum / 12.0;

}
}
