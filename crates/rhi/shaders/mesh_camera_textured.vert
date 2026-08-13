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
    gl_Position = camera.view_projection * vec4(vertex_position, 1.0);
    fragment_colour = vertex_colour * camera.light;
    fragment_uv = vertex_uv;
    // The distance at which the fade is complete, in world units. A
    // little over the arena's diagonal, so its far corner is faint
    // rather than lost. The same constant as the untextured path: two
    // pipelines drawing one world must fade alike or the seam shows.
    const float FADE_DISTANCE = 48.0;
    fragment_fade = clamp(gl_Position.w / FADE_DISTANCE, 0.0, 1.0);
}
