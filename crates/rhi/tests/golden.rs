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
//! rasterizer, the test writes the rendered candidate plus a provenance
//! sidecar and FAILS — a golden enters the tree only through a human
//! looking at it and committing it. A refresh is the same ritual with
//! the old artifact deleted first.

// The tripwire ban on filesystem access protects engine code; the
// golden harness's entire job is comparing against committed artifacts.
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;

use renew_rhi::{
    AdapterKind, Color, Device, DeviceDesc, DeviceError, Extent, PipelineDesc, TargetFormat,
    Validation, builtin,
};

/// `Ok(None)` is the graceful skip; other failures surface as `Err`
/// for the calling test to unwrap (test-only panics live in `#[test]`
/// bodies, where the lint allowance applies). Under `RENEW_GOLDEN=1`
/// (the CI rendering lane) a skip is a failure: that lane exists to
/// run these tests, so an environment that cannot must redden it.
fn device_or_skip() -> Result<Option<Device>, DeviceError> {
    let strict = std::env::var_os("RENEW_GOLDEN").is_some_and(|v| v == "1");
    match Device::new(&DeviceDesc {
        app_name: "renew-rhi-golden-tests",
        validation: Validation::IfAvailable,
    }) {
        Ok(device) => Ok(Some(device)),
        Err(DeviceError::LoaderUnavailable { message }) if !strict => {
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
            std::env::var_os("RENEW_GOLDEN").is_none_or(|v| v != "1"),
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
    let sidecar = dir.join("triangle-256x256.provenance.txt");
    let provenance = format!(
        "triangle-256x256.rgba — RGBA8, tightly packed, row-major, {SIZE}x{SIZE}\n\
         rendered by: {} (kind {:?}, vendor {:#06x}, device {:#06x}, driver {})\n\
         shaders: crates/rhi/shaders (see its compile record)\n\
         refresh ritual: delete the .rgba, run this test on the pinned software\n\
         rasterizer, inspect the freshly written candidate, commit both files.\n",
        adapter.name, adapter.kind, adapter.vendor_id, adapter.device_id, adapter.driver_version
    );

    if !golden.exists() {
        std::fs::create_dir_all(&dir).expect("create goldens dir");
        std::fs::write(&golden, &pixels).expect("write golden candidate");
        std::fs::write(&sidecar, provenance).expect("write provenance sidecar");
        panic!(
            "golden was missing; candidate written to {} — inspect the image, \
             commit it with its sidecar, and re-run",
            golden.display()
        );
    }

    let expected = std::fs::read(&golden).expect("read committed golden");
    if pixels != expected {
        let actual = dir.join("triangle-256x256.actual.rgba");
        std::fs::write(&actual, &pixels).expect("write actual for diffing");
        let first_diff = pixels
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(usize::MAX);
        panic!(
            "rendered bytes diverge from the golden (first difference at byte \
             {first_diff}, lengths {} vs {}); actual written to {}",
            pixels.len(),
            expected.len(),
            actual.display()
        );
    }
}
