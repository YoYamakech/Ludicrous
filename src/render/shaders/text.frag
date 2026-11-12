#version 330 core
in vec2 v_uv;
in vec4 v_color;

uniform sampler2D u_atlas;

out vec4 frag_color;

void main() {
    // The atlas stores coverage in the red channel (GL_R8).
    float coverage = texture(u_atlas, v_uv).r;
    frag_color = vec4(v_color.rgb, v_color.a * coverage);
}
