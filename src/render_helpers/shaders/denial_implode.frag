vec4 close_color(vec3 coords_geo, vec3 size_geo) {
    vec2 uv = coords_geo.xy;
    vec2 size = size_geo.xy;
    float p = clamp(realand_clamped_progress, 0.0, 1.0);
    // Flutter Curves.easeInBack uses the classic 1.70158 back coefficient.
    float eased = 2.70158 * p * p * p - 1.70158 * p * p;
    float window_scale = max(0.02, 1.0 - 0.98 * eased);
    float rotation = 0.035 * eased;
    vec2 relative = (uv - vec2(0.5)) * size;
    float cosine = cos(-rotation);
    float sine = sin(-rotation);
    vec2 unrotated = vec2(
        relative.x * cosine - relative.y * sine,
        relative.x * sine + relative.y * cosine
    ) / window_scale;
    vec2 sample_uv = unrotated / size + vec2(0.5);

    vec4 color = vec4(0.0);
    if (sample_uv.x >= 0.0 && sample_uv.x <= 1.0 && sample_uv.y >= 0.0 && sample_uv.y <= 1.0) {
        vec3 texture_coords = realand_geo_to_tex * vec3(sample_uv, 1.0);
        color = texture2D(realand_tex, texture_coords.st);
    }
    return color * (1.0 - realand_interval(p, 0.56, 1.0));
}
// Denial-derived effect: Copyright (C) 2026 Doctor Logix.
// GPL-3.0-or-later; see resources/licenses/denial-GPL-3.0-or-later.txt.
