#version 450

// The textured camera fragment stage with a shadow term: the sampled
// texel, tinted by the vertex colour, dimmed where the light recorded
// something nearer, faded toward the horizon like every camera path.
//
// Two sampled slots, one per set — the canonical single-binding layout
// repeated: set 0 is the atlas, set 1 the shadow map a depth-only pass
// rendered from the light this frame.
//
// **The shadow test is reversed-Z, like everything else.** The map
// holds the depth of the surface NEAREST the light — the largest value
// under the engine's convention — so a fragment is lit exactly when
// its own light-space depth reaches what the map recorded, within a
// bias. The light's projection is orthographic, which makes light
// depth LINEAR in distance, so one small constant bias covers the
// whole box instead of growing with it.

layout(set = 0, binding = 0) uniform sampler2D atlas;
layout(set = 1, binding = 0) uniform sampler2D shadow_map;

layout(location = 0) in vec4 fragment_colour;
layout(location = 1) in float fragment_fade;
layout(location = 2) in vec2 fragment_uv;
layout(location = 3) in vec4 fragment_light_position;

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
// The same block mesh_camera_textured.frag reads, for the same reason:
// two pipelines drawing one world must fade alike or the seam shows.
layout(std140, set = 2, binding = 0) uniform Air {
    vec4 horizon;
} air;

// How much surface survives in shadow. Well above zero: a shadow is a
// dimming of a lit world, not a hole in it.
const float SHADOW_DIM = 0.55;

// Depth slack for the self-comparison, in light NDC units — linear in
// world distance under the orthographic light, so it means the same
// thing everywhere in the box.
const float BIAS = 0.003;

void main() {
    vec4 texel = texture(atlas, fragment_uv);
    vec3 surface = texel.rgb * fragment_colour.rgb;
    // The light's clip space, normalised. w is one under an
    // orthographic projection, but the divide keeps this stage honest
    // about what clip space is.
    vec3 light_ndc = fragment_light_position.xyz / fragment_light_position.w;
    // Clip x and y map straight to the map's uv — the same no-flip
    // convention the full-target quad documents: the first texture row
    // is the top row and clip y already points down.
    vec2 shadow_uv = light_ndc.xy * 0.5 + 0.5;
    float shade = 1.0;
    bool inside = all(greaterThanEqual(shadow_uv, vec2(0.0)))
        && all(lessThanEqual(shadow_uv, vec2(1.0)))
        && light_ndc.z >= 0.0
        && light_ndc.z <= 1.0;
    if (inside) {
        float nearest = texture(shadow_map, shadow_uv).r;
        // Reversed-Z: nearer is larger. Lit means this fragment is the
        // nearest thing the light sees here, within the bias.
        shade = light_ndc.z >= nearest - BIAS ? 1.0 : SHADOW_DIM;
    }
    out_colour = vec4(
        mix(surface * shade, air.horizon.rgb, fragment_fade * air.horizon.a),
        fragment_colour.a * texel.a
    );
}
