#version 450

// The camera mesh path with a texture: the same geometry and the same
// matrix as `mesh_camera.vert`, plus the coordinate the fragment stage
// samples with.
//
// **A second pair rather than a branch in the first.** A uniform saying
// "textured or not" would cost a fetch and a branch per fragment for a
// decision fixed when the pipeline was built, and the pipelines differ
// anyway: this one carries a descriptor set and the plain one does not.
//
// The matrix arrives as a push-constant block, exactly as in
// `mesh_camera.vert` — see that file for why it left the instance
// stream. Layout here and the `VertexAttribute` slice at pipeline
// creation describe the same bytes: binding 0 is location 0 = vec3
// position in world space, location 1 = vec4 colour, location 2 = vec2
// texture coordinate. No per-instance stream. Change one and the other
// in the same commit or the draw reads garbage.

layout(push_constant) uniform Camera {
    mat4 view_projection;
    // How brightly the whole scene is lit, multiplied into every colour.
    // White leaves a scene exactly as it was, which is what every caller
    // that does not care about light passes.
    //
    // **Here rather than in the fragment stage.** It is one value for the
    // whole draw, so applying it per vertex costs three multiplies per
    // vertex instead of three per fragment, and the interpolation of a
    // uniformly scaled colour is the uniformly scaled interpolation.
    vec4 light;
} camera;

// The per-renderer block every camera pipeline binds, in three words:
// the fade's, which the fragment stages read; the sway's, which only
// this stage does; and the sway's own opt-in flag. A stage may declare
// a leading subset of a block's members — validation is per stage —
// so the fragment shaders keep their sixteen-byte view of this buffer.
//
// **The sway, word by word.** `sway.xy` is how far a fully bent vertex
// is pushed across the ground plane at the top of the swing, in world
// units — direction and strength folded together by whoever owns the
// wind. `sway.z` is where the swing is in its cycle, in radians; the
// caller advances it with time. `sway.w` is radians of extra phase per
// world unit, which turns a field moving in lockstep into travelling
// waves. `bend.x` says whether this renderer's draws are swayers at
// all — the flag, not the reach, because a wind that calms to zero
// must not flip what a mesh's alphas mean (see Air::swaying). `bend.z`
// is where the weight rides: zero means the vertex's own alpha, spent
// below; nonzero is a per-draw even weight, for meshes whose alpha is
// spoken for — the blended pair reads it as translucency, and one
// channel cannot mean both (see Air::bending_evenly).
layout(std140, set = 1, binding = 0) uniform Air {
    vec4 horizon;
    vec4 sway;
    vec4 bend;
} air;

layout(location = 0) in vec3 vertex_position;
layout(location = 1) in vec4 vertex_colour;
layout(location = 2) in vec2 vertex_uv;

layout(location = 0) out vec4 fragment_colour;
// How far away this vertex is, as a fraction of the distance at which
// the fade is complete. Computed here rather than in the fragment stage
// because `w` is linear in view distance and depth is not -- see
// `mesh_camera.vert`, which learned it the hard way.
layout(location = 1) out float fragment_fade;
layout(location = 2) out vec2 fragment_uv;

void main() {
    // Whether this renderer's draws sway at all: the declared flag, not
    // the reach, so calm air bends nothing while every alpha keeps the
    // meaning its mesh was authored to (see Air::swaying). For a draw
    // that never opted in, the position below is passed through with no
    // arithmetic against it at all — the goldens hold this stage to
    // byte identity, and untouched input is how identity is certain
    // rather than probable.
    bool bent = air.bend.x != 0.0;
    // The vertex colour's alpha is the bend weight while a draw sways:
    // zero pins a vertex, one bends it the whole reach. Unless the air
    // carries an even weight — then every vertex bends by that word and
    // alpha keeps the meaning its mesh was authored to. The two phase
    // rates keep the crest line off both axes and the diagonal, so a
    // field reads as weather rather than as a scan.
    bool evenly = air.bend.z != 0.0;
    float weight = evenly ? air.bend.z : vertex_colour.a;
    float swing = sin(air.sway.z + (vertex_position.x + 0.7 * vertex_position.z) * air.sway.w);
    vec3 placed = bent
        ? vertex_position + vec3(air.sway.x, 0.0, air.sway.y) * (swing * weight)
        : vertex_position;
    gl_Position = camera.view_projection * vec4(placed, 1.0);
    // A swaying draw spends the weight here: the fragment stage sees
    // alpha one, so a cutout's mask and a blend keep their meaning.
    // A still draw's alpha keeps its old meaning untouched — and so
    // does an even swayer's, whose weight rode the air instead.
    fragment_colour =
        vec4(vertex_colour.rgb, bent && !evenly ? 1.0 : vertex_colour.a) * camera.light;
    fragment_uv = vertex_uv;
    // The distance at which the fade is complete, in world units: the
    // caller's word when one was given, the compiled forty-eight when
    // not. Only the caller knows how big its world is — this constant
    // was sized to one arena and then met a world half again wider.
    const float FADE_DISTANCE = 48.0;
    float fade_over = air.bend.y > 0.0 ? air.bend.y : FADE_DISTANCE;
    fragment_fade = clamp(gl_Position.w / fade_over, 0.0, 1.0);
}
