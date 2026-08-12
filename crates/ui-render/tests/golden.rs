//! The presenter's image oracle: computed, exact on every adapter.
//!
//! The same argument the 2D renderer's own golden makes, inherited
//! deliberately: opaque backgrounds at texel-aligned positions over a
//! solid clear. With alpha 1 the premultiplied blend degenerates to
//! replacement, texel-aligned edges land on pixel boundaries, and the
//! sampled region is uniform white — so a sprite's pixels are exactly
//! its tint, the expected image is computable in the test, and the
//! comparison is byte-exact with no committed artifact. The recorded
//! fallback is scoping this to the software rasterizer on the first
//! divergence report — no debate — and the scheduled sunset is the
//! move to a linear working space, which re-decides this test the way
//! the 2D renderer's own Testing notes record.

use renew_math::Alpha;
use renew_render2d::{AtlasDesc, Canvas, SpriteRenderer};
use renew_rhi::{
    Color, Device, DeviceDesc, DeviceError, Extent, Pass, RenderDesc, TargetFormat, Validation,
};
use renew_ui::{Edges, Fixed, Size, Style, Ui, UiLimits};
use renew_ui_render::{UiPresenter, atlas};

const SIZE: u32 = 64;
/// 51/255, 102/255, 153/255: unambiguous UNORM conversions.
const CLEAR: Color = Color {
    r: 51.0 / 255.0,
    g: 102.0 / 255.0,
    b: 153.0 / 255.0,
    a: 1.0,
};
/// The format every target in this file is created with.
///
/// Named once so the expectations below can be a function of it. When the
/// working space changes, this constant moves and every byte derived from
/// it follows — rather than a scatter of literals each of which is wrong
/// in the same way and none of which says why.
const TARGET: TargetFormat = TargetFormat::Rgba8Unorm;

/// What the attachment stores for the clear above.
///
/// Derived rather than written down. Under UNORM this is exactly
/// `[51, 102, 153, 255]`, which is what it always was — an authored byte
/// survives `round(255 x b/255)` unchanged. The point is what happens when
/// the format changes: these bytes follow it, and the assertions that read
/// them do not fail before a golden bootstrap path can write a candidate.
#[allow(
    clippy::expect_used,
    reason = "a colour target that stores no colour is the defect"
)]
fn clear_bytes() -> [u8; 4] {
    let channel = |value: f32| TARGET.stores(value).expect("a color target stores color");
    [
        channel(CLEAR.r),
        channel(CLEAR.g),
        channel(CLEAR.b),
        channel(CLEAR.a),
    ]
}
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn strict() -> bool {
    std::env::var_os("RENEW_GOLDEN").is_some_and(|v| v == "1")
}

