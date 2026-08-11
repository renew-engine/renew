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
use renew_render2d::{AtlasDesc, Canvas, SpriteRenderer, attachment};
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
const CLEAR_BYTES: [u8; 4] = [51, 102, 153, 255];
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
        TargetFormat::Rgba8Unorm,
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
    let color = [attachment(CLEAR)];
    let items = [sprites.item()];
    let passes = [Pass::new(&color, &items)];
    target.render(&RenderDesc::new(&passes)).expect("render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    let mut expected = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..SIZE * SIZE {
        expected.extend_from_slice(&CLEAR_BYTES);
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
