# Shaders

GLSL sources are the truth; the `.spv` files beside them are the exact
bytes the engine embeds and the golden tests attest. Source and blob
change together in one commit — one without the other is a review
reject.

## Compile record (provenance)

Compiled 2026-07-30 with `glslc` from the pinned Vulkan SDK 1.4.328.1:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O triangle.vert -o triangle.vert.spv
> glslc -O triangle.frag -o triangle.frag.spv
```

`instanced.vert` and `instanced.frag` were compiled 2026-08-03, version
output observed again rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

> glslc -O instanced.vert -o instanced.vert.spv
> glslc -O instanced.frag -o instanced.frag.spv
```

`textured.vert` and `textured.frag` were compiled 2026-08-01, with the
version output observed again rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O textured.vert -o textured.vert.spv
> glslc -O textured.frag -o textured.frag.spv
```

`instanced_depth.vert` and `instanced_depth.frag` were compiled
2026-08-04, with the version output observed again rather than assumed
unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O instanced_depth.vert -o instanced_depth.vert.spv
> glslc -O instanced_depth.frag -o instanced_depth.frag.spv
```

`mesh.vert` and `mesh.frag` were compiled 2026-08-05, with the version
output observed again rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O mesh.vert -o mesh.vert.spv
> glslc -O mesh.frag -o mesh.frag.spv
```

To recompile: install the same SDK version, run the same commands, and
update this record with the observed `--version` output in the same
commit as the new bytes.

**Run them from this directory.** `glslc` embeds the source path it was
given into the module's debug information, so `glslc -O instanced.vert`
and `glslc -O crates/rhi/shaders/instanced.vert` produce **different
bytes from identical source**. Every command recorded above is a bare
filename, which only works from here. Discovered 2026-08-05 while
checking that a comment-only edit left the blobs untouched: compiled from
the repository root all three appeared to have changed, and compiled from
this directory all three were byte-identical to what is committed. A
recompile from the wrong directory produces a blob diff that looks like a
real change and is not.

**Comment-only edits do not change the bytes**, verified the same way —
so a header comment may be corrected without touching the `.spv` beside
it, and the source-and-blob-together rule is satisfied by the blob
legitimately not moving rather than by skipping it.

## mesh_camera.vert and mesh_camera.frag — compiled 2026-08-06

Same SDK and the same working directory rule as the entries above (glslc
embeds the source path, so it is run from this directory):

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O mesh_camera.vert -o mesh_camera.vert.spv
> glslc -O mesh_camera.frag -o mesh_camera.frag.spv
```

1228 bytes and 748 bytes. Both recompiled on 2026-08-06 and compared
against the committed blobs: byte-identical.

**A fragment stage of its own, not `mesh.frag.spv`.** The camera path
fades a fragment toward the horizon with distance, so it does not merely
pass the interpolated colour through and cannot share the plain path's
binary. The vertex stage supplies the distance, because `w` is linear in
it and depth is not.

### The correction this entry is

An earlier version of this section recorded **1024 bytes** and said the
path *"reuses `mesh.frag.spv` unchanged"*. Both were false by the time
they were committed: the vertex source changed after the number was
written, and the fragment stage was added in the same commit without
touching this file. The `--version` transcript was two lines short of
what the tool prints, and the SDK path carried a stray control byte where
`\1` belonged, so the recorded command was not runnable as written.

It is recorded rather than quietly overwritten because the numbers here
exist to be checked, and a reader who finds one wrong is entitled to know
whether the file has been wrong before. The rule at the top of this file
— source and blob change together in one commit — is what failed, and it
failed in the direction the rule exists to prevent: the blob moved and
its record did not.

## mesh.vert and mesh_camera.vert — recompiled 2026-08-06, for the texture coordinate

