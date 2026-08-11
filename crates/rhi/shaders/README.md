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
