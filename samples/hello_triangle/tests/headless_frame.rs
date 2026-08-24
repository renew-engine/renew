//! The loop-driven pixel oracle: after N frames the image holds the
//! colour the world computed for the N-th step, and nothing else.
//!
//! A state hash can be perfectly stable while the renderer ignores the
//! world entirely — this is the test that says otherwise. The expected
//! bytes are *computed* from the tick count rather than compared against
//! a committed image, so there is no artifact to refresh and no ritual
//! to keep honest: a loop that runs one step too many, or one too few,
//! changes the pixels.
//!
//! Skips (with a SKIP line) where there is no Vulkan runtime. Under
//! `RENEW_FRAME_STRICT=1` — the lane that exists to run this — a skip is
//! a failure instead, because a lane that passes by skipping proves
//! nothing.

use renew_sample_hello_triangle::{Draw, EXTENT, HeadlessRun, SampleError};

fn strict() -> bool {
    std::env::var_os("RENEW_FRAME_STRICT").is_some_and(|value| value == "1")
}

/// `Ok(None)` is the graceful skip; anything else is a failure for the
/// calling test to unwrap, so the panic lives in a `#[test]` body where
/// the test relaxation applies.
fn start_or_skip(seed: u64, draw: Draw) -> Result<Option<HeadlessRun>, SampleError> {
    match HeadlessRun::start(seed, draw) {
        Ok(run) => Ok(Some(run)),
        Err(SampleError::Unavailable(reason)) if !strict() => {
            eprintln!("SKIP: {reason}");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// The expected image: every pixel is the world's own colour, converted
/// the one way a conformant adapter is required to convert it.
fn expected_pixel(run: &HeadlessRun) -> [u8; 4] {
    let [red, green, blue] = run.world().clear_rgb8();
    [red, green, blue, 255]
}

#[test]
fn the_readback_holds_the_colour_the_world_computed_for_the_last_tick() {
    const FRAMES: u64 = 8;
    let Some(mut run) = start_or_skip(0, Draw::ClearOnly).expect("bring-up") else {
        return;
    };
    run.run(FRAMES).expect("eight clear frames");

    // Exactly one step per frame, by construction of the synthetic
    // clock — so the last tick is the eighth, and seed zero strides by
    // one. Stated as a literal as well as computed: if both moved
    // together, the oracle would be measuring itself.
    assert_eq!(run.world().ticks(), FRAMES);
    let expected = expected_pixel(&run);
    assert_eq!(expected, [8, 0, 0, 255], "the walked colour after 8 ticks");

    let adapter = run.adapter().name.clone();
    let pixels = run.read_back();
    assert_eq!(
        pixels.len(),
        (EXTENT.width as usize) * (EXTENT.height as usize) * 4
    );
    for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
        assert_eq!(*pixel, expected, "pixel {index} on adapter {adapter}");
    }

    // A repaint of the same tick — the clear-only frame shape, drawn
    // again — must reproduce the same image, exactly as the triangle
    // test proves for the drawing shape.
    let before = run.read_back().to_vec();
    run.redraw().expect("a clear-only repaint of the same tick");
    assert_eq!(
        before,
        run.read_back(),
        "a clear-only repaint diverged from the frame it repainted"
    );
}

/// The anti-vacuity half: one more step is one different image. Without
/// this, a renderer that painted a constant colour would satisfy the
/// test above on the day the world stopped moving.
#[test]
fn one_more_step_paints_a_different_image() {
    let Some(mut run) = start_or_skip(0, Draw::ClearOnly).expect("bring-up") else {
        return;
    };
    run.run(8).expect("eight clear frames");
    let after_eight = run.read_back().to_vec();
    run.run(1).expect("a ninth frame");
    let after_nine = run.read_back().to_vec();
    assert_eq!(expected_pixel(&run), [9, 0, 0, 255]);
    assert_ne!(after_eight, after_nine, "the ninth step changed nothing");
}

/// With the triangle drawn, the bytes are the adapter's business — but
/// two draws of one tick must still agree, and the triangle must
/// actually cover the middle of the image. Together they say the
/// pipeline ran and ran deterministically, without a committed golden.
#[test]
fn the_triangle_covers_the_image_and_one_tick_drawn_twice_is_the_same_bytes() {
    let Some(mut run) = start_or_skip(0, Draw::Triangle).expect("bring-up") else {
        return;
    };
    run.run(4).expect("four drawn frames");
    let clear = expected_pixel(&run);

    run.redraw().expect("a repaint of the same tick");
    let first = run.read_back().to_vec();
    run.redraw().expect("a second repaint of the same tick");
    let second = run.read_back().to_vec();
    assert_eq!(first, second, "one tick drawn twice diverged");

    let pixel_at = |x: u32, y: u32| {
        let base = ((y * EXTENT.width + x) * 4) as usize;
        [
            first[base],
            first[base + 1],
            first[base + 2],
            first[base + 3],
        ]
    };
    let centre = pixel_at(EXTENT.width / 2, EXTENT.height / 2);
    assert_ne!(centre, clear, "the triangle did not reach the middle");
    assert_eq!(pixel_at(0, 0), clear, "the corner is not the clear colour");
}

/// The report is the run's own account of itself, and the schedule it
/// describes is the one the synthetic clock asked for.
#[test]
fn the_report_says_exactly_what_the_synthetic_clock_asked_for() {
    let Some(mut run) = start_or_skip(3, Draw::Triangle).expect("bring-up") else {
        return;
    };
    run.run(16).expect("sixteen frames");
    let report = run.report();
    assert_eq!(report.seed, 3);
    assert_eq!(report.stats.frames(), 16);
    assert_eq!(report.stats.ticks(), 16, "one step per frame, exactly");
    assert_eq!(report.stats.steps_dropped(), 0);
    assert_eq!(report.state_hash, run.world().state_hash());
    // The measured half exists and is separate: sixteen frames were
    // timed, all of them drawn, and none of that reaches the digest.
    let json = report.json();
    assert!(json.contains("\"timing\":{\"count\":16,"), "{json}");
    assert!(json.contains("\"drawn\":16,\"skipped\":0"), "{json}");
    assert!(!report.digest_line().contains("min_ns"));
}