The per-vertex record gained a `vec2` at location 2. Both vertex shaders declare it; neither
consumes it yet.

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O mesh.vert -o mesh.vert.spv
> glslc -O mesh_camera.vert -o mesh_camera.vert.spv
```

**`mesh.vert.spv` did not move**, and that is the comment-only case this file already allows: the
new input is unused, so the optimiser drops it and the bytes are what they were. 752 bytes,
unchanged.

**`mesh_camera.vert.spv` did move**, at the same 1228 bytes, and the reason is the one thing about
this change worth reading twice. Locations are **one space across both bindings, per-vertex
first**. A third per-vertex attribute therefore pushes every per-instance attribute up by one, so
the camera matrix moved from locations 2..=5 to 3..=6. Nothing in the Rust says those numbers — they
are computed from the layout — so the GLSL is the only place they are written down, and a shader
left at 2..=5 would read the matrix from whatever the vertex stream happened to have there.

That failure produces **a picture**, not an error: a wrong matrix draws a wrong world perfectly. The
rendering crate's camera oracles are what catch it, and they were run against a real adapter before
this record was written.

## mesh_camera_textured.vert and .frag - compiled 2026-08-06

The camera mesh path with a texture: the same geometry and matrix as
`mesh_camera.vert`, plus the coordinate the fragment stage samples with,
and a combined image sampler at set 0, binding 0.

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O mesh_camera_textured.vert -o mesh_camera_textured.vert.spv
> glslc -O mesh_camera_textured.frag -o mesh_camera_textured.frag.spv
```

1376 bytes and 1072 bytes, both recompiled from a clean source tree and
compared against the committed blobs on 2026-08-06.

**A second pair rather than a branch in the first.** The two pipelines
differ in what they bind, not only in what they compute: this one carries
a descriptor set and the plain pair does not. A uniform choosing between
them would cost a fetch and a branch per fragment for a decision fixed
when the pipeline was built.

The fade constants are duplicated from `mesh_camera.frag` rather than
shared, because GLSL has no way to share them and two pipelines drawing
one world must fade alike or the seam between them shows. A test in the
rendering crate pins the horizon colour against its Rust constant; the
fade distance is pinned by nothing but this sentence.

## mesh_textured.vert and .frag - compiled 2026-08-06

The plain mesh path with a texture: clip-space positions drawn straight,
plus the coordinate the fragment stage samples with, and a combined image
sampler at set 0, binding 0.

```
> glslc -O mesh_textured.vert -o mesh_textured.vert.spv
> glslc -O mesh_textured.frag -o mesh_textured.frag.spv
```

Same toolchain and transcript as the entry above. 900 bytes and 856
bytes, both recompiled and compared against the committed blobs.

**No matrix and no fade here, unlike the camera pair.** These positions
are already clip space, so `w` is one everywhere and a fade computed from
it would be a constant tint. A caller that wants distance on this path has
projected the world itself and knows the distances it used.

## push_color.vert and .frag - compiled 2026-08-11

**Test fixtures, not builtins**: the smallest consumer of a vertex-stage
push-constant range, embedded by the device suite alone (`tests/device.rs`,
`tests/present_smoke.rs`) and deliberately absent from `builtin` — an
engine-facing export with no engine consumer would be dead public surface.
They live here so one directory carries every GLSL source and one record
attests every blob.

Version output observed again rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O push_color.vert -o push_color.vert.spv
> glslc -O push_color.frag -o push_color.frag.spv
```

1064 bytes and 324 bytes, the exact blobs the tests embed. The vertex
stage reads a sixteen-byte `push_constant` block (one vec4 color) and
covers the target with the classic oversized triangle from
`gl_VertexIndex`, so a frame's every pixel answers with exactly the bytes
that frame pushed — which is what makes the device test's oracle a
readback comparison rather than a smoke call.

## mesh_camera.vert, mesh_camera_textured.vert - recompiled 2026-08-11

The camera pairs' vertex stages: the matrix leaves the per-instance
stream (binding 1, locations 3..=6, deleted) and arrives as a
sixty-four-byte `push_constant` block. A GLSL `mat4` inside a push block
is column-major by default, so the byte order the pack type writes is
unchanged. Version output observed again rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O mesh_camera.vert -o mesh_camera.vert.spv
> glslc -O mesh_camera.frag -o mesh_camera.frag.spv
> glslc -O mesh_camera_textured.vert -o mesh_camera_textured.vert.spv
```

