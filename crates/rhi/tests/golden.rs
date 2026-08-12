//! Golden-image tests: rendering correctness as bytes.
//!
//! G1 (clear-exact) runs on every conformant adapter — float-to-UNORM
//! conversion is specified, so a cleared target has one right answer.
//! G2 (triangle) makes structural assertions everywhere and an exact
//! byte-for-byte comparison against the committed golden only on a
//! software rasterizer, where rasterization is pinned by the CI
//! toolchain pin rather than GPU/driver variance.
//!
//! Bootstrap ritual: when the golden artifact is missing on a software
//! rasterizer, the test writes a CANDIDATE file (never the canonical
//! name) plus a provenance sidecar and FAILS — a golden enters the tree
//! only through a human inspecting the candidate and committing it
//! under the canonical name. Re-running without that human step keeps
//! failing; nothing can pass against an uninspected file. A refresh is
//! the same ritual with the old artifact deleted first.

// The tripwire ban on filesystem access protects engine code; the
// golden harness's entire job is comparing against committed artifacts.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use renew_rhi::{
    AdapterKind, Attachment, BindingDesc, BindingSource, Blend, ClearValue, Color, DepthState,
    Device, DeviceDesc, DeviceError, Extent, Item, LoadOp, MeshDesc, Pass, PipelineDesc,
    RenderDesc, RenderImageDesc, RenderImageKind, SamplerDesc, StoreOp, TargetFormat, TextureDesc,
    Validation, builtin,
};

/// The one color attachment these frames render into: cleared, stored.
fn clear(color: Color) -> [Attachment; 1] {
    [Attachment::new(
        LoadOp::Clear(ClearValue::Color(color)),
        StoreOp::Store,
    )]
}

fn strict() -> bool {
    std::env::var_os("RENEW_GOLDEN").is_some_and(|v| v == "1")
}

