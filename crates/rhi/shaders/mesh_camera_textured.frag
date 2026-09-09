#version 450

// The camera path's textured fragment stage: a sampled texel, tinted by
// the interpolated vertex colour, faded with distance toward a horizon.
//
// **The texture multiplies the colour rather than replacing it.** The
// colour carries two things the texture cannot: which way the face
// points, and how enclosed each of its corners is. Replacing it would
// throw away the shading that makes a room read as a room, and leave a
// world that is evenly lit and therefore flat again — with a pattern on
// it. They compose, and the order does not matter because both are
// multiplications.
//
// The one descriptor this crate defines: a combined image sampler at set
// 0, binding 0, in the fragment stage.

layout(set = 0, binding = 0) uniform sampler2D atlas;

layout(location = 0) in vec4 fragment_colour;
layout(location = 1) in float fragment_fade;
layout(location = 2) in vec2 fragment_uv;

layout(location = 0) out vec4 out_colour;

// **What distance fades toward, said by whoever is drawing.**
//
// It was a pair of compiled-in constants, the colour matched by hand to
// what this repository's own samples clear to. That made the fade correct
// for exactly one backdrop: a caller clearing to any other colour got a
// haze of the wrong one hanging in front of its sky, which reads as dirty
// glass rather than as distance. Only the caller knows what it clears to.
//
// `rgb` is that colour, in the same linear space this mix happens in.
// `a` is how much of it shows at the far plane — short of one, so
// geometry at the very back stays faintly visible rather than vanishing
// into the backdrop, and a room's far wall still reads as a wall.
//
// Every camera path reads the same block, so two pipelines drawing one
// world fade alike and no seam shows between them.
layout(std140, set = 1, binding = 0) uniform Air {
    vec4 horizon;
} air;

void main() {
    vec4 texel = texture(atlas, fragment_uv);
    vec3 surface = texel.rgb * fragment_colour.rgb;
    out_colour = vec4(
        mix(surface, air.horizon.rgb, fragment_fade * air.horizon.a),
        fragment_colour.a * texel.a
    );
}
