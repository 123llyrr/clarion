
void main() {
    vec3 coords_geo = clarion_input_to_geo * vec3(clarion_v_coords, 1.0);
    vec3 size_geo = vec3(clarion_geo_size, 1.0);

    coords_geo.xy = clarion_inverse_glide(coords_geo.xy, 1.0 - clarion_clamped_progress);
    float popin_scale = mix(clarion_popin_ratio, 1.0, clarion_clamped_progress);
    coords_geo.xy = (coords_geo.xy - vec2(0.5)) / max(popin_scale, 0.001) + vec2(0.5);

    vec4 color = open_color(coords_geo, size_geo);

    color = color * clarion_alpha;

#if defined(DEBUG_FLAGS)
    if (clarion_tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
