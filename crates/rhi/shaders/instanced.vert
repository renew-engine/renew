#version 450

// The per-frame data path's proof shader: a small quad per instance,
// corners expanded from gl_VertexIndex (the house style -- no per-vertex
// buffer exists), placement and colour read per instance from the one
// bound vertex buffer at instance rate.
//
// Layout here and the `VertexAttribute` slice declared as this pipeline's instance input
// describe the same bytes: location 0 = vec2 centre (NDC), location 1 =
// vec4 colour. Change one and the other in the same commit or the draw
// reads garbage.

layout(location = 0) in vec2 instance_centre;
layout(location = 1) in vec4 instance_colour;

layout(location = 0) out vec4 vertex_colour;

void main() {
    // Two triangles, four unique corners, six indices; a 0.25-NDC-wide
    // quad so several instances fit a small golden without overlap.
    const vec2 corners[6] = vec2[](
        vec2(-0.125, -0.125),
        vec2( 0.125, -0.125),
        vec2(-0.125,  0.125),
        vec2( 0.125, -0.125),
        vec2( 0.125,  0.125),
        vec2(-0.125,  0.125)
    );
    gl_Position = vec4(instance_centre + corners[gl_VertexIndex], 0.0, 1.0);
    vertex_colour = instance_colour;
}
