#version 450

layout(push_constant) uniform BorderPushConstants {
    mat4 matrix;
    vec4 color_from;
    vec4 color_to;
    vec4 outer_radius;
    vec2 geo_size;
    vec2 grad_offset;
    vec2 grad_vec;
    float colorspace; // 0: sRGB, 1: sRGB-linear, 2: OKLab, 3: OKLCh
    float hue_interpolation; // 0: shorter, 1: longer, 2: increasing, 3: decreasing
    float grad_width;
    float border_width;
    float alpha;
    float scale;
    float rotation_angle;
    float rounding_power;
    vec2 _geo_padding;
    vec4 geo_bounds;
} pc;

layout(location = 0) in vec2 in_uv;
layout(location = 0) out vec4 out_color;

vec4 premul_rect(vec4 color) {
    color.rgb *= color.a;
    return color;
}

vec4 premul_lch(vec4 color) {
    color.xy *= color.a;
    return color;
}

vec4 unpremul_rect(vec4 color) {
    if (color.a == 0.0)
        return color;

    color.rgb /= color.a;
    return color;
}

vec4 unpremul_lch(vec4 color) {
    if (color.a == 0.0)
        return color;

    color.xy /= color.a;
    return color;
}

vec4 premul_mix_unpremul_rect(vec4 color1, vec4 color2, float ratio) {
    vec4 mixed = mix(premul_rect(color1), premul_rect(color2), ratio);
    return unpremul_rect(mixed);
}

vec4 premul_mix_unpremul_lch(vec4 color1, vec4 color2, float ratio) {
    vec4 mixed = mix(premul_lch(color1), premul_lch(color2), ratio);
    return unpremul_lch(mixed);
}

vec3 srgb_to_linear(vec3 color) {
    return pow(color, vec3(2.2));
}

vec3 linear_to_srgb(vec3 color) {
    return pow(color, vec3(1.0 / 2.2));
}

vec3 lab_to_lch(vec3 color) {
    float c = sqrt(pow(color.y, 2.0) + pow(color.z, 2.0));
    float h = degrees(atan(color.z, color.y)) ;
    h += h <= 0.0 ?
        360.0 :
        0.0 ;
    return vec3(
        color.x,
        c,
        h
    );
}

vec3 lch_to_lab(vec3 color) {
    float a = color.y * clamp(cos(radians(color.z)), -1.0, 1.0);
    float b = color.y * clamp(sin(radians(color.z)), -1.0, 1.0);
    return vec3(
        color.x,
        a,
        b
    );
}

vec3 linear_to_oklab(vec3 color){
    mat3 rgb_to_lms = mat3(
        vec3(0.4122214708, 0.5363325363, 0.0514459929),
        vec3(0.2119034982, 0.6806995451, 0.1073969566),
        vec3(0.0883024619, 0.2817188376, 0.6299787005)
    );
    mat3 lms_to_oklab = mat3(
        vec3(0.2104542553, 0.7936177850, -0.0040720468),
        vec3(1.9779984951, -2.4285922050, 0.4505937099),
        vec3(0.0259040371, 0.7827717662, -0.8086757660)
    );
    vec3 lms = color * rgb_to_lms;
    lms = pow(lms, vec3(1.0 / 3.0));
    return lms * lms_to_oklab;
}

vec3 oklab_to_linear(vec3 color){
    mat3 oklab_to_lms = mat3(
        vec3(1.0, 0.3963377774, 0.2158037573),
        vec3(1.0, -0.1055613458, -0.0638541728),
        vec3(1.0, -0.0894841775, -1.2914855480)
    );
    mat3 lms_to_rgb = mat3(
        vec3(4.0767416621, -3.3077115913, 0.2309699292),
        vec3(-1.2684380046, 2.6097574011, -0.3413193965),
        vec3(-0.0041960863, -0.7034186147, 1.7076147010)
    );
    vec3 lms = color * oklab_to_lms;
    lms = pow(lms, vec3(3.0));
    return lms * lms_to_rgb;
}

