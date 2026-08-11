#version 450

// The particle billboard: six generated vertices per instance, expanded
// from the camera's own right and up so every quad faces the eye — the
// one thing a particle must do that a mesh never needs.
//
// The camera arrives as a push-constant block: the matrix, then the
// billboard basis as two vec4s (xyz used, w padding). Ninety-six bytes,
// under the guaranteed minimum, recorded per draw.
//
// The instance stream at binding 1 is forty-eight bytes: centre and
// size in one vec4, a premultiplied colour, and the atlas rectangle
// (min u, min v, max u, max v). Layout here and the `VertexAttribute`
// slice at pipeline creation describe the same bytes; change one and
// the other in the same commit or the draw reads garbage.

layout(push_constant) uniform Camera {
    mat4 view_projection;
    vec4 right;
    vec4 up;
} camera;

layout(location = 0) in vec4 centre_size;
layout(location = 1) in vec4 instance_colour;
layout(location = 2) in vec4 uv_rect;

layout(location = 0) out vec4 fragment_colour;
layout(location = 1) out vec2 fragment_uv;

void main() {
    // Two triangles over the unit square, from the vertex index.
    const vec2 corners[6] = vec2[](
        vec2(-0.5, -0.5),
        vec2(0.5, -0.5),
        vec2(0.5, 0.5),
        vec2(-0.5, -0.5),
        vec2(0.5, 0.5),
        vec2(-0.5, 0.5)
    );
    vec2 corner = corners[gl_VertexIndex];
    vec3 world = centre_size.xyz
        + (camera.right.xyz * corner.x + camera.up.xyz * corner.y) * centre_size.w;
    gl_Position = camera.view_projection * vec4(world, 1.0);
    fragment_colour = instance_colour;
    fragment_uv = mix(uv_rect.xy, uv_rect.zw, corner + 0.5);
}