/// `Ok(None)` is the graceful skip; other failures surface as `Err`
/// for the calling test to unwrap (test-only panics live in `#[test]`
/// bodies, where the lint allowance applies). Under `RENEW_GOLDEN=1`
/// (the CI rendering lane) a skip is a failure, and the validation
/// layer must actually be active — the lane's oracle can never go
/// silently vacuous.
fn device_or_skip() -> Result<Option<Device>, DeviceError> {
    match Device::new(&DeviceDesc {
        app_name: "renew-rhi-golden-tests",
        validation: Validation::IfAvailable,
    }) {
        Ok(device) => {
            assert!(
                device.validation_active() || !strict(),
                "RENEW_GOLDEN=1 but the validation layer is not active — \
                 the rendering lane's oracle would be vacuous"
            );
            Ok(Some(device))
        }
        Err(DeviceError::LoaderUnavailable { message }) if !strict() => {
            eprintln!("SKIP: no Vulkan runtime: {message}");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn assert_no_validation_errors(device: &Device) {
    let report = device.validation_report();
    assert_eq!(
        report.errors, 0,
        "validation errors; first messages: {:?}",
        report.first_messages
    );
}

fn goldens_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")
}

/// FNV-1a 64 over a byte buffer: a cheap content fingerprint for
/// forensics lines and sidecars.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Write RGBA8 pixels as a binary PPM (P6, alpha dropped) beside the
/// goldens — the humanly-viewable form of a mismatch or candidate.
fn write_ppm(path: &Path, pixels: &[u8], width: u32, height: u32) -> std::io::Result<()> {
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    for pixel in pixels.chunks_exact(4) {
        ppm.extend_from_slice(&pixel[..3]);
    }
    std::fs::write(path, ppm)
}

/// G1: a cleared target holds exactly the specified conversion of the
/// clear color, in every pixel.
#[test]
fn clear_is_byte_exact_everywhere() {
    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let mut target = device
        .create_offscreen_target(Extent {
            width: 64,
            height: 64,
        })
        .expect("offscreen target");
    // 51/255, 102/255, 153/255: unambiguous UNORM conversions.
    let color = clear(Color::new(51.0 / 255.0, 102.0 / 255.0, 153.0 / 255.0, 1.0));
    target
        .render(&RenderDesc::new(&[Pass::new(&color, &[])]))
        .expect("clear render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    let expected = [51u8, 102, 153, 255];
    for (index, pixel) in pixels.chunks_exact(4).enumerate() {
        assert_eq!(
            pixel,
            expected,
            "pixel {index} diverged on adapter {:?}",
            device.adapter()
        );
    }
    // Teardown first, oracle second: destruction-time findings count.
    drop(target);
    assert_no_validation_errors(&device);
}

/// G2: the built-in triangle — structure everywhere, exact bytes on a
/// software rasterizer.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "render, structural checks, and the bootstrap ritual are one narrative"
)]
fn triangle_matches_structure_and_the_committed_golden() {
    const SIZE: u32 = 256;
    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");
    // `Blend::Opaque` spelled out where the default normally stands:
    // the committed golden below is the proof that the explicit variant
    // and the default are the same bytes.
    let pipeline = device
        .create_pipeline(
            &PipelineDesc::new(builtin::TRIANGLE, TargetFormat::Rgba8Srgb).blend(Blend::Opaque),
        )
        .expect("triangle pipeline");
    let color = clear(Color::new(0.0, 0.0, 0.0, 1.0));
    let items = [Item::new(&pipeline)];
    let passes = [Pass::new(&color, &items)];
    target
        .render(&RenderDesc::new(&passes))
        .expect("triangle render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    // Determinism self-check: the same frame twice is the same bytes,
    // on every adapter — the cheap local form of the golden property.
    target
        .render(&RenderDesc::new(&passes))
        .expect("second triangle render");
    let mut second = vec![0u8; target.byte_len()];
    target.read_back_into(&mut second);
    assert_eq!(pixels, second, "same frame rendered twice diverged");
    // Teardown first, oracle second: destruction-time findings count.
    drop(target);
    drop(pipeline);
    assert_no_validation_errors(&device);

    // Structure, adapter-independent: corners lie outside the triangle
    // (clear black), the center lies inside (not clear, opaque).
    let pixel_at = |x: u32, y: u32| {
        let base = ((y * SIZE + x) * 4) as usize;
        [
            pixels[base],
            pixels[base + 1],
            pixels[base + 2],
            pixels[base + 3],
        ]
    };
    let clear_bytes = [0u8, 0, 0, 255];
    for (x, y) in [(0, 0), (SIZE - 1, 0), (0, SIZE - 1), (SIZE - 1, SIZE - 1)] {
        assert_eq!(pixel_at(x, y), clear_bytes, "corner ({x},{y}) not clear");
    }
    let center = pixel_at(SIZE / 2, SIZE / 2);
    assert_ne!(
        center, clear_bytes,
        "center pixel not covered by the triangle"
    );
    assert_eq!(center[3], 255, "center pixel not opaque");

    // Exact comparison only on the strict lane, whose stack is the
    // pinned toolchain the golden's bytes attest. Any other software
    // rasterizer (a distro lavapipe, a contributor's local build) has
    // its own rasterization bits — structure above is its gate.
    let adapter = device.adapter();
    if adapter.kind != AdapterKind::SoftwareRasterizer {
        assert!(
            !strict(),
            "RENEW_GOLDEN=1 but the selected adapter is {:?} ({}) — the \
             rendering lane must run on the pinned software rasterizer",
            adapter.kind,
            adapter.name
        );
        eprintln!(
            "SKIP exact-golden: adapter {:?} ({}) is not a software rasterizer",
            adapter.kind, adapter.name
        );
        return;
    }
    if !strict() {
        eprintln!(
            "SKIP exact-golden: software rasterizer {} outside the pinned lane \
             (set RENEW_GOLDEN=1 only where the stack matches the golden's provenance)",
            adapter.name
        );
        return;
    }

    let dir = goldens_dir();
    let golden = dir.join("triangle-256x256.rgba");
    let rendered_hash = fnv1a(&pixels);
    let provenance = format!(
        "triangle-256x256.rgba — RGBA8, tightly packed, row-major, {SIZE}x{SIZE}\n\
         fnv1a-64 of the pixel bytes: {rendered_hash:#018x}\n\
         rendered by: {} (kind {:?}, vendor {:#06x}, device {:#06x}, driver {})\n\
         shaders: crates/rhi/shaders (see its compile record)\n\
         ritual: the test never writes the canonical file above — it writes\n\
         *.candidate.rgba and fails; a human inspects the candidate (a .ppm\n\
         is written beside it), renames it to the canonical name, and commits\n\
         it with this sidecar. To refresh: delete the canonical file, rerun\n\
         on the pinned software rasterizer, repeat the ritual.\n",
        adapter.name, adapter.kind, adapter.vendor_id, adapter.device_id, adapter.driver_version
    );

    if !golden.exists() {
        std::fs::create_dir_all(&dir).expect("create goldens dir");
        let candidate = dir.join("triangle-256x256.candidate.rgba");
        std::fs::write(&candidate, &pixels).expect("write golden candidate");
        write_ppm(
            &dir.join("triangle-256x256.candidate.ppm"),
            &pixels,
            SIZE,
            SIZE,
        )
        .expect("write candidate ppm");
        std::fs::write(dir.join("triangle-256x256.provenance.txt"), provenance)
            .expect("write provenance sidecar");
        panic!(
            "golden is missing; candidate written to {} (fnv1a {rendered_hash:#018x}) — \
             inspect the .ppm, rename the candidate to the canonical name, and commit \
             it with its sidecar. This test never passes until a human does that.",
            candidate.display()
        );
    }

    let expected = std::fs::read(&golden).expect("read committed golden");
    if pixels != expected {
        let actual = dir.join("triangle-256x256.actual.rgba");
        std::fs::write(&actual, &pixels).expect("write actual for diffing");
        write_ppm(
            &dir.join("triangle-256x256.actual.ppm"),
            &pixels,
            SIZE,
            SIZE,
        )
        .expect("write actual ppm");
        let first_diff = pixels
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(usize::MAX);
        panic!(
            "rendered bytes diverge from the golden: first difference at byte {first_diff}, \
             lengths {} vs {}, fnv1a {rendered_hash:#018x} vs {:#018x}; actual written to {}",
            pixels.len(),
            expected.len(),
            fnv1a(&expected),
            actual.display()
        );
    }
}

/// G3: a sampled texture — exact bytes on every conformant adapter.
///
/// **This one needs no software rasterizer and no committed artifact.**
/// G2 pins its bytes to a pinned rasterizer because a triangle has a
/// silhouette, and which pixels a silhouette edge covers is where
/// implementations differ. This quad has no silhouette — it covers the
/// target exactly — so the only edge in play is the diagonal the two
/// triangles share, and a shared edge is not a place implementations
/// are free to differ: the spec requires a sample on it to be covered
/// by exactly one of the two, never both and never neither.
///
/// **That distinction is worth stating precisely, because the naive
/// version of it is false here.** At this size the diagonal runs
/// through eight pixel centres exactly, which is the most fragile
/// arrangement such a test can have. It is nonetheless safe three times
/// over: whichever triangle claims a sample, both interpolate the same
/// affine UV plane, so the coordinate is identical either way; blending
/// is disabled, so even a double hit would write the same bytes; and
/// nearest filtering has margin to spare — pixel centres land on texel
/// coordinates 0.125, 0.375, 0.625, 0.875, never within rounding
/// distance of a texel boundary.
///
/// The rest is specified arithmetic: nearest maps each pixel to exactly
/// one texel, and UNORM-to-UNORM passes bytes through unchanged. So the
/// expected image is computed here rather than read from a file, and
/// the assertion is as strong on real hardware as on a rasterizer.
///
/// Because the quad is drawn from `gl_VertexIndex` with no vertex
/// buffer, what this proves is the resource path: an image uploaded
/// through a staging buffer, transitioned to shader-read, bound through
/// a descriptor set written at binding creation, and sampled.
#[test]
fn a_sampled_texture_is_byte_exact_everywhere() {
    // Four texels, one per quadrant of the target. The size is a
    // multiple of two so every output pixel centre falls unambiguously
    // inside one texel rather than on a boundary.
    const SIZE: u32 = 8;
    const TEXELS: u32 = 2;
    #[rustfmt::skip]
    const ATLAS: [u8; 16] = [
        10, 20, 30, 255,    40, 50, 60, 255,
        70, 80, 90, 255,    100, 110, 120, 255,
    ];

    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let texture = device
        .create_texture(&TextureDesc::new(
            Extent {
                width: TEXELS,
                height: TEXELS,
            },
            &ATLAS,
        ))
        .expect("texture upload");
    let sampler = device
        .create_sampler(&SamplerDesc::atlas())
        .expect("sampler");
    // The accessor and `Debug` are exercised here rather than in the
    // device suite, which skips wherever the validation layer is absent.
    // Asserted on content: the extent must be the one the texture was
    // built from, which is a claim about the field being set from the
    // right place rather than about formatting.
    assert_eq!(texture.extent().width, TEXELS);
    assert_eq!(texture.extent().height, TEXELS);
    let shown = format!("{texture:?}");
    assert!(shown.starts_with("Texture"), "{shown}");
    assert!(shown.contains("extent"), "{shown}");

    let binding = device
        .create_binding(&BindingDesc::new(
            BindingSource::Texture(&texture),
            &sampler,
        ))
        .expect("binding");
    let pipeline = device
        .create_pipeline(
            &PipelineDesc::new(builtin::TEXTURED, TargetFormat::Rgba8Srgb).sampled_bindings(1),
        )
        .expect("sampled pipeline");
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");
    // A clear colour that appears nowhere in the atlas, so a quad that
    // failed to cover the target would show as unwritten rather than
    // blending into a plausible result.
    let color = clear(Color::new(1.0, 0.0, 1.0, 1.0));
    let items = [Item::new(&pipeline).bindings(&[&binding])];
    let passes = [Pass::new(&color, &items)];
    target
        .render(&RenderDesc::new(&passes))
        .expect("textured render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    for y in 0..SIZE {
        for x in 0..SIZE {
            // Clip space runs top-to-bottom in y and the atlas's first
            // row is its top row, so neither axis flips.
            let texel = ((y * TEXELS) / SIZE) * TEXELS + (x * TEXELS) / SIZE;
            let expected = &ATLAS[(texel as usize) * 4..(texel as usize) * 4 + 4];
            let offset = ((y * SIZE + x) as usize) * 4;
            assert_eq!(
                &pixels[offset..offset + 4],
                expected,
                "pixel ({x},{y}) should sample texel {texel} on adapter {:?}",
                device.adapter()
            );
        }
    }

    // The binding must keep the texture and sampler alive on its own:
    // the caller's handles go away here, and the set the next draw
    // samples through still points at a live view and sampler.
    drop(texture);
    drop(sampler);

    // **Then draw again, and this is the part that does the proving.**
    // Validation reports a destroyed image view at the draw that samples
    // it, not at the moment it is destroyed — so a test that drops these
    // and merely tears down asserts nothing, and would pass just as
    // happily with the keep-alive deleted. Rendering once more after the
    // caller's handles are gone is what makes the claim testable.
    target
        .render(&RenderDesc::new(&passes))
        .expect("render after the caller dropped its handles");
    target.read_back_into(&mut pixels);
    let texel = &ATLAS[..4];
    assert_eq!(
        &pixels[..4],
        texel,
        "the second draw must sample the same texels as the first"
    );

    // Teardown first, oracle second: destruction-time findings count.
    // The target goes first so its retention table releases the
    // binding's last frame reference before the binding itself drops.
    drop(target);
    drop(pipeline);
    drop(binding);
    assert_no_validation_errors(&device);
}

/// Two textures through ONE pipeline, then the same two swapped — the
/// shape the binding type exists for. The pair fragment stage reads
/// slot 0 left of the midline and slot 1 right of it, so each half
/// must answer with its own atlas byte-exactly, and the swapped frame
/// must answer with the halves exchanged through the same pipeline
/// object with nothing recreated. One sampler serves both bindings,
/// which is its own claim: a sampler is an input to a binding, never
/// owned by one.
#[test]
fn two_textures_share_one_pipeline() {
    const SIZE: u32 = 8;
    const TEXELS: u32 = 2;
    #[rustfmt::skip]
    const LEFT_ATLAS: [u8; 16] = [
        10, 20, 30, 255,    40, 50, 60, 255,
        70, 80, 90, 255,    100, 110, 120, 255,
    ];
    #[rustfmt::skip]
    const RIGHT_ATLAS: [u8; 16] = [
        200, 15, 25, 255,   210, 45, 55, 255,
        220, 75, 85, 255,   230, 105, 115, 255,
    ];
    /// The texel a target pixel samples, and which atlas it reads under
    /// the given slot order — the CPU statement of the fragment stage's
    /// midline split.
    fn expected(atlases: [&[u8; 16]; 2], x: u32, y: u32) -> &[u8] {
        let atlas = atlases[usize::from(x >= SIZE / 2)];
        let texel = (((y * TEXELS) / SIZE) * TEXELS + (x * TEXELS) / SIZE) as usize;
        &atlas[texel * 4..texel * 4 + 4]
    }

    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let size = Extent {
        width: TEXELS,
        height: TEXELS,
    };
    let left_texture = device
        .create_texture(&TextureDesc::new(size, &LEFT_ATLAS))
        .expect("left texture");
    let right_texture = device
        .create_texture(&TextureDesc::new(size, &RIGHT_ATLAS))
        .expect("right texture");
    let sampler = device
        .create_sampler(&SamplerDesc::atlas())
        .expect("sampler");
    let left = device
        .create_binding(&BindingDesc::new(
            BindingSource::Texture(&left_texture),
            &sampler,
        ))
        .expect("left binding");
    // The Debug form is asserted here rather than in the device suite,
    // which skips wherever the validation layer is absent — the same
    // reasoning the sampler's Debug assertion records in the fault
    // suite.
    let shown = format!("{left:?}");
    assert!(shown.starts_with("Binding"), "{shown}");
    let right = device
        .create_binding(&BindingDesc::new(
            BindingSource::Texture(&right_texture),
            &sampler,
        ))
        .expect("right binding");
    let pipeline = device
        .create_pipeline(
            &PipelineDesc::new(builtin::TEXTURED_PAIR, TargetFormat::Rgba8Srgb).sampled_bindings(2),
        )
        .expect("two-slot pipeline");
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");
    let color = clear(Color::new(1.0, 0.0, 1.0, 1.0));
    let mut pixels = vec![0u8; target.byte_len()];

    for (order, atlases) in [
        ([&left, &right], [&LEFT_ATLAS, &RIGHT_ATLAS]),
        // The swap: the same pipeline draws the halves exchanged,
        // because which texture a draw samples is the item's to say.
        ([&right, &left], [&RIGHT_ATLAS, &LEFT_ATLAS]),
    ] {
        let items = [Item::new(&pipeline).bindings(&order)];
        let passes = [Pass::new(&color, &items)];
        target
            .render(&RenderDesc::new(&passes))
            .expect("two-slot render");
        target.read_back_into(&mut pixels);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let offset = ((y * SIZE + x) as usize) * 4;
                assert_eq!(
                    &pixels[offset..offset + 4],
                    expected(atlases, x, y),
                    "pixel ({x},{y}) under slot order {:?} on adapter {:?}",
                    atlases.map(|atlas| atlas[0]),
                    device.adapter()
                );
            }
        }
    }

    // Teardown first, oracle second, the whole cast: the bindings
    // release their inner holds only after they drop, so the sources
    // go last among the resources and everything precedes the oracle.
    drop(target);
    drop(pipeline);
    drop(left);
    drop(right);
    drop(left_texture);
    drop(right_texture);
    drop(sampler);
    assert_no_validation_errors(&device);
}

