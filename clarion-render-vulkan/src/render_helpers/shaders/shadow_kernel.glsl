// Separable convolution of Clarion's actual four-corner silhouette.
// Each row contributes its horizontal Gaussian probability mass; an
// eight-point Gauss-Legendre rule integrates the bounded vertical interval.
float clarion_normal_cdf(float x) {
    // Smooth normal-CDF approximation (Hendrycks/Gimpel, 2016). Clamp tails
    // before cubing so every finite geometry/blur ratio stays representable.
    float z = clamp(x, -8.0, 8.0);
    return 1.0 / (1.0 + exp(-1.5957691216 * (z + 0.044715 * z * z * z)));
}

float clarion_corner_inset(float edge_distance, float radius) {
    float r = max(radius, 0.0);
    float d = clamp(r - edge_distance, 0.0, r);
    // Factored form avoids cancellation of r*r - d*d at the endpoint.
    return r - sqrt(max((r - d) * (r + d), 0.0));
}

float clarion_shadow_row(vec2 point, vec2 size, vec4 radii, float sigma, float y) {
    float left = max(clarion_corner_inset(y, radii.x), clarion_corner_inset(size.y - y, radii.w));
    float right = size.x - max(clarion_corner_inset(y, radii.y), clarion_corner_inset(size.y - y, radii.z));
    float horizontal = max(clarion_normal_cdf((right - point.x) / sigma)
        - clarion_normal_cdf((left - point.x) / sigma), 0.0);
    float dy = (y - point.y) / sigma;
    return horizontal * exp(-0.5 * dy * dy) * (0.3989422804 / sigma);
}

float clarion_shadow_mass(vec2 point, vec2 size, vec4 radii, float sigma) {
    if (any(lessThanEqual(size, vec2(0.0)))) return 0.0;
    // Integrate the steeper boundary direction analytically. Near a top or
    // bottom corner this avoids asking a small quadrature to resolve the
    // almost vertical slope of a circle's horizontal cross-section.
    vec2 edge = max(-point, point - size);
    if (edge.y > edge.x) {
        point = point.yx;
        size = size.yx;
        radii = radii.xwzy;
    }
    float lower = max(0.0, point.y - 4.0 * sigma);
    float upper = min(size.y, point.y + 4.0 * sigma);
    if (upper <= lower) return 0.0;
    float midpoint = 0.5 * (lower + upper);
    float half_span = 0.5 * (upper - lower);
    vec4 nodes = vec4(0.1834346425, 0.5255324099, 0.7966664774, 0.9602898565);
    vec4 weights = vec4(0.3626837834, 0.3137066459, 0.2223810345, 0.1012285363);
    float mass = 0.0;
    for (int i = 0; i < 4; i++) {
        float offset_y = half_span * nodes[i];
        mass += weights[i] * (clarion_shadow_row(point, size, radii, sigma, midpoint - offset_y)
            + clarion_shadow_row(point, size, radii, sigma, midpoint + offset_y));
    }
    return clamp(mass * half_span, 0.0, 1.0);
}
