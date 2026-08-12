//! The golden-replay oracle: a committed input trace replayed through
//! the promoted loop, drawn through the real sprite pipeline onto a
//! real offscreen target, compared against a committed image.
//!
//! The digest suites prove replayed state is bit-identical; this file
//! proves the state *looks* right — placement, occlusion, the corpse
//! where the rules froze it — which is the half no hash can see.
//!
//! Two checkpoints, one per committed trace: `soar` alive among pipes,
//! `sink` a still life (death freezes pipe advance). Exact comparison
//! on the pinned software-rasterizer lane via the candidate ritual;
//! structural assertions everywhere else. Golden-based rather than
//! computed on purpose: the coming linear-space change kills computed
//! pixels, and these images re-golden through the refresh ritual.

// The tripwire ban on filesystem access protects engine code; the
// golden harness's entire job is comparing against committed artifacts.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use renew_render2d::{AtlasDesc, Canvas, Region, Sprite, SpriteRenderer};
use renew_rhi::{
    AdapterKind, Color, Device, DeviceDesc, DeviceError, Extent, Pass, RenderDesc, TargetFormat,
    Validation,
};
use renew_sample_glide::{SceneSprite, Tile, scene, world_at};
use renew_sample_glide_world::{VIEW_HEIGHT, VIEW_WIDTH, World};

/// 51/255, 102/255, 153/255: unambiguous UNORM conversions — the sky.
const SKY: Color = Color {
    r: renew_rhi::srgb::decode(51),
    g: renew_rhi::srgb::decode(102),
    b: renew_rhi::srgb::decode(153),
    a: 1.0,
};
const SKY_BYTES: [u8; 4] = [51, 102, 153, 255];
const BIRD_BYTES: [u8; 4] = [255, 208, 0, 255];
const PIPE_BYTES: [u8; 4] = [0, 160, 40, 255];

/// The 4×2 test card: an opaque bird region and an opaque pipe region,
/// premultiplied trivially by their alpha of one.
const ATLAS_EXTENT: Extent = Extent {
    width: 4,
    height: 2,
};
const BIRD_REGION: Region = Region {
    x: 0,
    y: 0,
    width: 2,
    height: 2,
};
const PIPE_REGION: Region = Region {
    x: 2,
    y: 0,
    width: 2,
    height: 2,
};

fn atlas_bytes() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 * 2 * 4);
    for _y in 0..2u32 {
        for x in 0..4u32 {
            let texel = if x < 2 { BIRD_BYTES } else { PIPE_BYTES };
            bytes.extend_from_slice(&texel);
        }
    }
    bytes
}

fn strict() -> bool {
    std::env::var_os("RENEW_GOLDEN").is_some_and(|v| v == "1")
}

