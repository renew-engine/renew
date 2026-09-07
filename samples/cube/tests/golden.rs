//! A picture of the voxel world, compared against a committed one.
//!
//! Everything else that looks at this sample's rendering asks structural
//! questions — is it a PNG, is it the right size, did more than one
//! colour appear. Those catch a projection that stopped projecting and
//! nothing finer. **No committed 3D image in this tree was compared byte
//! for byte before this file**, so a change that moved every block three
//! pixels to the left, or lost the shadow, or swapped two atlas tiles,
//! would have passed every lane.
//!
//! Two checkpoints over the two scripts that change the world: the arena
//! as it is built, and the same arena four hundred ticks into the
//! building script, by which point blocks have been broken and placed.
//! The second is the one that would notice a mesh change the first
//! cannot see, because the first draws a world nothing has touched.
//!
//! Exact comparison on the pinned software-rasterizer lane through the
//! candidate ritual; structural assertions everywhere else, so a machine
//! with no Vulkan runtime still runs this file and still learns
//! something.

#![cfg(feature = "render")]
// The tripwire ban on filesystem access protects engine code; this
// harness exists to compare against committed artifacts.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use renew_rhi::{AdapterKind, Device, DeviceDesc, DeviceError, Validation};
use renew_sample_cube::render::{ClipSurface, SIZE, build, draw_clip_space_with};
use renew_sample_cube::{Options, Script, run_world};

/// Whether a skip is allowed.
///
/// `RENEW_GOLDEN=1` marks the one lane whose stack matches the goldens'
/// provenance. Everywhere else a missing device is a fact about the
/// machine; there, it is a lane that failed to do its job.
fn strict() -> bool {
    std::env::var_os("RENEW_GOLDEN").is_some_and(|v| v == "1")
}

