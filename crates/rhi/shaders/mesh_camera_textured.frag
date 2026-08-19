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

// How much of the horizon shows at the far plane. Short of one, so
// geometry at the very back stays faintly visible rather than vanishing
// into the backdrop, and a room's far wall still reads as a wall.
const float MAX_FADE = 0.72;

// What distance fades toward. The same colour the samples clear to, so
// the fade reads as depth rather than as a grey wash. Identical to the
// untextured path's: two pipelines drawing one world must agree.
// What distance fades toward, supplied per frame.
//
// **A block, not a push constant, and not because of space.** This engine
// declares its push range for the vertex stage alone, so a fragment
// shader cannot read one at all — which is the whole reason this value
// was a compiled-in constant for as long as it was. A uniform block is
// visible to both stages and is the only channel that reaches here.
//
// `w` is unused. std140 rounds a `vec3` to sixteen bytes regardless, so
// the padding exists either way and a named spare is honester than a
// silent one.
layout(std140, set = 1, binding = 0) uniform Fade {
    vec4 horizon;
} fade;

void main() {
    vec4 texel = texture(atlas, fragment_uv);
    vec3 surface = texel.rgb * fragment_colour.rgb;
    out_colour = vec4(
        mix(surface, fade.horizon.rgb, fragment_fade * MAX_FADE),
        fragment_colour.a * texel.a
    );
}
