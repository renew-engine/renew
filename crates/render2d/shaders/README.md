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

`sprite.vert.spv` (2092 bytes) and `sprite.frag.spv` (6404 bytes) were
both compiled that day from the sources beside them. Both grew: the
vertex stage gained the ninth instance attribute and the three flat
varyings that carry the smear and the source's own bounds, and the
fragment stage gained the eight-tap branch, which is most of the
fragment blob's new size — the tap is inlined eight times, each copy
carrying its own bounds test.

The eight taps are averaged by a **tree**, and the blob is where that is
checked: `spirv-dis sprite.frag.spv` shows seven `OpFAdd %v4float` whose
operands nest as `((t0+t1)+(t2+t3)) + ((t4+t5)+(t6+t7))`, never a
running sum. The optimiser is not permitted to reassociate float adds,
and on this compiler it does not.

To recompile: install the same SDK version, run the same commands, and
update this record with the observed `--version` output in the same
commit as the new bytes.
