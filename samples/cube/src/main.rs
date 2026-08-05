//! The `cube` binary.

use renew_sample_cube::{describe, describe_json, parse, run, usage};

fn main() -> std::process::ExitCode {
    let options = match parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("cube: {}", error.message());
            return std::process::ExitCode::FAILURE;
        }
    };
    if options.help {
        print!("{}", usage());
        return std::process::ExitCode::SUCCESS;
    }
    let report = run(&options);
    if options.json {
        println!("{}", describe_json(&report));
    } else {
        println!("{}", describe(&report));
    }
    std::process::ExitCode::SUCCESS
}
