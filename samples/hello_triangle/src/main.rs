//! The sample's process boundary, and nothing else.
//!
//! Every line of behaviour lives in the library beside this file: a
//! binary's code is invisible to unit tests, and a driver nobody can
//! test is a driver nobody should trust.

/// **Two callers, and only one of them is the system.**
///
/// An app bundle launch has no terminal and asks for a window. A command
/// line - `simctl spawn`, which is how the determinism and offscreen
/// lanes reach a simulator at all - asks for whatever it was told to
/// ask for. Entering the windowed doorway without an application traps:
/// the first version of this dispatched on the target alone and turned a
/// headless render into exit 133.
///
/// The decision itself lives in the library, where a test can reach it;
/// this file only acts on it, which is the rule the module header states.
#[cfg(all(target_os = "ios", feature = "window"))]
fn main() -> std::process::ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if renew_sample_hello_triangle::ios_wants_a_window(&arguments) {
        renew_sample_hello_triangle::ios_main()
    }
    renew_sample_hello_triangle::exit_code(renew_sample_hello_triangle::run_cli(arguments))
}

#[cfg(not(all(target_os = "ios", feature = "window")))]
fn main() -> std::process::ExitCode {
    renew_sample_hello_triangle::exit_code(renew_sample_hello_triangle::run_cli(
        std::env::args().skip(1),
    ))
}