/// `Ok(None)` is the graceful skip; under `RENEW_GOLDEN=1` (the CI
/// rendering lane) a skip is a failure and validation must be active.
/// Same harness as the two golden suites before it; the copy is
/// deliberate — a fourth copy is the cue to extract a shared harness.
fn device_or_skip() -> Result<Option<Device>, DeviceError> {
    match Device::new(&DeviceDesc {
        app_name: "renew-glide-golden-tests",
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

/// Render `world`'s scene once and read it back.
fn capture(device: &Device, world: &World) -> Result<Vec<u8>, String> {
    let atlas = atlas_bytes();
    let canvas = Canvas::new(VIEW_WIDTH, VIEW_HEIGHT).ok_or("zero view")?;
    let capacity = core::num::NonZeroU32::new(32).ok_or("zero capacity")?;
    let mut renderer = SpriteRenderer::new(
        device,
        &AtlasDesc::new(ATLAS_EXTENT, &atlas),
        canvas,
        TargetFormat::Rgba8Srgb,
        capacity,
    )
    .map_err(|error| error.to_string())?;
    let mut target = device
        .create_offscreen_target(Extent {
            width: VIEW_WIDTH,
            height: VIEW_HEIGHT,
        })
        .map_err(|error| error.to_string())?;

    let mut sprites: Vec<SceneSprite> = Vec::new();
    scene(world, &mut sprites);
    renderer.begin();
    for sprite in &sprites {
        let region = match sprite.tile {
            Tile::Bird => BIRD_REGION,
            Tile::Pipe => PIPE_REGION,
        };
        renderer.push(&Sprite::new(region, sprite.x, sprite.y).size(sprite.width, sprite.height));
    }
    let color = [renew_rhi::color_attachment(SKY)];
    let items = [renderer.item()];
    let passes = [Pass::new(&color, &items)];
    let frame = RenderDesc::new(&passes);
    target.render(&frame).map_err(|error| error.to_string())?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    // Determinism self-check: the same frame twice is the same bytes.
    target.render(&frame).map_err(|error| error.to_string())?;
    let mut second = vec![0u8; target.byte_len()];
    target.read_back_into(&mut second);
    if pixels != second {
        return Err("same frame rendered twice diverged".to_string());
    }
    Ok(pixels)
}

fn pixel_at(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let base = ((y * VIEW_WIDTH + x) * 4) as usize;
    [
        pixels[base],
        pixels[base + 1],
        pixels[base + 2],
        pixels[base + 3],
    ]
}

/// The full ritual for one checkpoint: structural checks everywhere,
/// exact bytes against the committed golden on the pinned lane only,
/// candidate + provenance + a refusing Err when the golden does not
/// exist yet (the caller's expect is the designed failure). Fallible
/// because helpers outside test bodies carry no panic allowance.
#[allow(
    clippy::too_many_lines,
    reason = "the bootstrap ritual is one narrative"
)]
fn compare_against_golden(device: &Device, name: &str, pixels: &[u8]) -> Result<(), String> {
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
    let golden = dir.join(format!("{name}.rgba"));
    let rendered_hash = fnv1a(pixels);
    let provenance = format!(
        "{name}.rgba — RGBA8, tightly packed, row-major, {VIEW_WIDTH}x{VIEW_HEIGHT}\n\
         fnv1a-64 of the pixel bytes: {rendered_hash:#018x}\n\
         rendered by: {} (kind {:?}, vendor {:#06x}, device {:#06x}, driver {})\n\
         shaders: crates/render2d/shaders (see its compile record)\n\
         scene: the committed trace named in the file name, replayed to the\n\
         tick in the file name through the driver's own loop\n\
         ritual: the test never writes the canonical file above — it writes\n\
         *.candidate.rgba and fails; a human inspects the candidate (a .ppm\n\
         is written beside it), renames it to the canonical name, and commits\n\
         it with this sidecar. To refresh: delete the canonical file, rerun\n\
         on the pinned software rasterizer, repeat the ritual.\n",
        adapter.name, adapter.kind, adapter.vendor_id, adapter.device_id, adapter.driver_version
    );

    if !golden.exists() {
        std::fs::create_dir_all(&dir).map_err(|error| format!("create goldens dir: {error}"))?;
        let candidate = dir.join(format!("{name}.candidate.rgba"));
        std::fs::write(&candidate, pixels)
            .map_err(|error| format!("write golden candidate: {error}"))?;
        write_ppm(
            &dir.join(format!("{name}.candidate.ppm")),
            pixels,
            VIEW_WIDTH,
            VIEW_HEIGHT,
        )
        .map_err(|error| format!("write candidate ppm: {error}"))?;
        std::fs::write(dir.join(format!("{name}.provenance.txt")), provenance)
            .map_err(|error| format!("write provenance sidecar: {error}"))?;
        return Err(format!(
            "golden is missing; candidate written to {} (fnv1a {rendered_hash:#018x}) — \
             inspect the .ppm, rename the candidate to the canonical name, and commit \
             it with its sidecar. This test never passes until a human does that.",
            candidate.display()
        ));
    }

    let expected =
        std::fs::read(&golden).map_err(|error| format!("read committed golden: {error}"))?;
    if pixels != expected {
        let actual = dir.join(format!("{name}.actual.rgba"));
        std::fs::write(&actual, pixels)
            .map_err(|error| format!("write actual for diffing: {error}"))?;
        write_ppm(
            &dir.join(format!("{name}.actual.ppm")),
            pixels,
            VIEW_WIDTH,
            VIEW_HEIGHT,
        )
        .map_err(|error| format!("write actual ppm: {error}"))?;
        let first_diff = pixels
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(usize::MAX);
        return Err(format!(
            "rendered bytes diverge from the golden: first difference at byte {first_diff}, \
             lengths {} vs {}, fnv1a {rendered_hash:#018x} vs {:#018x}; actual written to {}",
            pixels.len(),
            expected.len(),
            fnv1a(&expected),
            actual.display()
        ));
    }
    // Nothing is written here — see the note in the 2D renderer's copy of
    // this ritual. A passing comparison leaves the tree exactly as it found
    // it; the sidecar is authored by the bootstrap path above, which fails,
    // and which is therefore the one moment a human is already looking.
    Ok(())
}

#[test]
fn soar_at_tick_600_matches_the_committed_image() {
    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let world = world_at("soar", 600).expect("the committed trace replays");

    // Premises: a picture of a dead or empty world proves nothing.
    assert!(world.alive(), "soar must still be flying at the checkpoint");
    assert!(world.score() > 0, "soar must have scored by the checkpoint");
    assert!(world.pipes() > 0, "pipes must be on screen");
    assert_eq!(world.tick(), 600);

    // The two roads must meet: the committed trace IS the autopilot's
    // recording (the fixed-point test beside the recorder proves the
    // bytes), so replaying its events and re-flying the pilot must
    // produce digest-identical worlds — the anchor that keeps the
    // promoted loop honest.
    let mut reflown = World::new(7);
    for _ in 0..600 {
        let flap = reflown.autopilot();
        reflown.step(flap);
    }
    assert_eq!(
        world.digest(),
        reflown.digest(),
        "the replayed trace and the re-flown pilot diverged — the loop drifted"
    );

    let digest_before_rendering = world.digest();
    let pixels = capture(&device, &world).expect("capture");
    assert_eq!(
        world.digest(),
        digest_before_rendering,
        "rendering must be a read: the scene and the capture may not move the state"
    );
    assert_no_validation_errors(&device);

    // Structure, adapter-independent: sky at all four corners, the
    // bird's opaque color at its centre (replacement — alpha one end
    // to end).
    for (x, y) in [
        (0, 0),
        (VIEW_WIDTH - 1, 0),
        (0, VIEW_HEIGHT - 1),
        (VIEW_WIDTH - 1, VIEW_HEIGHT - 1),
    ] {
        assert_eq!(pixel_at(&pixels, x, y), SKY_BYTES, "corner ({x},{y})");
    }
    #[allow(
        clippy::cast_sign_loss,
        reason = "the bird's centre is on-screen and non-negative at this pinned checkpoint"
    )]
    let bird_y = world.bird_y_units() as u32;
    #[allow(
        clippy::cast_sign_loss,
        reason = "the bird's fixed column is positive by the rules"
    )]
    let bird_x = renew_sample_glide_world::BIRD_X_UNITS as u32;
    assert_eq!(
        pixel_at(&pixels, bird_x, bird_y),
        BIRD_BYTES,
        "the bird's centre pixel"
    );

    compare_against_golden(&device, "soar-600", &pixels).expect("the golden ritual");
}

