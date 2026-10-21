// Clarion's rounded geometry kernel. All inputs are in logical pixels.
// A rounded box is the intersection of four quarter-disc half spaces and
// the rectangle. Tangent lines extend each quarter disc to the other edges,
// including asymmetric radii which cross the middle of the rectangle.
float clarion_corner_distance(vec2 q, float radius, float power) {
    vec2 v = max(q, vec2(0.0));
    float norm;
    if (power <= 2.0) {
        norm = length(v);
    } else {
        // Divide before exponentiation so large radii or squircle powers
        // cannot overflow pow(pixel_distance, power).
        float largest = max(v.x, v.y);
        vec2 unit = v / max(largest, 0.000001);
        norm = largest * pow(pow(unit.x, power) + pow(unit.y, power), 1.0 / power);
    }
    return norm + min(max(q.x, q.y), 0.0) - radius;
}

float clarion_box_distance(vec2 p, vec2 size, vec4 radii, float power) {
    vec4 r = max(radii, vec4(0.0));
    vec2 middle = size * 0.5;
    if (all(equal(r, vec4(r.x)))) {
        return clarion_corner_distance(abs(p - middle) - middle + r.x, r.x, power);
    }
    vec2 opposite = size - p;
    float top = max(
        clarion_corner_distance(vec2(r.x) - p, r.x, power),
        clarion_corner_distance(vec2(r.y) - vec2(opposite.x, p.y), r.y, power));
    float bottom = max(
        clarion_corner_distance(vec2(r.z) - opposite, r.z, power),
        clarion_corner_distance(vec2(r.w) - vec2(p.x, opposite.y), r.w, power));
    return max(top, bottom);
}

float clarion_shape_alpha(vec2 p, vec2 size, vec4 radii, float scale, float power) {
    if (any(lessThanEqual(size, vec2(0.0)))) return 0.0;
    float distance_pixels = clarion_box_distance(p, size, radii, max(power, 2.0)) * scale;
    return 1.0 - smoothstep(-0.5, 0.5, distance_pixels);
}

#ifndef CLARION_MASK_SCALE
#define CLARION_MASK_SCALE clarion_scale
#endif
#ifndef CLARION_MASK_POWER
#ifdef HAS_ROUNDING_POWER
#define CLARION_MASK_POWER rounding_power
#else
#define CLARION_MASK_POWER 2.0
#endif
#endif

// Retain the public custom-shader hook.
float clarion_rounding_alpha(vec2 coords, vec2 size, vec4 corner_radius) {
    return clarion_shape_alpha(coords, size, corner_radius, CLARION_MASK_SCALE, CLARION_MASK_POWER);
}
