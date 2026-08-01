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

To recompile: install the same SDK version, run the same commands, and
update this record with the observed `--version` output in the same
commit as the new bytes.
