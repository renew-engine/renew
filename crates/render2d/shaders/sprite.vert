#version 450

// The sprite quad: corners expanded from gl_VertexIndex (the house
// style -- no per-vertex buffer exists), everything else read per
// instance from the one bound vertex buffer at instance rate.
//
// Layout here and the `VertexAttribute` slice declared as this
// pipeline's instance input describe the same bytes: locations 0 to 3 =
// the four corners in NDC -- local top-left, top-right, bottom-left,
// bottom-right, already turned, scaled and placed on the CPU --
// location 4/5 = vec2 UV at the first and the last corner, location 6
// = vec4 premultiplied tint. Change one and the other in the same
// commit or the draw reads garbage.

layout(location = 0) in vec2 instance_corner_a;
layout(location = 1) in vec2 instance_corner_b;
layout(location = 2) in vec2 instance_corner_c;
layout(location = 3) in vec2 instance_corner_d;
layout(location = 4) in vec2 instance_uv0;
layout(location = 5) in vec2 instance_uv1;
layout(location = 6) in vec4 instance_tint;

layout(location = 0) out vec2 vertex_uv;
layout(location = 1) out vec4 vertex_tint;

void main() {
    // Two triangles, four unique corners, six indices; per-corner
    // selectors pick a corner per axis. Same corner order as the
    // instanced builtin, so the winding story is one story.
    const vec2 select[6] = vec2[](
        vec2(0.0, 0.0),
        vec2(1.0, 0.0),
        vec2(0.0, 1.0),
        vec2(1.0, 0.0),
        vec2(1.0, 1.0),
        vec2(0.0, 1.0)
    );
    vec2 corner = select[gl_VertexIndex];
    // A nested mix with 0/1 weights: along the top edge, along the
    // bottom edge, then between them. For an axis-aligned sprite the
    // top and bottom edges share x per corner and the left and right
    // share y, so each mix has either the operands of the previous
    // single mix or two equal operands -- the same arithmetic on the
    // same values under either way a driver evaluates mix.
    vec2 top = mix(instance_corner_a, instance_corner_b, corner.x);
    vec2 bottom = mix(instance_corner_c, instance_corner_d, corner.x);
    gl_Position = vec4(mix(top, bottom, corner.y), 0.0, 1.0);
    vertex_uv = mix(instance_uv0, instance_uv1, corner);
    vertex_tint = instance_tint;
}