1156 bytes and 1304 bytes for the two vertex stages. `mesh_camera.frag`
was recompiled in the same run for a comment-only source edit and its
blob came out **byte-identical** (compared with `cmp` against the
committed bytes before the edit) — which is the expected result stated
so nobody re-derives whether comments reach SPIR-V. The arithmetic in
all three stages is untouched: the same `mat4 * vec4` multiply reading
the same sixty-four bytes from a different channel, which is what the
byte-compared renders downstream attest.

## particle.vert and .frag - compiled 2026-08-11

The particle billboard: six generated vertices per instance expanded
from the camera's right and up (pushed beside the matrix in one
ninety-six-byte block), a forty-eight-byte instance stream at binding 1
(centre and size, premultiplied colour, atlas rectangle), and a fragment
stage that multiplies the atlas texel by the instance colour. Version
output observed again rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O particle.vert -o particle.vert.spv
> glslc -O particle.frag -o particle.frag.spv
```

1992 bytes and 564 bytes, the exact blobs `builtin` embeds.

## textured_pair.frag - compiled 2026-08-11

The two-slot fragment stage: the canonical single-binding set layout
repeated at set 0 and set 1, the left half of the target sampling
slot 0 and the right half slot 1 — so a wrong bind order shows as a
visibly wrong image. It shares `textured.vert`, whose blob is
untouched. Version output observed again rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O textured_pair.frag -o textured_pair.frag.spv
```

784 bytes, the exact blob `builtin` embeds.

## mesh_camera_shadow.vert and .frag - compiled 2026-08-11

The shadowed camera mesh pair: the textured pair's stages plus a
shadow term. The vertex stage carries TWO matrices in one 128-byte
push block (the camera's and the light's — exactly the guaranteed push
ceiling) and hands the fragment stage the vertex's light-space
position; the fragment stage samples the atlas at set 0 and the shadow
map at set 1, compares its own light depth against the map's under
reversed-Z with a constant bias (linear light depth — the light is
orthographic), and dims where something nearer was recorded. Fade
constants identical to the textured pair's, because two pipelines
drawing one world must fade alike. Version output observed again
rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O mesh_camera_shadow.vert -o mesh_camera_shadow.vert.spv
> glslc -O mesh_camera_shadow.frag -o mesh_camera_shadow.frag.spv
```

1484 bytes and 2148 bytes, the exact blobs `builtin` embedded at the
time. The shadow CASTER needed no new shader then: a depth-only pipeline
reused `mesh_camera.vert` unchanged, its colour output simply having no
consumer.

**Both statements were superseded on 2026-08-19** — the vertex stage was
recompiled when a scene light joined the block, and the caster gained a
stage of its own so that both halves read one record. See the dated
section at the end of this file.

`mesh_camera_cutout.frag` was compiled 2026-08-18, version output
observed again rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O mesh_camera_cutout.frag -o mesh_camera_cutout.frag.spv
```

1180 bytes, the exact blob `builtin` embeds. It needs no new vertex
stage: it shares `mesh_camera_textured.vert.spv`, because the two paths
differ only in what the fragment stage throws away. Fade constants are
the textured pair's, for the same reason the shadowed pair's are.

## uniform_tint.vert and .frag - compiled 2026-08-18

**Test fixtures, not builtins**, for the reason `push_color` is one: the
smallest consumer of a uniform block, embedded by the device suite alone
(`tests/device.rs`) and deliberately absent from `builtin`, where an
engine-facing export with no engine consumer would be dead public
surface.

