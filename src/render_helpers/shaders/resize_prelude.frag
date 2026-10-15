precision highp float;

#if defined(DEBUG_FLAGS)
uniform float realand_tint;
#endif

varying vec2 realand_v_coords;
uniform vec2 realand_size;

uniform mat3 realand_input_to_curr_geo;
uniform mat3 realand_curr_geo_to_prev_geo;
uniform mat3 realand_curr_geo_to_next_geo;
uniform vec2 realand_curr_geo_size;

uniform sampler2D realand_tex_prev;
uniform mat3 realand_geo_to_tex_prev;

uniform sampler2D realand_tex_next;
uniform mat3 realand_geo_to_tex_next;

uniform float realand_progress;
uniform float realand_clamped_progress;

uniform vec4 realand_corner_radius;
uniform float realand_clip_to_geometry;

uniform float realand_alpha;
uniform float realand_scale;

float realand_rounding_alpha(vec2 coords, vec2 size, vec4 corner_radius);