/// `Ok(None)` is the graceful skip; under `RENEW_GOLDEN=1` a skip is a
/// failure and validation must be active — the lane's oracle can never
/// go silently vacuous. The same harness as the 2D renderer's goldens;
/// the copy is deliberate, and a fourth copy is the cue to extract a
/// shared one.
fn device_or_skip() -> Result<Option<Device>, DeviceError> {
    match Device::new(&DeviceDesc {
        app_name: "renew-ui-render-golden-tests",
        validation: Validation::IfAvailable,
    }) {
        Ok(device) => {
            assert!(
                device.validation_active() || !strict(),
                "RENEW_GOLDEN=1 but the validation layer is not active"
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

/// Paint `color` over a rectangle of the expected image — the same
/// replacement an opaque sprite performs.
fn paint(image: &mut [u8], x: u32, y: u32, width: u32, height: u32, color: [u8; 4]) {
    for row in y..y + height {
        for column in x..x + width {
            let at = ((row * SIZE + column) * 4) as usize;
            image[at..at + 4].copy_from_slice(&color);
        }
    }
}

/// **The presenter's whole road, held to computed bytes:** a solved
/// tree with two opaque texel-aligned backgrounds captures, emits, and
/// lands exactly where the solver put it, in exactly its background
/// colours, twice identically.
#[test]
fn a_presented_tree_lands_in_computed_pixels() {
    let Some(device) = device_or_skip().expect("device creation failed for a non-skip reason")
    else {
        return;
    };

    // The tree: two opaque panels at (8,8) and (32,8), 16 px square,
    // placed by margins so the solver's own arithmetic chooses the
    // texel-aligned spots the oracle expects.
    let mut ui = Ui::new(UiLimits { nodes: 8 });
    let root = ui.root();
    let panel = |margin_left: i32, background: [u8; 4]| Style {
        width: Size::Px(Fixed::from_int(16)),
        height: Size::Px(Fixed::from_int(16)),
        margin: Edges {
            left: Fixed::from_int(margin_left),
            top: Fixed::from_int(8),
            ..Edges::default()
        },
        background,
        ..Style::default()
    };
    let red = ui.insert(root).expect("room");
    ui.set_style(red, panel(8, RED));
    let blue = ui.insert(root).expect("room");
    ui.set_style(blue, panel(8, BLUE));
    ui.solve(Fixed::from_int(64), Fixed::from_int(64));

    let mut presenter = UiPresenter::new(8);
    presenter.advance(&ui);

    let canvas = Canvas::new(SIZE, SIZE).expect("nonzero canvas");
    let capacity = core::num::NonZeroU32::new(8).expect("nonzero capacity");
    let mut sprites = SpriteRenderer::new(
        &device,
        &AtlasDesc::new(
            Extent {
                width: atlas::WIDTH,
                height: atlas::HEIGHT,
            },
            &atlas::pixels(),
        ),
        canvas,
        TARGET,
        capacity,
    )
    .expect("sprite renderer");
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");

    sprites.begin();
    presenter.emit(Alpha::ZERO, &mut sprites);
    assert_eq!(sprites.sprites(), 2, "two panels, no transparent root");
    let color = [renew_rhi::color_attachment(CLEAR)];
    let items = [sprites.item()];
    let passes = [Pass::new(&color, &items)];
    target.render(&RenderDesc::new(&passes)).expect("render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    let mut expected = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..SIZE * SIZE {
        expected.extend_from_slice(&clear_bytes());
    }
    // Painted from the rectangles the solver itself answers, so the
    // oracle and the picture share one source of truth; the corner
    // pins below keep the solver honest about where that is.
    for (node, colour) in [(red, RED), (blue, BLUE)] {
        let rect = ui.rect(node).expect("solved");
        let x = u32::try_from(rect.x.trunc_int()).expect("on-canvas");
        let y = u32::try_from(rect.y.trunc_int()).expect("on-canvas");
        let width = u32::try_from(rect.width.trunc_int()).expect("on-canvas");
        let height = u32::try_from(rect.height.trunc_int()).expect("on-canvas");
        paint(&mut expected, x, y, width, height, colour);
    }
    assert_eq!(
        ui.rect(red).expect("solved").x,
        Fixed::from_int(8),
        "the fixture's arithmetic: red's margin puts it at x 8"
    );
    assert_eq!(
        ui.rect(blue).expect("solved").x,
        Fixed::from_int(32),
        "and blue follows red's outer edge plus its own margin"
    );
    assert_eq!(
        pixels, expected,
        "the presented tree must land exactly where the solver put it"
    );

    // Determinism self-check: the same frame twice is the same bytes.
    target.render(&RenderDesc::new(&passes)).expect("again");
    let mut second = vec![0u8; target.byte_len()];
    target.read_back_into(&mut second);
    assert_eq!(pixels, second, "same frame rendered twice diverged");

    drop(target);
    drop(sprites);
    let report = device.validation_report();
    assert_eq!(
        report.errors, 0,
        "validation errors; first messages: {:?}",
        report.first_messages
    );
}

/// **Text lands where measurement says, and nowhere else.** The glyph
/// oracle is deliberately weaker than the panel oracle above:
/// antialiased edge texels blend, and blended bytes are the recorded
/// reason the exact tier is scoped to opaque draws — so this test is
/// exact only where exactness is arguable (a fully opaque glyph core
/// under an opaque tint replaces, byte for byte), structural where it
/// is not (ink appears inside the measured advance box, nothing
/// changes outside it), and deterministic throughout (twice, same
/// bytes). The committed-bytes tier for full glyph images arrives
/// with the linear-space move that re-decides all of these.
#[test]
fn text_lands_inside_its_measured_box() {
    let Some(device) = device_or_skip().expect("device creation failed for a non-skip reason")
    else {
        return;
    };

    let canvas = Canvas::new(SIZE, SIZE).expect("nonzero canvas");
    let capacity = core::num::NonZeroU32::new(16).expect("nonzero capacity");
    let mut sprites = SpriteRenderer::new(
        &device,
        &AtlasDesc::new(
            Extent {
                width: atlas::WIDTH,
                height: atlas::HEIGHT,
            },
            &atlas::pixels(),
        ),
        canvas,
        TARGET,
        capacity,
    )
    .expect("sprite renderer");
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");

    let text = "Hi";
    let width = renew_ui::text::measure(text).trunc_int();
    let width = u32::try_from(width).expect("a two-glyph label fits any canvas");
    sprites.begin();
    renew_ui_render::emit_text(&mut sprites, 4.0, 4.0, text, [1.0, 1.0, 1.0, 1.0]);
    let color = [renew_rhi::color_attachment(CLEAR)];
    let items = [sprites.item()];
    let passes = [Pass::new(&color, &items)];
    target.render(&RenderDesc::new(&passes)).expect("render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    let texel = |x: u32, y: u32| {
        let at = ((y * SIZE + x) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    };
    // Structure: ink somewhere inside the measured box...
    let line_height = renew_ui_render::LINE_HEIGHT;
    let mut inked = 0u32;
    for y in 4..4 + line_height {
        for x in 4..4 + width {
            if texel(x, y) != clear_bytes() {
                inked += 1;
            }
        }
    }
    assert!(
        inked > 10,
        "two glyphs must leave more than a few texels of ink"
    );
    // ...and none past the bearing margin around it — bearings and
    // antialiasing may reach that far, and nothing may reach further.
    let bearing = renew_ui_render::BEARING;
    assert!(
        bearing <= 4,
        "the fixture pens at x 4; a bake with a wider bearing must move the pen"
    );
    for y in 0..SIZE {
        for x in 0..SIZE {
            let inside = (4 - bearing..4 + width + bearing).contains(&x)
                && (4..4 + line_height).contains(&y);
            if !inside {
                assert_eq!(
                    texel(x, y),
                    clear_bytes(),
                    "ink outside the measured box at ({x}, {y}) — measurement and picture disagree"
                );
            }
        }
    }
    // Exact where exactness holds: a thirteen-pixel glyph has fully
    // opaque core texels, and an opaque texel under an opaque tint
    // REPLACES — so exact white must exist in the picture. (Every
    // output alpha is 255 over an opaque clear, so edge texels cannot
    // be told apart by alpha; the exact claim is existential, and the
    // blended edges are exactly why the full-image tier waits for the
    // linear-space move.)
    let mut exact_cores = 0u32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            if texel(x, y) == [255, 255, 255, 255] {
                exact_cores += 1;
            }
        }
    }
    assert!(
        exact_cores > 0,
        "no fully-opaque glyph core replaced exactly: the bake lost its solid texels or the blend is wrong"
    );
    // Deterministic throughout.
    target.render(&RenderDesc::new(&passes)).expect("again");
    let mut second = vec![0u8; target.byte_len()];
    target.read_back_into(&mut second);
    assert_eq!(pixels, second, "same text rendered twice diverged");

    drop(target);
    drop(sprites);
    let report = device.validation_report();
    assert_eq!(
        report.errors, 0,
        "validation errors: {:?}",
        report.first_messages
    );
}
