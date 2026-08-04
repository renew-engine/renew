#version 450

// The depth path's proof shader: the instanced quad with a per-instance
// depth. Corners expanded from gl_VertexIndex (the house style -- no
// per-vertex buffer exists); placement, depth and colour read per
// instance from the one bound vertex buffer at instance rate.
//
// Layout here and the `InstanceAttribute` slice at pipeline creation
// describe the same bytes: location 0 = vec4 (xy centre in NDC, z
// depth, w unused), location 1 = vec4 colour. Change one and the other
// in the same commit or the draw reads garbage.

layout(location = 0) in vec4 instance_centre_depth;
layout(location = 1) in vec4 instance_colour;

layout(location = 0) out vec4 vertex_colour;

void main() {
    // Two triangles, four unique corners, six indices; a 0.5-NDC-wide
    // quad so two instances overlap decisively on a small image and the
    // depth test decides the winner.
    const vec2 corners[6] = vec2[](
        vec2(-0.25, -0.25),
        vec2( 0.25, -0.25),
        vec2(-0.25,  0.25),
        vec2( 0.25, -0.25),
        vec2( 0.25,  0.25),
        vec2(-0.25,  0.25)
    );
    gl_Position = vec4(
        instance_centre_depth.xy + corners[gl_VertexIndex],
        instance_centre_depth.z,
        1.0
    );
    vertex_colour = instance_colour;
}
