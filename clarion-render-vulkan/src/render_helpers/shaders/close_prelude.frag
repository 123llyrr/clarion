precision highp float;

#if defined(DEBUG_FLAGS)
uniform float clarion_tint;
#endif

varying vec2 clarion_v_coords;
uniform vec2 clarion_size;

uniform mat3 clarion_input_to_geo;
uniform vec2 clarion_geo_size;

uniform sampler2D clarion_tex;
uniform mat3 clarion_geo_to_tex;

uniform float clarion_progress;
uniform float clarion_clamped_progress;
uniform float clarion_random_seed;

uniform float clarion_glide_enabled;
uniform float clarion_glide_tilt;
uniform float clarion_glide_depth;

uniform float clarion_alpha;
uniform float clarion_scale;

float clarion_interval(float value, float begin, float end) {
    return clamp((value - begin) / (end - begin), 0.0, 1.0);
}

float clarion_bezier_component(float t, float first, float second) {
    float inv = 1.0 - t;
    return 3.0 * inv * inv * t * first + 3.0 * inv * t * t * second + t * t * t;
}

float clarion_bezier_derivative(float t, float first, float second) {
    float inv = 1.0 - t;
    return 3.0 * inv * inv * first
         + 6.0 * inv * t * (second - first)
         + 3.0 * t * t * (1.0 - second);
}

// Flutter Cubic transforms time by solving the x component before evaluating y.
float clarion_cubic_bezier(float x, vec4 control) {
    x = clamp(x, 0.0, 1.0);
    float t = x;
    for (int iteration = 0; iteration < 6; iteration++) {
        float error = clarion_bezier_component(t, control.x, control.z) - x;
        float slope = clarion_bezier_derivative(t, control.x, control.z);
        if (abs(slope) > 0.0001) {
            t = clamp(t - error / slope, 0.0, 1.0);
        }
    }
    return clarion_bezier_component(t, control.y, control.w);
}

vec2 clarion_inverse_glide(vec2 output_uv, float amount) {
    if (clarion_glide_enabled < 0.5 || amount <= 0.0) {
        return output_uv;
    }

    float angle = -clarion_glide_tilt * amount;
    float sine = sin(angle);
    float cosine = max(0.05, cos(angle));
    float depth = clarion_glide_depth * amount;
    float screen_y = output_uv.y - 0.5;
    float divisor = cosine - screen_y * sine;
    if (abs(divisor) < 0.02) {
        divisor = divisor < 0.0 ? -0.02 : 0.02;
    }

    float plane_y = screen_y * (1.0 + depth) / divisor;
    float perspective = 1.0 + depth + plane_y * sine;
    float plane_x = (output_uv.x - 0.5) * perspective;
    return vec2(plane_x + 0.5, plane_y + 0.5);
}
