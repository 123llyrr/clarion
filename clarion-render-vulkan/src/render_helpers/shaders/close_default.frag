vec4 close_color(vec3 coords_geo, vec3 size_geo) {
    vec2 uv = coords_geo.xy;
    vec2 size = size_geo.xy;
    float p = clamp(clarion_clamped_progress, 0.0, 1.0);
    float eased = clarion_cubic_bezier(p, vec4(0.3, 0.0, 0.8, 0.15));
    float window_scale = 1.0 - 0.045 * eased;
    vec2 sample_uv = (uv - vec2(0.5)) / window_scale + vec2(0.5);
    if (sample_uv.x < 0.0 || sample_uv.x > 1.0
            || sample_uv.y < 0.0 || sample_uv.y > 1.0) {
        return vec4(0.0);
    }
    vec3 coords_tex = clarion_geo_to_tex * vec3(sample_uv, 1.0);
    return texture2D(clarion_tex, coords_tex.st) * (1.0 - eased);
}
