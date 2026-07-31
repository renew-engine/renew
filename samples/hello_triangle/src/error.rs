//! Why a run stopped: one enum, three meanings, three exit codes — and
//! the four places the renderer's own errors are translated into it.

use core::fmt;

use renew_rhi::{DeviceError, PipelineError, TargetError};

/// Why the sample could not finish what it was asked to do.
///
/// The three variants are the three things a caller — a shell, a test, a
/// CI lane — has to tell apart: an environment that cannot host this run,
/// a command line asking for something impossible, and a genuine
/// failure. Collapsing the first into the third is what turns "this
/// machine has no GPU" into a red build.
#[derive(Debug)]
pub enum SampleError {
    /// The environment cannot run this here — no Vulkan runtime, no
    /// display server, no presentable surface. Reported as a skip.
    Unavailable(String),
    /// The command line asks for something this build cannot do.
    Usage(String),
    /// Something that should have worked did not.
    Failed(String),
}

impl SampleError {
    /// A failure with the operation that produced it in front of it.
    #[must_use]
    pub fn failed(context: &str, cause: &dyn fmt::Display) -> Self {
        Self::Failed(format!("{context}: {cause}"))
    }
}

impl fmt::Display for SampleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "unavailable: {reason}"),
            Self::Usage(message) => write!(f, "usage: {message}"),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SampleError {}

/// A device that would not come up. A missing runtime is the machine
/// saying "not here", which every other kind of failure is not.
#[must_use]
pub fn device_error(error: DeviceError) -> SampleError {
    match error {
        DeviceError::LoaderUnavailable { message } => {
            SampleError::Unavailable(format!("no GPU runtime: {message}"))
        }
        other => SampleError::failed("device bring-up", &other),
    }
}

/// A target that would not come up. A surface this machine cannot
/// present to is the same "not here" answer: real on CI runners without
/// a compositor, and not a defect in the sample.
#[must_use]
pub fn target_error(error: TargetError) -> SampleError {
    match error {
        TargetError::PresentUnsupported { reason } => {
            SampleError::Unavailable(format!("cannot present to this surface: {reason}"))
        }
        other => SampleError::failed("render target", &other),
    }
}

/// A pipeline that would not build. Never the environment's fault: the
/// shaders are compiled into the binary.
#[must_use]
#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err hands the error over by value; a by-reference signature would need a closure at every call site"
)]
pub fn pipeline_error(error: PipelineError) -> SampleError {
    SampleError::failed("pipeline", &error)
}

/// A frame that would not draw.
#[must_use]
#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err hands the error over by value; a by-reference signature would need a closure at every call site"
)]
pub fn render_error(error: TargetError) -> SampleError {
    SampleError::failed("render", &error)
}

#[cfg(test)]
mod tests {
    use super::{SampleError, device_error, pipeline_error, render_error, target_error};
    use renew_rhi::{DeviceError, PipelineError, TargetError};

    #[test]
    fn each_kind_says_which_kind_it_is() {
        assert_eq!(
            SampleError::Unavailable("no Vulkan runtime".to_string()).to_string(),
            "unavailable: no Vulkan runtime"
        );
        assert_eq!(
            SampleError::Usage("unknown argument".to_string()).to_string(),
            "usage: unknown argument"
        );
        // A genuine failure carries no prefix: it is the message.
        assert_eq!(
            SampleError::failed("pipeline", &"bad SPIR-V").to_string(),
            "pipeline: bad SPIR-V"
        );
    }

    #[test]
    fn a_missing_runtime_is_unavailable_and_everything_else_is_a_failure() {
        let missing = device_error(DeviceError::LoaderUnavailable {
            message: "vulkan-1.dll not found".to_string(),
        });
        assert!(matches!(missing, SampleError::Unavailable(_)), "{missing}");
        assert!(missing.to_string().contains("vulkan-1.dll"));

        let lost = device_error(DeviceError::DeviceLost);
        assert!(matches!(lost, SampleError::Failed(_)), "{lost}");
        assert!(lost.to_string().starts_with("device bring-up:"));
    }

    #[test]
    fn a_surface_that_cannot_present_is_unavailable_and_everything_else_is_a_failure() {
        let unsupported = target_error(TargetError::PresentUnsupported {
            reason: "no swapchain extension",
        });
        assert!(matches!(unsupported, SampleError::Unavailable(_)));
        assert!(unsupported.to_string().contains("no swapchain extension"));

        let timeout = target_error(TargetError::Timeout {
            call: "vkWaitForFences",
        });
        assert!(matches!(timeout, SampleError::Failed(_)), "{timeout}");
    }

    #[test]
    fn pipeline_and_render_failures_name_the_stage_they_came_from() {
        let pipeline = pipeline_error(PipelineError::InvalidSpirv {
            stage: "vertex",
            reason: "bad magic number",
        });
        assert!(pipeline.to_string().starts_with("pipeline:"));
        assert!(pipeline.to_string().contains("bad magic number"));

        let render = render_error(TargetError::DeviceLost);
        assert!(render.to_string().starts_with("render:"));
    }
}