/// `Ok(None)` is the graceful skip; under `RENEW_GOLDEN=1` a skip is a
/// failure and validation must be active.
///
/// **This is the fourth copy of this harness, and the third one said the
/// fourth was the cue to extract a shared one.** That cue has now fired.
/// It is not extracted here because doing it properly means moving the
/// ritual out of three passing suites in the same change that adds a
/// picture nothing has ever compared, and those are two different risks
/// to take at once. The copy is deliberate and it is the last one that
/// should be written.
fn device_or_skip() -> Result<Option<Device>, DeviceError> {
    match Device::new(&DeviceDesc {
        app_name: "renew-cube-golden-tests",
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

/// FNV-1a 64 over a byte buffer: a content fingerprint for the sidecar.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// RGBA8 pixels as a binary PPM (P6, alpha dropped) — the humanly
/// viewable form of a candidate or a mismatch.
fn write_ppm(path: &Path, pixels: &[u8]) -> std::io::Result<()> {
    let mut ppm = format!("P6\n{SIZE} {SIZE}\n255\n").into_bytes();
    for pixel in pixels.as_chunks::<4>().0 {
        ppm.extend_from_slice(&pixel[..3]);
    }
    std::fs::write(path, ppm)
}

/// One checkpoint: a script, how long it runs, and the name its picture
/// carries.
struct Checkpoint {
    name: &'static str,
    script: Script,
    ticks: u32,
}

/// The arena as built, and the arena after the building script has been
/// at it.
///
/// **Four hundred ticks rather than a round fifty**, because the digging
/// script needs a few hundred to reach the mound and break it — the same
/// number the containment test uses, and for the same reason: a shorter
/// run draws a world the script has not changed yet, which is a second
/// picture of the first checkpoint.
const CHECKPOINTS: [Checkpoint; 2] = [
    Checkpoint {
        name: "arena-1",
        script: Script::Stand,
        ticks: 1,
    },
    Checkpoint {
        name: "built-400",
        script: Script::Build,
        ticks: 400,
    },
];

/// Run the script and draw the world it leaves behind.
fn capture(device: &Device, checkpoint: &Checkpoint) -> Result<Vec<u8>, String> {
    let world = run_world(&Options {
        script: checkpoint.script,
        ticks: checkpoint.ticks,
        ..Options::default()
    });
    let scene = build(world.grid());
    draw_clip_space_with(device, &scene, ClipSurface::Textured)
        .map_err(|error| format!("{}: {error}", checkpoint.name))
}

fn pixel_at(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

/// The checks that hold on any adapter, so a machine with no pinned lane
/// still learns something from running this.
///
/// These are the assertions the workflow's inline script used to make
/// about the encoded PNG, moved to where they can be read and probed. It
/// counted colours over the whole image rather than down one column
/// after a column probe passed against a uniform backdrop; that lesson
/// is kept.
fn assert_structure(pixels: &[u8], name: &str) {
    assert_eq!(
        pixels.len(),
        (SIZE as usize) * (SIZE as usize) * 4,
        "{name}: the readback is not {SIZE}x{SIZE} of RGBA"
    );

    let mut colours = std::collections::BTreeSet::new();
    for pixel in pixels.as_chunks::<4>().0 {
        colours.insert(*pixel);
    }
    assert!(
        colours.len() >= 4,
        "{name}: only {} colour(s) in the whole frame; the geometry did not draw",
        colours.len()
    );

    let corner = pixel_at(pixels, 0, 0);
    let centre = pixel_at(pixels, SIZE / 2, SIZE / 2);
    assert_ne!(
        centre, corner,
        "{name}: the middle of the picture is the backdrop, so nothing drew there"
    );
}

/// The full ritual for one checkpoint: exact bytes against the committed
/// golden on the pinned lane only, candidate plus provenance plus a
/// refusing `Err` when the golden does not exist yet.
///
/// **Byte-exact, decided before the first picture was recorded rather
/// than after one flaked.** The tolerance this tranche's sibling needed
/// was for stacked additive light, whose result depends on the order
/// fragments arrive; this frame has no blending at all — opaque textured
/// geometry resolved by a depth test, one unfiltered shadow tap, no
/// multisampling — so there is no stage here whose output is
/// order-dependent. If the pinned lane disagrees with itself anyway,
/// that is a finding about the lane and it gets a measured bound like
/// the one before it, not a shrug.
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
        "{name}.rgba — RGBA8, tightly packed, row-major, {SIZE}x{SIZE}\n\
         fnv1a-64 of the pixel bytes: {rendered_hash:#018x}\n\
         rendered by: {} (kind {:?}, vendor {:#06x}, device {:#06x}, driver {})\n\
         shaders: the mesh pipelines in the 3D renderer (see its compile record)\n\
         scene: the script named in the file name, run headless for the tick\n\
         count in the file name, then drawn through the textured mesh\n\
         pipeline onto an offscreen target\n\
         comparison: exact. This frame has no blending — opaque geometry,\n\
         a depth test, one unfiltered shadow tap — so nothing in it depends\n\
         on the order fragments arrive, which is what forced a tolerance on\n\
         the sprite game's crash frame.\n\
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
        write_ppm(&dir.join(format!("{name}.candidate.ppm")), pixels)
            .map_err(|error| format!("write candidate ppm: {error}"))?;
        std::fs::write(dir.join(format!("{name}.provenance.txt")), provenance)
            .map_err(|error| format!("write provenance sidecar: {error}"))?;
        return Err(format!(
            "golden is missing; candidate written to {} (fnv1a {rendered_hash:#018x}) — \
             inspect the .ppm, rename the candidate to the canonical name, and commit \
             it with its sidecar. This test never passes until an inspector does that.",
            candidate.display()
        ));
    }

    let expected =
        std::fs::read(&golden).map_err(|error| format!("read committed golden: {error}"))?;
    if pixels != expected.as_slice() {
        let actual = dir.join(format!("{name}.actual.rgba"));
        std::fs::write(&actual, pixels).map_err(|error| format!("write actual: {error}"))?;
        write_ppm(&dir.join(format!("{name}.actual.ppm")), pixels)
            .map_err(|error| format!("write actual ppm: {error}"))?;
        let first_diff = pixels
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(usize::MAX);
        let differing = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .zip(expected.as_chunks::<4>().0.iter())
            .filter(|(a, b)| a != b)
            .count();
        // The renderer belongs in this message. A divergence here is
        // either the change under test or the machine under it, and
        // those need opposite responses — so the first thing a reader
        // needs is something to compare against the committed sidecar.
        return Err(format!(
            "{name}: the picture changed. {differing} of {} pixels differ, first at byte \
             {first_diff}; rendered {} bytes (fnv1a {rendered_hash:#018x}) against a committed \
             {} bytes (fnv1a {:#018x}). Rendered by {} (kind {:?}, driver {}) — compare that \
             against {name}.provenance.txt before assuming the change under test is at fault. \
             The actual bytes and a .ppm of them are beside the golden.",
            (SIZE as usize) * (SIZE as usize),
            pixels.len(),
            expected.len(),
            fnv1a(&expected),
            adapter.name,
            adapter.kind,
            adapter.driver_version
        ));
    }
    Ok(())
}

/// **The world looks the way it looked**, checkpoint by checkpoint.
///
/// One test rather than one per checkpoint, because device creation is
/// the expensive part and a second device on a software rasterizer costs
/// more than the draw does.
///
/// Probed by giving the pass an empty item list: the frame comes back
/// one colour and this fails with `only 1 colour(s) in the whole frame;
/// the geometry did not draw`.
///
/// **That probe reaches the structural half only, and the byte-exact
/// half is unprobed until this runs on the pinned lane** — off that lane
/// the comparison is skipped by design, so no mutant applied here can
/// redden it. Its first real exercise is the run that finds no committed
/// golden, writes a candidate and fails on purpose; that failure is the
/// evidence the arm works, and it is the reason the ritual is built to
/// fail rather than to record.
#[test]
fn the_world_looks_the_way_it_looked() {
    let Some(device) = device_or_skip().expect("device creation must not fail outright") else {
        return;
    };

    // **Every checkpoint is compared before any of them reports.** The
    // obvious loop panics on the first mismatch, and that is wrong here
    // for a reason the refresh ritual made concrete: on a bootstrap run
    // the first checkpoint writes its candidate and fails, so the second
    // never renders and never writes one. Adopting two pictures would
    // then take two full ritual runs, and a refresh that regenerates
    // fewer files than it deleted is precisely the incident the
    // deletion list upstream was written to prevent. Collect, then
    // report.
    let mut refusals = Vec::new();
    for checkpoint in &CHECKPOINTS {
        let pixels = capture(&device, checkpoint).expect("the world draws");
        assert_structure(&pixels, checkpoint.name);
        if let Err(refusal) = compare_against_golden(&device, checkpoint.name, &pixels) {
            refusals.push(refusal);
        }
    }
    assert!(
        refusals.is_empty(),
        "{} of {} checkpoints did not match:\n\n{}",
        refusals.len(),
        CHECKPOINTS.len(),
        refusals.join("\n\n")
    );

    assert_no_validation_errors(&device);
}

/// **The two checkpoints are different pictures.**
///
/// Cheap, and it is the assertion that would have caught the version of
/// this file where both entries ran the same script: two goldens of the
/// same frame look like coverage and are not. It runs on any adapter,
/// because it compares two renders from one machine against each other
/// rather than against anything committed.
///
/// Probed by pointing `built-400` at `Script::Stand`: the pictures
/// become identical, this fails with `both checkpoints drew the same
/// picture`, and the golden test above still passes — which is the whole
/// point of writing it down separately.
#[test]
fn the_building_script_changes_what_is_drawn() {
    let Some(device) = device_or_skip().expect("device creation must not fail outright") else {
        return;
    };

    let first = capture(&device, &CHECKPOINTS[0]).expect("the arena draws");
    let second = capture(&device, &CHECKPOINTS[1]).expect("the built world draws");

    assert_ne!(
        first, second,
        "both checkpoints drew the same picture, so one of them is not a checkpoint"
    );
    assert_no_validation_errors(&device);
}
