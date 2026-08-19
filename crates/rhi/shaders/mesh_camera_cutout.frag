#version 450

// The camera path's cutout fragment stage: the textured stage, plus a
// fragment that is thrown away where the texture says there is nothing.
//
// **Why a second stage rather than blending.** A texture with holes in it
// — foliage, a grate, a sprite standing in the world — drew as a solid
// rectangle on the textured path, because that pipeline replaces the
// target wherever a fragment lands and writes depth while doing it. So
// the hole was opaque and it also hid whatever stood behind it.
//
// Blending would fix the colour and not the depth: a transparent fragment
// that still writes depth occludes what comes after it, and getting that
// right needs the draws sorted back to front, which is a cost every
// consumer would pay for the sake of textures that are mostly binary
// anyway. Discarding costs nothing, needs no sorting, and is exactly
// right for the common case — a texel is either there or it is not.
//
// What this is not: partial transparency. A texel at half alpha is kept
// whole, not blended. Glass wants the blended path and a caller willing
// to sort.

layout(set = 0, binding = 0) uniform sampler2D atlas;

layout(location = 0) in vec4 fragment_colour;
layout(location = 1) in float fragment_fade;
layout(location = 2) in vec2 fragment_uv;

layout(location = 0) out vec4 out_colour;

// How much of the horizon shows at the far plane. Short of one, so
// geometry at the very back stays faintly visible rather than vanishing
// into the backdrop. Identical to the textured and untextured paths':
// pipelines drawing one world must fade alike or the seam shows.
const float MAX_FADE = 0.72;

// What distance fades toward, matching the other camera paths exactly.
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

// Below this, the fragment is not drawn at all.
//
// **Half, and the reason it is not lower.** The test is against the
// texel's own alpha multiplied by the vertex colour's, so a caller can
// fade a whole draw out and have it vanish rather than pop. Half is the
// midpoint of that range: an authored cutout is nearly always fully
// opaque or fully clear, and a threshold in the middle is the furthest
// from both, which makes it the least sensitive to a texel that has been
// resampled or premultiplied on the way in.
const float KEEP_ABOVE = 0.5;

void main() {
    vec4 texel = texture(atlas, fragment_uv);
    float alpha = fragment_colour.a * texel.a;
    if (alpha < KEEP_ABOVE) {
        discard;
    }
    vec3 surface = texel.rgb * fragment_colour.rgb;
    // Opaque out: what survives the cut is drawn whole. The alpha is not
    // carried through, because this pipeline does not blend and a value
    // nothing reads is a value that will one day be read wrongly.
    out_colour = vec4(mix(surface, fade.horizon.rgb, fragment_fade * MAX_FADE), 1.0);
}
