#version 450

// The mesh path's proof shader, and the first in this tree to read a
// per-vertex buffer. Every other vertex stage here expands corners from
// gl_VertexIndex because no per-vertex stream existed; this one takes
// its positions from geometry, which is the whole point of the change
// that introduced it.
//
// Positions are clip space, straight through. There is no model, view or
// projection matrix here deliberately: a camera is a later step, and a
// shader that multiplied by an identity it was handed would be pretending
// to do work the crate cannot yet supply. A consumer drawing world
// geometry transforms on the way into the vertex buffer until then.
//
// Layout here and the `VertexAttribute` slice at pipeline creation
// describe the same bytes: location 0 = vec3 position (clip space),
// location 1 = vec4 colour. Change one and the other in the same commit
// or the draw reads garbage.

layout(location = 0) in vec3 vertex_position;
layout(location = 1) in vec4 vertex_colour;

layout(location = 0) out vec4 fragment_colour;

void main() {
    gl_Position = vec4(vertex_position, 1.0);
    fragment_colour = vertex_colour;
}
