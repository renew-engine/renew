//! The renderer half's oracle: a pool's packed bytes through the
//! billboard pipeline land where the arithmetic says, in the colours
//! the arithmetic says, twice identically.
//!
//! Computed expectations rather than a committed image: every value
//! below is chosen so the UNORM conversions are exact, which keeps the
//! comparison bytes-against-arithmetic on any conformant adapter.
#![cfg(feature = "render")]

use renew_particles::{
    CameraPush, EffectDesc, INSTANCE_STRIDE, ParticleBlend, ParticleRenderer, ParticleSystem, Seed,
    StreamId, VelocityCone,
};
use renew_rhi::{
    Attachment, ClearValue, Color, Device, DeviceDesc, DeviceError, Extent, LoadOp, Pass,
    RenderDesc, StoreOp, TargetFormat, Validation,
};

/// The instance colour, as the light behind bytes 64, 128 and 32.
const AUTHORED: [f32; 4] = [
    renew_rhi::srgb::decode(64),
    renew_rhi::srgb::decode(128),
    renew_rhi::srgb::decode(32),
    1.0,
];

fn strict() -> bool {
    std::env::var_os("RENEW_GOLDEN").is_some_and(|v| v == "1")
}

fn device_or_skip() -> Result<Option<Device>, DeviceError> {
    match Device::new(&DeviceDesc {
        app_name: "renew-particles-golden",
        validation: Validation::IfAvailable,
    }) {
        Ok(device) => Ok(Some(device)),
        Err(DeviceError::LoaderUnavailable { message }) if !strict() => {
            eprintln!("SKIP: no Vulkan runtime: {message}");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// One still particle, one white texel, one additive draw: the centre
/// of the picture answers with exactly the instance's colour, the
/// corner stays the clear, and the same frame twice is the same bytes.
#[test]
fn a_still_particle_draws_its_exact_colour() -> Result<(), Box<dyn std::error::Error>> {
    const SIZE: u32 = 64;
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    if device.depth_format_name().is_none() {
        assert!(!strict(), "the rendering lane's adapter must offer depth");
        eprintln!("SKIP: adapter offers no chain depth format");
        return Ok(());
    }
    let mut target = device.create_offscreen_target(Extent {
        width: SIZE,
        height: SIZE,
    })?;

    // A still particle: zero spread, zero speed, zero gravity, unit
    // drag — the pool's arithmetic leaves it exactly where it burst.
    let desc = EffectDesc {
        capacity: 4,
        lifetime: (10.0, 10.0),
        velocity: VelocityCone {
            axis: [0.0, 1.0, 0.0],
            spread: 0.0,
            speed: (0.0, 0.0),
        },
        gravity: [0.0, 0.0, 0.0],
        drag_per_step: 1.0,
        size: (1.0, 1.0),
        // The light those authored bytes stand for, not the bytes over
        // 255. A particle colour is chosen by looking at it, so it is
        // display-encoded; the attachment encodes on write, so handing it
        // the decoded light is what stores the byte back unchanged. The
        // expectation below is still the authored value, which is the
        // point of decoding here rather than restating it there.
        color: (AUTHORED, AUTHORED),
        tile: [0.0, 0.0, 1.0, 1.0],
    };
    let mut system = ParticleSystem::new(
        &desc,
        Seed::from_u64(20_260_811),
        StreamId::from_name("golden"),
    );
    // Centre of clip space, halfway into the reversed depth range: the
    // quad covers clip [-0.5, 0.5] squared through an identity camera.
    system.burst([0.0, 0.0, 0.5], 1);
    let mut instances = vec![0u8; desc.capacity as usize * INSTANCE_STRIDE];
    let live = system.write_instances(&mut instances);
    assert_eq!(live, 1, "one particle burst, one packed");

    let white = [255u8; 16];
    let renderer = ParticleRenderer::new(
        &device,
        TargetFormat::Rgba8Srgb,
        Extent {
            width: 2,
            height: 2,
        },
        &white,
        ParticleBlend::Additive,
        desc.capacity,
    )?;
    let identity = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let camera = CameraPush::from_parts(identity, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);

    let color = [Attachment::new(
        LoadOp::Clear(ClearValue::Color(Color::new(0.0, 0.0, 0.0, 1.0))),
        StoreOp::Store,
    )];
    // Depth cleared to the reversed far plane; the particle's 0.5 wins.
    let depth = Attachment::new(LoadOp::Clear(ClearValue::Depth(0.0)), StoreOp::Discard);
    let items = [renderer.item(&instances, live, &camera)];
    let pass = Pass::new(&color, &items).depth(depth);
    target.render(&RenderDesc::new(&[pass]))?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    let at = |x: u32, y: u32| {
        let index = ((y * SIZE + x) * 4) as usize;
        [
            pixels[index],
            pixels[index + 1],
            pixels[index + 2],
            pixels[index + 3],
        ]
    };
    // White texel times the colour, added to black: exactly the colour.
    // Alpha saturates against the opaque clear.
    assert_eq!(
        at(SIZE / 2, SIZE / 2),
        [64, 128, 32, 255],
        "the billboard's centre carries the instance colour exactly"
    );
    assert_eq!(
        at(1, 1),
        [0, 0, 0, 255],
        "the corner is outside the quad and stays the clear"
    );

    // The same frame twice is the same bytes — the cheap local form of
    // the golden property, on every adapter.
    let items = [renderer.item(&instances, live, &camera)];
    let pass = Pass::new(&color, &items).depth(depth);
    target.render(&RenderDesc::new(&[pass]))?;
    let mut second = vec![0u8; target.byte_len()];
    target.read_back_into(&mut second);
    assert_eq!(pixels, second, "the same frame rendered twice diverged");

    // The Debug form reports the type, not handles — pinned here where
    // a renderer exists to format.
    let shown = format!("{renderer:?}");
    assert!(shown.contains("ParticleRenderer"), "{shown}");

    // A scratch buffer too short for the live count is refused by
    // name, never truncated into a quiet wrong draw.
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let short = [0u8; 8];
        let _ = renderer.item(&short, live, &camera);
    }));
    assert!(refused.is_err(), "a short scratch buffer must refuse");
    Ok(())
}