/// The atlas trio the sampled tests start from: texture, sampler, and
/// the binding over both.
fn atlas_fixture(
    device: &Device,
    texels: u32,
    atlas: &[u8],
) -> Result<(renew_rhi::Texture, renew_rhi::Sampler, renew_rhi::Binding), String> {
    let texture = device
        .create_texture(&TextureDesc::new(
            Extent {
                width: texels,
                height: texels,
            },
            atlas,
        ))
        .map_err(|error| format!("texture upload: {error}"))?;
    let sampler = device
        .create_sampler(&SamplerDesc::atlas())
        .map_err(|error| format!("sampler: {error}"))?;
    let binding = device
        .create_binding(&BindingDesc::new(
            BindingSource::Texture(&texture),
            &sampler,
        ))
        .map_err(|error| format!("atlas binding: {error}"))?;
    Ok((texture, sampler, binding))
}

/// The color round-trip: pass one renders the atlas quad into a
/// render image, pass two samples that image onto the surface — through
/// ONE pipeline, whose two items name two different bindings. The
/// surface must answer with exactly the bytes the direct sampled test
/// proves, because at equal sizes the nearest-sampled copy is the
/// identity: what this adds is the whole rendered-then-sampled path —
/// attachment write, layout transition to shader-read, sampled read —
/// under active validation, with a CPU oracle.
#[test]
fn a_rendered_image_samples_back_byte_exact() {
    const SIZE: u32 = 8;
    const TEXELS: u32 = 2;
    #[rustfmt::skip]
    const ATLAS: [u8; 16] = [
        10, 20, 30, 255,    40, 50, 60, 255,
        70, 80, 90, 255,    100, 110, 120, 255,
    ];

    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let (texture, sampler, atlas_binding) =
        atlas_fixture(&device, TEXELS, &ATLAS).expect("atlas fixture");
    let image = device
        .create_render_image(&RenderImageDesc::new(
            RenderImageKind::Color,
            Extent {
                width: SIZE,
                height: SIZE,
            },
        ))
        .expect("render image");
    // The image's own Debug and accessors, asserted here for the same
    // reason the binding's are: the device suite skips where the
    // validation layer is absent.
    assert_eq!(image.extent().width, SIZE);
    assert_eq!(image.kind(), RenderImageKind::Color);
    let shown = format!("{image:?}");
    assert!(shown.starts_with("RenderImage"), "{shown}");
    let image_binding = device
        .create_binding(&BindingDesc::new(BindingSource::Image(&image), &sampler))
        .expect("image binding");
    let pipeline = device
        .create_pipeline(
            &PipelineDesc::new(builtin::TEXTURED, TargetFormat::Rgba8Srgb).sampled_bindings(1),
        )
        .expect("sampled pipeline");
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");

    let clear_value = Attachment::new(
        LoadOp::Clear(ClearValue::Color(Color::new(1.0, 0.0, 1.0, 1.0))),
        StoreOp::Store,
    );
    let color = clear(Color::new(1.0, 0.0, 1.0, 1.0));
    let into_image = [Item::new(&pipeline).bindings(&[&atlas_binding])];
    // Two sampling items: the second mention of an already-sampled
    // image must recognise the transition already happened, not emit
    // it twice — identical draws, so the pixels also prove it.
    let onto_surface = [
        Item::new(&pipeline).bindings(&[&image_binding]),
        Item::new(&pipeline).bindings(&[&image_binding]),
    ];
    // The second targeting pass draws NOTHING over a Load — so the
    // sampled pixels below are the proof that Load actually preserved
    // the first pass's contents, while the pass itself drives the
    // between-pass walk arm and a second retention mention.
    let load_value = Attachment::new(LoadOp::Load, StoreOp::Store);
    let passes = [
        Pass::render_to(&image, clear_value, &into_image),
        Pass::render_to(&image, load_value, &[]),
        Pass::new(&color, &onto_surface),
    ];
    let mut pixels = vec![0u8; target.byte_len()];
    // Twice, identically: the second frame re-walks the image from
    // UNDEFINED — frame-scoped contents re-proven, not assumed.
    for round in 0..2 {
        target
            .render(&RenderDesc::new(&passes))
            .expect("round-trip render");
        target.read_back_into(&mut pixels);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let texel = ((y * TEXELS) / SIZE) * TEXELS + (x * TEXELS) / SIZE;
                let expected = &ATLAS[(texel as usize) * 4..(texel as usize) * 4 + 4];
                let offset = ((y * SIZE + x) as usize) * 4;
                assert_eq!(
                    &pixels[offset..offset + 4],
                    expected,
                    "round {round}, pixel ({x},{y}) should carry texel {texel} through the \
                     image on adapter {:?}",
                    device.adapter()
                );
            }
        }
    }

    // Teardown first, oracle second, the whole cast.
    drop(target);
    drop(pipeline);
    drop(image_binding);
    drop(atlas_binding);
    drop(image);
    drop(texture);
    drop(sampler);
    assert_no_validation_errors(&device);
}

