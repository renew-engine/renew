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

// See mesh_camera_textured.frag; identical, because two pipelines
// drawing one world must fade alike or the seam shows.
const float MAX_FADE = 0.72;
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
layout(std140, set = 2, binding = 0) uniform Fade {
    vec4 horizon;
} fade;

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
        mix(surface * shade, fade.horizon.rgb, fragment_fade * MAX_FADE),
        fragment_colour.a * texel.a
    );
}
