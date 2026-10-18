#version 450
layout(push_constant) uniform ColorPushConstants {
    mat4 matrix;
    vec4 uv_bounds;
    vec4 gamut[3]; // [0].w absolute normalization; [1].w identity; [2].w luminance gain
    vec4 source; // transfer, minimum nits, code-one span nits, reference nits
    vec4 target;
    vec4 mapping; // source peak, target peak, intent (0 perceptual / 1 relative / 2 absolute), clamp
    vec4 tone_gamut[3]; // working RGB → actual output RGB, for the shoulder metric
} pc;
layout(binding = 0) uniform sampler2D tex;
layout(location = 0) in vec2 uv;
layout(location = 0) out vec4 out_color;
const float m1 = 2610.0 / 16384.0;
const float m2 = 2523.0 / 32.0;
const float c1 = 3424.0 / 4096.0;
const float c2 = 2413.0 / 128.0;
const float c3 = 2392.0 / 128.0;

float pq_decode(float value) {
    float n = pow(clamp(value, 0.0, 1.0), 1.0 / m2);
    return 10000.0 * pow(max(n - c1, 0.0) / max(c2 - c3 * n, 1e-8), 1.0 / m1);
}
float pq_encode(float nits) {
    float value = clamp(nits / 10000.0, 0.0, 1.0);
    if (value == 0.0) return 0.0;
    float n = pow(value, m1);
    return pow((c1 + c2 * n) / (1.0 + c3 * n), m2);
}
float decode_channel(float value, vec4 description) {
    if (description.x == 3.0) return pq_decode(value);
    float n = abs(value);
    if (description.x == 0.0) {
        n = n <= 0.04045 ? n / 12.92 : pow((n + 0.055) / 1.055, 2.4);
    } else if (description.x == 1.0) {
        n = pow(n, 2.2);
    }
    return description.y + sign(value) * n * description.z;
}
float encode_channel(float nits, vec4 description) {
    if (description.x == 3.0) return pq_encode(nits);
    float value = (nits - description.y) / description.z;
    float n = abs(value);
    if (description.x == 0.0) {
        n = n <= 0.0031308 ? n * 12.92 : 1.055 * pow(n, 1.0 / 2.4) - 0.055;
    } else if (description.x == 1.0) {
        n = pow(n, 1.0 / 2.2);
    }
    return sign(value) * n;
}
float tone_map(float value, float reference, float source_peak, float target_peak) {
    float input_range = source_peak / reference;
    float output_range = target_peak / reference;
    if (source_peak <= target_peak * 1.001) return min(value, target_peak);
    float v = max((output_range * (1.0 + input_range) - input_range) / (input_range * input_range), 0.0);
    float x = clamp(value / reference, 0.0, input_range);
    return clamp(x * (1.0 + x * v) / (1.0 + x) * reference, 0.0, target_peak);
}
void main() {
    vec4 sample_color = texture(tex, uv);
    float alpha = clamp(sample_color.a, 0.0, 1.0);
    if (alpha <= 0.0) { out_color = vec4(0.0); return; }
    if (pc.gamut[1].w == 1.0) {
        // Exact for min=0, code-one=reference=peak=80 and identical sRGB.
        // Clamp after premultiplication equivalently to the full color pass.
        out_color = vec4(clamp(sample_color.rgb, vec3(0.0), vec3(alpha)), alpha);
        return;
    }
    if (pc.gamut[1].w == 2.0 && all(greaterThanEqual(sample_color.rgb, vec3(0.0)))
        && all(lessThanEqual(sample_color.rgb, vec3(alpha)))) {
        // Default SDR in an HDR scene: perceptual black/white adaptation and
        // target normalization cancel exactly. Keep the general transform for
        // extended values and RGB greater than alpha, even from UNORM clients.
        out_color = vec4(sample_color.rgb, alpha);
        return;
    }
    vec3 encoded = sample_color.rgb / alpha;
    vec3 linear = vec3(decode_channel(encoded.r, pc.source), decode_channel(encoded.g, pc.source), decode_channel(encoded.b, pc.source));
    linear = vec3(dot(pc.gamut[0].xyz, linear), dot(pc.gamut[1].xyz, linear), dot(pc.gamut[2].xyz, linear));
    float scale = pc.mapping.z < 2.0 ? pc.target.w / pc.source.w : pc.gamut[0].w;
    float offset = 0.0;
    if (pc.mapping.z == 0.0) {
        scale = (pc.target.w - pc.target.y) / (pc.source.w - pc.source.y);
        offset = pc.target.y - pc.source.y * scale;
    }
    linear = linear * scale + vec3(offset);
    float source_peak = pc.mapping.x * scale + offset;
    vec3 tone_rgb = vec3(dot(pc.tone_gamut[0].xyz, linear), dot(pc.tone_gamut[1].xyz, linear), dot(pc.tone_gamut[2].xyz, linear));
    float maximum = max(max(tone_rgb.r, tone_rgb.g), tone_rgb.b);
    if (pc.mapping.z == 0.0 && maximum > 0.0) {
        linear *= tone_map(maximum, pc.target.w, source_peak, pc.mapping.y) / maximum;
    }
    if (pc.mapping.w != 0.0) linear = clamp(linear, vec3(0.0), vec3(pc.mapping.y));
    // Panel brightness acts on absolute linear luminance, after tone mapping
    // and clipping, before PQ encoding. Never multiply encoded PQ code values.
    linear *= pc.gamut[2].w;
    vec3 result = vec3(encode_channel(linear.r, pc.target), encode_channel(linear.g, pc.target), encode_channel(linear.b, pc.target));
    out_color = vec4(result * alpha, alpha);
}
