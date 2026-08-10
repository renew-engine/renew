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
// They stay compiled in deliberately, even now that a per-draw channel
// exists: the push block is vertex-stage and this constant folds in the
// fragment stage, and moving where the arithmetic folds changes its
// floating-point result — the committed pictures pin the arithmetic as
// it is, so the constants move only when something needs them to vary.
//
// Layout matches `mesh_camera.vert`: location 0 is the interpolated
// colour, location 1 the interpolated distance.

layout(location = 0) in vec4 fragment_colour;
layout(location = 1) in float fragment_fade;

layout(location = 0) out vec4 out_colour;

// How much of the horizon shows at the far plane. Short of one, so
// geometry at the very back stays faintly visible rather than vanishing
// into the backdrop, and a room's far wall still reads as a wall.
const float MAX_FADE = 0.72;

// What distance fades toward. The same colour the samples clear to, so
// the fade reads as depth rather than as a grey wash — a fade toward some
// other colour would look like haze sitting in front of the backdrop.
const vec3 HORIZON = vec3(0.09, 0.10, 0.13);

void main() {
    out_colour = vec4(
        mix(fragment_colour.rgb, HORIZON, fragment_fade * MAX_FADE),
        fragment_colour.a
    );
}
