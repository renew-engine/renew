#version 450

// Interpolated vertex colour through; the vertex stage decided
// everything. Flat-coloured geometry therefore comes out byte-exact on
// any conformant adapter, which is what lets the mesh oracle compute its
// expected image instead of committing one.

layout(location = 0) in vec4 fragment_colour;
layout(location = 0) out vec4 out_colour;

void main() {
    out_colour = fragment_colour;
}
