#version 450

// The sprite quad: corners expanded from gl_VertexIndex (the house
// style -- no per-vertex buffer exists), everything else read per
// instance from the one bound vertex buffer at instance rate.
//
// Layout here and the `VertexAttribute` slice declared as this
// pipeline's instance input describe the same bytes: locations 0 to 3 =
// the four corners in NDC -- local top-left, top-right, bottom-left,
// bottom-right, already turned, scaled and placed on the CPU --
// location 4/5 = vec2 UV at the first and the last corner, location 6
// = vec4 premultiplied tint, location 7 = vec2 effect (saturation,
// flash), location 8 = vec2 smear in UV units. Change one and the other
// in the same commit or the draw reads garbage.

layout(location = 0) in vec2 instance_corner_a;
layout(location = 1) in vec2 instance_corner_b;
layout(location = 2) in vec2 instance_corner_c;
layout(location = 3) in vec2 instance_corner_d;
layout(location = 4) in vec2 instance_uv0;
layout(location = 5) in vec2 instance_uv1;
layout(location = 6) in vec4 instance_tint;
layout(location = 7) in vec2 instance_effect;
layout(location = 8) in vec2 instance_smear;

layout(location = 0) out vec2 vertex_uv;
layout(location = 1) out vec4 vertex_tint;
// Flat: the pair is per instance, and interpolating two constants would
// be the same value computed the long way.
layout(location = 2) flat out vec2 vertex_effect;
// The smear, and the source's own bounds. Flat for the same reason the
// effect pair is: all three are per instance, and nothing attests them
// yet, so there is no interpolated value to preserve.
layout(location = 3) flat out vec2 vertex_smear;
layout(location = 4) flat out vec2 vertex_uv_lo;
layout(location = 5) flat out vec2 vertex_uv_hi;

void main() {
    // Two triangles, four unique corners, six indices; per-corner
    // selectors pick a corner per axis. Same corner order as the
    // instanced builtin, so the winding story is one story.
    const vec2 select[6] = vec2[](
        vec2(0.0, 0.0),
        vec2(1.0, 0.0),
        vec2(0.0, 1.0),
        vec2(1.0, 0.0),
        vec2(1.0, 1.0),
        vec2(0.0, 1.0)
    );
    vec2 corner = select[gl_VertexIndex];
    // A nested mix with 0/1 weights: along the top edge, along the
    // bottom edge, then between them. For an axis-aligned sprite the
    // top and bottom edges share x per corner and the left and right
    // share y, so each mix has either the operands of the previous
    // single mix or two equal operands -- the same arithmetic on the
    // same values under either way a driver evaluates mix.
    vec2 top = mix(instance_corner_a, instance_corner_b, corner.x);
    vec2 bottom = mix(instance_corner_c, instance_corner_d, corner.x);
    gl_Position = vec4(mix(top, bottom, corner.y), 0.0, 1.0);
    vertex_uv = mix(instance_uv0, instance_uv1, corner);
    vertex_tint = instance_tint;
    vertex_effect = instance_effect;
    vertex_smear = instance_smear;
    // The source's own bounds, recovered by undoing the extension the
    // packer applied: it grew the rectangle by half the smear on each
    // side, so half the smear back in is where the art actually ends.
    // The fragment stage refuses taps outside these, which is what
    // stops a smear reading a neighbour's texels.
    //
    // **Equal to the packer's own edge up to a rounding, not exactly.**
    // The packer scales the UV span by half the projected smear over the
    // drawn size; this undoes it by halving the smear already expressed
    // in UV. Same value, different order of operations, so the two can
    // land an ulp apart. It costs nothing here: the bound is a mask, the
    // sampler is nearest, and a texel spans many ulps of UV — a tap
    // would have to fall within an ulp of the source's edge for the
    // difference to change which side of the mask it lands on.
    vec2 half_smear = abs(instance_smear) * 0.5;
    vertex_uv_lo = min(instance_uv0, instance_uv1) + half_smear;
    vertex_uv_hi = max(instance_uv0, instance_uv1) - half_smear;
}
