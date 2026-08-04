#version 450

// Flat instance colour through; the vertex stage decided everything.

layout(location = 0) in vec4 vertex_colour;
layout(location = 0) out vec4 out_colour;

void main() {
    out_colour = vertex_colour;
}
