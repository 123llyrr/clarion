precision highp float;

#if defined(DEBUG_FLAGS)
uniform float clarion_tint;
#endif

varying vec2 clarion_v_coords;
uniform vec2 clarion_size;

uniform mat3 clarion_input_to_curr_geo;
uniform mat3 clarion_curr_geo_to_prev_geo;
uniform mat3 clarion_curr_geo_to_next_geo;
uniform vec2 clarion_curr_geo_size;

uniform sampler2D clarion_tex_prev;
uniform mat3 clarion_geo_to_tex_prev;

uniform sampler2D clarion_tex_next;
uniform mat3 clarion_geo_to_tex_next;

uniform float clarion_progress;
uniform float clarion_clamped_progress;

uniform vec4 clarion_corner_radius;
uniform float clarion_clip_to_geometry;

uniform float clarion_alpha;
uniform float clarion_scale;

float clarion_rounding_alpha(vec2 coords, vec2 size, vec4 corner_radius);
