uniform vec2 liquid_glass;
vec4 realand_sample_background(vec2 uv) {
    if (liquid_glass.x <= 0.0) return texture2D(tex, uv);
    vec2 coords = (input_to_geo * vec3(uv, 1.0)).xy;
    vec3 lens = realand_glass_lens(coords, geo_size, corner_radius, liquid_glass.x);
    vec2 offset = realand_glass_uv_offset(lens.xy, geo_size, input_to_geo);
    vec4 color = texture2D(tex, clamp(uv + offset, vec2(0.0), vec2(1.0)));
    color.rgb = mix(color.rgb, vec3(color.a), clamp(lens.z * liquid_glass.y, 0.0, 1.0));
    return color;
}
