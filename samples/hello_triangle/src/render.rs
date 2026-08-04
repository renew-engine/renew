//! Where a frame goes, and the colour it goes there with.

use renew_frame::Alpha;
use renew_rhi::{
    Attachment, ClearValue, Color, LoadOp, OffscreenTarget, RenderDesc, StoreOp, TargetError,
    TargetFormat,
};
#[cfg(feature = "window")]
use renew_rhi::{PresentOutcome, WindowTarget};

use crate::world::World;

/// The two places this sample draws: the offscreen image the headless
/// run reads back, and the window's swapchain.
///
/// A concrete two-variant enum inside the sample, not a `Renderer` trait
/// in the engine. The renderer is already the abstraction boundary the
/// engine chose; inventing a second one in another crate — with exactly
/// one implementation behind it — would be an interface built to dodge a
/// dependency.
///
/// **Not boxed, deliberately.** The window variant is the larger of the
/// two and grew again when the target gained per-slot buffer retention
/// — 472 bytes against 200 (measured on `x86_64` Windows). Boxing would
/// trade 272 bytes of stack, on a value held exactly once for the
/// lifetime of the sample, for a heap allocation at creation and a
/// pointer chase on **every frame**. That is the wrong direction on the
/// one path this crate is here to measure, and the allocation gate on
/// the window path now watches it.
#[expect(
    clippy::large_enum_variant,
    reason = "one long-lived value; indirection would cost a pointer chase per frame to save stack               bytes that are never duplicated"
)]
pub enum Surface {
    Offscreen(OffscreenTarget),
    #[cfg(feature = "window")]
    Window(WindowTarget),
}

impl Surface {
    /// The format a pipeline must target to draw here.
    #[must_use]
    pub fn format(&self) -> TargetFormat {
        match self {
            // The offscreen target's format is fixed by the renderer;
            // it exposes no accessor because there is nothing to choose.
            Self::Offscreen(_) => TargetFormat::Rgba8Unorm,
            #[cfg(feature = "window")]
            Self::Window(target) => target.format(),
        }
    }

    /// Draw one frame.
    ///
    /// `Ok(true)` means the frame reached its destination. `Ok(false)`
    /// means the window went dormant (minimized, or its swapchain went
    /// stale) and presented nothing — a skipped frame for the timing
    /// summary, never an error, and never a reason to stop stepping.
    ///
    /// # Errors
    ///
    /// [`TargetError`] as the renderer reports it: a lost device, a
    /// timed-out submission, an exhausted heap.
    pub fn render(&mut self, desc: &RenderDesc<'_>) -> Result<bool, TargetError> {
        match self {
            Self::Offscreen(target) => target.render(desc).map(|()| true),
            #[cfg(feature = "window")]
            Self::Window(target) => target
                .render(desc)
                .map(|outcome| outcome == PresentOutcome::Presented),
        }
    }
}

/// The one color attachment every frame here renders into: cleared to
/// `clear`, stored. The frame itself is composed at each call site —
/// the borrows in a composed frame end at the render call, so nothing
/// stores it.
#[must_use]
pub fn clear_attachment(clear: Color) -> Attachment {
    Attachment::new(LoadOp::Clear(ClearValue::Color(clear)), StoreOp::Store)
}

/// The clear colour for a frame standing `alpha` of the way past the
/// last executed step: the world's own colour, nudged toward the colour
/// the next step will paint.
///
/// This is the only place the interpolation factor is consumed, and it
/// is render-side by construction — the world is never asked about it.
/// At `alpha == 0` the result is exactly `k / 255` per channel, which
/// every conformant adapter converts to the byte `k`; every headless
/// frame lands exactly on a step boundary, which is why the headless
/// oracle can compare bytes at all.
#[must_use]
pub fn clear_color(world: &World, alpha: Alpha) -> Color {
    let current = world.clear_rgb8();
    let next = world.next_clear_rgb8();
    let mix = |from: u8, to: u8| {
        let from = f32::from(from);
        (from + (f32::from(to) - from) * alpha.get()) / 255.0
    };
    Color::new(
        mix(current[0], next[0]),
        mix(current[1], next[1]),
        mix(current[2], next[2]),
        1.0,
    )
}

#[cfg(test)]
mod tests {
    use super::clear_color;
    use crate::world::World;
    use renew_frame::{Alpha, Nanos, Step};
    use renew_rhi::Color;

    fn stepped(seed: u64, steps: u64) -> World {
        let mut world = World::new(seed);
        for tick in 0..steps {
            world.step(Step {
                tick,
                dt: Nanos::from_nanos(16_666_667),
                sim_time: Nanos::from_nanos(tick * 16_666_667),
            });
        }
        world
    }

    /// The property the pixel oracle rests on: on a step boundary the
    /// colour is exactly the world's own channels over 255, with no
    /// rounding anywhere for the adapter to disagree about.
    #[test]
    fn on_a_step_boundary_the_colour_is_exactly_the_worlds_own() {
        let world = stepped(0, 8);
        assert_eq!(world.clear_rgb8(), [8, 0, 0]);
        assert_eq!(
            clear_color(&world, Alpha::ZERO),
            Color::new(8.0 / 255.0, 0.0, 0.0, 1.0)
        );
    }

    #[test]
    fn between_steps_the_colour_leans_toward_the_next_one() {
        let world = stepped(0, 8);
        // Alpha comes from a plan, so it cannot be built directly here;
        // half a timestep of banked time is what produces one half.
        let mut frame = renew_frame::FrameLoop::new(
            renew_frame::Timestep::HZ_60,
            renew_frame::StepBudget::DEFAULT,
            renew_frame::Timestamp::from_nanos(0),
        );
        let plan = frame.begin_frame(renew_frame::Timestamp::from_nanos(8_333_333));
        let alpha = plan.alpha();
        assert!(alpha.get() > 0.49 && alpha.get() < 0.51, "{alpha:?}");
        let colour = clear_color(&world, alpha);
        let low = 8.0 / 255.0;
        let high = 9.0 / 255.0;
        assert!(colour.r > low && colour.r < high, "{colour:?}");
        // The channels the walk has not reached yet stay where they are.
        assert!((colour.g - 0.0).abs() < f32::EPSILON);
        assert!((colour.a - 1.0).abs() < f32::EPSILON);
    }
}
