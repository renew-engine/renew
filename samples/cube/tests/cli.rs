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
        window_ticks: None,
        show: false,
        json: false,
        help: false,
        window: false,
        view: renew_sample_cube::View::Player,
        render: None,
        atlas: None,
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
        json.contains("\"schema_version\":2"),
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

/// A viewpoint is three numbers, and anything else says so.
#[test]
fn a_viewpoint_is_three_numbers_and_says_so_when_it_is_not() {
    // Bit patterns: these are parsed literals, so exactness is the claim,
    // and comparing bits is the tree's own idiom for that.
    assert_eq!(
        renew_sample_cube::triple("1,2,3")
            .expect("three numbers")
            .map(f32::to_bits),
        [1.0f32, 2.0, 3.0].map(f32::to_bits)
    );
    assert_eq!(
        renew_sample_cube::triple(" -1.5 , 0 , 2.25 ")
            .expect("spaces are allowed")
            .map(f32::to_bits),
        [-1.5f32, 0.0, 2.25].map(f32::to_bits)
    );
    for bad in ["1,2", "1,2,3,4", "1,two,3", ""] {
        let refused = format!("{:?}", renew_sample_cube::triple(bad));
        assert!(
            refused.starts_with("Err("),
            "`{bad}` is not a viewpoint and should be refused: {refused}"
        );
    }
}

/// `--view` names a viewpoint, and an unknown one is refused.
#[test]
fn the_view_flag_selects_a_viewpoint() {
    let iso = parse(["--view".to_string(), "iso".to_string()]).expect("iso is a view");
    assert_eq!(iso.view, renew_sample_cube::View::Isometric);

    let free = parse([
        "--eye".to_string(),
        "1,2,3".to_string(),
        "--look-at".to_string(),
        "4,5,6".to_string(),
    ])
    .expect("two points are a view");
    assert_eq!(
        free.view,
        renew_sample_cube::View::Free {
            eye: [1.0, 2.0, 3.0],
            target: [4.0, 5.0, 6.0]
        }
    );

    // The other order too: each flag fills in the half the other left,
    // and only one order was exercised until this line existed.
    let reversed = parse([
        "--look-at".to_string(),
        "4,5,6".to_string(),
        "--eye".to_string(),
        "1,2,3".to_string(),
    ])
    .expect("order should not matter");
    assert_eq!(reversed.view, free.view, "the flags should commute");

    let refused = format!(
        "{:?}",
        parse(["--view".to_string(), "sideways".to_string()])
    );
    assert!(
        refused.contains("sideways"),
        "the refusal names it: {refused}"
    );
}

/// A flag everyone knows, given a value nobody does.
///
/// The distinction is not pedantry: told the *flag* is unknown, a reader
/// checks the spelling of `--view`, which was never the problem.
#[test]
fn a_bad_view_name_blames_the_value_rather_than_the_flag() {
    let refused = parse(["--view".to_string(), "sideways".to_string()]);
    assert!(
        matches!(&refused, Err(CliError::UnknownView(name)) if name == "sideways"),
        "got {refused:?}"
    );
    let message = refused.unwrap_err().message();
    assert!(message.contains("sideways"), "{message}");
    assert!(
        message.contains("player") && message.contains("iso"),
        "the refusal should say what the choices are: {message}"
    );
}

/// The names the parser takes are the names the usage text offers.
#[test]
fn both_spellings_of_the_isometric_view_are_accepted() {
    for name in ["iso", "isometric"] {
        let options = parse(["--view".to_string(), name.to_string()])
            .unwrap_or_else(|error| panic!("`{name}` should parse: {}", error.message()));
        assert_eq!(
            options.view,
            renew_sample_cube::View::Isometric,
            "for `{name}`"
        );
    }
}

/// **A build with no window says both roads.** Asking a build without the
/// feature to play is not a mistake a reader can see from the outside —
/// the flag exists, it parses, and the binary simply cannot honour it —
/// so the refusal has to name the way to get one.
#[cfg(not(feature = "window"))]
#[test]
fn a_build_without_a_window_says_how_to_get_one() {
    use renew_sample_cube::run_cli;

    let code = run_cli(["--window".to_string()]);
    assert_eq!(
        code, 2,
        "asking for a window this build has not is a refusal"
    );
}

/// The same refusal, read rather than counted: it names the tool's road
/// and the cargo one, in that order, because a reader who typed a `renew`
/// command has no use for a cargo flag on its own.
#[cfg(not(feature = "window"))]
#[test]
fn the_refusal_names_both_roads() {
    use renew_sample_cube::run_cli;

    // `play` is private, so the message is reached the way a user reaches
    // it. Capturing stderr from inside the process is not worth a
    // dependency; what this pins is that the run refuses rather than
    // silently doing nothing, and the wording is asserted in the unit
    // test beside the function.
    assert_eq!(run_cli(["--window".to_string()]), 2);
    assert_eq!(
        run_cli(["--window".to_string(), "--json".to_string()]),
        2,
        "the refusal does not depend on how the answer was to be formatted"
    );
}

