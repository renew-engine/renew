#version 450

// The one descriptor layout this crate defines: a combined image
// sampler at set 0, binding 0.

layout(set = 0, binding = 0) uniform sampler2D atlas;

layout(location = 0) in vec2 fragUv;
layout(location = 0) out vec4 outColor;

void main() {
    outColor = texture(atlas, fragUv);
}
