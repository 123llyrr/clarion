#version 450
layout(push_constant) uniform Push { mat4 matrix; vec4 uv; } pc;
layout(location=0) out vec2 realand_v_coords;
void main() {
    vec2 p = vec2(gl_VertexIndex & 1, gl_VertexIndex >> 1);
    realand_v_coords = mix(pc.uv.xy, pc.uv.zw, p);
    gl_Position = pc.matrix * vec4(p, 0, 1);
}
