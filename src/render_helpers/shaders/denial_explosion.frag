// Window dissolution for the clean shard effect. Foreground fragments are batched separately.

vec4 close_color(vec3 coords_geo, vec3 size_geo) {
    vec2 uv = coords_geo.xy;
    vec2 size = size_geo.xy;
    vec2 relative = (uv - vec2(0.5)) * size;
    float p = clamp(clarion_clamped_progress, 0.0, 1.0);

    // Only fragments remain after the window dissolves. Never paint a radial flash or ring.
    if (p >= 0.38) {
        return vec4(0.0);
    }

    // The child transform uses Denial's emphasized-accelerate curve only during 10%-38%.
    float vanish = clarion_interval(p, 0.10, 0.38);
    float eased = clarion_cubic_bezier(vanish, vec4(0.3, 0.0, 0.8, 0.15));
    float punch = sin(clarion_interval(p, 0.0, 0.13) * 3.141592653589793) * 0.025;
    float window_scale = 1.0 + punch - 0.18 * eased;
    float rotation = -0.012 * eased;

    float cosine = cos(-rotation);
    float sine = sin(-rotation);
    vec2 unrotated = vec2(
        relative.x * cosine - relative.y * sine,
        relative.x * sine + relative.y * cosine
    ) / max(window_scale, 0.001);
    vec2 sample_uv = unrotated / size + vec2(0.5);

    vec4 body = vec4(0.0);
    if (sample_uv.x >= 0.0 && sample_uv.x <= 1.0
            && sample_uv.y >= 0.0 && sample_uv.y <= 1.0) {
        vec3 texture_coords = clarion_geo_to_tex * vec3(sample_uv, 1.0);
        body = texture2D(clarion_tex, texture_coords.st) * (1.0 - eased);
    }
    return body;
}
// Denial-derived effect: Copyright (C) 2026 Doctor Logix.
// GPL-3.0-or-later; see resources/licenses/denial-GPL-3.0-or-later.txt.