/// A quad over the left half of clip space at `depth`, packed to the
/// mesh layout's 36-byte records: positions pass straight through the
/// mesh vertex stage; colour and uv ride along unread — the layout
/// describes the record, not the use.
fn left_half_quad(depth: f32) -> Vec<u8> {
    let mut vertices = Vec::new();
    for [x, y] in [
        [-1.0f32, -1.0],
        [0.0, -1.0],
        [0.0, 1.0],
        [-1.0, -1.0],
        [0.0, 1.0],
        [-1.0, 1.0],
    ] {
        for value in [x, y, depth] {
            vertices.extend_from_slice(&value.to_ne_bytes());
        }
        for _ in 0..6 {
            vertices.extend_from_slice(&0.0f32.to_ne_bytes());
        }
    }
    vertices
}

/// The shadow shape: a depth-only pass — no fragment stage, no color
/// attachment — writes a half-screen quad's depth into a depth-kinded
/// render image, and a sampling pass reads it back onto the surface.
/// Depth formats sample as (D, 0, 0, 1), so the surface's red channel
/// is the depth buffer itself: the quad's clip-space z where it
/// covered, the reversed-Z far clear where it did not — a CPU oracle
/// over the whole rendered-depth path.
#[test]
fn a_depth_only_pass_writes_depth_a_sampler_reads_back() {
    const SIZE: u32 = 8;
    // Chosen so both depth formats round-trip to one byte without a
    // tie: 0.25 is exact in f32 and in UNORM24 lands at 63.75 * (1/255)
    // steps — 64 after conversion, unambiguously.
    const QUAD_DEPTH: f32 = 0.25;

    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let image = match device.create_render_image(&RenderImageDesc::new(
        RenderImageKind::Depth,
        Extent {
            width: SIZE,
            height: SIZE,
        },
    )) {
        Ok(image) => image,
        // An adapter whose depth format cannot be sampled refuses at
        // creation by design; off the strict lane that is a skip, on it
        // the refusal must fail loudly.
        Err(error) => {
            assert!(
                !strict(),
                "RENEW_GOLDEN=1 but the depth render image was refused: {error}"
            );
            eprintln!("SKIP: depth render image refused: {error}");
            return;
        }
    };
    let sampler = device
        .create_sampler(&SamplerDesc::atlas())
        .expect("sampler");
    let depth_binding = device
        .create_binding(&BindingDesc::new(BindingSource::Image(&image), &sampler))
        .expect("depth binding");
    // The caster: a mesh pipeline with no fragment stage, reusing the
    // full mesh pair's vertex stage — its colour output simply has no
    // consumer.
    let caster = device
        .create_pipeline(
            &PipelineDesc::depth_mesh(builtin::MESH_VS_SPV, builtin::MESH_LAYOUT)
                .depth_state(DepthState::read_write()),
        )
        .expect("depth-only pipeline");
    let reader = device
        .create_pipeline(
            &PipelineDesc::new(builtin::TEXTURED, TargetFormat::Rgba8Srgb).sampled_bindings(1),
        )
        .expect("reader pipeline");
    let vertices = left_half_quad(QUAD_DEPTH);
    let mesh = device
        .create_mesh(&MeshDesc::new(&vertices, 36, &[0, 1, 2, 3, 4, 5]))
        .expect("caster quad");
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");

    // Reversed-Z: the clear is the far plane at 0.0, and the quad's
    // GREATER_OR_EQUAL 0.25 wins where it covers.
    let depth_ops = Attachment::new(LoadOp::Clear(ClearValue::Depth(0.0)), StoreOp::Store);
    let color = clear(Color::new(1.0, 0.0, 1.0, 1.0));
    let casting = [Item::new(&caster).mesh(&mesh)];
    let reading = [Item::new(&reader).bindings(&[&depth_binding])];
    // The second casting pass draws NOTHING over a Load — the sampled
    // halves below prove the depth Load preserved the quad's writes,
    // while the pass drives the depth image's between-pass walk arm.
    let depth_again = Attachment::new(LoadOp::Load, StoreOp::Store);
    let passes = [
        Pass::render_to(&image, depth_ops, &casting),
        Pass::render_to(&image, depth_again, &[]),
        Pass::new(&color, &reading),
    ];
    target
        .render(&RenderDesc::new(&passes))
        .expect("shadow render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    for y in 0..SIZE {
        for x in 0..SIZE {
            // Depth samples as (D, 0, 0, 1); UNORM8 conversion of 0.25
            // is 64, of the far clear 0.
            let expected: [u8; 4] = if x < SIZE / 2 {
                [64, 0, 0, 255]
            } else {
                [0, 0, 0, 255]
            };
            let offset = ((y * SIZE + x) as usize) * 4;
            assert_eq!(
                &pixels[offset..offset + 4],
                &expected,
                "pixel ({x},{y}) on adapter {:?} (format {:?})",
                device.adapter(),
                device.depth_format_name()
            );
        }
    }

    // Teardown first, oracle second, the whole cast.
    drop(target);
    drop(caster);
    drop(reader);
    drop(mesh);
    drop(depth_binding);
    drop(image);
    drop(sampler);
    assert_no_validation_errors(&device);
}

