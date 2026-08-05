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

## mesh_camera.vert — compiled 2026-08-06

Same SDK and the same working directory rule as the entries above (glslc
embeds the source path, so it is run from this directory):

```
> C:\VulkanSDK.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89

> glslc -O mesh_camera.vert -o mesh_camera.vert.spv
```

1024 bytes. It reuses `mesh.frag.spv` unchanged — the fragment stage only
passes the interpolated colour through, and a second identical binary
would be a second thing to keep in step.
