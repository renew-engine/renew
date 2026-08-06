#version 450

// The mesh path with a camera: the same geometry `mesh.vert` draws, but
// transformed on the GPU by a matrix the caller supplies.
//
// **Why the matrix arrives as per-instance vertex input rather than as a
// push constant or a uniform.** Neither exists in this crate: there is no
// push-constant range anywhere, and the one descriptor set layout binds a
// combined image sampler to the fragment stage. Per-instance input at
// binding 1 does exist, is proven by the sprite path, and was left
// composable by the change that introduced per-vertex buffers precisely
// so a camera could ride it. So the matrix is four vec4 columns at
// locations 2 to 5, supplied once for a single instance.
//
// Column-major, matching `renew_math::Mat4`, which stores four Vec4
// columns in order. `mat4(c0, c1, c2, c3)` in GLSL takes columns too, so
// the bytes go across unchanged.
//
// The multiply is `matrix * position`, and `gl_Position` carries a real
// `w` — so the hardware performs the perspective divide and the clipper
// handles geometry behind the eye. That is the whole reason this exists:
// dividing on the CPU would mean clipping polygons against the near
// plane in the caller, because a triangle crossing w = 0 cannot be
// divided at all.
//
// Layout here and the `VertexAttribute` slices at pipeline creation
// describe the same bytes: binding 0 is location 0 = vec3 position
// (world space now, not clip), location 1 = vec4 colour; binding 1 is
// locations 2..=5, the matrix columns. Change one and the other in the
// same commit or the draw reads garbage.

layout(location = 0) in vec3 vertex_position;
layout(location = 1) in vec4 vertex_colour;

layout(location = 2) in vec4 view_projection_0;
layout(location = 3) in vec4 view_projection_1;
layout(location = 4) in vec4 view_projection_2;
layout(location = 5) in vec4 view_projection_3;

layout(location = 0) out vec4 fragment_colour;
// How far away this vertex is, as a fraction of the distance at which
// the fade is complete.
//
// **Computed here because `w` is linear in view distance and depth is
// not.** After a perspective projection `gl_Position.w` is the distance
// along the view direction, while `gl_FragCoord.z` is compressed toward
// the near plane -- with a near plane of a twentieth of a block, depth
// passes 0.99 within a few blocks and a fade driven by it turns the whole
// room to fog. That was not a guess; it was the first picture.
layout(location = 1) out float fragment_fade;

void main() {
    mat4 view_projection = mat4(
        view_projection_0,
        view_projection_1,
        view_projection_2,
        view_projection_3
    );
    gl_Position = view_projection * vec4(vertex_position, 1.0);
    fragment_colour = vertex_colour;
    // The distance at which the fade is complete, in world units. A
    // little over the arena's diagonal, so its far corner is faint
    // rather than lost.
    const float FADE_DISTANCE = 48.0;
    fragment_fade = clamp(gl_Position.w / FADE_DISTANCE, 0.0, 1.0);
}
