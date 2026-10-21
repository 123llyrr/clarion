#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform float clarion_scale;

uniform vec2 geo_size;
uniform vec4 corner_radius;
uniform mat3 input_to_geo;
#define HAS_ROUNDING_POWER
uniform float rounding_power;

float clarion_rounding_alpha(vec2 coords, vec2 size, vec4 corner_radius);
vec4 postprocess(vec4 color);
vec4 clarion_sample_background(vec2 uv);

// Establish coverage before any background/glass sampling. A completely
// clipped fragment cannot contribute color, including postprocess background.
void main() {
    vec2 geometry = (input_to_geo * vec3(v_coords, 1.0)).xy;
    float coverage = 0.0;
    if (all(greaterThanEqual(geometry, vec2(0.0))) &&
            all(lessThanEqual(geometry, vec2(1.0)))) {
        coverage = clarion_rounding_alpha(geometry * geo_size, geo_size, corner_radius);
    }

    vec4 pixel = vec4(0.0);
    if (coverage > 0.0) {
        vec4 sampled = clarion_sample_background(v_coords);
#if defined(NO_ALPHA)
        sampled.a = 1.0;
#endif
        pixel = (postprocess(sampled) * coverage) * alpha;
    }
#if defined(DEBUG_FLAGS)
    // Debug tint applies even to clipped fragments, preserving the existing
    // diagnostic footprint. Do not use discard for the zero-coverage path.
    if (tint == 1.0) pixel = vec4(0.0, 0.2, 0.0, 0.2) + pixel * 0.8;
#endif
    gl_FragColor = pixel;
}
