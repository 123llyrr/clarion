// SPDX-License-Identifier: GPL-3.0-or-later
// Rounded-distance and SDF-gradient refraction adapted from zaroutt/Niri-glass,
// commit 72ee16331994b43a9460b713b828a218a023a857 (clipped_surface.frag),
// which credits 4v3ngR/kwin-effects-glass. Clarion changes: logical-pixel bounded
// displacement, one texture sample, no animated noise or dispersion loop.
// See resources/licenses/niri-glass-GPL-3.0.txt and liquid-glass-sources.txt.
float clarion_glass_distance(vec2 p, vec2 half_size, vec4 radii) {
    float r = p.x > 0.0
        ? (p.y > 0.0 ? radii.z : radii.y)
        : (p.y > 0.0 ? radii.w : radii.x);
    vec2 q = abs(p) - half_size + vec2(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0))) - r;
}

// xy: inward displacement in logical pixels; z: stationary edge illumination.
vec3 clarion_glass_lens(vec2 coords, vec2 size, vec4 radii, float strength) {
    vec2 half_size = size * 0.5;
    vec2 p = coords * size - half_size;
    float dist = clarion_glass_distance(p, half_size, radii);
    float band = min(12.0, max(2.0, min(size.x, size.y) * 0.2));
    if (dist > 0.0 || dist < -band) return vec3(0.0);
    vec2 gradient = vec2(
        clarion_glass_distance(p + vec2(0.5, 0.0), half_size, radii)
            - clarion_glass_distance(p - vec2(0.5, 0.0), half_size, radii),
        clarion_glass_distance(p + vec2(0.0, 0.5), half_size, radii)
            - clarion_glass_distance(p - vec2(0.0, 0.5), half_size, radii));
    vec2 normal = -gradient / max(length(gradient), 0.0001);
    float bevel = clamp(1.0 + dist / band, 0.0, 1.0);
    vec2 offset = normal * min(strength, min(size.x, size.y) * 0.15)
        * bevel * bevel * (1.0 - 0.5 * bevel);
    float rim = 1.0 - smoothstep(0.0, 1.6, -dist);
    float light = (rim + 0.15 * bevel * bevel)
        * (0.3 + 0.7 * abs(dot(normal, vec2(0.70710678))));
    return vec3(offset, light);
}

// Invert only the affine 2x2 linear part (also handles output rotation/flip).
vec2 clarion_glass_uv_offset(vec2 offset, vec2 size, mat3 input_to_geo) {
    vec2 delta = offset / max(size, vec2(0.0001));
    float a = input_to_geo[0].x, b = input_to_geo[1].x;
    float c = input_to_geo[0].y, d = input_to_geo[1].y;
    float determinant = a * d - b * c;
    if (abs(determinant) < 0.000001) return vec2(0.0);
    return vec2(d * delta.x - b * delta.y, a * delta.y - c * delta.x) / determinant;
}
