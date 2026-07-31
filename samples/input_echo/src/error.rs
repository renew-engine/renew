//! Why a run stopped: one enum, three meanings, three exit codes.

use core::fmt;

/// Why the sample could not finish what it was asked to do.
///
/// The three variants are the three things a caller — a shell, a test, a
/// CI lane — has to tell apart: an environment that cannot host this run,
/// a command line asking for something impossible, and a genuine
/// failure. Collapsing the first into the third is what turns "this
/// runner has no display" into a red build.
#[derive(Debug)]
pub enum SampleError {
    /// No display server here. Reported as a skip, never as a failure.
    Unavailable(String),
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
            Self::Unavailable(reason) => write!(f, "unavailable: {reason}"),
            Self::Usage(message) => write!(f, "usage: {message}"),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SampleError {}

#[cfg(test)]
mod tests {
    use super::SampleError;

    #[test]
    fn each_kind_says_which_kind_it_is() {
        assert_eq!(
            SampleError::Unavailable("no display".to_string()).to_string(),
            "unavailable: no display"
        );
        assert_eq!(
            SampleError::Usage("unknown argument".to_string()).to_string(),
            "usage: unknown argument"
        );
        // A genuine failure carries no prefix: it is the message.
        assert_eq!(
            SampleError::failed("writing the stats file", &"access denied").to_string(),
            "writing the stats file: access denied"
        );
    }
}
