//! Turn the committed golden images into PNGs for the README.
//!
//! **A conversion, not a render.** The `.rgba` files beside the golden
//! test are the exact frames CI compares, rendered on the pinned
//! software rasterizer and carrying a provenance sidecar that names it.
//! Drawing a fresh picture here would need a second copy of the device
//! bring-up the test already owns, and would show a frame nothing
//! verifies — this shows the one that is verified, which is the better
//! picture to put in front of a reader.
//!
//! Run by hand when a golden is refreshed:
//!
//! ```text
//! cargo run -p renew-sample-glide --example make_picture
//! ```

use std::path::Path;

/// The goldens' shape, stated in their own provenance sidecars.
const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let goldens = root.join("tests").join("goldens");

    let mut written = 0;
    for name in ["soar-600", "dive-361", "sink-240"] {
        let source = goldens.join(format!("{name}.rgba"));
        let Ok(pixels) = std::fs::read(&source) else {
            eprintln!("{}: not there; nothing to convert", source.display());
            continue;
        };
        let expected = (WIDTH as usize) * (HEIGHT as usize) * 4;
        if pixels.len() != expected {
            eprintln!(
                "{}: {} bytes, expected {expected} — the goldens' shape has changed and this \
                 example's constants have not",
                source.display(),
                pixels.len()
            );
            continue;
        }
        match renew_png::encode(WIDTH, HEIGHT, &pixels) {
            Ok(png) => {
                let destination = root.join(format!("{name}.png"));
                match std::fs::write(&destination, &png) {
                    Ok(()) => {
                        println!("{}: {} bytes", destination.display(), png.len());
                        written += 1;
                    }
                    Err(error) => eprintln!("{}: {error}", destination.display()),
                }
            }
            Err(error) => eprintln!("{name}: {error}"),
        }
    }
    println!("{written} picture(s) written");
}
