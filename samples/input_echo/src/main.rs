//! The sample's process boundary, and nothing else.
//!
//! Every line of behaviour lives in the library beside this file: a
//! binary's code is invisible to unit tests, and a driver nobody can
//! test is a driver nobody should trust.

fn main() -> std::process::ExitCode {
    renew_sample_input_echo::exit_code(renew_sample_input_echo::run_cli(std::env::args().skip(1)))
}