/// One instance record, packed exactly as `INSTANCED_LAYOUT` declares:
/// centre vec2, colour vec4. The layout slice, the shader's locations
/// and this function describe the same bytes.
fn instance(centre: [f32; 2], colour: [f32; 4]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(24);
    for v in centre {
        bytes.extend_from_slice(&v.to_ne_bytes());
    }
    for v in colour {
        bytes.extend_from_slice(&v.to_ne_bytes());
    }
    bytes
}

/// The per-frame data path, end to end, with a computed oracle: two
/// instances at known centres in known colours, pixels asserted at the
/// centres and at a corner the quads do not reach. Then a second frame
/// with different bytes through the SAME buffer, asserting the new
/// colours — the copy lands per call, not once at creation.
#[test]
fn instanced_quads_draw_this_frames_bytes() -> Result<(), Box<dyn std::error::Error>> {
    fn at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    }
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: 64,
        height: 64,
    };
    let mut target = device.create_offscreen_target(extent)?;
    let pipeline = device.create_pipeline(
        &PipelineDesc::new(builtin::INSTANCED, TargetFormat::Rgba8Srgb)
            .instance_input(builtin::INSTANCED_LAYOUT),
    )?;
    let buffer = device.create_buffer(64, renew_rhi::BufferUsage::PerFrame)?;
    assert_eq!(buffer.capacity(), 64, "capacity is per frame, as created");
    assert_eq!(
        format!("{buffer:?}"),
        "Buffer { capacity: 64, .. }",
        "debug output carries the capacity and no addresses"
    );

    // NDC (-0.5, -0.5) is pixel (16, 16); (+0.5, +0.5) is (48, 48).
    let mut bytes = instance([-0.5, -0.5], [1.0, 0.0, 0.0, 1.0]);
    bytes.extend(instance([0.5, 0.5], [0.0, 0.0, 1.0, 1.0]));

    let color = clear(Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let items = [Item::new(&pipeline).frame_data(renew_rhi::FrameData::new(&buffer, &bytes, 2))];
    target.render(&RenderDesc::new(&[Pass::new(&color, &items)]))?;

    let mut pixels = vec![0u8; (extent.width * extent.height * 4) as usize];
    target.read_back_into(&mut pixels);
    assert_eq!(
        at(&pixels, extent.width, 16, 16),
        [255, 0, 0, 255],
        "first instance's centre"
    );
    assert_eq!(
        at(&pixels, extent.width, 48, 48),
        [0, 0, 255, 255],
        "second instance's centre"
    );
    assert_eq!(
        at(&pixels, extent.width, 0, 0),
        [0, 0, 0, 255],
        "clear colour where no quad reaches"
    );

    // Second frame, same buffer, different bytes: green replaces red.
    let bytes = instance([-0.5, -0.5], [0.0, 1.0, 0.0, 1.0]);
    let items = [Item::new(&pipeline).frame_data(renew_rhi::FrameData::new(&buffer, &bytes, 1))];
    target.render(&RenderDesc::new(&[Pass::new(&color, &items)]))?;
    target.read_back_into(&mut pixels);
    assert_eq!(
        at(&pixels, extent.width, 16, 16),
        [0, 255, 0, 255],
        "this frame's bytes, not last frame's"
    );
    assert_eq!(
        at(&pixels, extent.width, 48, 48),
        [0, 0, 0, 255],
        "one instance now: the second quad is gone"
    );

    assert_no_validation_errors(&device);
    Ok(())
}

/// One depth-instance record, packed exactly as `INSTANCED_DEPTH_LAYOUT`
/// declares: (centre.xy, depth, unused) vec4, colour vec4. The layout
/// slice, the shader's locations and this function describe the same
/// bytes.
fn depth_instance(centre: [f32; 2], depth: f32, colour: [f32; 4]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32);
    for v in [centre[0], centre[1], depth, 0.0] {
        bytes.extend_from_slice(&v.to_ne_bytes());
    }
    for v in colour {
        bytes.extend_from_slice(&v.to_ne_bytes());
    }
    bytes
}

