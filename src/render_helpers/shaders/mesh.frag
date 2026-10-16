#version 100

precision mediump float;

uniform sampler2D tex;
uniform float alpha;

varying vec2 clarion_v_uv;

void main() {
    gl_FragColor = texture2D(tex, clarion_v_uv) * alpha;
}
