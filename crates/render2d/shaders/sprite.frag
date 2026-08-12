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

layout(location = 0) out vec4 fragment_color;

void main() {
    vec4 authored = texture(atlas, vertex_uv);
    vec4 premultiplied = vec4(authored.rgb * authored.a, authored.a);
    fragment_color = premultiplied * vertex_tint;
}