/// The depth-test oracle: two fully overlapping quads at decisively
/// separated depths (far above D24/D32 quantization), drawn far-first
/// AND near-first — the near quad wins both ways, which only a working
/// depth test produces; painter's order would let the far quad win the
/// second frame. Structural on every adapter; committed golden on the
/// pinned lane. The pass's depth attachment stores nothing
/// (`StoreOp::Discard`) — arm-covered here, not semantics-verified,
/// because nothing in this suite ever loads depth across passes.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "two draw orders plus the committed-golden ritual; splitting hides the symmetry"
)]
fn depth_test_keeps_the_near_quad_in_either_draw_order() -> Result<(), Box<dyn std::error::Error>> {
    const SIZE: u32 = 64;
    fn at(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * SIZE + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    }
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    if device.depth_format_name().is_none() {
        assert!(
            !strict(),
            "the rendering lane's adapter must offer a depth format"
        );
        eprintln!("SKIP: adapter offers no chain depth format");
        return Ok(());
    }
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let mut target = device.create_offscreen_target(extent)?;
    let pipeline = device.create_pipeline(
        &PipelineDesc::new(builtin::INSTANCED_DEPTH, TargetFormat::Rgba8Srgb)
            .instance_input(builtin::INSTANCED_DEPTH_LAYOUT)
            .depth_state(DepthState::read_write()),
    )?;
    let buffer = device.create_buffer(64, renew_rhi::BufferUsage::PerFrame)?;
    // Depth is reversed: nearer is LARGER, and the far plane is zero.
    // The blue quad is still the near one and still wins — the values
    // flipped with the convention so the oracle's meaning (and its
    // committed golden) survive the flip byte-identically.
    let far = depth_instance([0.0, 0.0], 0.25, [1.0, 0.0, 0.0, 1.0]);
    let near = depth_instance([0.0, 0.0], 0.75, [0.0, 0.0, 1.0, 1.0]);
    let far_first: Vec<u8> = [far.clone(), near.clone()].concat();
    let near_first: Vec<u8> = [near, far].concat();
    let black = Color::new(0.0, 0.0, 0.0, 1.0);
    let depth_attachment = Attachment::new(LoadOp::Clear(ClearValue::Depth(0.0)), StoreOp::Discard);
    let mut pixels = vec![0u8; target.byte_len()];

    let color = clear(black);
    let items =
        [Item::new(&pipeline).frame_data(renew_rhi::FrameData::new(&buffer, &far_first, 2))];
    let pass = Pass::new(&color, &items).depth(depth_attachment);
    target.render(&RenderDesc::new(&[pass]))?;
    target.read_back_into(&mut pixels);
    assert_eq!(
        at(&pixels, SIZE / 2, SIZE / 2),
        [0, 0, 255, 255],
        "far-first: the near quad wins on top"
    );
    assert_eq!(at(&pixels, 0, 0), [0, 0, 0, 255], "corner stays clear");

    let items =
        [Item::new(&pipeline).frame_data(renew_rhi::FrameData::new(&buffer, &near_first, 2))];
    let pass = Pass::new(&color, &items).depth(depth_attachment);
    target.render(&RenderDesc::new(&[pass]))?;
    target.read_back_into(&mut pixels);
    assert_eq!(
        at(&pixels, SIZE / 2, SIZE / 2),
        [0, 0, 255, 255],
        "near-first: the far quad must FAIL the depth test — painter's order would paint it"
    );
    assert_eq!(at(&pixels, 0, 0), [0, 0, 0, 255], "corner stays clear");

    // Teardown first, oracle second: destruction-time findings count.
    drop(target);
    drop(pipeline);
    assert_no_validation_errors(&device);

    // Exact comparison only on the strict lane, as G2: any other
    // rasterizer proves structure above, not bytes.
    let adapter = device.adapter();
    if adapter.kind != AdapterKind::SoftwareRasterizer {
        assert!(
            !strict(),
            "RENEW_GOLDEN=1 but the selected adapter is {:?} ({}) — the \
             rendering lane must run on the pinned software rasterizer",
            adapter.kind,
            adapter.name
        );
        eprintln!(
            "SKIP exact-golden: adapter {:?} ({}) is not a software rasterizer",
            adapter.kind, adapter.name
        );
        return Ok(());
    }
    if !strict() {
        eprintln!(
            "SKIP exact-golden: software rasterizer {} outside the pinned lane \
             (set RENEW_GOLDEN=1 only where the stack matches the golden's provenance)",
            adapter.name
        );
        return Ok(());
    }

    let dir = goldens_dir();
    let golden = dir.join("depth-64x64.rgba");
    let rendered_hash = fnv1a(&pixels);
    let provenance = format!(
        "depth-64x64.rgba — RGBA8, tightly packed, row-major, {SIZE}x{SIZE}\n\
         the near-first frame of the depth oracle (blue near quad over red far)\n\
         fnv1a-64 of the pixel bytes: {rendered_hash:#018x}\n\
         rendered by: {} (kind {:?}, vendor {:#06x}, device {:#06x}, driver {})\n\
         depth format: {}\n\
         shaders: crates/rhi/shaders (see its compile record)\n\
         ritual: the test never writes the canonical file above — it writes\n\
         *.candidate.rgba and fails; a human inspects the candidate (a .ppm\n\
         is written beside it), renames it to the canonical name, and commits\n\
         it with this sidecar. To refresh: delete the canonical file, rerun\n\
         on the pinned software rasterizer, repeat the ritual.\n",
        adapter.name,
        adapter.kind,
        adapter.vendor_id,
        adapter.device_id,
        adapter.driver_version,
        device.depth_format_name().unwrap_or("(none)")
    );

    if !golden.exists() {
        std::fs::create_dir_all(&dir).expect("create goldens dir");
        let candidate = dir.join("depth-64x64.candidate.rgba");
        std::fs::write(&candidate, &pixels).expect("write golden candidate");
        write_ppm(&dir.join("depth-64x64.candidate.ppm"), &pixels, SIZE, SIZE)
            .expect("write candidate ppm");
        std::fs::write(dir.join("depth-64x64.provenance.txt"), provenance)
            .expect("write provenance sidecar");
        panic!(
            "golden is missing; candidate written to {} (fnv1a {rendered_hash:#018x}) — \
             inspect the .ppm, rename the candidate to the canonical name, and commit \
             it with its sidecar. This test never passes until a human does that.",
            candidate.display()
        );
    }

    let expected = std::fs::read(&golden).expect("read committed golden");
    if pixels != expected {
        let actual = dir.join("depth-64x64.actual.rgba");
        std::fs::write(&actual, &pixels).expect("write actual for diffing");
        write_ppm(&dir.join("depth-64x64.actual.ppm"), &pixels, SIZE, SIZE)
            .expect("write actual ppm");
        let first_diff = pixels
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(usize::MAX);
        panic!(
            "rendered bytes diverge from the golden: first difference at byte {first_diff}, \
             lengths {} vs {}, fnv1a {rendered_hash:#018x} vs {:#018x}; actual written to {}",
            pixels.len(),
            expected.len(),
            fnv1a(&expected),
            actual.display()
        );
    }
    Ok(())
}

