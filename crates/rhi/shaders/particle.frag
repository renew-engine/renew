#version 450

// The particle fragment stage: the atlas texel times the instance's
// premultiplied colour. Multiplied rather than replaced, so a tile's
// shape carries the effect's colour — the same reasoning the textured
// mesh path records for its tint.

layout(set = 0, binding = 0) uniform sampler2D atlas;

layout(location = 0) in vec4 fragment_colour;
layout(location = 1) in vec2 fragment_uv;

layout(location = 0) out vec4 out_colour;

void main() {
    out_colour = texture(atlas, fragment_uv) * fragment_colour;
}
