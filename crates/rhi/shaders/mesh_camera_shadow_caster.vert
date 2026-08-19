#version 450

// The caster half of the shadowed camera path: the world as the LIGHT
// sees it, depth only, no fragment stage.
//
// **The same push block the lit stage reads**, and that is the whole
// point of this file existing. The camera matrix and the scene light are
// declared here and never read — deliberately — because one record for
// both halves means the map cannot be written with one light and
// sampled with another. While the light was packed twice, once for the
// caster and once for the lit pass, a caller could hand them different
// matrices and get a shadow that landed somewhere nothing stood.
//
// It also turns a whole class of host-side mistake into a visible one.
// Pack the light's columns where its rows belong and both stages are
// wrong the *same* way: every surface then self-compares against its own
// depth and the shadow disappears, which a golden refuses loudly. With
// two encodings the same mistake moved the cast a little, which a golden
// can miss.
//
// Replaces the previous caster, which reused the ordinary camera vertex
// stage and computed a colour and a fade that a depth-only pass throws
// away.

layout(push_constant) uniform Matrices {
    // Unread here: the caster draws from the light, not the camera. It
    // is declared so the two stages share one block and one layout.
    mat4 view_projection;
    // Rows 0..2 of the light's affine view-projection. Row 3 is
    // (0, 0, 0, 1) and is not sent.
    vec4 light_row_0;
    vec4 light_row_1;
    vec4 light_row_2;
    // Unread here, for the same reason as the camera matrix: a depth
    // pass has no colour to light.
    vec4 light;
} matrices;

layout(location = 0) in vec3 vertex_position;
// Declared and unused, as the plain mesh stage declares what it does not
// consume: the vertex layout is the mesh's, not this stage's.
layout(location = 1) in vec4 vertex_colour;
layout(location = 2) in vec2 vertex_uv;

void main() {
    vec4 world = vec4(vertex_position, 1.0);
    // Identical expression to mesh_camera_shadow.vert's, deliberately:
    // the fragment stage compares its light-space z against the depth
    // this pass rasterizes, within a constant bias, so any difference
    // between the two would be a bias nobody chose.
    gl_Position = vec4(
        dot(matrices.light_row_0, world),
        dot(matrices.light_row_1, world),
        dot(matrices.light_row_2, world),
        1.0
    );
}