/// The pass-ordering oracle: pass 1 clears and draws A; pass 2 LOADS
/// the image and draws B — where they overlap B wins, where only A
/// reached the pixels survive the Load, binding `LoadOp::Load` and the
/// between-pass barrier in one computed image. Where the adapter offers
/// a depth format, both passes also carry a cleared depth attachment,
/// so the between-pass depth barrier records in the same frame (each
/// pass clears its own depth; nothing loads depth across passes). No
/// committed artifact: exact quad interiors on flat colours are
/// adapter-independent.
#[test]
fn a_second_pass_loads_and_draws_over_the_first() -> Result<(), Box<dyn std::error::Error>> {
    const SIZE: u32 = 64;
    fn at(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * SIZE + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    }
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let mut target = device.create_offscreen_target(extent)?;
    let with_depth = device.depth_format_name().is_some();
    let mut desc = PipelineDesc::new(builtin::INSTANCED_DEPTH, TargetFormat::Rgba8Srgb)
        .instance_input(builtin::INSTANCED_DEPTH_LAYOUT);
    if with_depth {
        desc = desc.depth_state(DepthState::read_write());
    }
    let pipeline = device.create_pipeline(&desc)?;
    // One buffer per item: one buffer, one item, per frame is the
    // contract, and this frame carries two items.
    let buffer_a = device.create_buffer(32, renew_rhi::BufferUsage::PerFrame)?;
    let buffer_b = device.create_buffer(32, renew_rhi::BufferUsage::PerFrame)?;
    // Quads are 0.5 NDC wide (half-width 0.25): A at x -0.2 covers
    // NDC [-0.45, 0.05], B at x +0.2 covers [-0.05, 0.45] — pixel
    // columns ~17..33, ~30..46, overlapping around the centre column.
    let bytes_a = depth_instance([-0.2, 0.0], 0.5, [1.0, 0.0, 0.0, 1.0]);
    let bytes_b = depth_instance([0.2, 0.0], 0.5, [0.0, 0.0, 1.0, 1.0]);
    let black = Color::new(0.0, 0.0, 0.0, 1.0);

    let color = clear(black);
    let load = [Attachment::new(LoadOp::Load, StoreOp::Store)];
    let items_a =
        [Item::new(&pipeline).frame_data(renew_rhi::FrameData::new(&buffer_a, &bytes_a, 1))];
    let items_b =
        [Item::new(&pipeline).frame_data(renew_rhi::FrameData::new(&buffer_b, &bytes_b, 1))];
    let mut pass_a = Pass::new(&color, &items_a);
    let mut pass_b = Pass::new(&load, &items_b);
    if with_depth {
        let fresh = Attachment::new(LoadOp::Clear(ClearValue::Depth(0.0)), StoreOp::Discard);
        pass_a = pass_a.depth(fresh);
        pass_b = pass_b.depth(fresh);
    }
    let passes = [pass_a, pass_b];
    target.render(&RenderDesc::new(&passes))?;

    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    assert_eq!(
        at(&pixels, 27, SIZE / 2),
        [255, 0, 0, 255],
        "where only pass 1 drew, the Load preserved its pixels"
    );
    assert_eq!(
        at(&pixels, 32, SIZE / 2),
        [0, 0, 255, 255],
        "where both drew, the second pass wins"
    );
    assert_eq!(
        at(&pixels, 36, SIZE / 2),
        [0, 0, 255, 255],
        "where only pass 2 drew"
    );
    assert_eq!(
        at(&pixels, 0, 0),
        [0, 0, 0, 255],
        "the first pass's clear survives the Load where nothing drew"
    );

    // Teardown first, oracle second: destruction-time findings count.
    drop(target);
    drop(pipeline);
    assert_no_validation_errors(&device);
    Ok(())
}

/// The bytes one mesh vertex occupies, as `MESH_LAYOUT` declares them.
const MESH_VERTEX_STRIDE: u32 = 12 + 16 + 8;

