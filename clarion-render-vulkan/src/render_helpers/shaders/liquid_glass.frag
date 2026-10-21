uniform vec2 liquid_glass;
vec4 clarion_sample_background(vec2 uv) {
    vec2 sample_at = uv;
    float highlight = 0.0;
    if (liquid_glass.x > 0.0) {
        vec2 geometry_uv = (input_to_geo * vec3(uv,1.0)).xy;
        vec3 lens = clarion_glass_lens(geometry_uv,geo_size,corner_radius,liquid_glass.x);
        sample_at = clamp(uv + clarion_glass_uv_offset(lens.xy,geo_size,input_to_geo),0.0,1.0);
        highlight = clamp(lens.z * liquid_glass.y,0.0,1.0);
    }
    vec4 pixel = texture2D(tex,sample_at);
    return vec4(mix(pixel.rgb,vec3(pixel.a),highlight),pixel.a);
}
