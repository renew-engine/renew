//! The headless driver: a synthetic clock, an offscreen image, and one
//! readback at the end.
//!
//! This is the mode every gated number comes from. It reads no clock
//! into the schedule, so the tick count, the digests and the pixels are
//! a pure function of `(frames, seed)` — byte-comparable across runs,
//! processes and machines. The one clock it does read brackets each
//! frame for the timing summary, which is recorded and never gated.

use renew_frame::{
    Alpha, FrameLoop, FrameStats, FrameTiming, Nanos, StepBudget, Timestamp, Timestep,
};
use renew_platform::Clock;
use renew_rhi::{
    AdapterInfo, Device, DeviceDesc, Extent, PipelineDesc, RenderDesc, RenderPipeline, Validation,
    builtin,
};

use crate::cli::{Options, Report};
use crate::error::{SampleError, device_error, pipeline_error, render_error, target_error};
use crate::render::{Surface, clear_color};
use crate::world::World;

/// The headless image size. Small on purpose: the oracle compares every
/// pixel of it, and 64×64 is enough to prove the loop reached the
/// renderer.
pub const EXTENT: Extent = Extent {
    width: 64,
    height: 64,
};

/// The synthetic frame interval: exactly one timestep, so a run of N
/// frames executes exactly N steps, banks nothing, and drops nothing.
/// The expected numbers are readable without running anything, which is
/// what makes a wrong one obvious.
const FRAME_INTERVAL_NS: u64 = Timestep::HZ_60.nanos().get();

/// Frames whose cost is not part of the steady state: a driver's
/// lazy initialization lands in the first frames it renders. Everything
/// that allocates — device, target, pipeline, the readback buffer —
/// happens before frame zero, so the steady state is frames
/// `[WARMUP_FRAMES, N)`.
pub const WARMUP_FRAMES: u64 = 3;

/// Whether a frame draws the triangle or only clears.
///
/// Clear-only is not a CLI mode: it exists so the pixel oracle can
/// assert *every* pixel against the colour the world computed, with no
/// triangle covering the evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Draw {
    ClearOnly,
    Triangle,
}

/// A headless run in progress: everything it owns, and nothing it does
/// not.
///
/// The frame loop knows none of these things exist. It is handed a
/// timestamp and answers with a plan; this type is what turns that plan
/// into pixels.
pub struct HeadlessRun {
    device: Device,
    surface: Surface,
    pipeline: Option<RenderPipeline>,
    /// Measured, never gated: it brackets each frame for the timing
    /// summary and never reaches the schedule.
    clock: Clock,
    frame: FrameLoop,
    world: World,
    stats: FrameStats,
    timing: FrameTiming,
    /// Allocated once, before the first frame: reading back into a fresh
    /// buffer every frame would allocate inside the steady state.
    pixels: Vec<u8>,
    seed: u64,
    /// Frames planned so far — the synthetic clock's only input.
    planned: u64,
}

impl HeadlessRun {
    /// Bring up the renderer and anchor the schedule.
    ///
    /// # Errors
    ///
    /// [`SampleError::Unavailable`] when there is no GPU runtime here —
    /// the graceful skip every machine without one takes;
    /// [`SampleError::Failed`] when bring-up should have worked.
    pub fn start(seed: u64, draw: Draw) -> Result<Self, SampleError> {
        let device = Device::new(&DeviceDesc {
            app_name: "renew-hello-triangle",
            validation: Validation::Off,
        })
        .map_err(device_error)?;
        let target = device
            .create_offscreen_target(EXTENT)
            .map_err(target_error)?;
        let pixels = vec![0u8; target.byte_len()];
        let surface = Surface::Offscreen(target);
        let pipeline = match draw {
            Draw::ClearOnly => None,
            Draw::Triangle => Some(
                device
                    .create_pipeline(&PipelineDesc::new(
                        builtin::TRIANGLE_VS_SPV,
                        builtin::TRIANGLE_FS_SPV,
                        surface.format(),
                        builtin::TRIANGLE_VERTEX_COUNT,
                    ))
                    .map_err(pipeline_error)?,
            ),
        };
        Ok(Self {
            device,
            surface,
            pipeline,
            clock: Clock::start(),
            // Anchored after bring-up: time spent creating a device is
            // not time the simulation owes, and banking it would open
            // the run with a clamped burst and a drop count that means
            // nothing.
            frame: FrameLoop::new(
                Timestep::HZ_60,
                StepBudget::DEFAULT,
                Timestamp::from_nanos(0),
            ),
            world: World::new(seed),
            stats: FrameStats::new(),
            timing: FrameTiming::new(),
            pixels,
            seed,
            planned: 0,
        })
    }

