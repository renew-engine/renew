#version 450

// The camera path's blended fragment stage: the textured stage, with
// the output premultiplied by its own alpha so the pipeline's
// `src + dst * (1 - src.a)` composites what the numbers claim.
//
// **Why premultiplication happens here and not in the caller.** The
// blend equation this stage feeds assumes source colour already scaled
// by source alpha; the textured stage's output is not, because opaque
// and cutout targets ignore alpha entirely and scaling there would dim
// every solid surface. A translucent draw whose vertex alpha is a half
// must land as half its colour, and asking every consumer to
// pre-scale vertex colours by vertex alpha is the kind of convention
// that is right until one caller forgets — one multiply at the end of
// this stage makes the contract arithmetic instead of discipline.
//
// **What this is not: sorted.** Blending is order-dependent by its
// equation; the pipeline composites honestly and the caller owes the
// order, back to front, which the pairing's docs say in bold. Depth is
// tested and not written by the pipeline this stage serves, so
// translucent geometry respects the opaque world and never occludes
// what is drawn after it.

layout(set = 0, binding = 0) uniform sampler2D atlas;

layout(location = 0) in vec4 fragment_colour;
layout(location = 1) in float fragment_fade;
layout(location = 2) in vec2 fragment_uv;

layout(location = 0) out vec4 out_colour;

// The same block every other camera path reads, for the same fade —
// see mesh_camera_textured.frag, which carries the reasoning.
layout(std140, set = 1, binding = 0) uniform Air {
    vec4 horizon;
} air;

void main() {
    vec4 texel = texture(atlas, fragment_uv);
    vec3 surface = texel.rgb * fragment_colour.rgb;
    float alpha = fragment_colour.a * texel.a;
    vec3 faded = mix(surface, air.horizon.rgb, fragment_fade * air.horizon.a);
    out_colour = vec4(faded * alpha, alpha);
}
