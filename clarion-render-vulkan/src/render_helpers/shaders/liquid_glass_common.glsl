// SPDX-License-Identifier: GPL-3.0-or-later
// Historical migration record, retained pending the full source review:
// the previous distance/finite-difference implementation was adapted from
// zaroutt/Niri-glass 72ee16331994b43a9460b713b828a218a023a857,
// which credits 4v3ngR/kwin-effects-glass. Its notices remain in
// resources/licenses/niri-glass-GPL-3.0.txt and liquid-glass-sources.txt.
//
// Clarion analytic bevel lens. Geometry is in logical pixels; no finite
// differences or neighbouring distance evaluations are required.
// Historical origin notices remain during the repository-wide migration.
vec3 clarion_glass_corner(vec3 nearest, vec2 edge, float radius, vec2 direction) {
    vec2 q = vec2(max(radius, 0.0)) - edge;
    if (min(q.x, q.y) <= 0.0) return nearest;
    float magnitude = length(q);
    float distance_to_arc = magnitude - radius;
    if (distance_to_arc <= nearest.z) return nearest;
    return vec3(direction * q / max(magnitude, 0.000001), distance_to_arc);
}

// xy is the outward unit normal, z the signed distance to the silhouette.
vec3 clarion_glass_boundary(vec2 point, vec2 size, vec4 radius) {
    vec2 edges = max(-point, point - size);
    vec3 nearest = edges.x > edges.y
        ? vec3(point.x < size.x * 0.5 ? -1.0 : 1.0, 0.0, edges.x)
        : vec3(0.0, point.y < size.y * 0.5 ? -1.0 : 1.0, edges.y);
    nearest = clarion_glass_corner(nearest, point, radius.x, vec2(-1.0,-1.0));
    nearest = clarion_glass_corner(nearest, vec2(size.x-point.x,point.y), radius.y, vec2(1.0,-1.0));
    nearest = clarion_glass_corner(nearest, size-point, radius.z, vec2(1.0,1.0));
    return clarion_glass_corner(nearest, vec2(point.x,size.y-point.y), radius.w, vec2(-1.0,1.0));
}

// Public shader hook: inward logical-pixel displacement and stationary light.
vec3 clarion_glass_lens(vec2 coords, vec2 size, vec4 radii, float strength) {
    if (strength <= 0.0 || any(lessThanEqual(size, vec2(0.0)))) return vec3(0.0);
    vec3 boundary = clarion_glass_boundary(coords * size, size, radii);
    float band = clamp(min(size.x,size.y) * 0.2, 2.0, 12.0);
    if (boundary.z > 0.0 || boundary.z <= -band) return vec3(0.0);
    float depth = clamp(-boundary.z / band, 0.0, 1.0);
    // Compact, C1 profile: refraction joins the unmodified interior without
    // a derivative discontinuity; the edge carries stationary illumination.
    float bevel = 1.0 - depth;
    float profile = bevel * bevel * (3.0 - 2.0 * bevel);
    float amplitude = min(strength, 0.15 * min(size.x,size.y));
    vec2 displacement = -boundary.xy * (amplitude * profile * 0.5);
    float rim = 1.0 - smoothstep(0.0, min(1.6,band), -boundary.z);
    float lighting = (rim + 0.15 * profile) * (0.3 + 0.7 * abs(dot(boundary.xy,vec2(0.7071067812))));
    return vec3(displacement,lighting);
}

// Solve the affine coordinate differential with perpendicular columns. The
// Jacobian is normalised before inversion, avoiding tiny-but-invertible
// scales being mistaken for a singular transform by an absolute determinant.
vec2 clarion_glass_uv_offset(vec2 offset, vec2 size, mat3 input_to_geo) {
    vec2 a = input_to_geo[0].xy;
    vec2 b = input_to_geo[1].xy;
    float magnitude = max(max(abs(a.x),abs(a.y)),max(abs(b.x),abs(b.y)));
    if (magnitude <= 0.0 || any(lessThanEqual(size,vec2(0.0)))) return vec2(0.0);
    a /= magnitude;
    b /= magnitude;
    float determinant = a.x*b.y-a.y*b.x;
    if (abs(determinant) < 0.0000001) return vec2(0.0);
    vec2 rhs = offset / size / magnitude;
    return vec2(dot(rhs,vec2(b.y,-b.x)),dot(rhs,vec2(-a.y,a.x))) / determinant;
}
