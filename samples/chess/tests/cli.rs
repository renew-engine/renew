//! The command line, and the line a run answers with.

use renew_sample_chess::{
    CliError, MAX_DEPTH, Mode, Options, Report, describe, describe_json, next_move, parse, run,
    run_cli, usage,
};
use renew_sample_chess_rules::{Board, Outcome};

fn args(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

/// `run` for the cases where the setup is legal by construction, so every test
/// below is not obliged to say so. The ones about refusals call `run` itself.
#[expect(
    clippy::expect_used,
    reason = "a test helper: a panic here is the failure being reported"
)]
fn ran(options: &Options) -> Report {
    run(options).expect("a setup with no illegal move in it")
}

const KIWIPETE: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";

#[test]
fn no_arguments_counts_to_four_from_the_start() {
    let options = parse(Vec::new()).expect("nothing to object to");
    assert_eq!(options.mode, Mode::Count);
    assert_eq!(options.depth, 4);
    assert_eq!(options.position, Board::initial());
    assert!(!options.json);
}

#[test]
fn every_flag_parses() {
    let options = parse(args("--play --depth 12 --json")).expect("well formed");
    assert_eq!(options.mode, Mode::Play);
    assert_eq!(options.depth, 12);
    assert!(options.json);

    assert!(parse(args("--help")).expect("well formed").help);
    assert!(parse(args("-h")).expect("well formed").help);
    assert_eq!(
        parse(args("--play --count")).expect("well formed").mode,
        Mode::Count,
        "the last mode named wins"
    );

    let with_position = parse(
        std::iter::once("--fen".to_string())
            .chain(std::iter::once(KIWIPETE.to_string()))
            .collect::<Vec<_>>(),
    )
    .expect("a published position");
    assert_ne!(with_position.position, Board::initial());
}

#[test]
fn a_malformed_command_line_is_refused_and_names_what_is_wrong() {
    assert_eq!(
        parse(args("--wat")),
        Err(CliError::UnknownFlag("--wat".to_string()))
    );
    assert_eq!(
        parse(args("--depth")),
        Err(CliError::MissingValue("--depth"))
    );
    assert_eq!(parse(args("--fen")), Err(CliError::MissingValue("--fen")));
    assert_eq!(
        parse(args("--depth deep")),
        Err(CliError::NotANumber("deep".to_string()))
    );
    assert_eq!(
        parse(args("--fen banana")),
        Err(CliError::BadPosition("banana".to_string()))
    );

    for (error, subject) in [
        (CliError::UnknownFlag("--wat".to_string()), "--wat"),
        (CliError::MissingValue("--fen"), "--fen"),
        (CliError::NotANumber("deep".to_string()), "deep"),
        (CliError::BadPosition("banana".to_string()), "banana"),
        (CliError::DepthTooGreat(9), "9"),
    ] {
        assert!(
            error.message().contains(subject),
            "the refusal does not name `{subject}`: {}",
            error.message()
        );
    }
}

/// **A depth that would outlive the caller is refused with a number**, because
/// a command that appears to hang is worse than one that says no. Perft grows
/// about thirtyfold per level.
#[test]
fn a_count_too_deep_is_refused_and_a_play_that_deep_is_not() {
    assert_eq!(
        parse(args("--count --depth 9")),
        Err(CliError::DepthTooGreat(9))
    );
    assert!(
        parse(args("--count --depth 6")).is_ok(),
        "the limit itself is allowed"
    );
    assert_eq!(MAX_DEPTH, 6);

    // Playing nine half-moves is nothing, so the limit must not apply to it —
    // and the flag order must not matter either.
    assert!(parse(args("--play --depth 9")).is_ok());
    assert!(
        parse(args("--depth 9 --play")).is_ok(),
        "the limit belongs to the mode, which may be named after the depth"
    );
}

/// **Every flag the parser accepts appears in the usage text.**
#[test]
fn the_usage_text_documents_every_flag() {
    let text = usage();
    for flag in ["--count", "--play", "--depth", "--fen", "--json", "--help"] {
        assert!(text.contains(flag), "usage does not mention {flag}");
    }
}

/// **The oracle, through the command line.** These counts are published facts
/// about chess, so a wrong one here is a wrong rule rather than a wrong
/// opinion.
#[test]
fn counting_matches_the_published_numbers() {
    for (depth, expected) in [(1u32, 20u64), (2, 400), (3, 8_902), (4, 197_281)] {
        let report = ran(&Options {
            depth,
            ..Options::default()
        });
        assert_eq!(report.nodes, expected, "perft({depth}) from the start");
    }

    let kiwipete = Board::from_fen(KIWIPETE).expect("a published position");
    let report = ran(&Options {
        position: kiwipete,
        depth: 3,
        ..Options::default()
    });
    assert_eq!(report.nodes, 97_862, "perft(3) from Kiwipete");
}

