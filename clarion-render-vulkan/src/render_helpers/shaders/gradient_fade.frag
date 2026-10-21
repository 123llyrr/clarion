#version 100
//_DEFINES_
#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif
precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif
uniform vec2 cutoff;
uniform float alpha;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

void main() {
    float coverage = 1.0;
    if (cutoff.x < cutoff.y) {
        coverage = clamp((cutoff.y - v_coords.x) / (cutoff.y - cutoff.x), 0.0, 1.0);
    }
    // Outside the crop the result is known without touching the texture. Keep
    // debug composition after this decision so its visible tint is preserved.
    vec4 premultiplied = vec4(0.0);
    if (coverage > 0.0) {
        premultiplied = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
        premultiplied.a = 1.0;
#endif
        premultiplied *= coverage;
    }
    premultiplied *= alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0) {
        premultiplied = premultiplied * 0.8 + vec4(0.0, 0.2, 0.0, 0.2);
    }
#endif
    gl_FragColor = premultiplied;
}