/// One mesh vertex, packed exactly as `MESH_LAYOUT` declares:
/// clip-space position vec3, colour vec4, texture coordinate vec2. The
/// layout slice, the shader's locations and this function describe the
/// same bytes.
///
/// The coordinate is zero here: these oracles draw through the untextured
/// mesh shaders, which do not consume it. It is packed because the layout
/// says the record contains it, and a record that disagrees with its
/// layout fails at the draw rather than here.
fn mesh_vertex(position: [f32; 3], colour: [f32; 4]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(MESH_VERTEX_STRIDE as usize);
    for value in position {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    for value in colour {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    for value in [0.0f32, 0.0] {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

/// G6: the mesh path — vertex buffer, index buffer, indexed draw — with
/// a computed oracle rather than a committed artifact.
///
/// **No committed golden, on G3's argument rather than in spite of it.**
/// A committed golden exists for the triangle because a silhouette edge
/// is where implementations differ. The geometry here is two triangles
/// covering the target exactly, in one flat colour, so the only edge in
/// play is the diagonal the two share — and a shared edge is not
/// somewhere implementations may differ: a sample on it is covered by
/// exactly one of the two, never both and never neither. With one
/// colour across all four vertices, interpolation cannot vary the answer
/// either, so every pixel has one right value on every conformant
/// adapter.
///
/// **What makes this prove indices rather than merely draw:** the second
/// frame keeps the same four vertices and submits half the index list.
/// A path that ignored the index buffer would draw the same picture
/// twice; a path that read it draws half the target and leaves the rest
/// at the clear colour. The vertex buffer is unchanged between them, so
/// the index list is the only thing that can account for the difference.
#[test]
fn an_indexed_mesh_draws_the_triangles_its_indices_name() -> Result<(), Box<dyn std::error::Error>>
{
    const SIZE: u32 = 32;
    fn at(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * SIZE + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    }
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let mut target = device.create_offscreen_target(extent)?;
    let pipeline = device.create_pipeline(&PipelineDesc::mesh(
        builtin::MESH,
        TargetFormat::Rgba8Srgb,
        builtin::MESH_LAYOUT,
    ))?;

    // Four corners of the target in clip space, all one colour. Corner
    // order: 0 top-left, 1 top-right, 2 bottom-right, 3 bottom-left.
    let green = [0.0, 1.0, 0.0, 1.0];
    let mut vertices = Vec::new();
    for corner in [
        [-1.0f32, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ] {
        vertices.extend(mesh_vertex(corner, green));
    }
    // Two triangles sharing the 0-2 diagonal, covering the whole target.
    let whole = [0u32, 1, 2, 0, 2, 3];
    let mesh = device.create_mesh(&MeshDesc::new(&vertices, MESH_VERTEX_STRIDE, &whole))?;
    assert_eq!(mesh.vertex_count(), 4, "four corners");
    assert_eq!(mesh.index_count(), 6, "two triangles");
    assert_eq!(
        mesh.vertex_stride(),
        MESH_VERTEX_STRIDE,
        "vec3 position, vec4 colour, vec2 texture coordinate"
    );
    let shown = format!("{mesh:?}");
    assert!(shown.starts_with("Mesh"), "{shown}");
    assert!(shown.contains("index_count"), "{shown}");

    // A clear colour that appears nowhere in the geometry, so a draw
    // that failed to cover shows as unwritten rather than plausible.
    let magenta = clear(Color::new(1.0, 0.0, 1.0, 1.0));
    let items = [Item::new(&pipeline).mesh(&mesh)];
    target.render(&RenderDesc::new(&[Pass::new(&magenta, &items)]))?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    for y in 0..SIZE {
        for x in 0..SIZE {
            assert_eq!(
                at(&pixels, x, y),
                [0, 255, 0, 255],
                "pixel ({x},{y}) is not covered by the indexed quad on adapter {:?}",
                device.adapter()
            );
        }
    }

    // Half the index list, same vertices: only the first triangle. It
    // spans corners 0, 1, 2 — top-left, top-right, bottom-right — so the
    // bottom-left corner falls outside it and keeps the clear colour.
    let half = device.create_mesh(&MeshDesc::new(&vertices, MESH_VERTEX_STRIDE, &whole[..3]))?;
    assert_eq!(half.index_count(), 3, "one triangle");
    let items = [Item::new(&pipeline).mesh(&half)];
    target.render(&RenderDesc::new(&[Pass::new(&magenta, &items)]))?;
    target.read_back_into(&mut pixels);
    assert_eq!(
        at(&pixels, SIZE - 1, 0),
        [0, 255, 0, 255],
        "the top-right corner is inside the one triangle the indices name"
    );
    assert_eq!(
        at(&pixels, 0, SIZE - 1),
        [255, 0, 255, 255],
        "the bottom-left corner is outside it — a path ignoring the index list would cover it"
    );

    // **This does NOT prove retention, and saying so is the point.**
    // Dropping a mesh handle and drawing again is the shape of the
    // texture keep-alive proof, but it cannot carry the same weight
    // here: this target is synchronous, so `render` has already waited
    // its fence and no submit outlives the call — the caller's own
    // borrow covers the only window in which the GPU reads. What the
    // lines below actually check is that one mesh survives a sibling's
    // drop and keeps drawing, which is a keep-alive smoke rather than a
    // race. **The retention table is load-bearing only on the window
    // path**, where `render` returns before the GPU finishes, and that
    // is where a proof of it has to live.
    let items = [Item::new(&pipeline).mesh(&mesh)];
    let passes = [Pass::new(&magenta, &items)];
    target.render(&RenderDesc::new(&passes))?;
    drop(half);
    target.render(&RenderDesc::new(&passes))?;
    target.read_back_into(&mut pixels);
    assert_eq!(
        at(&pixels, 0, SIZE - 1),
        [0, 255, 0, 255],
        "the whole-quad mesh still draws after another mesh handle was dropped"
    );

    // One mesh, several items, in one frame — the rule that applies to
    // per-frame buffers deliberately does not reach geometry, because
    // there is no copy to race.
    let twice = [
        Item::new(&pipeline).mesh(&mesh),
        Item::new(&pipeline).mesh(&mesh),
    ];
    target.render(&RenderDesc::new(&[Pass::new(&magenta, &twice)]))?;
    target.read_back_into(&mut pixels);
    assert_eq!(
        at(&pixels, SIZE / 2, SIZE / 2),
        [0, 255, 0, 255],
        "two items may name one mesh"
    );

    // Teardown first, oracle second: destruction-time findings count.
    drop(target);
    drop(pipeline);
    assert_no_validation_errors(&device);
    Ok(())
}

/// G7: one item carrying **both** geometry and per-frame bytes — the two
/// vertex streams bound in one draw, at two bindings with two input
/// rates, across one location space.
///
/// **This is the combination nothing in the tree consumes**, and it is
/// exercised here rather than left to the first caller for two reasons.
/// It is the arm of the retention enumeration that no other test
/// reaches, so without it a mesh drawn beside instance data would be
/// retained by code proven only by reading. And it is the shape the
/// camera first took at the renderer step — a per-instance transform
/// riding the buffer that already existed — before the camera moved to
/// push constants; the next consumer expected to want both streams is
/// per-chunk instancing, and the binding numbers stay proven for it.
///
/// The oracle is the same flat-colour full-target quad as G6, drawn with
/// a one-instance stream attached. Its pixels must be unchanged: the
/// instance data is bound and unread by this shader, so a wrong binding
/// index shows up as a validation error rather than as a colour, which is
/// why validation is consulted at the end.
#[test]
fn a_mesh_and_per_frame_bytes_bind_two_streams_in_one_draw()
-> Result<(), Box<dyn std::error::Error>> {
    const SIZE: u32 = 16;
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let mut target = device.create_offscreen_target(extent)?;
    let pipeline = device.create_pipeline(
        &PipelineDesc::mesh(builtin::MESH, TargetFormat::Rgba8Srgb, builtin::MESH_LAYOUT)
            .instance_input(builtin::INSTANCED_LAYOUT),
    )?;
    let mut vertices = Vec::new();
    for corner in [
        [-1.0f32, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ] {
        vertices.extend(mesh_vertex(corner, [0.0, 0.0, 1.0, 1.0]));
    }
    let mesh = device.create_mesh(&MeshDesc::new(
        &vertices,
        MESH_VERTEX_STRIDE,
        &[0, 1, 2, 0, 2, 3],
    ))?;
    let buffer = device.create_buffer(64, renew_rhi::BufferUsage::PerFrame)?;
    let bytes = instance([0.0, 0.0], [1.0, 1.0, 1.0, 1.0]);

    let magenta = clear(Color::new(1.0, 0.0, 1.0, 1.0));
    let items = [Item::new(&pipeline)
        .mesh(&mesh)
        .frame_data(renew_rhi::FrameData::new(&buffer, &bytes, 1))];
    target.render(&RenderDesc::new(&[Pass::new(&magenta, &items)]))?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    for (index, pixel) in pixels.chunks_exact(4).enumerate() {
        assert_eq!(
            pixel,
            [0, 0, 255, 255],
            "pixel {index} is not the mesh's own colour on adapter {:?} — a per-instance stream              bound where the per-vertex one belongs would change it",
            device.adapter()
        );
    }

    // Teardown first, oracle second: a wrong binding index is a
    // validation finding rather than a colour, so this is the assertion
    // that actually judges the two-stream layout.
    drop(target);
    drop(pipeline);
    assert_no_validation_errors(&device);
    Ok(())
}

/// The over-length refusal is retained in release, exactly as the
/// readback length guard is: the length bounds a copy into mapped
/// device memory, which makes it a memory-safety boundary.
#[test]
fn oversized_frame_data_is_a_retained_contract_check() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let mut target = device.create_offscreen_target(Extent {
        width: 16,
        height: 16,
    })?;
    let pipeline = device.create_pipeline(
        &PipelineDesc::new(builtin::INSTANCED, TargetFormat::Rgba8Srgb)
            .instance_input(builtin::INSTANCED_LAYOUT),
    )?;
    let buffer = device.create_buffer(8, renew_rhi::BufferUsage::PerFrame)?;
    let bytes = [0u8; 9];
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let color = clear(Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        let items =
            [Item::new(&pipeline).frame_data(renew_rhi::FrameData::new(&buffer, &bytes, 1))];
        let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
    }));
    assert!(
        refused.is_err(),
        "nine bytes into an eight-byte region must refuse"
    );
    Ok(())
}
