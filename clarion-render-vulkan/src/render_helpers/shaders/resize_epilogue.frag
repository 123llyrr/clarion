
void main() {
    vec3 coords_curr_geo = clarion_input_to_curr_geo * vec3(clarion_v_coords, 1.0);
    vec3 size_curr_geo = vec3(clarion_curr_geo_size, 1.0);
    bool clipped = clarion_clip_to_geometry == 1.0;
    bool admitted = !clipped
                 || (all(greaterThanEqual(coords_curr_geo.xy, vec2(0.0)))
                  && all(lessThanEqual(coords_curr_geo.xy, vec2(1.0))));
    vec4 color = vec4(0.0);
    // Rejected fragments never invoke the user/default color function. Keep
    // alpha/debug composition below this branch: debug tint covers rejected
    // fragments as it did before, even when alpha is zero.
    if (admitted) {
        color = resize_color(coords_curr_geo, size_curr_geo);
        if (clipped) {
            color *= clarion_rounding_alpha(coords_curr_geo.xy * size_curr_geo.xy,
                                           size_curr_geo.xy, clarion_corner_radius);
        }
    }
    color *= clarion_alpha;
#if defined(DEBUG_FLAGS)
    if (clarion_tint == 1.0) {
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
    }
#endif
    gl_FragColor = color;
}
