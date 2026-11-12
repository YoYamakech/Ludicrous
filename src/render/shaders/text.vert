#version 330 core
// Per-instance: x, y, w, h (pixels), then atlas uv rect, then rgba.
layout(location = 0) in vec2 a_corner;   // unit quad, 0..1
layout(location = 1) in vec4 a_rect;
layout(location = 2) in vec4 a_uv;
layout(location = 3) in vec4 a_color;

uniform vec2 u_viewport;

out vec2 v_uv;
out vec4 v_color;

void main() {
    vec2 px = a_rect.xy + a_corner * a_rect.zw;
    vec2 ndc = vec2(px.x / u_viewport.x * 2.0 - 1.0,
                    1.0 - px.y / u_viewport.y * 2.0);
    gl_Position = vec4(ndc, 0.0, 1.0);
    v_uv = a_uv.xy + a_corner * (a_uv.zw - a_uv.xy);
    v_color = a_color;
}
