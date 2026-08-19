#version 450

// The camera mesh path with a texture, a shadow map, and a scene light:
// the textured pair's vertex stage, plus the light's view of the same
// vertex, plus how brightly the whole scene is lit.
//
// **Three things in 128 bytes, which is exactly the guaranteed push
// ceiling.** The camera's matrix takes the vertex to the screen (64);
// the light's takes it to the shadow map's clip space (48); the scene
// light is a colour multiplied into every vertex colour (16). The naive
// layout — two full mat4s and a colour — is 144 and does not fit, which
// is why no path carried a light and a shadow at once until now.
//
// **The light's matrix is three rows, not four columns.** Its projection
// is orthographic and its view is rigid, so the product is affine and
// its bottom row is exactly (0, 0, 0, 1) — carrying that row would be
// carrying a constant. The three rows that vary are stored as vec4s and
// applied by dot product, which costs 48 bytes.
//
// A `mat4x3` would NOT work and is the trap to avoid: std430 pads each
// of its four three-component columns back to sixteen bytes, so it is
// still 64 and the block is still 144, while silently reading padding
// if the host packed 48. The saving comes from rows, not from a
// narrower matrix type.
//
// **The caster reads this same block**, and that is what keeps the two
// halves honest: the map cannot be written with one light and sampled
// with another, because there is only one light to write and sample.
//
// Layout here and the `VertexAttribute` slice at pipeline creation
// describe the same bytes: location 0 = vec3 position in world space,
// location 1 = vec4 colour, location 2 = vec2 texture coordinate.
// Change one and the other in the same commit or the draw reads
// garbage.

layout(push_constant) uniform Matrices {
    mat4 view_projection;
    // Rows 0..2 of the light's affine view-projection. Row 3 is
    // (0, 0, 0, 1) and is not sent.
    vec4 light_row_0;
    vec4 light_row_1;
    vec4 light_row_2;
    // How brightly the whole scene is lit, multiplied into every
    // colour — the same meaning, and the same stage, as the unshadowed
    // camera path's, so a world drawn half by each dims alike.
    //
    // Alpha IS multiplied, along with the other three; it is a no-op
    // only because the host pins the slot to one. That pin is load
    // bearing rather than tidy: a light that dimmed alpha would dissolve
    // cutout geometry as a scene darkened, since the cutout stage
    // thresholds on the interpolated alpha.
    vec4 light;
} matrices;

layout(location = 0) in vec3 vertex_position;
layout(location = 1) in vec4 vertex_colour;
layout(location = 2) in vec2 vertex_uv;

layout(location = 0) out vec4 fragment_colour;
// See mesh_camera_textured.vert: `w` is linear in view distance, depth
// is not, so the fade fraction is computed here.
layout(location = 1) out float fragment_fade;
layout(location = 2) out vec2 fragment_uv;
// This vertex in the light's clip space, interpolated so the fragment
// stage can compare its own light-depth against the shadow map's.
layout(location = 3) out vec4 fragment_light_position;

void main() {
    vec4 world = vec4(vertex_position, 1.0);
    gl_Position = matrices.view_projection * world;
    // Multiplied here rather than in the fragment stage, which is where
    // the unshadowed camera path applies it too — a uniformly scaled
    // colour interpolates to the uniformly scaled interpolation, so the
    // two orders agree and the cheaper one wins.
    fragment_colour = vertex_colour * matrices.light;
    fragment_uv = vertex_uv;
    // The dropped row is (0, 0, 0, 1), so w is one by construction —
    // written, not computed. **The caster computes this same
    // expression**, and the two must stay the same expression: the
    // fragment stage compares this z against the depth that one
    // rasterized, within a constant bias.
    fragment_light_position = vec4(
        dot(matrices.light_row_0, world),
        dot(matrices.light_row_1, world),
        dot(matrices.light_row_2, world),
        1.0
    );
    // The same constant as every camera path: two pipelines drawing
    // one world must fade alike or the seam shows.
    const float FADE_DISTANCE = 48.0;
    fragment_fade = clamp(gl_Position.w / FADE_DISTANCE, 0.0, 1.0);
}
