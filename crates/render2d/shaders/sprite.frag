#version 450

// One sample, one multiply: the atlas texel (premultiplied by hand at
// authoring time) times the instance tint (premultiplied by the same
// convention). The pipeline's blend state does the compositing; this
// stage only produces premultiplied color.

layout(set = 0, binding = 0) uniform sampler2D atlas;

layout(location = 0) in vec2 vertex_uv;
layout(location = 1) in vec4 vertex_tint;

layout(location = 0) out vec4 fragment_color;

void main() {
    fragment_color = texture(atlas, vertex_uv) * vertex_tint;
}
