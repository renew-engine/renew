# MoltenVK

Vulkan-on-Metal translation, so the engine's one Vulkan backend reaches Apple platforms
without a second renderer. Upstream: <https://github.com/KhronosGroup/MoltenVK>.

**Prebuilt binaries, committed on purpose.** They are what makes CI and a developer machine
link identical bytes, and what lets a clean clone build offline. Their provenance is
recorded here rather than in a build script, because a binary nobody can trace is a binary
nobody should link.

| file | slice | sha256 |
|---|---|---|
| `ios-arm64/libMoltenVK.dylib` | iOS device, arm64 | `6cd5888490d08b36356d04c50c05e2eda5b9bcb3d0338ece693a28987de96aff` |
| `ios-arm64_x86_64-simulator/libMoltenVK.dylib` | iOS simulator, arm64 + x86_64 | `c6027cfbc343e9595cd8b072aab9f587cde4f8f0a52ad6159dd5794769592e1d` |

Both extracted from the **v1.4.2** release, file `MoltenVK-all.tar`
(sha256 `562a15a29bc358446a56a4091c5f7e08f604184187c1d34f712148b61ef17276`), from
`MoltenVK/MoltenVK/dynamic/MoltenVK.xcframework/<slice>/MoltenVK.framework/MoltenVK`.

**Not from `MoltenVK-ios.tar`, and that is worth knowing before anyone updates these.** The
iOS release bundle carries only `ios-arm64` — device, no simulator, as its own xcframework
`Info.plist` states. Every iOS lane here runs on the simulator, so that bundle would ship
bytes that cannot run anywhere this project can execute iOS code. The simulator slice exists
only in the all-platforms bundle.

**Dynamic, not static, and that is a decision with a date.** The engine opens Vulkan by
dlopening `libvulkan.dylib`; a dynamic MoltenVK answers to that name with no change to how
any other platform obtains its entry points, while a static one must be linked at build time
through a different mechanism entirely. The App Store's objection to dylibs constrains a
shipped application, and this repository ships none — when it does, the static form returns
for it.

`LICENSE` is MoltenVK's own, Apache-2.0, carried because vendoring makes its notice
obligations this repository's to meet.

## Updating

Bump the version, re-extract both slices from `MoltenVK-all.tar`, replace the digests above,
and re-run the iOS lanes. A version bump moves bytes that link into a binary, so it is a
deliberate change and not a refresh.
