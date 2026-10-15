#version 100

uniform mat3 matrix;

attribute vec2 position;
attribute vec2 local;
attribute vec4 color;
attribute float shape;
attribute float inv_width;
attribute float aspect;

varying vec2 realand_v_local;
varying vec4 realand_v_color;
varying float realand_v_shape;
varying float realand_v_inv_width;
varying float realand_v_aspect;

void main() {
    realand_v_local = local;
    realand_v_color = color;
    realand_v_shape = shape;
    realand_v_inv_width = inv_width;
    realand_v_aspect = aspect;
    gl_Position = vec4(matrix * vec3(position, 1.0), 1.0);
}
