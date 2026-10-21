precision highp float;
uniform float clarion_alpha;
uniform float clarion_scale;
uniform vec2 clarion_size;
varying vec2 clarion_v_coords;
#if defined(DEBUG_FLAGS)
uniform float clarion_tint;
#endif
uniform float colorspace;
uniform float hue_interpolation;
uniform vec4 color_from;
uniform vec4 color_to;
uniform vec2 grad_offset;
uniform float grad_width;
uniform vec2 grad_vec;
uniform mat3 input_to_geo;
uniform vec2 geo_size;
uniform vec4 outer_radius;
uniform float border_width;
uniform float rounding_power;
#define HAS_ROUNDING_POWER

float clarion_rounding_alpha(vec2 p, vec2 size, vec4 radius);
vec4 clarion_gradient_mix(vec4 a, vec4 b, float t, float space, float hue);
float clarion_gradient_position(vec2 p, vec2 origin, vec2 axis, float width);

void main() {
    vec2 p = (input_to_geo * vec3(clarion_v_coords, 1.0)).xy;
    float t = clarion_gradient_position(p, grad_offset, grad_vec, grad_width);
    float mask = clarion_rounding_alpha(p, geo_size, outer_radius);
    if (border_width > 0.0) {
        float hole = clarion_rounding_alpha(p - border_width,
            geo_size - 2.0 * border_width, max(outer_radius - border_width, 0.0));
        mask *= 1.0 - hole;
    }
    vec4 pixel = clarion_gradient_mix(color_from, color_to, t, colorspace, hue_interpolation) * (mask * clarion_alpha);
#if defined(DEBUG_FLAGS)
    pixel = mix(pixel, vec4(0.0, 0.2, 0.0, 0.2) + 0.8 * pixel, step(0.5, clarion_tint));
#endif
    gl_FragColor = pixel;
}