#[test]
fn counting_to_no_depth_is_the_position_itself() {
    let report = ran(&Options {
        depth: 0,
        ..Options::default()
    });
    assert_eq!(report.nodes, 1);
    assert_eq!(report.result, Outcome::Ongoing);
}

/// Playing answers with a digest, which is what the cross-target lane compares
/// — perft cannot serve that purpose, because a count is the same number on a
/// broken machine and a working one.
#[test]
fn playing_is_reproducible_and_discriminating() {
    let options = Options {
        mode: Mode::Play,
        depth: 30,
        ..Options::default()
    };
    assert_eq!(ran(&options), ran(&options), "the same game plays the same");
    assert_eq!(ran(&options).played, 30);

    let shorter = ran(&Options {
        depth: 29,
        ..options.clone()
    });
    assert_ne!(ran(&options).digest, shorter.digest);
}

/// A game that ends stops rather than playing on, and says how many moves it
/// actually managed.
#[test]
fn playing_past_the_end_of_a_game_stops_there() {
    // A position already checkmated: no legal move exists.
    let mated = Board::from_fen("rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3")
        .expect("well formed");
    let report = ran(&Options {
        mode: Mode::Play,
        position: mated,
        depth: 20,
        ..Options::default()
    });
    assert_eq!(report.played, 0, "there was nothing to play");
    assert_eq!(report.result, Outcome::Checkmate);
    assert_eq!(next_move(&mated), None);
}

#[test]
fn the_next_move_is_the_first_legal_one() {
    let board = Board::initial();
    let chosen = next_move(&board).expect("twenty to choose from");
    assert_eq!(
        chosen,
        *renew_sample_chess_rules::legal(&board)
            .as_slice()
            .first()
            .expect("twenty to choose from")
    );
}

#[test]
fn the_answer_reads_and_parses() {
    let counted = ran(&Options {
        depth: 3,
        ..Options::default()
    });
    let line = describe(&counted);
    assert!(line.starts_with("chess count depth=3"));
    assert!(line.contains("nodes=8902"));

    let json = describe_json(&counted);
    assert!(json.contains("\"schema_version\":1"));
    assert!(json.contains("\"sample\":\"chess\""));
    assert!(json.contains("\"mode\":\"count\""));
    assert!(json.contains("\"nodes\":8902"));
    assert!(json.starts_with('{') && json.ends_with('}'));

    let played = ran(&Options {
        mode: Mode::Play,
        depth: 10,
        ..Options::default()
    });
    let line = describe(&played);
    assert!(line.starts_with("chess play moves=10"));
    assert!(line.contains(&format!("digest=0x{:016x}", played.digest)));
    assert!(describe_json(&played).contains("\"mode\":\"play\""));
}

/// Every ending has a name, so a caller reading the output never sees a state
/// it cannot interpret.
#[test]
fn every_ending_prints_under_a_name() {
    let cases = [
        (
            "rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3",
            "checkmate",
        ),
        ("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1", "stalemate"),
        ("4k3/8/8/8/8/8/8/4K2R w - - 100 60", "fifty-move"),
        ("4k3/8/8/8/8/8/8/4K3 w - - 0 1", "ongoing"),
    ];
    for (fen, name) in cases {
        let report = ran(&Options {
            mode: Mode::Play,
            position: Board::from_fen(fen).expect("well formed"),
            depth: 0,
            ..Options::default()
        });
        assert!(
            describe(&report).contains(name),
            "expected {name} for {fen}, got {}",
            describe(&report)
        );
        assert!(describe_json(&report).contains(name));
    }
}

/// The whole binary, driven without spawning one.
#[test]
fn the_command_line_answers_with_an_exit_code() {
    assert_eq!(run_cli(args("--count --depth 2")), 0);
    assert_eq!(run_cli(args("--play --depth 4 --json")), 0);
    assert_eq!(run_cli(args("--help")), 0, "help is not a failure");
    assert_eq!(run_cli(args("--wat")), 1);
    assert_eq!(run_cli(args("--depth deep")), 1);
    assert_eq!(run_cli(args("--fen banana")), 1);
    assert_eq!(run_cli(args("--count --depth 9")), 1);
}
