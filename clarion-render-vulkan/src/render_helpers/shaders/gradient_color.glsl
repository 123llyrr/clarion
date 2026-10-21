// Clarion gradient arithmetic. Matrices and transfer functions are the
// mathematical sRGB/Oklab definitions; see docs/rewrite-render-kernels.txt.
float clarion_linear_channel(float encoded) {
    float a = abs(encoded);
    return sign(encoded) * (a <= 0.04045 ? a / 12.92 : pow((a + 0.055) / 1.055, 2.4));
}

float clarion_encoded_channel(float linear) {
    float a = abs(linear);
    return sign(linear) * (a <= 0.0031308 ? a * 12.92 : 1.055 * pow(a, 1.0 / 2.4) - 0.055);
}

vec3 clarion_linear_rgb(vec3 color) {
    return vec3(clarion_linear_channel(color.r), clarion_linear_channel(color.g), clarion_linear_channel(color.b));
}

vec3 clarion_encoded_rgb(vec3 color) {
    return vec3(clarion_encoded_channel(color.r), clarion_encoded_channel(color.g), clarion_encoded_channel(color.b));
}

vec3 clarion_rgb_lab(vec3 rgb) {
    vec3 lms = vec3(
        dot(rgb, vec3(0.4122214708, 0.5363325363, 0.0514459929)),
        dot(rgb, vec3(0.2119034982, 0.6806995451, 0.1073969566)),
        dot(rgb, vec3(0.0883024619, 0.2817188376, 0.6299787005)));
    lms = sign(lms) * pow(abs(lms), vec3(1.0 / 3.0));
    return vec3(
        dot(lms, vec3(0.2104542553, 0.7936177850, -0.0040720468)),
        dot(lms, vec3(1.9779984951, -2.4285922050, 0.4505937099)),
        dot(lms, vec3(0.0259040371, 0.7827717662, -0.8086757660)));
}

vec3 clarion_lab_rgb(vec3 lab) {
    vec3 root = vec3(
        lab.x + dot(lab.yz, vec2(0.3963377774, 0.2158037573)),
        lab.x + dot(lab.yz, vec2(-0.1055613458, -0.0638541728)),
        lab.x + dot(lab.yz, vec2(-0.0894841775, -1.2914855480)));
    vec3 lms = root * root * root;
    return vec3(
        dot(lms, vec3(4.0767416621, -3.3077115913, 0.2309699292)),
        dot(lms, vec3(-1.2684380046, 2.6097574011, -0.3413193965)),
        dot(lms, vec3(-0.0041960863, -0.7034186147, 1.7076147010)));
}

float clarion_hue_delta(float a, float b, float path) {
    float delta = b - a;
    if (path == 0.0) {
        if (delta > 180.0) delta -= 360.0;
        if (delta < -180.0) delta += 360.0;
    } else if (path == 1.0) {
        if (delta > 0.0 && delta < 180.0) delta -= 360.0;
        else if (delta <= 0.0 && delta > -180.0) delta += 360.0;
    } else if (path == 2.0) {
        if (delta < 0.0) delta += 360.0;
    } else {
        if (delta > 0.0) delta -= 360.0;
    }
    return delta;
}

vec4 clarion_gradient_mix(vec4 first, vec4 last, float t, float space, float path) {
    float opacity = mix(first.a, last.a, t);
    if (opacity <= 0.0) return vec4(0.0);
    if (all(equal(first, last))) return vec4(first.rgb * first.a, first.a);
    if (space == 0.0) return mix(vec4(first.rgb * first.a, first.a), vec4(last.rgb * last.a, last.a), t);
    vec3 a = clarion_linear_rgb(first.rgb);
    vec3 b = clarion_linear_rgb(last.rgb);
    if (space >= 2.0) {
        a = clarion_rgb_lab(a);
        b = clarion_rgb_lab(b);
    }
    if (space == 3.0) {
        float chroma_a = length(a.yz);
        float chroma_b = length(b.yz);
        // atan(0,0) is undefined in GLSL, including a black endpoint whose
        // alpha differs from the other black endpoint.
        float hue_a = chroma_a < 0.00001 ? 0.0 : mod(degrees(atan(a.z, a.y)) + 360.0, 360.0);
        float hue_b = chroma_b < 0.00001 ? 0.0 : mod(degrees(atan(b.z, b.y)) + 360.0, 360.0);
        // A grey or invisible endpoint has no hue to contribute.
        if (chroma_a < 0.00001 || first.a == 0.0) hue_a = hue_b;
        if (chroma_b < 0.00001 || last.a == 0.0) hue_b = hue_a;
        float hue = radians(hue_a + clarion_hue_delta(hue_a, hue_b, path) * t);
        vec2 lc = mix(vec2(a.x, chroma_a) * first.a, vec2(b.x, chroma_b) * last.a, t) / opacity;
        a = vec3(lc.x, lc.y * cos(hue), lc.y * sin(hue));
    } else {
        a = mix(a * first.a, b * last.a, t) / opacity;
    }
    if (space >= 2.0) a = clarion_lab_rgb(a);
    return vec4(clamp(clarion_encoded_rgb(a), 0.0, 1.0) * opacity, opacity);
}

float clarion_gradient_position(vec2 p, vec2 offset, vec2 vector, float width) {
    float denominator = dot(vector, vector);
    if (denominator <= 0.0) return 0.0;
    vec2 origin = p + offset;
    if ((vector.x < 0.0) != (vector.y < 0.0)) origin.x -= width;
    return clamp(dot(origin, vector) / denominator + (vector.y < 0.0 ? 1.0 : 0.0), 0.0, 1.0);
}
