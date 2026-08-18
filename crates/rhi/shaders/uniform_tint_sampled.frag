#version 450

// Test fixture, not a builtin: the one shape that proves where a uniform
// block actually lands when a pipeline ALSO samples.
//
// A block is read at set `sampled_bindings` — after every sampled slot —
// so a pipeline with one sampler finds its block at set 1. Nothing else in
// the tree declares both, which left that rule exercised only in its
// degenerate case, at set 0, where an off-by-one in the set index or in
// the dynamic-offset ordering cannot show.
//
// It shares `textured.vert`, whose blob is untouched: the vertex stage
// needs nothing from either channel.
//
// The atlas is sampled and the block scales it, so the readback is only
// correct if BOTH arrived. A white atlas makes the expected answer the
// block's own tints, byte for byte, and the multiplier is the block's very
// last scalar — so a set index that pointed the descriptor at the wrong
// layout takes the whole image with it rather than shifting a channel.

layout(set = 0, binding = 0) uniform sampler2D atlas;

layout(std140, set = 1, binding = 0) uniform Block {
    vec4 tints[8];
    mat4 warp;
} block;

layout(location = 0) in vec2 fragUv;
layout(location = 0) out vec4 outColor;

void main() {
    int band = int(gl_FragCoord.x) & 7;
    outColor = texture(atlas, fragUv) * block.tints[band] * block.warp[3].w;
}
