//! Why a run stopped: two meanings headless, a third with a window.
//!
//! The "environment unavailable" variant rides the windowed feature —
//! in a headless-only build there is no display to be missing, and an
//! error nothing can construct is an arm no test can reach.

use core::fmt;

/// Why the sample could not finish what it was asked to do.
#[derive(Debug)]
pub enum SampleError {
    /// The command line asks for something this sample cannot do.
    Usage(String),
    /// Something that should have worked did not.
    Failed(String),
    /// The environment cannot host this mode: a window was asked for
    /// and no display can provide one. Named separately so the message
    /// says whose fault it is — nobody's.
    #[cfg(feature = "window")]
    Unavailable(String),
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
            #[cfg(feature = "window")]
            Self::Unavailable(message) => f.write_str(message),
        }
    }
}
