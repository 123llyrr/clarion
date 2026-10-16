#version 100

uniform mat3 matrix;

attribute vec2 position;
attribute vec2 local;
attribute vec4 color;
attribute float shape;
attribute float inv_width;
attribute float aspect;

varying vec2 clarion_v_local;
varying vec4 clarion_v_color;
varying float clarion_v_shape;
varying float clarion_v_inv_width;
varying float clarion_v_aspect;

void main() {
    clarion_v_local = local;
    clarion_v_color = color;
    clarion_v_shape = shape;
    clarion_v_inv_width = inv_width;
    clarion_v_aspect = aspect;
    gl_Position = vec4(matrix * vec3(position, 1.0), 1.0);
}
