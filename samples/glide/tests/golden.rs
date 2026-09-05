//! The golden-replay oracle: a committed input trace replayed through
//! the promoted loop, drawn through the real sprite pipeline onto a
//! real offscreen target, compared against a committed image.
//!
//! The digest suites prove replayed state is bit-identical; this file
//! proves the state *looks* right — placement, occlusion, the corpse
//! where the rules froze it — which is the half no hash can see.
//!
//! Three checkpoints over two committed traces: `soar` alive among
//! pipes, the same trace earlier at its fastest dive — where the bird
//! is smeared along its fall — and `sink` a still life (death freezes
//! pipe advance, and a corpse does not smear). Exact comparison
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
use renew_sample_glide::{Effects, SPRITE_BUDGET, SceneSprite, Tile, scene, world_at};
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
/// The bird's leading texel column, orange rather than yellow.
///
/// A tilt's sign is invisible on a solid square: a turn and its negative
/// are mirror images of the same shape, and at the corpse's eighth-turn
/// they paint the same diamond. The beak is what makes the sign
/// something a picture can show, and something a test can read: it is
/// the local `+x` half of the body, which the corner map sends downward
/// as the tilt goes positive.
const BEAK_BYTES: [u8; 4] = [255, 96, 0, 255];
const PIPE_BYTES: [u8; 4] = [0, 160, 40, 255];
/// The spark texel: opaque white, so what a spark shows is its tint.
const SPARK_BYTES: [u8; 4] = [255, 255, 255, 255];

/// The tick the `sink` bird hits the floor — observed by re-flying the
/// trace, and asserted as a premise by the crash checkpoint rather than
/// trusted here.
const DEATH_TICK: u64 = 108;
/// Six ticks after the crash: the burst is a tenth of a second old, so
/// every spark is still in the air and none has expired.
const CRASH_TICK: u64 = DEATH_TICK + 6;

/// The 12×2 test card: an opaque bird region whose right column is the
/// beak, an opaque pipe region, a white spark region the tint colours,
/// and a transparent texel on every side of each — the gutter a turned
/// sprite needs, because a rotated edge resolves to a texel inside its
/// own region only up to rounding, and without it a corner would sample
/// its neighbour's art. Every sprite this game draws now turns: the
/// bird tilts, and a spark spins.
const ATLAS_EXTENT: Extent = Extent {
    width: 12,
    height: 2,
};
const BIRD_REGION: Region = Region {
    x: 1,
    y: 0,
    width: 2,
    height: 2,
};
const PIPE_REGION: Region = Region {
    x: 5,
    y: 0,
    width: 2,
    height: 2,
};
/// White, so a spark's colour is entirely its tint's — the region
/// carries coverage and nothing else.
const SPARK_REGION: Region = Region {
    x: 9,
    y: 0,
    width: 2,
    height: 2,
};

