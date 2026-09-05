# Shaders

GLSL sources are the truth; the `.spv` files beside them are the exact
bytes the crate embeds and the golden tests attest. Source and blob
change together in one commit — one without the other is a review
reject. Same ritual as the rendering crate's `shaders/` directory.

## Compile record (provenance)

Compiled 2026-09-05 with `glslc` from the pinned Vulkan SDK 1.4.328.1,
version output observed at compile time:

```
> C:\VulkanSDK\1.4.328.1\Bin\glslc.exe --version
shaderc v2023.8 v2025.3-10-gc7e73e8
spirv-tools v2025.4 v2022.4-970-g19042c89
glslang 11.1.0-1302-gd213562e

Target: SPIR-V 1.0

> glslc -O sprite.vert -o sprite.vert.spv
> glslc -O sprite.frag -o sprite.frag.spv
```

`sprite.vert.spv` (1704 bytes) and `sprite.frag.spv` (1340 bytes) were
both compiled that day from the sources beside them. Both grew: the
vertex stage gained the eighth instance attribute and the flat varying
that carries it, and the fragment stage gained the two effect lines.

To recompile: install the same SDK version, run the same commands, and
update this record with the observed `--version` output in the same
commit as the new bytes.
