#version 450

// A full-target quad as two triangles from gl_VertexIndex: no buffers,
// no vertex input. The texture coordinate is derived from the same
// index, so the only thing this stage needs from outside is which of
// the six corners it is drawing.
//
// Clip space runs -1..1 left-to-right and top-to-bottom, and the
// texture's first row is its top row, so v maps straight from y with
// no flip: corner (-1,-1) is (0,0).

layout(location = 0) out vec2 fragUv;

vec2 corners[6] = vec2[](
    vec2(-1.0, -1.0),
    vec2( 1.0, -1.0),
    vec2( 1.0,  1.0),
    vec2(-1.0, -1.0),
    vec2( 1.0,  1.0),
    vec2(-1.0,  1.0)
);

void main() {
    vec2 corner = corners[gl_VertexIndex];
    gl_Position = vec4(corner, 0.0, 1.0);
    fragUv = corner * 0.5 + 0.5;
}