fn atlas_bytes() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(12 * 2 * 4);
    for _y in 0..2u32 {
        for x in 0..12u32 {
            let texel = match x {
                1 => BIRD_BYTES,
                2 => BEAK_BYTES,
                5 | 6 => PIPE_BYTES,
                9 | 10 => SPARK_BYTES,
                _ => [0, 0, 0, 0],
            };
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
    for pixel in pixels.as_chunks::<4>().0 {
        ppm.extend_from_slice(&pixel[..3]);
    }
    std::fs::write(path, ppm)
}

/// Render `world`'s scene once and read it back, with `effects`' live
/// sparks drawn over it.
///
/// The sparks are appended after the scene, which is what puts them on
/// top: this crate's fill order is draw order.
fn capture(device: &Device, world: &World, effects: &Effects) -> Result<Vec<u8>, String> {
    let atlas = atlas_bytes();
    let canvas = Canvas::new(VIEW_WIDTH, VIEW_HEIGHT).ok_or("zero view")?;
    let capacity = core::num::NonZeroU32::new(SPRITE_BUDGET).ok_or("zero capacity")?;
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
    effects.fill(&mut sprites);
    renderer.begin();
    for sprite in &sprites {
        let region = match sprite.tile {
            Tile::Bird => BIRD_REGION,
            Tile::Pipe => PIPE_REGION,
            Tile::Spark => SPARK_REGION,
        };
        renderer.push(
            &Sprite::new(region, sprite.x, sprite.y)
                .size(sprite.width, sprite.height)
                .rotation(sprite.rotation)
                .saturation(sprite.saturation)
                .smear(sprite.smear[0], sprite.smear[1])
                .tint(sprite.tint),
        );
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

/// Re-fly the `sink` trace — a bird that never flaps — to `ticks`,
/// observing the effects after every executed step.
///
/// **Re-flown rather than replayed** because the effects are a function
/// of the world's *history*, not of any one state: the burst fires on
/// the step where liveness falls, and a world handed to
/// `Effects::new` after that step has already happened has no edge left
/// to fire on. Every caller checks the re-flown digest against the
/// committed trace's, so the two roads are held together.
fn reflown_sink(ticks: u64) -> (World, Effects) {
    let mut world = World::new(7);
    let mut effects = Effects::new(&world);
    for _ in 0..ticks {
        world.step(false);
        effects.observe(&world);
    }
    (world, effects)
}

/// Effects for a checkpoint whose bird is still alive.
///
/// Nothing can have burst: the burst is the falling edge of liveness,
/// and a world that is alive now has not crossed it. Building the pool
/// from the world rather than re-flying is therefore exact and cheap —
/// and the tests assert `live() == 0` rather than trusting this note.
fn no_effects_yet(world: &World) -> Effects {
    Effects::new(world)
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

/// What an opaque atlas texel becomes when the sprite is fully
/// desaturated: its own luminance, in a byte.
///
/// Derived rather than written down, because the two greys this file
/// needs — the body's and the beak's — are close enough that a wrong
/// constant would look plausible. Decode each channel to linear light,
/// take the luminance the fragment stage takes, encode once through the
/// same target format the attachment uses. The result is compared within
/// one code, since a luminance is not a fixed point of the transfer
/// function.
#[allow(
    clippy::expect_used,
    reason = "a colour target that stores no colour is the defect, and this file already \
              takes that position where it derives the clear's bytes"
)]
fn grey_of(texel: [u8; 4]) -> u8 {
    let linear = |byte: u8| renew_rhi::srgb::decode(byte);
    let luma = 0.2126f32.mul_add(
        linear(texel[0]),
        0.7152f32.mul_add(linear(texel[1]), 0.0722 * linear(texel[2])),
    );
    TargetFormat::Rgba8Srgb
        .stores(luma)
        .expect("a color target stores color")
}

/// Is this pixel that colour, within one code per channel?
///
/// One code because a grey derived through the transfer function lands
/// on one of two neighbouring bytes depending on how the hardware
/// rounds; the coloured cases pass a zero tolerance and are exact.
fn matches(pixel: [u8; 4], want: [u8; 4], tolerance: u8) -> bool {
    (0..4).all(|channel| pixel[channel].abs_diff(want[channel]) <= tolerance)
}
/// The three structural claims about the tilted bird, checked on every
/// adapter rather than only on the pinned lane: the body is where the
/// rules put it, nothing of the pipe's art reaches it, and the beak
/// points the way the velocity says.
///
/// **Why not the centre pixel any more.** The centre used to be asserted
/// as the bird's body colour. The bird's right texel column is the beak
/// now, so the centre pixel is orange at zero tilt and either colour
/// once turned. The sample moves three pixels left instead: its centre
/// `(37.5, bird_y + 0.5)` is `(-2.5, +0.5)` from the geometric centre,
/// so under a tilt of `θ` its local x is `-2.5·cos θ + 0.5·sin θ`, which
/// over the whole tilt range stays between `-2.12` and `-1.41` — always
/// in the left, body half, never within a pixel and a half of the
/// body/beak boundary.
///
/// **The pipe-in-the-box check is a guard, not a probe.** No mutant
/// reddens it today, and the reason is worth stating: the sheet gained a
/// transparent gutter around each region so a turned edge could not
/// sample its neighbour, and moving the pipe's texels flush against the
/// beak leaves both goldens green. With nearest sampling and a UV
/// interpolated between the source rectangle's own corners, a rotated
/// quad cannot resolve outside that rectangle at all — the gutter is
/// defence for a filter this engine does not yet use, and for the day
/// someone widens a region or draws a pipe across the bird. It stays
/// because it costs one comparison per pixel of a small box and fails
/// loudly if either of those happens.
/// **The sign check is what the beak exists for.** A solid square cannot
/// show a tilt's sign: a turn and its negative are mirror images of the
/// same shape, and at an eighth turn they paint the same diamond. Over
/// the box, the mean vertical offset of the beak's pixels is positive
/// when the bird is falling and negative when it is rising, with margin:
/// at the terminal tilt the mean is about `+2.24` px, at the flap tilt
/// about `-1.68`, and even at a fiftieth of a turn about `+0.33`.
fn assert_bird_structure(
    pixels: &[u8],
    world: &World,
    label: &str,
    body: [u8; 4],
    beak: [u8; 4],
    tolerance: u8,
) {
    #[allow(
        clippy::cast_sign_loss,
        reason = "the bird's centre is on-screen and non-negative at both pinned checkpoints"
    )]
    let bird_y = world.bird_y_units() as u32;
    #[allow(
        clippy::cast_sign_loss,
        reason = "the bird's fixed column is positive by the rules"
    )]
    let bird_x = renew_sample_glide_world::BIRD_X_UNITS as u32;

    let sample = pixel_at(pixels, bird_x - 3, bird_y);
    assert!(
        matches(sample, body, tolerance),
        "{label}: the body three pixels left of centre should be {body:?} within {tolerance}, read {sample:?}"
    );

    // The box a twelve-pixel body fits in at any angle, clipped to the
    // view: half the diagonal is 8.49, so nine pixels each way.
    let half_box = 9u32;
    let top = bird_y.saturating_sub(half_box);
    let bottom = (bird_y + half_box).min(VIEW_HEIGHT - 1);
    let left = bird_x.saturating_sub(half_box);
    let right = (bird_x + half_box).min(VIEW_WIDTH - 1);

    let mut beak_offsets = 0.0f64;
    let mut beak_pixels = 0u32;
    for y in top..=bottom {
        for x in left..=right {
            let pixel = pixel_at(pixels, x, y);
            assert_ne!(
                pixel, PIPE_BYTES,
                "{label}: pipe art at ({x},{y}), inside the bird's box — the atlas gutter \
                 is what should stop a turned edge sampling its neighbour"
            );
            if matches(pixel, beak, tolerance) {
                beak_offsets += f64::from(y) + 0.5 - f64::from(bird_y);
                beak_pixels += 1;
            }
        }
    }

    assert!(
        beak_pixels > 0,
        "{label}: no beak pixel in the bird's box, so the tilt's sign is unobservable"
    );
    let mean = beak_offsets / f64::from(beak_pixels);
    let velocity = world.bird_velocity();
    assert!(
        velocity != 0,
        "{label}: a zero velocity leaves the sign check with nothing to say; \
         the checkpoint is chosen so this does not happen"
    );
    if velocity > 0 {
        assert!(
            mean > 0.0,
            "{label}: falling at {velocity} units per tick, but the beak's mean offset is \
             {mean} — the nose should be below the centre"
        );
    } else {
        assert!(
            mean < 0.0,
            "{label}: rising at {velocity} units per tick, but the beak's mean offset is \
             {mean} — the nose should be above the centre"
        );
    }
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
         *.candidate.rgba and fails; an inspector — a person, or a session\n\
         that records on the pull request what it inspected — renames the\n\
         candidate to the canonical name (a .ppm is written beside it) and\n\
         commits it with this sidecar. To refresh: delete the canonical\n\
         file, rerun on the pinned software rasterizer, repeat the ritual.\n",
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
             it with its sidecar. This test never passes until an inspector does \
             that — a person, or a session that records on the pull request what \
             it inspected.",
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
        // **The renderer belongs in this message**, and its absence cost a
        // day. A divergence here is either the change under test or the
        // machine under it, and those need opposite responses: the first
        // is a bug to fix, the second is a golden that cannot gate
        // anything. This message used to give offsets, lengths and hashes
        // - everything about the bytes, nothing about what produced them
        // - so an investigation that should have begun by comparing this
        // string against the committed provenance sidecar instead began
        // by re-reading a diff that touched no rendering code at all.
        return Err(format!(
            "rendered bytes diverge from the golden: first difference at byte {first_diff}, \
             lengths {} vs {}, fnv1a {rendered_hash:#018x} vs {:#018x}; rendered by {} ({:?}); \
             actual written to {}. If that renderer differs from the one named in the \
             provenance sidecar beside the golden, this is the machine and not the change.",
            pixels.len(),
            expected.len(),
            fnv1a(&expected),
            adapter.name,
            adapter.kind,
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

    let effects = no_effects_yet(&world);
    assert_eq!(
        effects.live(),
        0,
        "a living bird has crossed no death edge, so nothing may have burst"
    );
    let digest_before_rendering = world.digest();
    let pixels = capture(&device, &world, &effects).expect("capture");
    assert_eq!(
        world.digest(),
        digest_before_rendering,
        "rendering must be a read: the scene and the capture may not move the state"
    );
    assert_no_validation_errors(&device);

    // Structure, adapter-independent: sky at all four corners, then the
    // bird itself, which `assert_bird_structure` reads.
    for (x, y) in [
        (0, 0),
        (VIEW_WIDTH - 1, 0),
        (0, VIEW_HEIGHT - 1),
        (VIEW_WIDTH - 1, VIEW_HEIGHT - 1),
    ] {
        assert_eq!(pixel_at(&pixels, x, y), SKY_BYTES, "corner ({x},{y})");
    }
    // Alive: the atlas colours, exactly — no tolerance, because these
    // are the authored bytes with no transfer function in between.
    assert_bird_structure(&pixels, &world, "soar-600", BIRD_BYTES, BEAK_BYTES, 0);

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

    // **Re-flown, not replayed, and the two roads must meet.** This
    // checkpoint is far enough past the crash that every spark has
    // expired, and proving that needs a pool that actually burst — so
    // the effects come from re-flying the trace and observing each step,
    // and the re-flown digest is checked against the committed trace's,
    // which is the anchor the soaring checkpoint already carries.
    let (reflown, effects) = reflown_sink(240);
    assert_eq!(
        world.digest(),
        reflown.digest(),
        "the replayed trace and the re-flown fall diverged — the loop drifted"
    );
    assert_eq!(
        effects.live(),
        0,
        "the sparks burst at the crash and must be long dead by this tick, \
         or this frame is not the still life it is recorded as"
    );
    let digest_before_rendering = world.digest();
    let pixels = capture(&device, &world, &effects).expect("capture");
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
    // Dead: the same two texels desaturated, so the body and the beak
    // are two different greys — orange and yellow do not share a
    // luminance, which is what keeps the sign check readable on a corpse.
    let body = grey_of(BIRD_BYTES);
    let beak = grey_of(BEAK_BYTES);
    // Three codes apart, not merely unequal: the matcher accepts a
    // one-code window on each side, so two greys two apart would still
    // let a body pixel classify as beak and quietly blunt the sign check.
    assert!(
        body.abs_diff(beak) >= 3,
        "the corpse's greys are {body} and {beak} -- too close to tell apart \
         within the tolerance the matcher uses"
    );
    assert_bird_structure(
        &pixels,
        &world,
        "sink-240",
        [body, body, body, 255],
        [beak, beak, beak, 255],
        1,
    );

    compare_against_golden(&device, "sink-240", &pixels).expect("the golden ritual");
}

/// The diving bird, smeared along its fall.
///
/// **Why tick 361.** The bird reaches terminal velocity — the fastest
/// the rules allow, so no tick of any trace smears further — and does it
/// with the full complement of five pipes on screen and the bird still
/// alive, which the premises below assert rather than trust. A hundred
/// and one ticks of this trace sit at terminal velocity, so "the fastest
/// tick" alone does not name one; the tie is broken by taking the first
/// with five pipes in the frame, which is also the tick the scene tests
/// already pin.
///
/// **What the structure claims, and why not the usual bird check.** The
/// smear is projected onto the sprite's own drawn axes, and at a
/// forty-five degree dive that projection has a component along both of
/// them — so the average crosses the body/beak boundary and the pixel
/// three left of centre is no longer either atlas colour. The tilt's
/// sign is pinned by the soaring frame; what this frame is for is the
/// ghost, and the ghost is what it asserts:
///
/// - **It reaches past the body.** A twelve-unit square turned an eighth
///   of a turn has a half-diagonal of 8.49 units, so its own art cannot
///   put anything eleven units above or below the centre. The smear can:
///   nine and a half units of it, half each way, and both those pixels
///   come back coloured.
/// - **It ends.** Sixteen units out is sky again on both sides, so the
///   footprint grew by the smear rather than unboundedly.
/// - **It fades, monotonically, both ways.** Walking out from the
///   centre along the bird's own column, the red channel never rises.
///   That is the box filter's signature: one tap at a time leaves the
///   art, so coverage falls a step at a time and never recovers. A
///   smear that clamped its outside taps instead of dropping them would
///   hold the colour flat and then cut off.
///
/// Structural on every adapter; the band's exact bytes are the committed
/// picture's business.
#[test]
fn a_dive_at_tick_361_smears_along_the_fall() {
    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let world = world_at("soar", 361).expect("the committed trace replays");

    // Premises: a still bird has no ghost to photograph.
    assert!(world.alive(), "the diver must still be flying");
    assert_eq!(world.tick(), 361);
    assert_eq!(
        world.bird_velocity(),
        renew_sample_glide_world::TERMINAL_VELOCITY,
        "the checkpoint is the fastest the rules allow, so no frame smears further"
    );
    assert_eq!(
        world.pipes(),
        5,
        "the full complement of pipes is on screen"
    );

    let effects = no_effects_yet(&world);
    assert_eq!(
        effects.live(),
        0,
        "a living bird has crossed no death edge, so nothing may have burst"
    );
    let digest_before_rendering = world.digest();
    let pixels = capture(&device, &world, &effects).expect("capture");
    assert_eq!(
        world.digest(),
        digest_before_rendering,
        "rendering must be a read: the scene and the capture may not move the state"
    );
    assert_no_validation_errors(&device);

    // Five pipes fill this frame — every corner is pipe art, not sky,
    // which is the same "the picture is the world's" check the other
    // checkpoints make against their emptier skies.
    for (x, y) in [
        (0, 0),
        (VIEW_WIDTH - 1, 0),
        (0, VIEW_HEIGHT - 1),
        (VIEW_WIDTH - 1, VIEW_HEIGHT - 1),
    ] {
        assert_eq!(pixel_at(&pixels, x, y), PIPE_BYTES, "corner ({x},{y})");
    }

    #[allow(
        clippy::cast_sign_loss,
        reason = "the bird's centre is on-screen and non-negative at this checkpoint"
    )]
    let bird_y = world.bird_y_units() as u32;
    #[allow(
        clippy::cast_sign_loss,
        reason = "the bird's fixed column is positive by the rules"
    )]
    let bird_x = renew_sample_glide_world::BIRD_X_UNITS as u32;
    // Saturating rather than wrapping: every offset below keeps the row
    // on screen at this checkpoint, and a bug that moved the bird to the
    // top edge should read row zero rather than wrap to the bottom.
    let column = |dy: i32| pixel_at(&pixels, bird_x, bird_y.saturating_add_signed(dy));

    // The ghost reaches where the turned square cannot.
    for dy in [-11, 11] {
        assert_ne!(
            column(dy),
            SKY_BYTES,
            "{dy} units from the centre is past the turned body's 8.49-unit reach, so \
             only the smear can have coloured it"
        );
    }
    // And no further than it should.
    for dy in [-16, 16] {
        assert_eq!(
            column(dy),
            SKY_BYTES,
            "the footprint grew by the smear, so {dy} units out must still be sky"
        );
    }
    // And it fades a step at a time, in both directions.
    for dy in 0..16 {
        assert!(
            column(-dy)[0] >= column(-dy - 1)[0],
            "the ghost brightened walking up, at {dy} units above the centre"
        );
        assert!(
            column(dy)[0] >= column(dy + 1)[0],
            "the ghost brightened walking down, at {dy} units below the centre"
        );
    }

    // The same guard the other frames carry: nothing of the pipe's art
    // reaches the bird, now over a box grown to hold the smear.
    let half_box = 14u32;
    for y in bird_y.saturating_sub(half_box)..=(bird_y + half_box).min(VIEW_HEIGHT - 1) {
        for x in bird_x.saturating_sub(half_box)..=(bird_x + half_box).min(VIEW_WIDTH - 1) {
            assert_ne!(
                pixel_at(&pixels, x, y),
                PIPE_BYTES,
                "pipe art at ({x},{y}), inside the smeared bird's box"
            );
        }
    }

    compare_against_golden(&device, "dive-361", &pixels).expect("the golden ritual");
}

