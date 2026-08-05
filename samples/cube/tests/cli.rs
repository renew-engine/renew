//! The command line, and the line a run answers with.

use renew_sample_cube::{CliError, Options, Script, describe, describe_json, parse, run, usage};

fn args(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

#[test]
fn no_arguments_gives_the_defaults() {
    let options = parse(Vec::new()).expect("nothing to object to");
    assert_eq!(options, Options::default());
    assert_eq!(options.script, Script::Stand);
    assert_eq!(options.ticks, 600);
    assert!(!options.json);
    assert!(!options.help);
}

#[test]
fn every_flag_parses() {
    let options = parse(args("--script build --ticks 42 --json")).expect("well formed");
    assert_eq!(options.script, Script::Build);
    assert_eq!(options.ticks, 42);
    assert!(options.json);

    assert!(parse(args("--help")).expect("well formed").help);
    assert!(parse(args("-h")).expect("well formed").help);
    assert_eq!(
        parse(args("--script patrol")).expect("well formed").script,
        Script::Patrol
    );
}

/// A malformed command line is refused with a reason rather than guessed at,
/// and every message names what to do about it.
#[test]
fn a_malformed_command_line_is_refused_with_a_reason() {
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
        parse(args("--ticks lots")),
        Err(CliError::NotANumber("lots".to_string()))
    );
    assert_eq!(
        parse(args("--script fly")),
        Err(CliError::UnknownScript("fly".to_string()))
    );

    // Every refusal names the thing at fault, so a reader can find it without
    // re-reading their own command line character by character.
    for (error, subject) in [
        (CliError::UnknownFlag("--wat".to_string()), "--wat"),
        (CliError::MissingValue("--ticks"), "--ticks"),
        (CliError::NotANumber("lots".to_string()), "lots"),
        (CliError::UnknownScript("fly".to_string()), "fly"),
    ] {
        let message = error.message();
        assert!(
            message.contains(subject),
            "the refusal does not name `{subject}`: {message}"
        );
    }
    // And the one that can suggest an alternative does.
    assert!(
        CliError::UnknownScript("fly".to_string())
            .message()
            .contains("patrol"),
        "an unknown script should say what the real ones are"
    );
}

/// **Every flag the parser accepts appears in the usage text.** A flag a
/// reader cannot discover is a flag that does not exist as far as anyone but
/// its author is concerned.
#[test]
fn the_usage_text_documents_every_flag() {
    let text = usage();
    for flag in ["--script", "--ticks", "--json", "--help"] {
        assert!(text.contains(flag), "usage does not mention {flag}");
    }
    for name in ["stand", "patrol", "build"] {
        assert!(
            text.contains(name),
            "usage does not mention the {name} script"
        );
    }
}

#[test]
fn every_script_has_a_name_that_round_trips() {
    for script in [Script::Stand, Script::Patrol, Script::Build] {
        assert_eq!(Script::from_name(script.name()), Some(script));
    }
    assert_eq!(Script::from_name("nope"), None);
}

/// **The point of the binary**: the same run answers the same, which is what
/// lets three platforms compare one line.
#[test]
fn a_run_is_reproducible() {
    let options = Options {
        script: Script::Build,
        ticks: 300,
        show: false,
        json: false,
        help: false,
        render: None,
    };
    let first = run(&options);
    let second = run(&options);
    assert_eq!(first, second, "the same run must answer the same");

    // And a different script answers differently, or the digest is not
    // watching the thing it claims to.
    let other = run(&Options {
        script: Script::Patrol,
        ..options.clone()
    });
    assert_ne!(first.digest, other.digest);

    // As does a different length.
    let longer = run(&Options {
        ticks: 301,
        ..options.clone()
    });
    assert_ne!(first.digest, longer.digest);
}

#[test]
fn standing_still_lands_and_builds_edits_the_world() {
    let stood = run(&Options {
        script: Script::Stand,
        ticks: 300,
        ..Options::default()
    });
    assert!(stood.grounded, "it falls onto the floor and stays there");
    assert_eq!(stood.edits, (0, 0), "and touches nothing");
    assert!(stood.solids > 0, "the arena is not empty");

    let built = run(&Options {
        script: Script::Build,
        ticks: 300,
        ..Options::default()
    });
    let (broken, placed) = built.edits;
    assert!(broken > 0, "building digs");
    assert!(placed > 0, "and places");
}

#[test]
fn the_answer_reads_and_parses() {
    let report = run(&Options {
        script: Script::Patrol,
        ticks: 120,
        ..Options::default()
    });

    let line = describe(&report);
    assert!(line.starts_with("cube script=patrol"));
    assert!(line.contains("ticks=120"));
    assert!(line.contains(&format!("digest=0x{:016x}", report.digest)));

    let json = describe_json(&report);
    assert!(
        json.contains("\"schema_version\":1"),
        "a machine-readable output carries its version from the first release"
    );
    assert!(json.contains("\"sample\":\"cube\""));
    assert!(json.contains("\"script\":\"patrol\""));
    assert!(json.contains("\"ticks\":120"));
    assert!(json.contains(&format!("\"digest\":\"0x{:016x}\"", report.digest)));
    assert!(json.starts_with('{') && json.ends_with('}'));
}

/// A zero-tick run is answerable rather than a special case: it is the world
/// as constructed, which is a legitimate thing to ask for a digest of.
#[test]
fn a_run_of_no_ticks_answers() {
    let report = run(&Options {
        ticks: 0,
        ..Options::default()
    });
    assert_eq!(report.ticks, 0);
    assert!(!report.grounded, "it has not fallen yet");
    assert!(report.solids > 0);
}

/// The whole binary, driven without spawning one.
#[test]
fn the_command_line_answers_with_an_exit_code() {
    use renew_sample_cube::run_cli;

    assert_eq!(run_cli(args("--ticks 30")), 0, "a good run succeeds");
    assert_eq!(run_cli(args("--ticks 30 --json")), 0);
    assert_eq!(run_cli(args("--help")), 0, "help is not a failure");
    assert_eq!(run_cli(args("--wat")), 1, "an unknown flag fails");
    assert_eq!(
        run_cli(args("--script fly")),
        1,
        "so does an unknown script"
    );
    assert_eq!(run_cli(args("--ticks lots")), 1);
}

/// `--render` takes a path, and keeps it.
#[test]
fn the_render_flag_carries_its_path() {
    let options =
        parse(["--render".to_string(), "out.png".to_string()]).expect("a path is a legal value");
    assert_eq!(
        options.render.as_deref(),
        Some(std::path::Path::new("out.png"))
    );
}

/// And says which flag is short of one, like every other value flag.
#[test]
fn the_render_flag_without_a_path_says_which_flag() {
    let refused = format!("{:?}", parse(["--render".to_string()]));
    assert!(
        refused.contains("MissingValue") && refused.contains("--render"),
        "the refusal must name the flag: {refused}"
    );
}

/// A build that cannot draw refuses the flag rather than ignoring it,
/// and names both ways to get a build that can.
///
/// **Gated to the feature being off**, because that is the arm under
/// test. With the feature on the same command draws a picture, which is
/// covered by the lane that runs the renderer.
#[cfg(not(feature = "render"))]
#[test]
fn a_build_without_the_renderer_refuses_to_draw() {
    let code = renew_sample_cube::run_cli(
        ["--ticks", "1", "--render", "unwritten.png"]
            .into_iter()
            .map(str::to_string),
    );
    assert_eq!(code, 2, "refusing to draw is a usage error");
    assert!(
        !std::path::Path::new("unwritten.png").exists(),
        "a build that cannot draw must not leave a file behind"
    );
}
