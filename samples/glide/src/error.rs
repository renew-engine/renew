//! Why a run stopped: two meanings, two exit codes.
//!
//! No "environment unavailable" variant, deliberately: every mode this
//! sample has today is headless, so there is no display to be missing.
//! The variant arrives with the windowed feature, not before — an error
//! nothing can construct is an arm no test can reach.

use core::fmt;

/// Why the sample could not finish what it was asked to do.
#[derive(Debug)]
pub enum SampleError {
    /// The command line asks for something this sample cannot do.
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
            Self::Usage(message) | Self::Failed(message) => f.write_str(message),
        }
    }
}
