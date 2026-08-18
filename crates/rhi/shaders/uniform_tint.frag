#version 450

// Test fixture, not a builtin: answers with bytes read from a uniform
// block, so a frame's readback is a direct statement about what reached
// the shader.
//
// The block is 192 bytes — eight vec4s and a mat4 — which is past the
// 128-byte guaranteed push-constant ceiling by construction. A test that
// passes here could not have been served by the channel that already
// existed, which is the whole point of the fixture.
//
// Every pixel reads a DIFFERENT part of the block: the tint is chosen by
// the pixel's own x, cycling through all eight, and scaled by the very
// last scalar of the matrix. So a block bound at the wrong offset, a
// slot stride the driver disagrees with, or a short write all show as a
// wrong readback rather than as a plausible one — and the last scalar
// being the multiplier means a write that stops early takes the whole
// image to whatever the tail happened to hold.

layout(std140, set = 0, binding = 0) uniform Block {
    vec4 tints[8];
    mat4 warp;
} block;

layout(location = 0) out vec4 out_colour;

void main() {
    int band = int(gl_FragCoord.x) & 7;
    out_colour = block.tints[band] * block.warp[3].w;
}
