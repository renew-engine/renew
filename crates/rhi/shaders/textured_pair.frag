#version 450

// Two sampled slots on one pipeline: the canonical single-binding
// layout repeated, one set per slot. The left half of the target reads
// slot 0 and the right half slot 1, so a wrong bind order is a visibly
// wrong image rather than a plausible one.

layout(set = 0, binding = 0) uniform sampler2D left;
layout(set = 1, binding = 0) uniform sampler2D right;

layout(location = 0) in vec2 fragUv;
layout(location = 0) out vec4 outColor;

void main() {
    if (fragUv.x < 0.5) {
        outColor = texture(left, fragUv);
    } else {
        outColor = texture(right, fragUv);
    }
}
