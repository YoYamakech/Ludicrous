#version 330 core
in vec2 v_uv;
in vec4 v_color;
in vec4 v_clip;
in vec2 v_px;

uniform sampler2D u_atlas;
uniform vec2 u_window_size;
uniform float u_radius;

out vec4 frag_color;

float sd_round_box(vec2 p, vec2 half_size, float r) {
    vec2 q = abs(p) - half_size + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

void main() {
    if (v_px.x < v_clip.x || v_px.y < v_clip.y ||
        v_px.x > v_clip.x + v_clip.z || v_px.y > v_clip.y + v_clip.w) {
        discard;
    }

    // The atlas stores coverage in the red channel (GL_R8).
    float coverage = texture(u_atlas, v_uv).r;

    vec2 wc = u_window_size * 0.5;
    float wd = sd_round_box(v_px - wc, wc, u_radius);
    float mask = 1.0 - smoothstep(-0.5, 0.5, wd);

    float a = v_color.a * coverage * mask;
    frag_color = vec4(v_color.rgb * a, a);
}
