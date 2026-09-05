#version 450

// The atlas holds **authored** colour: display-encoded bytes somebody
// chose by looking, with straight alpha. The hardware decodes on sample
// because the texture is sRGB, so what arrives here is linear
// reflectance with its alpha untouched.
//
// Premultiplying happens here rather than at authoring time, and that is
// the whole point of the arrangement. The transfer function does not
// commute with the alpha multiply, so bytes premultiplied before encoding
// cannot be decoded correctly by anything -- which left authored sprite
// colour with nothing to decode it, and opaque mid-tones came out lifted
// by exactly one encode. Decode first, multiply second, and both halves
// are right.
//
// The tint is premultiplied by its own convention and multiplies through
// unchanged; the pipeline's blend state does the compositing.

layout(set = 0, binding = 0) uniform sampler2D atlas;

layout(location = 0) in vec2 vertex_uv;
layout(location = 1) in vec4 vertex_tint;
layout(location = 2) flat in vec2 vertex_effect;
layout(location = 3) flat in vec2 vertex_smear;
layout(location = 4) flat in vec2 vertex_uv_lo;
layout(location = 5) flat in vec2 vertex_uv_hi;

layout(location = 0) out vec4 fragment_color;

// One premultiplied sample -- or nothing, when the tap left the sprite's
// own source rectangle.
//
// Refusing the outside tap rather than clamping it is what keeps a smear
// from dragging in whatever the atlas happens to hold next door. The
// half-open bounds match nearest sampling: a coordinate in the last
// texel's span is inside, and the coordinate exactly on the far edge is
// the first texel beyond.
vec4 tap(vec2 uv) {
    if (all(greaterThanEqual(uv, vertex_uv_lo)) && all(lessThan(uv, vertex_uv_hi))) {
        vec4 authored = textureLod(atlas, uv, 0.0);
        return vec4(authored.rgb * authored.a, authored.a);
    }
    return vec4(0.0);
}

void main() {
    vec4 premultiplied;
    if (vertex_smear == vec2(0.0)) {
        // The single-sample path: the same texel and the same two
        // multiplies as before smearing existed. `textureLod` at level
        // zero rather than `texture` because the atlas has one mip level
        // and a nearest mipmap mode, so both select the same texel --
        // and an explicit level is legal inside control flow, where an
        // implicit derivative is not.
        vec4 authored = textureLod(atlas, vertex_uv, 0.0);
        premultiplied = vec4(authored.rgb * authored.a, authored.a);
    } else {
        // Eight samples spread across the smear, centred on the pixel:
        // the offsets run from -1/2 to +1/2 of the vector, so the
        // average is the sprite's time-average over the displacement
        // rather than a trail hanging off one end.
        //
        // **The average is not eight copies at an eighth opacity.**
        // Eight over-composited layers at `a/8` reach about `0.66*a`;
        // the mean of eight premultiplied samples keeps the sprite's own
        // opacity where every tap is opaque and fades it in proportion
        // where the motion only passed through. A tap outside the source
        // contributes zero, which is what an average over a footprint
        // that reaches past the art means.
        vec4 t0 = tap(vertex_uv + vertex_smear * (0.0 / 7.0 - 0.5));
        vec4 t1 = tap(vertex_uv + vertex_smear * (1.0 / 7.0 - 0.5));
        vec4 t2 = tap(vertex_uv + vertex_smear * (2.0 / 7.0 - 0.5));
        vec4 t3 = tap(vertex_uv + vertex_smear * (3.0 / 7.0 - 0.5));
        vec4 t4 = tap(vertex_uv + vertex_smear * (4.0 / 7.0 - 0.5));
        vec4 t5 = tap(vertex_uv + vertex_smear * (5.0 / 7.0 - 0.5));
        vec4 t6 = tap(vertex_uv + vertex_smear * (6.0 / 7.0 - 0.5));
        vec4 t7 = tap(vertex_uv + vertex_smear * (7.0 / 7.0 - 0.5));
        // A TREE, not a running sum, and that is arithmetic rather than
        // taste: for eight identical taps every step here is a
        // power-of-two scale -- `t + t` is `2t`, `2t + 2t` is `4t`, and
        // `8t * 0.125` is `t` again -- so a pixel whose taps all land in
        // one solid region is the texel it would have been without any
        // smear, exactly, on every adapter. A running sum passes through
        // `3t`, `5t`, `6t` and `7t`, which round.
        premultiplied = (((t0 + t1) + (t2 + t3)) + ((t4 + t5) + (t6 + t7))) * 0.125;
    }
    // Toward grey, then toward a silhouette of the sprite's own alpha.
    //
    // Written as `x + (target - x) * k` rather than as `mix`, and that
    // is load-bearing rather than a style: `mix`'s evaluation form is
    // implementation-defined, and `x + (y - x) * 1` is not `y` in
    // fp32. These forms multiply by an *exact* zero at the identity
    // values -- `1.0 - 1.0` is `0.0`, `(luma - rgb) * 0.0` is a signed
    // zero, and `rgb + 0.0` is `rgb` for every non-negative colour, the
    // same under fused multiply-add. So a sprite that asks for neither
    // effect draws the pixel it drew before these lanes existed, by
    // construction rather than by a tolerance, which is what lets every
    // committed image carry over unchanged.
    //
    // The two operations COMMUTE, so the order below is a choice of
    // presentation rather than of arithmetic: the flash target is a
    // neutral grey, the luminance weights sum to one, so a grey's
    // luminance is itself and desaturating after flashing reaches the
    // same colour. Saturation is written first because that is the lane
    // order the record declares, and running the two in the order the
    // record lists them is one less thing to remember.
    float luma = dot(premultiplied.rgb, vec3(0.2126, 0.7152, 0.0722));
    vec3 rgb = premultiplied.rgb + (vec3(luma) - premultiplied.rgb) * (1.0 - vertex_effect.x);
    // The flash target is the alpha, not white: a transparent texel
    // stays transparent, so a full flash is a silhouette that fades
    // with the sprite instead of squaring off its edges.
    rgb = rgb + (vec3(premultiplied.a) - rgb) * vertex_effect.y;
    fragment_color = vec4(rgb, premultiplied.a) * vertex_tint;
}
