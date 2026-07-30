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
    AdapterKind, Color, Device, DeviceDesc, DeviceError, Extent, PipelineDesc, TargetFormat,
    Validation, builtin,
};

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
    let clear = Color::new(51.0 / 255.0, 102.0 / 255.0, 153.0 / 255.0, 1.0);
    target.render(clear, None).expect("clear render");
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
    let pipeline = device
        .create_pipeline(&PipelineDesc {
            vertex_spirv: builtin::TRIANGLE_VS_SPV,
            fragment_spirv: builtin::TRIANGLE_FS_SPV,
            target_format: TargetFormat::Rgba8Unorm,
        })
        .expect("triangle pipeline");
    target
        .render(Color::new(0.0, 0.0, 0.0, 1.0), Some(&pipeline))
        .expect("triangle render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    // Determinism self-check: the same frame twice is the same bytes,
    // on every adapter — the cheap local form of the golden property.
    target
        .render(Color::new(0.0, 0.0, 0.0, 1.0), Some(&pipeline))
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

    // Exact comparison only where rasterization is toolchain-pinned.
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
