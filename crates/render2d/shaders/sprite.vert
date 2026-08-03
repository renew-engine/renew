#version 450

// The sprite quad: corners expanded from gl_VertexIndex (the house
// style -- no per-vertex buffer exists), everything else read per
// instance from the one bound vertex buffer at instance rate.
//
// Layout here and the `InstanceAttribute` slice at pipeline creation
// describe the same bytes: location 0/1 = vec2 NDC min/max, location
// 2/3 = vec2 UV min/max, location 4 = vec4 premultiplied tint. Change
// one and the other in the same commit or the draw reads garbage.

layout(location = 0) in vec2 instance_ndc_min;
layout(location = 1) in vec2 instance_ndc_max;
layout(location = 2) in vec2 instance_uv_min;
layout(location = 3) in vec2 instance_uv_max;
layout(location = 4) in vec4 instance_tint;

layout(location = 0) out vec2 vertex_uv;
layout(location = 1) out vec4 vertex_tint;

void main() {
    // Two triangles, four unique corners, six indices; per-corner
    // selectors mix min and max per axis. Same corner order as the
    // instanced builtin, so the winding story is one story.
    const vec2 select[6] = vec2[](
        vec2(0.0, 0.0),
        vec2(1.0, 0.0),
        vec2(0.0, 1.0),
        vec2(1.0, 0.0),
        vec2(1.0, 1.0),
        vec2(0.0, 1.0)
    );
    vec2 corner = select[gl_VertexIndex];
    gl_Position = vec4(mix(instance_ndc_min, instance_ndc_max, corner), 0.0, 1.0);
    vertex_uv = mix(instance_uv_min, instance_uv_max, corner);
    vertex_tint = instance_tint;
}
