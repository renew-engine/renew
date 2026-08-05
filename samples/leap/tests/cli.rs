//! The command line, and the line a run answers with.

use renew_sample_leap::{
    CliError, Options, Script, describe, describe_json, parse, run, run_cli, usage,
};

fn args(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

#[test]
fn no_arguments_gives_the_defaults() {
    let options = parse(Vec::new()).expect("nothing to object to");
    assert_eq!(options, Options::default());
    assert_eq!(options.script, Script::Stand);
    assert_eq!(options.ticks, 600);
}

#[test]
fn every_flag_parses() {
    let options = parse(args("--script hop --ticks 42 --json")).expect("well formed");
    assert_eq!(options.script, Script::Hop);
    assert_eq!(options.ticks, 42);
    assert!(options.json);
    assert!(parse(args("--help")).expect("well formed").help);
    assert!(parse(args("-h")).expect("well formed").help);
    assert_eq!(
        parse(args("--script dash")).expect("well formed").script,
        Script::Dash
    );
}

#[test]
fn a_malformed_command_line_is_refused_and_names_what_is_wrong() {
    assert_eq!(
        parse(args("--wat")),
        Err(CliError::UnknownFlag("--wat".to_string()))
    );
    assert_eq!(
        parse(args("--script")),
        Err(CliError::MissingValue("--script"))
    );
    assert_eq!(
        parse(args("--ticks")),
        Err(CliError::MissingValue("--ticks"))
    );
    assert_eq!(
        parse(args("--ticks soon")),
        Err(CliError::NotANumber("soon".to_string()))
    );
    assert_eq!(
        parse(args("--script fly")),
        Err(CliError::UnknownScript("fly".to_string()))
    );

    for (error, subject) in [
        (CliError::UnknownFlag("--wat".to_string()), "--wat"),
        (CliError::MissingValue("--ticks"), "--ticks"),
        (CliError::NotANumber("soon".to_string()), "soon"),
        (CliError::UnknownScript("fly".to_string()), "fly"),
    ] {
        assert!(
            error.message().contains(subject),
            "the refusal does not name `{subject}`: {}",
            error.message()
        );
    }
    assert!(
        CliError::UnknownScript("fly".to_string())
            .message()
            .contains("dash"),
        "an unknown script should say what the real ones are"
    );
}

/// **Every flag the parser accepts appears in the usage text.**
#[test]
fn the_usage_text_documents_every_flag() {
    let text = usage();
    for flag in ["--script", "--ticks", "--json", "--help"] {
        assert!(text.contains(flag), "usage does not mention {flag}");
    }
    for name in ["stand", "dash", "hop"] {
        assert!(text.contains(name), "usage does not mention {name}");
    }
}

#[test]
fn every_script_has_a_name_that_round_trips() {
    for script in [Script::Stand, Script::Dash, Script::Hop] {
        assert_eq!(Script::from_name(script.name()), Some(script));
    }
    assert_eq!(Script::from_name("nope"), None);
}

/// The same run answers the same, which is what lets three platforms compare
/// one line — and different runs answer differently, or the digest is not
/// watching what it claims to.
#[test]
fn a_run_is_reproducible_and_discriminating() {
    let options = Options {
        script: Script::Hop,
        ticks: 300,
        show: false,
        json: false,
        help: false,
    };
    assert_eq!(run(&options), run(&options));

    let other = run(&Options {
        script: Script::Dash,
        ..options
    });
    assert_ne!(run(&options).digest, other.digest);

    let longer = run(&Options {
        ticks: 301,
        ..options
    });
    assert_ne!(run(&options).digest, longer.digest);
}

#[test]
fn standing_still_lands_and_dashing_meets_the_wall() {
    let stood = run(&Options {
        script: Script::Stand,
        ticks: 200,
        ..Options::default()
    });
    assert!(stood.grounded, "it falls onto the floor and stays");
    assert!(!stood.against_wall, "and touches nothing");

    // Dashing right reaches the wall at x = 10 within its first leg.
    let dashed = run(&Options {
        script: Script::Dash,
        ticks: 120,
        ..Options::default()
    });
    assert!(dashed.grounded, "still on the floor");
    assert!(dashed.against_wall, "and pressed against the wall");
}

#[test]
fn the_answer_reads_and_parses() {
    let report = run(&Options {
        script: Script::Dash,
        ticks: 120,
        ..Options::default()
    });

    let line = describe(&report);
    assert!(line.starts_with("leap script=dash"));
    assert!(line.contains("ticks=120"));
    assert!(line.contains(&format!("digest=0x{:016x}", report.digest)));

    let json = describe_json(&report);
    assert!(json.contains("\"schema_version\":1"));
    assert!(json.contains("\"sample\":\"leap\""));
    assert!(json.contains("\"script\":\"dash\""));
    assert!(json.contains(&format!("\"digest\":\"0x{:016x}\"", report.digest)));
    assert!(json.starts_with('{') && json.ends_with('}'));
}

#[test]
fn a_run_of_no_ticks_answers() {
    let report = run(&Options {
        ticks: 0,
        ..Options::default()
    });
    assert_eq!(report.ticks, 0);
    assert!(!report.grounded, "it has not fallen yet");
}

/// The whole binary, driven without spawning one.
#[test]
fn the_command_line_answers_with_an_exit_code() {
    assert_eq!(run_cli(args("--ticks 30")), 0);
    assert_eq!(run_cli(args("--ticks 30 --show")), 0, "the picture prints");
    assert_eq!(
        run_cli(args("--ticks 30 --show --json")),
        0,
        "asking for both prints the machine-readable one, and does not crash"
    );
    assert_eq!(run_cli(args("--ticks 30 --json")), 0);
    assert_eq!(run_cli(args("--help")), 0, "help is not a failure");
    assert_eq!(run_cli(args("--wat")), 1);
    assert_eq!(run_cli(args("--script fly")), 1);
    assert_eq!(run_cli(args("--ticks soon")), 1);
}
