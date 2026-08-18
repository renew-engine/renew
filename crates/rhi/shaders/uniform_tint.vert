#version 450

// Test fixture, not a builtin: the smallest consumer of a uniform block.
// Covers the target with the classic oversized triangle generated from
// gl_VertexIndex, so every pixel of a frame is shaded and the oracle can
// be a readback comparison rather than a smoke call.
//
// No vertex input, no push constants: the block is the only channel this
// pipeline has, which is what makes a wrong readback point at it.

void main() {
    vec2 corner = vec2((gl_VertexIndex << 1) & 2, gl_VertexIndex & 2);
    gl_Position = vec4(corner * 2.0 - 1.0, 0.0, 1.0);
}
