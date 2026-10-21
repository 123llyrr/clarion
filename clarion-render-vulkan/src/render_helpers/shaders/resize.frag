// Source domains are closed unit squares. NaNs fail the admitted comparisons
// and never reach an implementation-dependent texture lookup.
vec4 resize_sample(sampler2D image, vec2 point) {
    bool admitted = all(greaterThanEqual(point, vec2(0.0)))
                 && all(lessThanEqual(point, vec2(1.0)));
    if (!admitted) {
        return vec4(0.0);
    }
    return texture2D(image, point);
}

vec4 resize_color(vec3 geometry_point, vec3 geometry_size) {
    // Exact endpoints have one live source. Preserve extrapolation for callers
    // outside [0,1]; this selector changes only the two exact endpoint cases.
    if (clarion_clamped_progress == 0.0) {
        return resize_sample(clarion_tex_prev,
                             (clarion_geo_to_tex_prev * geometry_point).xy);
    }
    if (clarion_clamped_progress == 1.0) {
        return resize_sample(clarion_tex_next,
                             (clarion_geo_to_tex_next * geometry_point).xy);
    }
    vec4 previous = resize_sample(clarion_tex_prev,
                                 (clarion_geo_to_tex_prev * geometry_point).xy);
    vec4 next = resize_sample(clarion_tex_next,
                             (clarion_geo_to_tex_next * geometry_point).xy);
    return mix(previous, next, clarion_clamped_progress);
}
