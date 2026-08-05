//! Process shell: argument strings in, exit code out. Everything else lives in
//! the library so tests can drive it without a process.

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(renew_sample_cube::run_cli(std::env::args().skip(1)))
}
