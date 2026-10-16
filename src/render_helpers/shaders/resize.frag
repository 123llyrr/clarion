vec4 resize_sample(sampler2D tex, vec2 coords) {
    // The combined draw bounds can extend beyond either input (for example,
    // when the next window has a larger client-side shadow). Do not stretch an
    // input's edge pixels across the other input's extra bounds.
    if (any(lessThan(coords, vec2(0.0))) || any(greaterThan(coords, vec2(1.0)))) {
        return vec4(0.0);
    }
    return texture2D(tex, coords);
}

vec4 resize_color(vec3 coords_curr_geo, vec3 size_curr_geo) {
    vec3 coords_tex_prev = clarion_geo_to_tex_prev * coords_curr_geo;
    vec4 color_prev = resize_sample(clarion_tex_prev, coords_tex_prev.st);

    vec3 coords_tex_next = clarion_geo_to_tex_next * coords_curr_geo;
    vec4 color_next = resize_sample(clarion_tex_next, coords_tex_next.st);

    vec4 color = mix(color_prev, color_next, clarion_clamped_progress);
    return color;
}
