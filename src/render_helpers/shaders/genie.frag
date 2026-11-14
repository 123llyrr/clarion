// Inverse-map a window sheet flowing through a fixed funnel, with +Y toward
// the Dock. Side-rail Bezier control points follow usagimaru/GenieWarpMesh
// (MIT): https://github.com/usagimaru/GenieWarpMesh/tree/5a6f2a4460dde2c487b0ac4138aa479732ee8d90
// The inverse mapping, intake clipping and four-edge adaptation are local.
// See genie.LICENSE for the source notice.
precision highp float;

varying vec2 clarion_v_coords;

uniform vec2 clarion_size;
uniform float clarion_scale;
uniform float clarion_alpha;
uniform sampler2D clarion_tex;

uniform vec4 window_rect;
uniform vec4 target_rect;
uniform vec4 texture_rect;
// Allocated texture pixels are mapped onto the used prefix of the offscreen
// canvas; a retained allocation may be larger than the current window.
uniform vec2 texture_scale;
uniform vec2 area_origin;
uniform float edge;
uniform float morph;

#if defined(DEBUG_FLAGS)
uniform float clarion_tint;
#endif

vec2 to_axis(vec2 p) {
    if (edge < 0.5) return p;
    if (edge < 1.5) return vec2(p.y, -p.x);
    if (edge < 2.5) return p.yx;
    return vec2(p.x, -p.y);
}

vec2 from_axis(vec2 p) {
    if (edge < 0.5) return p;
    if (edge < 1.5) return vec2(-p.y, p.x);
    if (edge < 2.5) return p.yx;
    return vec2(p.x, -p.y);
}

vec4 axis_rect(vec4 r) {
    vec2 a = to_axis(r.xy);
    vec2 b = to_axis(r.xy + r.zw);
    return vec4(min(a, b), abs(b - a));
}

// Invert the monotone axial component of a cubic Bezier whose side-rail
// controls lie at 45% and 65% of the window-to-mouth distance. Its transverse
// controls stay on the respective endpoints, giving vertical tangents at both
// ends instead of a kink where the neck meets the Dock. The derivative is
// bounded away from zero, so four Newton steps converge without a texture LUT.
float funnel_pull(float distance) {
    float u = clamp(distance, 0.0, 1.0);
    for (int i = 0; i < 4; i++) {
        float y = u * (1.35 + u * (-0.75 + 0.4 * u));
        float dy = 1.35 + u * (-1.5 + 1.2 * u);
        u = clamp(u - (y - distance) / dy, 0.0, 1.0);
    }
    return u * u * (3.0 - 2.0 * u);
}

void main() {
    vec4 w = axis_rect(window_rect);
    vec4 t = axis_rect(target_rect);
    // clarion_size is the physical render quad; all animation rectangles are logical.
    vec2 point = to_axis(area_origin + clarion_v_coords * (clarion_size / clarion_scale));

    float lead = smoothstep(0.0, 0.55, morph);
    // Form the neck before drawing the trailing edge away from the window.
    float gather = smoothstep(0.0, 0.4, morph);
    float lag = smoothstep(0.38, 1.0, morph);
    float mouth = t.y + t.w * 0.5;
    float far_edge = mix(w.y, mouth, lag);
    // Once the leading edge reaches the icon, keep feeding the sheet through
    // the mouth. Clipping there absorbs rows instead of squashing all content
    // into a persistent icon-sized rectangle at the end of the transition.
    float feed = smoothstep(0.55, 1.0, morph);
    float near_edge = mix(w.y + w.w, mouth, lead) + w.w * feed;
    float length = max(near_edge - far_edge, 0.001);
    float row = (point.y - far_edge) / length;

    // Evaluate the side rails in OUTPUT space, not the moving texture's row.
    // They stop moving once the neck is formed; content slides through them.
    float distance = clamp((point.y - w.y) / max(mouth - w.y, 0.001), 0.0, 1.0);
    float pull = funnel_pull(distance) * gather;
    float width = mix(w.z, t.z, pull);
    float center = mix(w.x + w.z * 0.5, t.x + t.z * 0.5, pull);
    float column = (point.x - center) / max(width, 0.001) + 0.5;

    // Let shadow/border padding outside the window geometry follow the sheet too.
    vec2 source = from_axis(w.xy + vec2(column, row) * w.zw);
    vec2 uv = (source - window_rect.xy) / window_rect.zw;
    uv = (uv - texture_rect.xy) / texture_rect.zw;
    vec2 extent = (edge < 0.5 || edge > 2.5)
        ? vec2(width, length) : vec2(length, width);
    vec2 coverage = clamp(min(uv, 1.0 - uv) * extent * texture_rect.zw * clarion_scale + 0.5, 0.0, 1.0);
    // Fullscreen windows and their shadows may initially overlap the icon.
    // Start the intake plane outside their texture so frame zero stays exact.
    vec4 texture_bounds = axis_rect(vec4(window_rect.xy + texture_rect.xy * window_rect.zw,
                                        texture_rect.zw * window_rect.zw));
    float intake = mix(max(mouth, texture_bounds.y + texture_bounds.w), mouth, lead);
    float intake_coverage = clamp((intake - point.y) * clarion_scale + 0.5, 0.0, 1.0);
    vec2 tex_uv = clamp(uv * texture_scale, vec2(0.0), texture_scale);
    vec4 color = texture2D(clarion_tex, tex_uv)
        * (coverage.x * coverage.y * intake_coverage * clarion_alpha);
#if defined(DEBUG_FLAGS)
    if (clarion_tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
