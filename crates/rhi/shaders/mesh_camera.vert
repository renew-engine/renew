#version 450

// The mesh path with a camera: the same geometry `mesh.vert` draws, but
// transformed on the GPU by a matrix the caller supplies.
//
// **The matrix arrives as a push-constant block.** It rode binding 1 as
// four per-instance vec4 columns before the push channel existed — a
// per-draw constant on a per-instance road, which pinned the instance
// count at one and held four attribute locations that real instancing
// wants. The push block is the channel built for exactly this: sixty-four
// bytes, vertex stage, recorded per draw, costing no buffer and no
// retention slot.
//
// Column-major, matching `renew_math::Mat4`, which stores four Vec4
// columns in order. A GLSL `mat4` inside a push-constant block is
// column-major by default, so the bytes go across unchanged — the same
// order the instance stream carried.
//
// The multiply is `matrix * position`, and `gl_Position` carries a real
// `w` — so the hardware performs the perspective divide and the clipper
// handles geometry behind the eye. That is the whole reason this exists:
// dividing on the CPU would mean clipping polygons against the near
// plane in the caller, because a triangle crossing w = 0 cannot be
// divided at all.
//
// Layout here and the `VertexAttribute` slice at pipeline creation
// describe the same bytes: binding 0 is location 0 = vec3 position
// (world space now, not clip), location 1 = vec4 colour, location 2 =
// vec2 texture coordinate. No per-instance stream — the pipeline
// declares none, and the block below is the sixty-four bytes its
// push-constant size names. Change one and the other in the same commit
// or the draw reads garbage.

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

layout(location = 0) in vec3 vertex_position;
layout(location = 1) in vec4 vertex_colour;
// Declared and unused here for the same reason as in `mesh.vert`: the
// record carries it, so the pipeline describes it.
layout(location = 2) in vec2 vertex_uv;

layout(location = 0) out vec4 fragment_colour;
// How far away this vertex is, as a fraction of the distance at which
// the fade is complete.
//
// **Computed here because `w` is linear in view distance and depth is
// not.** After a perspective projection `gl_Position.w` is the distance
// along the view direction, while `gl_FragCoord.z` spends nearly its
// whole value range beside the eye -- under the reversed mapping it
// falls from 1.0 to below 0.01 within a few blocks of a near plane a
// twentieth of a block away. A fade driven by depth therefore saturates
// almost immediately; driven the conventional way it turned the whole
// room to fog, and that was not a guess -- it was the first picture.
// The per-renderer block, for the one word this stage reads: how far
// away the fade completes. `bend.y` is that distance in world units;
// zero — every caller that never set it — selects the compiled
// forty-eight this stage always carried, so the silent caller's
// picture is untouched, byte for byte, by the exact argument the
// sway's flag word made: the arithmetic is identical when the select
// takes the constant. A stage may declare a leading subset of a
// block's members, so nothing else here changes.
layout(std140, set = 0, binding = 0) uniform Air {
    vec4 horizon;
    vec4 sway;
    vec4 bend;
} air;

layout(location = 1) out float fragment_fade;

void main() {
    gl_Position = camera.view_projection * vec4(vertex_position, 1.0);
    fragment_colour = (vertex_colour) * camera.light;
    // The distance at which the fade is complete, in world units: the
    // caller's word when one was given, the compiled forty-eight when
    // not. Only the caller knows how big its world is — this constant
    // was sized to one arena and then met a world half again wider.
    const float FADE_DISTANCE = 48.0;
    float fade_over = air.bend.y > 0.0 ? air.bend.y : FADE_DISTANCE;
    fragment_fade = clamp(gl_Position.w / fade_over, 0.0, 1.0);
}
