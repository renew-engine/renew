#version 450

// The camera path's fragment stage: the interpolated vertex colour, faded
// with distance toward a horizon colour.
//
// **Why fade at all.** Flat-shaded geometry with no lighting gives a
// viewer only one depth cue — the outline where two faces of different
// orientation meet. Inside a large room every wall is one flat colour, so
// a far wall and a near one look identical and the space reads as a paper
// cut-out rather than as a room. Distance fade is the cheapest cue that
// fixes it, and it is the one a first-person view most needs.
//
// **Why here and not in the vertices.** Colour is per-vertex on this
// path, so fading in the mesher would mean rebuilding the geometry every
// time the viewer moved — exactly what putting the camera matrix on the
// GPU was meant to stop. `gl_FragCoord.z` is the depth the hardware has
// already computed for the depth test, so this costs a mix.
//
// **It is not fog and does not pretend to be.** There is no density, no
// scattering, and no light. It is a readability aid with two constants.
// They arrive per draw through the uniform block below. They were
// compiled in until something needed them to vary, and something did:
// the colour has to match whatever the caller clears to, and a caller
// clearing to daylight got a fade toward near-black.
//
// Layout matches `mesh_camera.vert`: location 0 is the interpolated
// colour, location 1 the interpolated distance.

layout(location = 0) in vec4 fragment_colour;
layout(location = 1) in float fragment_fade;

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
layout(std140, set = 0, binding = 0) uniform Air {
    vec4 horizon;
} air;

void main() {
    out_colour = vec4(
        mix(fragment_colour.rgb, air.horizon.rgb, fragment_fade * air.horizon.a),
        fragment_colour.a
    );
}