/// The crash: sparks thrown up from the corpse, added as light.
///
/// **Why tick 114.** The `sink` trace never flaps, so the bird falls
/// from the start and hits the floor at tick 108 — observed, not
/// assumed, and asserted below. Six ticks later the burst is a tenth of
/// a second old: every spark is still in the air, the fastest have
/// cleared the corpse, and none has expired. That is the frame worth
/// recording.
///
/// **What the structure claims, and why it is adapter-independent.**
/// The sparks are drawn with a tint whose alpha is zero, which the
/// sprite renderer's Contract makes additive out of its one pipeline:
/// `src + dst·(1 − α_src)` at `α_src = 0` is addition, leaving the
/// destination's alpha alone. So a spark can only ever *brighten* what
/// it crosses, and the test asserts exactly that — somewhere above the
/// corpse there is a pixel brighter than the sky in all three channels,
/// and the sky's own alpha is untouched everywhere. Both hold on any
/// adapter, because neither depends on where the transfer function
/// rounds.
#[test]
fn a_crash_at_tick_114_throws_sparks_as_light() {
    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };

    // The death tick is a premise, not a guess: re-fly one tick short of
    // it and the bird must still be alive, re-fly to it and it must not.
    let (before, _) = reflown_sink(DEATH_TICK - 1);
    assert!(
        before.alive(),
        "the bird must still be flying one tick before the recorded death"
    );
    let (world, effects) = reflown_sink(CRASH_TICK);
    assert!(!world.alive(), "the bird must be dead at the crash frame");
    assert_eq!(world.tick(), CRASH_TICK);
    assert!(world.pipes() > 0, "pipes must be on screen");
    assert!(
        effects.live() > 0,
        "the burst must be in the air, or this picture proves nothing"
    );

    // The two roads meet here too: the committed trace replayed to this
    // tick is the world we re-flew.
    let replayed = world_at("sink", CRASH_TICK).expect("the committed trace replays");
    assert_eq!(
        replayed.digest(),
        world.digest(),
        "the replayed trace and the re-flown fall diverged — the loop drifted"
    );

    let digest_before_rendering = world.digest();
    let pixels = capture(&device, &world, &effects).expect("capture");
    assert_eq!(
        world.digest(),
        digest_before_rendering,
        "rendering must be a read: the scene, the sparks and the capture may not move the state"
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
        reason = "the corpse's centre is on-screen and non-negative at this checkpoint"
    )]
    let bird_y = world.bird_y_units() as u32;
    #[allow(
        clippy::cast_sign_loss,
        reason = "the bird's fixed column is positive by the rules"
    )]
    let bird_x = renew_sample_glide_world::BIRD_X_UNITS as u32;

    // Light, and only light: somewhere in the band above the corpse a
    // pixel is brighter than the sky in every channel. A spark that
    // occluded instead of adding would darken the blue channel, which
    // this catches; a spark that never drew would leave the band sky.
    let mut brighter = 0u32;
    let top = bird_y.saturating_sub(40);
    for y in top..bird_y {
        for x in bird_x.saturating_sub(24)..(bird_x + 24).min(VIEW_WIDTH - 1) {
            let p = pixel_at(&pixels, x, y);
            if p[0] > SKY_BYTES[0] && p[1] > SKY_BYTES[1] && p[2] > SKY_BYTES[2] {
                brighter += 1;
            }
        }
    }
    assert!(
        brighter > 0,
        "no pixel above the corpse is brighter than the sky in all three channels, \
         so the sparks either did not draw or did not add"
    );

    // Additive light never touches the destination's alpha, anywhere.
    for y in 0..VIEW_HEIGHT {
        for x in 0..VIEW_WIDTH {
            assert_eq!(
                pixel_at(&pixels, x, y)[3],
                255,
                "light must leave the target opaque, at ({x},{y})"
            );
        }
    }

    compare_against_golden(&device, "crash-114", &pixels).expect("the golden ritual");
}
