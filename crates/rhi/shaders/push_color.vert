#version 450

// Test fixture, not a builtin: the smallest consumer of a vertex-stage
// push-constant range, kept beside the engine shaders so one directory
// carries every source and one record attests every blob. One oversized
// triangle covers the whole target, and the pushed color rides to the
// fragment stage as a flat varying — so every pixel of a frame answers
// with exactly the bytes that frame pushed.

layout(push_constant) uniform Push {
    vec4 color;
} push;

layout(location = 0) out flat vec4 fragment_color;

void main() {
    // Three corners covering clip space: the classic full-target
    // triangle, so vertex_count is 3 and no vertex buffer exists.
    const vec2 corners[3] = vec2[](
        vec2(-1.0, -1.0),
        vec2(3.0, -1.0),
        vec2(-1.0, 3.0)
    );
    gl_Position = vec4(corners[gl_VertexIndex], 0.0, 1.0);
    fragment_color = push.color;
}
