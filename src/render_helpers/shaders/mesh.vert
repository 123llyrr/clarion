#version 100

uniform mat3 matrix;
uniform vec2 tex_scale;

attribute vec2 position;
attribute vec2 uv;

varying vec2 clarion_v_uv;

void main() {
    clarion_v_uv = uv * tex_scale;
    gl_Position = vec4(matrix * vec3(position, 1.0), 1.0);
}
