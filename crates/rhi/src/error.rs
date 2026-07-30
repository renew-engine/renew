//! Per-domain error enums, hand-written. Callers match variants; the
//! carried raw codes are diagnostic context only.

#![deny(unsafe_code)]

use std::fmt;

/// Why a device could not be created or has stopped working.
#[derive(Debug)]
#[non_exhaustive]
pub enum DeviceError {
    /// No Vulkan runtime is loadable here — the graceful-skip seam for
    /// machines without a GPU stack (mirrors the window seam's
    /// loop-unavailable case).
    LoaderUnavailable {
        message: String,
    },
    /// [`super::Validation::Required`] was requested and the validation
    /// layer is not installed.
    ValidationUnavailable,
    /// No adapter satisfies the stated requirement.
    NoSuitableAdapter {
        requirement: &'static str,
    },
    /// A creation call failed; `call` names it, `code` is the raw
    /// driver result for diagnostics.
    Creation {
        call: &'static str,
        code: i32,
    },
    OutOfHostMemory {
        call: &'static str,
    },
    /// The device was lost; every later operation on it returns this
    /// until it is dropped and recreated.
    DeviceLost,
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LoaderUnavailable { message } => {
                write!(f, "no usable GPU runtime: {message}")
            }
            Self::ValidationUnavailable => {
                write!(f, "validation was required but the layer is not installed")
            }
            Self::NoSuitableAdapter { requirement } => {
                write!(f, "no adapter satisfies: {requirement}")
            }
            Self::Creation { call, code } => write!(f, "{call} failed (code {code})"),
            Self::OutOfHostMemory { call } => write!(f, "{call}: out of host memory"),
            Self::DeviceLost => write!(f, "device lost"),
        }
    }
}

impl std::error::Error for DeviceError {}

/// Why a render target failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum TargetError {
    /// Creating the presentation surface failed.
    SurfaceCreation {
        code: i32,
    },
    /// The graphics queue cannot present to this surface — an explicit
    /// v0 limitation surfaced honestly.
    PresentUnsupported,
    Creation {
        call: &'static str,
        code: i32,
    },
    /// A fence wait exceeded the watchdog — a hang made diagnosable.
    Timeout {
        call: &'static str,
    },
    OutOfDeviceMemory {
        call: &'static str,
    },
    DeviceLost,
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SurfaceCreation { code } => write!(f, "surface creation failed (code {code})"),
            Self::PresentUnsupported => {
                write!(f, "the graphics queue cannot present to this surface")
            }
            Self::Creation { call, code } => write!(f, "{call} failed (code {code})"),
            Self::Timeout { call } => write!(f, "{call} timed out"),
            Self::OutOfDeviceMemory { call } => write!(f, "{call}: out of device memory"),
            Self::DeviceLost => write!(f, "device lost"),
        }
    }
}

impl std::error::Error for TargetError {}

/// Why a pipeline could not be built.
#[derive(Debug)]
#[non_exhaustive]
pub enum PipelineError {
    /// The provided bytes are not structurally plausible SPIR-V;
    /// `reason` names the specific check.
    InvalidSpirv {
        stage: &'static str,
        reason: &'static str,
    },
    Creation {
        call: &'static str,
        code: i32,
    },
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpirv { stage, reason } => {
                write!(f, "{stage} shader bytes rejected: {reason}")
            }
            Self::Creation { call, code } => write!(f, "{call} failed (code {code})"),
        }
    }
}

impl std::error::Error for PipelineError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_displays_its_context() {
        let cases: Vec<(String, &str)> = vec![
            (
                DeviceError::LoaderUnavailable {
                    message: "no icd".to_string(),
                }
                .to_string(),
                "no icd",
            ),
            (DeviceError::ValidationUnavailable.to_string(), "required"),
            (
                DeviceError::NoSuitableAdapter {
                    requirement: "graphics queue",
                }
                .to_string(),
                "graphics queue",
            ),
            (
                DeviceError::Creation {
                    call: "vkCreateInstance",
                    code: -1,
                }
                .to_string(),
                "vkCreateInstance",
            ),
            (
                DeviceError::OutOfHostMemory {
                    call: "vkCreateDevice",
                }
                .to_string(),
                "vkCreateDevice",
            ),
            (DeviceError::DeviceLost.to_string(), "lost"),
            (TargetError::SurfaceCreation { code: -9 }.to_string(), "-9"),
            (TargetError::PresentUnsupported.to_string(), "present"),
            (
                TargetError::Timeout {
                    call: "vkWaitForFences",
                }
                .to_string(),
                "timed out",
            ),
            (
                PipelineError::InvalidSpirv {
                    stage: "vertex",
                    reason: "bad magic",
                }
                .to_string(),
                "bad magic",
            ),
            (
                PipelineError::Creation {
                    call: "vkCreateGraphicsPipelines",
                    code: -2,
                }
                .to_string(),
                "-2",
            ),
        ];
        for (text, needle) in cases {
            assert!(text.contains(needle), "`{text}` missing `{needle}`");
        }
    }
}
