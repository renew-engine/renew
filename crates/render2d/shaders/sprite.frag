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

layout(location = 0) out vec4 fragment_color;

void main() {
    vec4 authored = texture(atlas, vertex_uv);
    vec4 premultiplied = vec4(authored.rgb * authored.a, authored.a);
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
