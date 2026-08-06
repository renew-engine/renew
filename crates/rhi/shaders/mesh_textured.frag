#version 450

// The plain mesh path's textured fragment stage: a sampled texel, tinted
// by the interpolated vertex colour.
//
// **The texture multiplies the colour rather than replacing it**, for the
// same reason as the camera path: the colour carries which way a face
// points and how enclosed its corners are, and a texel alone would draw
// an evenly lit world with a pattern on it.
//
// No fade here. This path's positions are already clip space, so there is
// no view distance to fade by -- see the vertex stage.
//
// The one descriptor this crate defines: a combined image sampler at set
// 0, binding 0, in the fragment stage.

layout(set = 0, binding = 0) uniform sampler2D atlas;

layout(location = 0) in vec4 fragment_colour;
layout(location = 1) in vec2 fragment_uv;

layout(location = 0) out vec4 out_colour;

void main() {
    vec4 texel = texture(atlas, fragment_uv);
    out_colour = vec4(texel.rgb * fragment_colour.rgb, texel.a * fragment_colour.a);
}
