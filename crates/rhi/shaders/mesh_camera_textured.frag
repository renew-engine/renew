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
const vec3 HORIZON = vec3(0.09, 0.10, 0.13);

void main() {
    vec4 texel = texture(atlas, fragment_uv);
    vec3 surface = texel.rgb * fragment_colour.rgb;
    out_colour = vec4(
        mix(surface, HORIZON, fragment_fade * MAX_FADE),
        fragment_colour.a * texel.a
    );
}
