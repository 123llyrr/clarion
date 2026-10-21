// Shared encoded-scene postprocessing. RGB may be additive, negative or HDR;
// alpha is not clamped or inferred from RGB. Only a literal zero RGBA sample
// has a background-only solution. See the independent domain/oracle tests.
#ifndef CLARION_POST_NOISE
#define CLARION_POST_NOISE noise
#endif
#ifndef CLARION_POST_SATURATION
#define CLARION_POST_SATURATION saturation
#endif
#ifndef CLARION_POST_BACKGROUND
#define CLARION_POST_BACKGROUND bg_color
#endif

// Historical attribution for the removed hash12 implementation, retained
// during migration: Sin-less white noise by David Hoskins (MIT License).
// https://www.shadertoy.com/view/4djSRW
// The balanced pixel grain below is designed from a finite-field permutation;
// it does not use that implementation. See the integer oracle for its contract.
float clarion_grain_permutation(float value) {
    // Cubing permutes the residues modulo 251, since gcd(3, 250) = 1.
    // Reduce the square first: every integer product stays below 2^16.
    float square = mod(value * value, 251.0);
    return mod(square * value, 251.0);
}

float clarion_pixel_grain(vec2 pixel_center) {
    vec2 pixel = mod(floor(pixel_center), 251.0);
    float row = clarion_grain_permutation(pixel.y);
    // Every row uses a nonzero stride, and therefore visits every level once.
    // Varying the stride avoids making all rows shifts of the same sequence.
    float stride = 1.0 + mod(pixel.y, 250.0);
    float position = mod(pixel.x * stride + row, 251.0);
    float rank = clarion_grain_permutation(position);
    return (rank + 0.5) / 251.0;
}

vec3 saturate(vec3 color, float saturation) {
    float luminance = dot(color, vec3(0.2126, 0.7152, 0.0722));
    // Add the missing/excess neutral contribution. Do not clamp extended RGB.
    return color + (vec3(luminance) - color) * (1.0 - saturation);
}

vec4 postprocess(vec4 source) {
    if (all(equal(source, vec4(0.0)))) return CLARION_POST_BACKGROUND;
    vec3 rgb = source.rgb;
    if (CLARION_POST_SATURATION != 1.0) {
        rgb = saturate(rgb, CLARION_POST_SATURATION);
    }
    // Zero alpha cancels this term even when RGB is additive/nonzero; such
    // RGB still follows the saturation and background paths above/below.
    if (source.a != 0.0 && CLARION_POST_NOISE > 0.0) {
        float perturbation = clarion_pixel_grain(gl_FragCoord.xy) - 0.5;
        rgb += perturbation * (CLARION_POST_NOISE * source.a);
    }
    vec4 result = vec4(rgb, source.a);
    if (source.a != 1.0) {
        result += CLARION_POST_BACKGROUND * (1.0 - source.a);
    }
    return result;
}
