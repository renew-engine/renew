#version 450

// The camera mesh path with a texture and a shadow map: the textured
// pair's vertex stage, plus the light's view of the same vertex.
//
// **Two matrices in one push block.** The camera's takes the vertex to
// the screen; the light's takes it to the shadow map's clip space, so
// the fragment stage can ask "what depth did the light record where
// this fragment stands?". Both arrive as push constants — 128 bytes,
// exactly the guaranteed push ceiling — because both change with the
// viewpoint, not the instance.
//
// Layout here and the `VertexAttribute` slice at pipeline creation
// describe the same bytes: location 0 = vec3 position in world space,
// location 1 = vec4 colour, location 2 = vec2 texture coordinate.
// Change one and the other in the same commit or the draw reads
// garbage.

layout(push_constant) uniform Matrices {
    mat4 view_projection;
    mat4 light_view_projection;
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
    fragment_colour = vertex_colour;
    fragment_uv = vertex_uv;
    fragment_light_position = matrices.light_view_projection * world;
    // The same constant as every camera path: two pipelines drawing
    // one world must fade alike or the seam shows.
    const float FADE_DISTANCE = 48.0;
    fragment_fade = clamp(gl_Position.w / FADE_DISTANCE, 0.0, 1.0);
}
