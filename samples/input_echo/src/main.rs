//! The sample's process boundary, and nothing else.
//!
//! Every line of behaviour lives in the library beside this file: a
//! binary's code is invisible to unit tests, and a driver nobody can
//! test is a driver nobody should trust.

/// On iOS this hands off and never returns: an app bundle is launched
/// with no arguments and no terminal, so the command line this parses
/// everywhere else does not exist there, and the doorway takes over.
#[cfg(target_os = "ios")]
fn main() -> std::process::ExitCode {
    renew_sample_input_echo::ios_main()
}

#[cfg(not(target_os = "ios"))]
fn main() -> std::process::ExitCode {
    renew_sample_input_echo::exit_code(renew_sample_input_echo::run_cli(std::env::args().skip(1)))
}