    /// One frame, whole: plan, step, draw, tally. Every per-frame cost
    /// the sample has is in here, which is what the allocation gate
    /// wraps.
    ///
    /// # Errors
    ///
    /// [`SampleError::Failed`] if the renderer could not draw the frame.
    pub fn advance(&mut self) -> Result<(), SampleError> {
        let started = self.clock.elapsed_nanos();
        self.planned = self.planned.saturating_add(1);
        // Synthetic time. The schedule's whole input is this number.
        let now = Timestamp::from_nanos(FRAME_INTERVAL_NS.saturating_mul(self.planned));
        let plan = self.frame.begin_frame(now);
        for step in plan.steps() {
            self.world.step(step);
        }
        let clear = clear_color(&self.world, plan.alpha());
        let mut desc = RenderDesc::new(clear);
        if let Some(pipeline) = self.pipeline.as_ref() {
            desc = desc.pipeline(pipeline);
        }
        let drawn = self.surface.render(&desc).map_err(render_error)?;
        self.stats.absorb(&plan);
        let cpu = self.clock.elapsed_nanos().saturating_sub(started);
        self.timing.record(Nanos::from_nanos(cpu), drawn);
        Ok(())
    }

    /// Draw the world again without advancing it.
    ///
    /// What an OS repaint with no intervening update does on the
    /// windowed path, available here so a test can assert the cheap
    /// adapter-independent property: one tick drawn twice is the same
    /// bytes, on any adapter, with no committed image to compare
    /// against. It plans no frame and records no timing, because it is
    /// not one.
    ///
    /// # Errors
    ///
    /// [`SampleError::Failed`] if the renderer could not draw.
    pub fn redraw(&mut self) -> Result<(), SampleError> {
        let clear = clear_color(&self.world, Alpha::ZERO);
        let mut desc = RenderDesc::new(clear);
        if let Some(pipeline) = self.pipeline.as_ref() {
            desc = desc.pipeline(pipeline);
        }
        self.surface.render(&desc).map_err(render_error)?;
        Ok(())
    }

    /// Run `frames` frames.
    ///
    /// # Errors
    ///
    /// Whatever [`HeadlessRun::advance`] reports, at the frame that
    /// reported it.
    pub fn run(&mut self, frames: u64) -> Result<(), SampleError> {
        for _ in 0..frames {
            self.advance()?;
        }
        Ok(())
    }

    /// The last rendered image, in the buffer allocated before the first
    /// frame. RGBA8, tightly packed, row-major.
    pub fn read_back(&mut self) -> &[u8] {
        // With windowing compiled out the enum has one variant and this
        // pattern is irrefutable. It stays an `if let` on purpose: the
        // alternative is a `match` whose window arm nothing can ever
        // reach — a swapchain image is not read back here — which trades
        // a warning in one configuration for a permanently uncovered
        // line in the other.
        #[cfg_attr(not(feature = "window"), allow(irrefutable_let_patterns))]
        if let Surface::Offscreen(target) = &self.surface {
            target.read_back_into(&mut self.pixels);
        }
        &self.pixels
    }

    /// The world as the last frame left it — the oracle's other half:
    /// what the pixels are compared *against* is computed here, not
    /// stored in a committed image.
    #[must_use]
    pub const fn world(&self) -> &World {
        &self.world
    }

    /// The adapter this ran on. Forensics for a test that disagrees with
    /// its oracle: "which GPU" is the first question.
    #[must_use]
    pub fn adapter(&self) -> &AdapterInfo {
        self.device.adapter()
    }

    /// What the run has to say for itself.
    #[must_use]
    pub fn report(&self) -> Report {
        Report {
            seed: self.seed,
            stats: self.stats,
            timing: self.timing,
            state_hash: self.world.state_hash(),
        }
    }
}

/// Run the headless sample end to end.
///
/// # Errors
///
/// [`SampleError::Unavailable`] with no GPU runtime; otherwise whatever
/// bring-up or a frame reported.
pub fn run(options: &Options) -> Result<Report, SampleError> {
    let mut run = HeadlessRun::start(options.seed, Draw::Triangle)?;
    run.run(options.frames)?;
    Ok(run.report())
}

#[cfg(test)]
mod tests {
    use super::{EXTENT, FRAME_INTERVAL_NS, WARMUP_FRAMES};
    use renew_frame::Timestep;

    #[test]
    fn the_synthetic_interval_is_exactly_one_timestep() {
        // Exactly one timestep per frame is what makes ticks == frames
        // and dropped == 0 the expected answer, so any other number in
        // the digest line is a defect rather than a coincidence.
        assert_eq!(FRAME_INTERVAL_NS, Timestep::HZ_60.nanos().get());
    }

    /// The two numbers the README and the allocation gate quote. They
    /// are asserted rather than described so a silent edit to either one
    /// makes a liar of the test instead of the documentation.
    #[test]
    fn the_headless_image_is_small_and_square_and_the_warmup_is_three_frames() {
        assert_eq!(EXTENT.width, EXTENT.height);
        assert_eq!(EXTENT.width, 64);
        assert_eq!(WARMUP_FRAMES, 3);
    }
}
