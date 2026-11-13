#version 330 core
in vec2 v_local;
in vec2 v_size;
in vec4 v_color;
in vec4 v_params;
in vec4 v_clip;
in vec2 v_px;

uniform vec2 u_window_size;
uniform float u_radius;

out vec4 frag_color;

// Signed distance to a rounded box centred at the origin.
float sd_round_box(vec2 p, vec2 half_size, float r) {
    vec2 q = abs(p) - half_size + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

void main() {
    // Per-instance clip: cheaper than glScissor and it nests naturally.
    if (v_px.x < v_clip.x || v_px.y < v_clip.y ||
        v_px.x > v_clip.x + v_clip.z || v_px.y > v_clip.y + v_clip.w) {
        discard;
    }

    float radius = min(v_params.x, min(v_size.x, v_size.y) * 0.5);
    float d = sd_round_box(v_local - v_size * 0.5, v_size * 0.5, radius);

    float cov;
    if (v_params.z > 0.5) {
        // Squiggle (v1.2 section 7.3): a wavy underline instead of a filled
        // box, so diagnostics need no second program and no extra draw call.
        // One period is ~6 physical pixels, which reads as a wave at 100% DPI
        // and stays a wave (not a blur) at 200%.
        float period = 6.0;
        float wave = sin((v_px.x / period) * 6.2831853) * 1.5;
        float centre = v_size.y * 0.5 - 0.5 + wave;
        float dist = abs(v_local.y - centre) - 0.9;
        cov = 1.0 - smoothstep(-0.5, 0.9, dist);
    } else if (v_params.y > 0.0) {
        // Border drawn inside the shape: keep the band -width..0.
        float ring = abs(d + v_params.y * 0.5) - v_params.y * 0.5;
        cov = 1.0 - smoothstep(-0.5, 0.5, ring);
    } else {
        cov = 1.0 - smoothstep(-0.5, 0.5, d);
    }

    // Window mask: clip every fragment to the rounded window shape.
    vec2 wc = u_window_size * 0.5;
    float wd = sd_round_box(v_px - wc, wc, u_radius);
    float mask = 1.0 - smoothstep(-0.5, 0.5, wd);

    float a = v_color.a * cov * mask;
    // Premultiplied alpha: the compositor expects rgb already multiplied.
    frag_color = vec4(v_color.rgb * a, a);
}
