
void main() {
    vec3 coords_geo = realand_input_to_geo * vec3(realand_v_coords, 1.0);
    vec3 size_geo = vec3(realand_geo_size, 1.0);

    coords_geo.xy = realand_inverse_glide(coords_geo.xy, realand_clamped_progress);

    vec4 color = close_color(coords_geo, size_geo);

    color = color * realand_alpha;

#if defined(DEBUG_FLAGS)
    if (realand_tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