vec4 color_mix(vec4 color1, vec4 color2, float color_ratio) {
    vec4 color_out;

    // srgb
    if (pc.colorspace == 0.0) {
        return mix(premul_rect(color1), premul_rect(color2), color_ratio);
    }

    color1.rgb = srgb_to_linear(color1.rgb);
    color2.rgb = srgb_to_linear(color2.rgb);

    // srgb-linear
    if (pc.colorspace == 1.0) {
        color_out = premul_mix_unpremul_rect(color1, color2, color_ratio);
    // oklab
    } else if (pc.colorspace == 2.0) {
        color1.xyz = linear_to_oklab(color1.rgb);
        color2.xyz = linear_to_oklab(color2.rgb);
        color_out = premul_mix_unpremul_rect(color1, color2, color_ratio);
        color_out.rgb = oklab_to_linear(color_out.xyz);
    // oklch
    } else if (pc.colorspace == 3.0) {
        color1.xyz = lab_to_lch(linear_to_oklab(color1.rgb));
        color2.xyz = lab_to_lch(linear_to_oklab(color2.rgb));
        color_out = premul_mix_unpremul_lch(color1, color2, color_ratio);

        float min_hue = min(color1.z, color2.z);
        float max_hue = max(color1.z, color2.z);
        float path_direct_distance = (max_hue - min_hue) * color_ratio;
        float path_mod_distance = (360.0 - max_hue + min_hue) * color_ratio;

        float path_mod =
            color1.z == min_hue ?
                mod(color1.z - path_mod_distance, 360.0) :
                mod(color1.z + path_mod_distance, 360.0) ;
        float path_direct =
            color1.z == min_hue ?
                color1.z + path_direct_distance :
                color1.z - path_direct_distance ;

        // shorter
        if (pc.hue_interpolation == 0.0) {
            color_out.z =
                max_hue - min_hue > 360.0 - max_hue + min_hue ?
                    path_mod :
                    path_direct ;
        // longer
        } else if (pc.hue_interpolation == 1.0) {
            color_out.z =
                max_hue - min_hue <= 360.0 - max_hue + min_hue ?
                    path_mod :
                    path_direct ;
        // increasing
        } else if (pc.hue_interpolation == 2.0) {
            color_out.z =
                color1.z > color2.z ?
                    path_mod :
                    path_direct ;
        // decreasing
        } else if (pc.hue_interpolation == 3.0) {
            color_out.z =
                color1.z <= color2.z ?
                    path_mod :
                    path_direct ;
        }
        color_out.rgb = clamp(oklab_to_linear(lch_to_lab(color_out.xyz)), 0.0, 1.0);
    }

    return premul_rect(vec4(linear_to_srgb(color_out.rgb), color_out.a));
}

float realand_rounding_alpha(vec2 coords, vec2 size, vec4 corner_radius) {
    vec2 center;
    float radius;

    if (coords.x < corner_radius.x && coords.y < corner_radius.x) {
        radius = corner_radius.x;
        center = vec2(radius, radius);
    } else if (size.x - corner_radius.y < coords.x && coords.y < corner_radius.y) {
        radius = corner_radius.y;
        center = vec2(size.x - radius, radius);
    } else if (size.x - corner_radius.z < coords.x && size.y - corner_radius.z < coords.y) {
        radius = corner_radius.z;
        center = vec2(size.x - radius, size.y - radius);
    } else if (coords.x < corner_radius.w && size.y - corner_radius.w < coords.y) {
        radius = corner_radius.w;
        center = vec2(radius, size.y - radius);
    } else {
        return 1.0;
    }

    vec2 delta = abs(coords - center);
    float power = max(pc.rounding_power, 2.0);
    float dist = pow(pow(delta.x, power) + pow(delta.y, power), 1.0 / power);
    float t = clamp((dist - radius) * pc.scale + 0.5, 0.0, 1.0);
    return 1.0 - t * t * (3.0 - 2.0 * t);
}

void main() {
    vec2 coords_geo = in_uv * pc.geo_size;

    vec2 grad_coords = coords_geo;
    if (pc.rotation_angle != 0.0) {
        vec2 center = pc.geo_size * 0.5;
        float rad = radians(pc.rotation_angle);
        float cos_a = cos(rad);
        float sin_a = sin(rad);
        vec2 d = coords_geo - center;
        grad_coords = vec2(d.x * cos_a - d.y * sin_a, d.x * sin_a + d.y * cos_a) + center;
    }

    vec2 coords = grad_coords + pc.grad_offset;
    if ((pc.grad_vec.x < 0.0 && 0.0 <= pc.grad_vec.y) || (0.0 <= pc.grad_vec.x && pc.grad_vec.y < 0.0)) {
        coords.x -= pc.grad_width;
    }

    float frac = dot(coords, pc.grad_vec) / dot(pc.grad_vec, pc.grad_vec);
    if (pc.grad_vec.y < 0.0) frac += 1.0;
    frac = clamp(frac, 0.0, 1.0);

    vec4 color = color_mix(pc.color_from, pc.color_to, frac);
    color = color * realand_rounding_alpha(coords_geo, pc.geo_size, pc.outer_radius);

    if (pc.border_width > 0.0) {
        vec2 inner_coords = coords_geo - vec2(pc.border_width);
        vec2 inner_geo_size = pc.geo_size - vec2(pc.border_width * 2.0);
        // Intersect the rounded mask with antialiased rectangle edges. A hard
        // inside test makes half-pixel border widths unstable across GPU backends.
        vec2 edge = min(inner_coords, inner_geo_size - inner_coords);
        float coverage = clamp(min(edge.x, edge.y) * pc.scale + 0.5, 0.0, 1.0);
        vec4 inner_radius = max(pc.outer_radius - vec4(pc.border_width), vec4(0.0));
        float inner_alpha = min(coverage, realand_rounding_alpha(inner_coords, inner_geo_size, inner_radius));
        color *= 1.0 - inner_alpha;
    }

    out_color = color * pc.alpha;
}
