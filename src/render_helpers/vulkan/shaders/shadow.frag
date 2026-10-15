#version 450
layout(push_constant) uniform ShadowPushConstants {
    mat4 matrix;
    vec4 color;
    vec4 corner_radius;
    vec4 window_radius;
    vec4 coords_bounds;
    vec4 geometry;
    vec4 window_geometry;
    vec4 params;
} pc;
layout(location = 0) in vec2 in_coords;
layout(location = 0) out vec4 out_color;
// Based on: https://madebyevan.com/shaders/fast-rounded-rectangle-shadows/
//
// License: CC0 (http://creativecommons.org/publicdomain/zero/1.0/)

// A standard gaussian function, used for weighting samples
float gaussian(float x, float sigma) {
  const float pi = 3.141592653589793;
  return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * pi) * sigma);
}

// This approximates the error function, needed for the gaussian integral
vec2 erf(vec2 x) {
  vec2 s = sign(x), a = abs(x);
  x = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
  x *= x;
  return s - s / (x * x);
}

// Return the blurred mask along the x dimension
float roundedBoxShadowX(float x, float y, float sigma, float corner, vec2 halfSize) {
  float delta = min(halfSize.y - corner - abs(y), 0.0);
  float curved = halfSize.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
  vec2 integral = 0.5 + 0.5 * erf((x + vec2(-curved, curved)) * (sqrt(0.5) / sigma));
  return integral.y - integral.x;
}

// Return the mask for the shadow of a box from lower to upper
float roundedBoxShadow(vec2 lower, vec2 upper, vec2 point, float sigma, float corner) {
  // Center everything to make the math easier
  vec2 center = (lower + upper) * 0.5;
  vec2 halfSize = (upper - lower) * 0.5;
  point -= center;

  // The signal is only non-zero in a limited range, so don't waste samples
  float low = point.y - halfSize.y;
  float high = point.y + halfSize.y;
  float start = clamp(-3.0 * sigma, low, high);
  float end = clamp(3.0 * sigma, low, high);

  // Accumulate samples (we can get away with surprisingly few samples)
  float step = (end - start) / 4.0;
  float y = start + step * 0.5;
  float value = 0.0;
  for (int i = 0; i < 4; i++) {
    value += roundedBoxShadowX(point.x, point.y - y, sigma, corner, halfSize) * gaussian(y, sigma) * step;
    y += step;
  }

  return value;
}

float realand_rounding_alpha(vec2 coords, vec2 size, vec4 corner_radius);

void main() {
    vec3 coords_geo = vec3(in_coords - pc.geometry.xy, 1.0);
    vec3 coords_window_geo = vec3(in_coords - pc.window_geometry.xy, 1.0);

    vec4 color = pc.color;

    float shadow_value;
    if (pc.params.x < 0.1) {
        // With low enough sigma just draw a rounded rectangle.
        // Include straight-edge antialiasing as well as the rounded corners.
        vec2 edge = min(coords_geo.xy, pc.geometry.zw - coords_geo.xy);
        float coverage = clamp(min(edge.x, edge.y) * pc.params.y + 0.5, 0.0, 1.0);
        shadow_value = coverage * realand_rounding_alpha(coords_geo.xy, pc.geometry.zw, pc.corner_radius);
    } else {
        shadow_value = roundedBoxShadow(
            vec2(0.0, 0.0),
            pc.geometry.zw,
            coords_geo.xy,
            pc.params.x,
            // FIXME: figure out how to blur with different corner radii.
            //
            // GTK seems to call blurring separately for the rect and for the 4 corners:
            // https://gitlab.gnome.org/GNOME/gtk/-/blob/gtk-4-16/gsk/gpu/shaders/gskgpuboxshadow.glsl
            pc.corner_radius.x
        );
    }
    color = color * shadow_value;

    // Cut out the inside of the window geometry if requested.
    if (pc.window_geometry.zw != vec2(0.0, 0.0)) {
        vec2 edge = min(coords_window_geo.xy, pc.window_geometry.zw - coords_window_geo.xy);
        float coverage = clamp(min(edge.x, edge.y) * pc.params.y + 0.5, 0.0, 1.0);
        float alpha = coverage * realand_rounding_alpha(coords_window_geo.xy, pc.window_geometry.zw, pc.window_radius);
        color = color * (1.0 - alpha);
    }

    color = color * pc.params.z;

#if defined(DEBUG_FLAGS)
    if (realand_tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    out_color = color;
}

float realand_rounding_alpha(vec2 coords, vec2 size, vec4 corner_radius) {
    vec2 center;
    float radius;

    if (coords.x < corner_radius.x && coords.y < corner_radius.x) {
        radius = corner_radius.x;
        center = vec2(radius, radius);
    } else if (size.x - corner_radius.y < coords.x && coords.y < corner_radius.y) {
        radius = corner_radius.y;
        center = vec2(size.x - radius, radius);
    } else if (size.x - corner_radius.z < coords.x && size.y - corner_radius.z < coords.y) {
        radius = corner_radius.z;
        center = vec2(size.x - radius, size.y - radius);
    } else if (coords.x < corner_radius.w && size.y - corner_radius.w < coords.y) {
        radius = corner_radius.w;
        center = vec2(radius, size.y - radius);
    } else {
        return 1.0;
    }

    float dist = distance(coords, center);

    // Manual smoothstep() between radius - half_px and radius + half_px
    // to avoid a division in clamp().
    float t = clamp((dist - radius) * pc.params.y + 0.5, 0.0, 1.0);
    return 1.0 - t * t * (3.0 - 2.0 * t);
}
