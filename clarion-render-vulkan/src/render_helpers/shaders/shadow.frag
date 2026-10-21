precision highp float;
uniform float clarion_alpha;
uniform float clarion_scale;
uniform vec2 clarion_size;
varying vec2 clarion_v_coords;
#if defined(DEBUG_FLAGS)
uniform float clarion_tint;
#endif
uniform vec4 shadow_color;
uniform float sigma;
uniform mat3 input_to_geo;
uniform vec2 geo_size;
uniform vec4 corner_radius;
uniform mat3 window_input_to_geo;
uniform vec2 window_geo_size;
uniform vec4 window_corner_radius;
float clarion_rounding_alpha(vec2 p, vec2 size, vec4 radii);
float clarion_shadow_mass(vec2 p, vec2 size, vec4 radii, float blur);

void main() {
    vec2 p = (input_to_geo * vec3(clarion_v_coords, 1.0)).xy;
    float mask = sigma < 0.1 ? clarion_rounding_alpha(p, geo_size, corner_radius)
        : clarion_shadow_mass(p, geo_size, corner_radius, sigma);
    if (all(greaterThan(window_geo_size, vec2(0.0)))) {
        vec2 hole_p = (window_input_to_geo * vec3(clarion_v_coords, 1.0)).xy;
        mask *= 1.0 - clarion_rounding_alpha(hole_p, window_geo_size, window_corner_radius);
    }
    vec4 pixel = shadow_color * (mask * clarion_alpha);
#if defined(DEBUG_FLAGS)
    pixel = mix(pixel, vec4(0.0, 0.2, 0.0, 0.2) + 0.8 * pixel, step(0.5, clarion_tint));
#endif
    gl_FragColor = pixel;
}