/// **Asking to play and to draw at once is refused**, rather than one of
/// the two being dropped without a word.
#[test]
fn playing_and_drawing_at_once_is_refused() {
    let cases = [
        (vec!["--window", "--render", "out.png"], "--render"),
        (vec!["--window", "--show"], "--show"),
        (vec!["--window", "--view", "player"], "--view"),
        (vec!["--window", "--eye", "1,2,3"], "--view"),
        (vec!["--window", "--look-at", "0,0,0"], "--view"),
    ];
    for (arguments, named) in cases {
        let owned: Vec<String> = arguments.iter().map(|a| (*a).to_string()).collect();
        let refused = parse(owned);
        assert!(
            matches!(&refused, Err(CliError::WindowAndStill(flag)) if *flag == named),
            "{arguments:?} should be refused naming `{named}`, got {refused:?}"
        );
        let message = refused.unwrap_err().message();
        assert!(message.contains(named), "{message}");
        assert!(message.contains("--window"), "{message}");
    }
}

/// The flags a window *does* honour keep working.
#[test]
fn a_window_still_takes_a_script_a_bound_and_a_format() {
    let arguments: Vec<String> = ["--window", "--script", "build", "--ticks", "30", "--json"]
        .iter()
        .map(|a| (*a).to_string())
        .collect();
    let options = parse(arguments).expect("these belong with a window");
    assert!(options.window);
    assert_eq!(options.script, Script::Build);
    assert_eq!(options.window_ticks, Some(30));
    assert!(options.json);
}

/// The atlas has a command line, because a texture nobody can look at is
/// a texture nobody can check.
#[test]
fn the_atlas_flag_parses_a_path() {
    let arguments: Vec<String> = ["--atlas", "blocks.png"]
        .iter()
        .map(|a| (*a).to_string())
        .collect();
    let options = parse(arguments).expect("--atlas takes a path");
    assert_eq!(
        options.atlas.as_deref(),
        Some(std::path::Path::new("blocks.png"))
    );
}

/// And says so when the path is missing.
#[test]
fn the_atlas_flag_needs_a_path() {
    let refused = parse(["--atlas".to_string()]);
    assert!(
        matches!(&refused, Err(CliError::MissingValue("--atlas"))),
        "got {refused:?}"
    );
}

/// **The atlas flag writes an atlas.** The parse tests above check the
/// path is read; this checks something arrives at it, which is the half
/// a reader cares about.
#[cfg(feature = "render")]
#[test]
fn asking_for_the_atlas_writes_a_png() {
    use renew_sample_cube::run_cli;

    let path = std::env::temp_dir().join("renew-cube-atlas-test.png");
    drop(std::fs::remove_file(&path));

    let code = run_cli(vec![
        "--atlas".to_string(),
        path.to_string_lossy().into_owned(),
    ]);
    assert_eq!(code, 0, "writing the atlas should succeed");

    let written = std::fs::read(&path).expect("the atlas should be on disk");
    assert_eq!(
        &written[..8],
        &[137, 80, 78, 71, 13, 10, 26, 10],
        "what was written is not a PNG"
    );
    // The dimensions the atlas declares, read back out of the header
    // rather than trusted: this is the one place the two could disagree.
    let width = u32::from_be_bytes([written[16], written[17], written[18], written[19]]);
    let height = u32::from_be_bytes([written[20], written[21], written[22], written[23]]);
    assert_eq!(
        (width, height),
        (
            renew_sample_cube::atlas::WIDTH,
            renew_sample_cube::atlas::HEIGHT
        )
    );

    drop(std::fs::remove_file(&path));
}

/// A path that cannot be written is said out loud rather than ignored.
#[cfg(feature = "render")]
#[test]
fn an_unwritable_atlas_path_is_refused() {
    use renew_sample_cube::run_cli;

    // A directory that does not exist, so the write fails for a reason
    // that has nothing to do with permissions and is the same on every
    // platform.
    let path = std::env::temp_dir()
        .join("renew-cube-no-such-directory")
        .join("atlas.png");
    assert_eq!(
        run_cli(vec![
            "--atlas".to_string(),
            path.to_string_lossy().into_owned(),
        ]),
        1,
        "a write that cannot happen is a failure, not a silent success"
    );
}

/// A build with no renderer says both roads, as it does for `--render`.
#[cfg(not(feature = "render"))]
#[test]
fn a_build_without_a_renderer_refuses_the_atlas() {
    use renew_sample_cube::run_cli;

    assert_eq!(
        run_cli(vec!["--atlas".to_string(), "blocks.png".to_string()]),
        1,
        "the atlas is one of the renderer's pictures"
    );
}
