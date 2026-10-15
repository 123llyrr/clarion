vec4 open_color(vec3 coords_geo, vec3 size_geo) {
    vec2 uv = coords_geo.xy;
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec4(0.0);
    }

    // Denial's four reveal origins are Flutter alignments (+/-0.75), which resolve to
    // 12.5% and 87.5% of the child bounds.
    float quadrant = floor(clamp(realand_random_seed, 0.0, 0.999999) * 4.0);
    vec2 origin = vec2(0.125);
    if (quadrant >= 1.0 && quadrant < 3.0) {
        origin.x = 0.875;
    }
    if (quadrant >= 2.0) {
        origin.y = 0.875;
    }

    // Match DesktopWindowSquircleRevealClipper: exponent 4, one logical pixel of
    // corner coverage, and 2^(1/4) expansion so progress 1 covers the farthest corner.
    float reveal = max(clamp(realand_clamped_progress, 0.0, 1.0), 0.001);
    float corner_coverage = 1.189207115002721;
    vec2 farthest = max(origin, vec2(1.0) - origin) * size_geo.xy;
    vec2 half_extent = (farthest + vec2(1.0)) * corner_coverage * reveal;
    vec2 delta = abs((uv - origin) * size_geo.xy) / max(half_extent, vec2(0.0001));
    vec2 squared = delta * delta;
    float superellipse = dot(squared, squared);

    // Flutter uses Clip.antiAlias. Approximate its one-device-pixel coverage without relying on
    // derivative extensions, which keeps this shader valid on GLES2 renderers.
    float edge = min(
        1.0,
        4.0 * max(1.0 / max(half_extent.x * realand_scale, 1.0),
                  1.0 / max(half_extent.y * realand_scale, 1.0))
    );
    float coverage = 1.0 - smoothstep(1.0 - edge, 1.0 + edge, superellipse);

    vec3 coords_tex = realand_geo_to_tex * vec3(uv, 1.0);
    return texture2D(realand_tex, coords_tex.st) * coverage;
}
// Denial-derived reveal: Copyright (C) 2026 Doctor Logix.
// GPL-3.0-or-later; see resources/licenses/denial-GPL-3.0-or-later.txt.