#[test]
fn sink_at_tick_240_matches_the_committed_image() {
    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let world = world_at("sink", 240).expect("the committed trace replays");

    // Premises: the still life must actually be one — dead bird, and
    // the pipes death froze still on screen.
    assert!(
        !world.alive(),
        "gravity must have won before the checkpoint"
    );
    assert_eq!(world.tick(), 240);
    assert!(world.pipes() > 0, "the frozen pipes must be on screen");

    let digest_before_rendering = world.digest();
    let pixels = capture(&device, &world).expect("capture");
    assert_eq!(
        world.digest(),
        digest_before_rendering,
        "rendering must be a read: the scene and the capture may not move the state"
    );
    assert_no_validation_errors(&device);

    for (x, y) in [
        (0, 0),
        (VIEW_WIDTH - 1, 0),
        (0, VIEW_HEIGHT - 1),
        (VIEW_WIDTH - 1, VIEW_HEIGHT - 1),
    ] {
        assert_eq!(pixel_at(&pixels, x, y), SKY_BYTES, "corner ({x},{y})");
    }
    #[allow(
        clippy::cast_sign_loss,
        reason = "the corpse froze on-screen; the world test pins the value"
    )]
    let bird_y = world.bird_y_units() as u32;
    #[allow(
        clippy::cast_sign_loss,
        reason = "the bird's fixed column is positive by the rules"
    )]
    let bird_x = renew_sample_glide_world::BIRD_X_UNITS as u32;
    assert_eq!(
        pixel_at(&pixels, bird_x, bird_y),
        BIRD_BYTES,
        "the corpse's centre pixel, frozen where death left it"
    );

    compare_against_golden(&device, "sink-240", &pixels).expect("the golden ritual");
}