The vertex stage covers the target with the classic oversized triangle
from `gl_VertexIndex` and reads nothing. The fragment stage reads a
**192-byte** `std140` block at set 0, binding 0 — eight `vec4` tints and
a `mat4` — and answers with `tints[int(gl_FragCoord.x) & 7]` scaled by
the matrix's last scalar.

**192 bytes, and the size is the point.** The guaranteed push-constant
ceiling is 128, so a readback that matches here could not have been
served by the channel that already existed. Every pixel reads a
different part of the block and the multiplier is its very last scalar,
so a block bound at the wrong offset, a slot stride the driver disagrees
with, or a copy that stopped short all show as a wrong image rather than
a plausible one.

Version output observed again rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O uniform_tint.vert -o uniform_tint.vert.spv
> glslc -O uniform_tint.frag -o uniform_tint.frag.spv
```

852 bytes and 880 bytes, the exact blobs the device suite embeds.

## uniform_tint_sampled.frag - compiled 2026-08-19

**A test fixture, and the only one that samples *and* reads a block.** A
uniform block is read at set `sampled_bindings`, after every sampled slot —
so a pipeline with one sampler finds its block at set 1. Every other test
declares zero sampled slots, which puts the block at set 0, where an
off-by-one in the set index, the layout list, the class check or the order
of `pDynamicOffsets` is the identity and cannot show.

It shares `textured.vert`, whose blob is untouched: the vertex stage needs
nothing from either channel. The atlas is sampled and the block scales it,
so a correct readback proves both arrived; a white atlas makes the expected
answer the block's own tints, byte for byte.

Version output observed again rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O uniform_tint_sampled.frag -o uniform_tint_sampled.frag.spv
```

1132 bytes, the exact blob the device suite embeds.

## mesh_camera_shadow.vert and mesh_camera_shadow_caster.vert - compiled 2026-08-19

**A scene light joins the shadowed path, and the caster gets a stage of its
own.** No renderer here could carry a light and a shadow at once: the lit
shadowed stage spent all 128 bytes — the guaranteed push ceiling — on two
matrices, and the naive union with a light colour is 144.

The sixteen bytes come from the light's matrix. Its projection is
orthographic and its view is rigid, so the product is affine and its bottom
row is exactly `(0, 0, 0, 1)`; carrying it would be carrying a constant. The
three rows that vary are three tight `vec4`s — 48 bytes — and both stages
write the fourth as a literal one. The block is now `mat4 view_projection`,
`vec4 light_row_0/1/2`, `vec4 light`: 64 + 48 + 16 = 128.

**`mat4x3` is the trap, and a test bans it.** That type looks like the same
saving and is not: std430 pads each of its four three-component columns back
to sixteen bytes, so it is 64 again and the block is 144 again, while a host
packing 48 would have the shader read padding. The saving comes from rows,
not from a narrower matrix type.

**The caster reads the same block**, where it used to reuse
`mesh_camera.vert` through a depth-only pipeline. One record for both halves
means the map cannot be written with one light and sampled with another —
and it turns a host-side row/column mistake from an invisible regression
into a loud one: with two encodings such a mistake moved the cast slightly
and the golden missed it; with one, every surface self-compares, the shadow
vanishes, and the golden that already existed refuses it. That was verified
by making the mistake on purpose and watching the test go red.

`mesh_camera_shadow.frag` is **untouched, byte for byte**: the light rides in
on `fragment_colour`, which it already multiplies, and
`fragment_light_position.w` interpolates a constant one, so its documented
no-op divide stays a no-op.

