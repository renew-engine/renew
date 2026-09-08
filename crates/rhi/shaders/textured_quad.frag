#version 450

// Four sampled slots on one pipeline: the two-slot layout carried to the
// ceiling this tree declares. Vulkan guarantees `maxBoundDescriptorSets`
// is at least four, so a pipeline shaped like this is one every
// conformant adapter accepts, and it is the widest a material can be
// without a sampler array.
//
// The target splits into quadrants rather than halves, so a bind order
// that is wrong in any one of the four places is a visibly wrong image
// rather than a plausible one: each slot owns a corner nothing else
// draws.
//
// This is the shape a normal-mapped material takes -- base colour,
// normal, metallic-roughness, occlusion -- and it exists to prove the
// pipeline layout reaches that far before anything depends on it.

layout(set = 0, binding = 0) uniform sampler2D lower_left;
layout(set = 1, binding = 0) uniform sampler2D lower_right;
layout(set = 2, binding = 0) uniform sampler2D upper_left;
layout(set = 3, binding = 0) uniform sampler2D upper_right;

layout(location = 0) in vec2 fragUv;
layout(location = 0) out vec4 outColor;

void main() {
    if (fragUv.y < 0.5) {
        outColor = fragUv.x < 0.5 ? texture(lower_left, fragUv)
                                  : texture(lower_right, fragUv);
    } else {
        outColor = fragUv.x < 0.5 ? texture(upper_left, fragUv)
                                  : texture(upper_right, fragUv);
    }
}
