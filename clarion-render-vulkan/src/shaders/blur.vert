#version 450
layout(push_constant) uniform BlurPushConstants { mat4 matrix; vec4 params; } pc;
// One affine tap footprint per strip vertex. Five varying locations carry
// 18 scalar components; the fragment stage performs no UV offset arithmetic.
layout(location=0) out vec2 tap_center;
layout(location=1) out vec4 tap_lower;
layout(location=2) out vec4 tap_upper;
layout(location=3) out vec4 tap_horizontal;
layout(location=4) out vec4 tap_vertical;
void main() {
    vec2 position = vec2(gl_VertexIndex & 1, gl_VertexIndex >> 1);
    vec2 center = vec2(position.x, pc.params.w != 0.0 ? 1.0-position.y : position.y);
    vec2 radius = pc.params.xy;
    tap_center = center;
    tap_lower = center.xyxy + vec4(-radius.x,-radius.y,radius.x,-radius.y);
    tap_upper = center.xyxy + vec4(-radius.x,radius.y,radius.x,radius.y);
    tap_horizontal = center.xyxy + vec4(-2.0*radius.x,0.0,2.0*radius.x,0.0);
    tap_vertical = center.xyxy + vec4(0.0,-2.0*radius.y,0.0,2.0*radius.y);
    gl_Position = pc.matrix * vec4(position,0.0,1.0);
}