Version output observed again rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O mesh_camera_shadow.vert -o mesh_camera_shadow.vert.spv
> glslc -O mesh_camera_shadow_caster.vert -o mesh_camera_shadow_caster.vert.spv
```

1780 bytes and 1120 bytes, read off disk after compiling.

**The offsets were checked, not assumed.** The caster declares two members it
never reads — the camera matrix and the light — so that both stages share one
layout, and nothing in the tree previously proved that an entirely-unread
block member keeps its offset through `-O`. `spirv-dis` on both blobs reports
`Offset 0 / 64 / 80 / 96 / 112` for the five members, identically. Had they
moved, the fallback was an explicit `layout(offset = …)` in the caster.

The four camera fragment stages were recompiled 2026-08-19, when the horizon
they fade toward became a uniform block instead of a compiled-in constant.
Version output observed again rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O mesh_camera.frag -o mesh_camera.frag.spv
> glslc -O mesh_camera_textured.frag -o mesh_camera_textured.frag.spv
> glslc -O mesh_camera_cutout.frag -o mesh_camera_cutout.frag.spv
> glslc -O mesh_camera_shadow.frag -o mesh_camera_shadow.frag.spv
```

940, 1264, 1372 and 2340 bytes, read off disk after compiling — 192 bytes
larger apiece, which is the block declaration and the two loads from it.

**The pictures did not move.** `Air::CLEAR_BLACK` carries exactly the values
the four shaders used to compile in, so every golden that fixed a faded pixel
before this change still fixes the same pixel after it. That is the evidence
the arithmetic folds the same way through a uniform as it did through a
constant, which was the stated reason for leaving the constants compiled in.

## mesh_camera_shadow.frag - recompiled 2026-08-25

The shadow term softened: the single map tap became nine, one texel
apart, averaged. One tap draws a hard lit/shadowed boundary, and a hard
boundary aliases — wherever a shadow edge crosses geometry near the
map's own sampling scale it renders as a row of sawteeth marching along
the surface. Nine taps turn the same boundary into a two-texel
gradient. Deep in light and deep in shadow all nine agree, so every
pixel away from an edge reads exactly as before — which is why the
arithmetic assertions in the golden and allocation suites, whose probes
sit well inside each region, hold without being touched. Version output
observed again rather than assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O mesh_camera_shadow.frag -o mesh_camera_shadow.frag.spv
```

3016 bytes, read off disk after compiling.

## mesh_camera_textured.vert - recompiled 2026-08-25 (sway)

The per-renderer block grew a sway word set and its own opt-in flag,
and this stage learned to read them: vertices displace across the
ground plane by the vertex colour's alpha as bend weight, phase
travelling with world position. The flag, not the reach, is the
opt-in — a wind calming to zero must not flip what a mesh's alphas
mean — and a draw that never opted in has its position passed through
with no arithmetic against it, which is what makes the byte-identity
golden certain rather than probable. A swaying draw spends the weight
in this stage and hands the fragment stage alpha one, so the cutout
mask keeps its meaning at weightless roots. The four fragment stages
declare the block's first sixteen bytes only — a leading subset,
which per-stage descriptor validation accepts on both lanes — so
their blobs are untouched by the widening. Version output observed again rather than
assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O mesh_camera_textured.vert -o mesh_camera_textured.vert.spv
```

2524 bytes, read off disk after compiling.

## the three camera vertex stages - recompiled 2026-08-25 (fade distance)

The distance at which the fade completes moved out of the shaders
and into the per-renderer block: each vertex stage reads the third
word's y and falls back to the compiled forty-eight when it is zero,
so a caller that says nothing keeps its picture byte for byte - the
same defaulting argument the sway's flag word carried, held by the
golden that compares a silent air against an explicit zero. The
plain and shadowed stages declare the block for the first time (a
leading subset was already every fragment stage's shape); the
registry holds all three as rangers that read the word and never
touch the horizon. Version output observed again rather than
assumed unchanged:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O mesh_camera.vert -o mesh_camera.vert.spv
> glslc -O mesh_camera_textured.vert -o mesh_camera_textured.vert.spv
> glslc -O mesh_camera_shadow.vert -o mesh_camera_shadow.vert.spv
```

1548, 2608 and 2044 bytes, read off disk after compiling.
