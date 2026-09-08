#version 450

// The plain mesh path with a texture: clip-space positions drawn
// straight, plus the coordinate the fragment stage samples with.
//
// **No matrix and no fade, unlike `mesh_camera_textured.vert`.** This
// path's positions are already clip space, so there is no view distance
// to fade by: `w` is one everywhere, and a fade computed from it would be
// a constant tint. A caller that wants distance here has projected the
// world itself and knows the distances it used.
//
// The record has five attributes and this stage reads the ones it
// needs. Locations, in order: 0 vec3 position, 1 vec4 colour, 2 vec2
// texture coordinate, 3 vec3 normal, 4 vec4 tangent (xyz, and the
// bitangent's sign in w). A stage may ignore any of them; what it may
// not do is disagree about which location is which, so change this
// list and MESH_LAYOUT in the same commit or the draw reads garbage.

layout(location = 0) in vec3 vertex_position;
layout(location = 1) in vec4 vertex_colour;
layout(location = 2) in vec2 vertex_uv;

layout(location = 0) out vec4 fragment_colour;
layout(location = 1) out vec2 fragment_uv;

void main() {
    gl_Position = vec4(vertex_position, 1.0);
    fragment_colour = vertex_colour;
    fragment_uv = vertex_uv;
}
