//! Every way a trace can be refused, one test per way.
//!
//! A parser of untrusted input is mostly its refusals, so they are tested
//! the way the accepting paths are: separately, by what they *say*, not by
//! the fact that something went wrong. Each test here pins one line of one
//! malformed file to one refusal and one line number. A single test that
//! asserted "these twenty files are all rejected" would still pass if
//! nineteen of them started being rejected for the twentieth's reason.
//!
//! **Negatives have to be near misses, not strangers.** A refusal test is
//! only as sharp as the distance between the input it refuses and the
//! input it must accept. `gamepad` and `thumb` and `meta` are strangers:
//! they share nothing with a legal word, so refusing them says only that
//! the reader has *some* notion of a vocabulary. They cannot tell an
//! exact-match reader from one that accepts any word beginning with a
//! legal one — and that reader accepts `event 0 close`, `err 0 close`
//! and, worse, `renew-tracex` as the file's own identity line. So every
//! table of words here is probed twice: with a stranger, and with a word
//! one character away from a legal one, in both directions.

use renew_trace::{TraceError, TraceErrorKind, parse};

const HEADER: &str = "renew-trace 2 sample=input_echo ticks=30 timestep_ns=16666667 budget=5";

/// The refusal a text produces. Every test goes through here, so a text
/// that is unexpectedly *accepted* fails loudly rather than quietly
/// passing some later assertion.
// A test helper, called only from `#[test]` fns: the tests-only panic
// allowance covers those, not their helpers, and this extends it in the
// same spirit — an accepted file in a file of refusals is a test failure
// and has to be reported as one.
#[allow(clippy::panic)]
fn refuse(text: &str) -> TraceError {
    match parse(text) {
        Ok(trace) => panic!("expected a refusal, read {} events", trace.events().len()),
        Err(error) => error,
    }
}

/// The refusal produced by one event line under the standard header.
fn refuse_event(line: &str) -> TraceError {
    refuse(&format!("{HEADER}\n{line}\n"))
}

#[test]
fn an_empty_file_is_not_a_trace() {
    let error = refuse("");
    assert_eq!(error.line(), 1);
    assert_eq!(error.kind(), &TraceErrorKind::Empty);
    assert_eq!(
        error.to_string(),
        "line 1: the file is empty; a trace is a header line followed by one line per event"
    );
}

/// The byte order mark is named rather than described, because it is
/// invisible: the file looks correct on screen, and any other message
/// would send a reader looking at the wrong thing.
#[test]
fn a_byte_order_mark_is_refused_by_name() {
    let error = refuse(&format!("\u{feff}{HEADER}\n"));
    assert_eq!(error.line(), 1);
    assert_eq!(error.kind(), &TraceErrorKind::ByteOrderMark);
    assert!(error.to_string().contains("byte order mark"), "{error}");
}

#[test]
fn a_file_that_is_only_a_byte_order_mark_still_names_it() {
    assert_eq!(refuse("\u{feff}").kind(), &TraceErrorKind::ByteOrderMark);
}

#[test]
fn a_first_line_that_is_not_a_header_is_refused() {
    let error = refuse("hello 0 sample=x\n");
    assert_eq!(error.line(), 1);
    assert_eq!(
        error.kind(),
        &TraceErrorKind::NotATrace {
            found: "hello".to_string()
        }
    );
}

/// The file's own identity line, probed one character away. A reader
/// that accepted anything *beginning* with the format's name would read
/// `renew-tracex` as a trace of this format, which is the one question
/// the first word of the first line exists to answer.
#[test]
fn a_word_that_merely_begins_with_the_format_name_is_not_a_header() {
    for word in ["renew-tracex", "renew-trace2", "renew-trac"] {
        let text = format!("{word} 0 sample=x ticks=1 timestep_ns=1 budget=1\n");
        assert_eq!(
            refuse(&text).kind(),
            &TraceErrorKind::NotATrace {
                found: word.to_string()
            },
            "{word} was read as a header"
        );
    }
}

#[test]
fn a_file_of_one_blank_line_is_a_missing_header() {
    assert_eq!(
        refuse("\n").kind(),
        &TraceErrorKind::NotATrace {
            found: String::new()
        }
    );
}

/// A trace line whose character is not typed text.
///
/// Three ways to fail and one message, because they are one fault from
/// a reader's side: the number is not something a person typed. A
/// surrogate and a code point past the last one are not characters at
/// all; a control character is one the live seam never delivers, and a
/// recording that carried it could drive a consumer into a state no
/// window can produce.
#[test]
fn a_character_that_is_not_typed_text_is_refused() {
    let header = "renew-trace 1 sample=x ticks=1 timestep_ns=1 budget=1\n";
    for value in [
        "55296",      // a high surrogate
        "57343",      // a low surrogate
        "1114112",    // one past the last code point
        "4294967295", // the whole width
        "0",          // NUL
        "13",         // carriage return
        "27",         // escape
        "127",        // delete
        "159",        // the end of the C1 range
    ] {
        let error = refuse(&format!("{header}e 0 text {value}\n"));
        assert_eq!(error.line(), 2, "the second line is the event");
        assert_eq!(
            error.kind(),
            &TraceErrorKind::NotTypedText {
                field: "the typed character",
                text: value.to_string()
            },
            "`{value}` must be refused as not typed text"
        );
    }
}

/// And the message says which of the two it is.
///
/// A number that does not fit its width and a number that fits and is
/// not text are different faults, and the older message described only
/// the first. A reader who meets the wrong one looks in the wrong place.
#[test]
fn not_typed_text_reads_differently_from_not_fitting() {
    let header = "renew-trace 1 sample=x ticks=1 timestep_ns=1 budget=1\n";
    let not_text = refuse(&format!("{header}e 0 text 55296\n")).to_string();
    assert!(
        not_text.contains("is not typed text"),
        "a surrogate is not a width problem: {not_text}"
    );
    let too_wide = refuse(&format!("{header}e 0 resize 4294967296 1\n")).to_string();
    assert!(
        too_wide.contains("does not fit its width"),
        "a number past the width still says so: {too_wide}"
    );
}

/// Text that IS typed text reads back.
///
/// The other half, so the refusals above cannot pass by the arm
/// refusing everything.
#[test]
fn a_typed_character_reads_back() {
    let header = "renew-trace 1 sample=x ticks=1 timestep_ns=1 budget=1\n";
    let trace = parse(&format!("{header}e 0 text 97\n")).expect("a letter is text");
    assert_eq!(trace.events().len(), 1);
}

#[test]
fn a_version_newer_than_this_reader_is_refused() {
    // Derived from the reader's own version rather than written out, so
    // that moving the format does not quietly turn this test into one
    // that hands the reader a file it can read.
    let newer = u64::from(renew_trace::FORMAT_VERSION).saturating_add(1);
    let error = refuse(&format!(
        "renew-trace {newer} sample=x ticks=1 timestep_ns=1 budget=1\n"
    ));
    assert_eq!(error.line(), 1);
    assert_eq!(
        error.kind(),
        &TraceErrorKind::UnsupportedVersion {
            found: newer,
            supported: renew_trace::FORMAT_VERSION
        }
    );
}

#[test]
fn a_version_that_is_not_a_number_is_refused() {
    assert_eq!(
        refuse("renew-trace v0 sample=x ticks=1 timestep_ns=1 budget=1\n").kind(),
        &TraceErrorKind::NotADecimalInteger {
            field: "the format version",
            text: "v0".to_string(),
        }
    );
}

#[test]
fn a_header_that_stops_before_its_version_is_refused() {
    assert_eq!(
        refuse("renew-trace\n").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "the format version"
        }
    );
}

#[test]
fn a_header_that_stops_before_a_field_names_the_field() {
    assert_eq!(
        refuse("renew-trace 0\n").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "`sample=<name>`"
        }
    );
    assert_eq!(
        refuse("renew-trace 1 sample=x ticks=1 timestep_ns=1\n").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "`budget=<u32>`"
        }
    );
}

/// The four fields the codec owns are positional, so a reader never has
/// to guess which one it is holding.
#[test]
fn a_header_field_in_the_wrong_place_is_refused() {
    let error = refuse("renew-trace 1 sample=x timestep_ns=1 ticks=1 budget=1\n");
    assert_eq!(
        error.kind(),
        &TraceErrorKind::HeaderFieldOutOfOrder {
            expected: "`ticks=<u64>`",
            found: "timestep_ns=1".to_string(),
        }
    );
}

#[test]
fn a_header_field_that_is_not_a_pair_is_refused() {
    assert_eq!(
        refuse("renew-trace 1 sample ticks=1 timestep_ns=1 budget=1\n").kind(),
        &TraceErrorKind::NotAKeyValuePair {
            field: "sample".to_string()
        }
    );
}

#[test]
fn a_caller_key_without_a_value_is_refused() {
    assert_eq!(
        refuse(&format!("{HEADER} seed\n")).kind(),
        &TraceErrorKind::NotAKeyValuePair {
            field: "seed".to_string()
        }
    );
}

#[test]
fn the_same_caller_key_twice_is_refused() {
    let error = refuse(&format!("{HEADER} seed=1 seed=2\n"));
    assert_eq!(error.line(), 1);
    assert_eq!(
        error.kind(),
        &TraceErrorKind::DuplicateHeaderKey {
            key: "seed".to_string()
        }
    );
}

/// A caller key may not shadow one of the codec's own, which would leave
/// two answers to one question in one file.
#[test]
fn a_caller_key_repeating_one_the_codec_owns_is_refused() {
    assert_eq!(
        refuse(&format!("{HEADER} ticks=9\n")).kind(),
        &TraceErrorKind::DuplicateHeaderKey {
            key: "ticks".to_string()
        }
    );
}

#[test]
fn a_caller_key_with_an_empty_value_is_refused() {
    assert_eq!(
        refuse(&format!("{HEADER} seed=\n")).kind(),
        &TraceErrorKind::UnwritableText {
            text: "seed=".to_string(),
            reason: "a header key and a header value are each at least one character",
        }
    );
}

#[test]
fn a_header_value_carrying_a_control_character_is_refused() {
    assert_eq!(
        refuse("renew-trace 1 sample=in\u{7}put ticks=1 timestep_ns=1 budget=1\n").kind(),
        &TraceErrorKind::UnwritableText {
            text: "sample=in\u{7}put".to_string(),
            reason: "a control character is invisible in a diff and in a log",
        }
    );
}

/// A tab is whitespace that survives a split on spaces, so it is the one
/// way whitespace can reach a field, and it is refused there.
#[test]
fn a_header_value_carrying_a_tab_is_refused() {
    assert_eq!(
        refuse(&format!("{HEADER} extent=640\tx480\n")).kind(),
        &TraceErrorKind::UnwritableText {
            text: "extent=640\tx480".to_string(),
            reason: "whitespace would split it into two fields",
        }
    );
}

#[test]
fn two_spaces_in_the_header_are_refused() {
    assert_eq!(
        refuse("renew-trace 1  sample=x ticks=1 timestep_ns=1 budget=1\n").kind(),
        &TraceErrorKind::BlankField
    );
}

#[test]
fn a_trailing_space_on_the_header_is_refused() {
    assert_eq!(
        refuse(&format!("{HEADER} \n")).kind(),
        &TraceErrorKind::BlankField
    );
}

#[test]
fn a_header_number_that_is_not_digits_is_refused() {
    assert_eq!(
        refuse("renew-trace 1 sample=x ticks=1_000 timestep_ns=1 budget=1\n").kind(),
        &TraceErrorKind::NotADecimalInteger {
            field: "`ticks=<u64>`",
            text: "1_000".to_string(),
        }
    );
    assert_eq!(
        refuse("renew-trace 1 sample=x ticks=1 timestep_ns=+1 budget=1\n").kind(),
        &TraceErrorKind::NotADecimalInteger {
            field: "`timestep_ns=<u64>`",
            text: "+1".to_string(),
        }
    );
}

#[test]
fn a_header_number_too_large_for_its_width_is_refused() {
    assert_eq!(
        refuse("renew-trace 1 sample=x ticks=99999999999999999999 timestep_ns=1 budget=1\n").kind(),
        &TraceErrorKind::IntegerTooLarge {
            field: "`ticks=<u64>`",
            text: "99999999999999999999".to_string(),
        }
    );
    assert_eq!(
        refuse("renew-trace 1 sample=x ticks=1 timestep_ns=1 budget=4294967296\n").kind(),
        &TraceErrorKind::IntegerTooLarge {
            field: "`budget=<u32>`",
            text: "4294967296".to_string(),
        }
    );
    assert_eq!(
        refuse("renew-trace 1 sample=x ticks=1 timestep_ns=1 budget=five\n").kind(),
        &TraceErrorKind::NotADecimalInteger {
            field: "`budget=<u32>`",
            text: "five".to_string(),
        }
    );
}

#[test]
fn a_second_header_is_refused_where_it_stands() {
    let error = refuse(&format!("{HEADER}\n{HEADER}\n"));
    assert_eq!(error.line(), 2);
    assert_eq!(error.kind(), &TraceErrorKind::HeaderAfterEvents);
}

#[test]
fn an_unknown_line_keyword_is_refused_rather_than_skipped() {
    let error = refuse_event("x 0 close");
    assert_eq!(error.line(), 2);
    assert_eq!(
        error.kind(),
        &TraceErrorKind::UnknownKeyword {
            keyword: "x".to_string()
        }
    );
    assert!(
        error.to_string().contains("refused rather than skipped"),
        "{error}"
    );
}

/// The event keyword is one character long, which makes it the easiest
/// word in the format to match loosely by accident: every one of these
/// begins with `e`, and a reader comparing prefixes would read all three
/// as ordinary event lines.
#[test]
fn a_line_keyword_that_merely_begins_with_the_event_keyword_is_unknown() {
    for keyword in ["event", "err", "ee"] {
        assert_eq!(
            refuse_event(&format!("{keyword} 0 close")).kind(),
            &TraceErrorKind::UnknownKeyword {
                keyword: keyword.to_string()
            },
            "{keyword} was read as an event line"
        );
    }
}

/// The header's own keys, probed the same way: `ticksx` is not `ticks`,
/// and a reader that thought otherwise would take its run length from a
/// field the writer never wrote.
#[test]
fn a_header_key_that_merely_begins_with_a_known_one_is_out_of_order() {
    assert_eq!(
        refuse("renew-trace 1 sample=x ticksx=1 timestep_ns=1 budget=1\n").kind(),
        &TraceErrorKind::HeaderFieldOutOfOrder {
            expected: "`ticks=<u64>`",
            found: "ticksx=1".to_string(),
        }
    );
}

/// A blank line is not nothing; it is a line this reader cannot read.
#[test]
fn a_blank_line_between_events_is_refused() {
    let error = refuse(&format!("{HEADER}\n\ne 0 close\n"));
    assert_eq!(error.line(), 2);
    assert_eq!(
        error.kind(),
        &TraceErrorKind::UnknownKeyword {
            keyword: String::new()
        }
    );
}

#[test]
fn an_unknown_event_kind_is_refused_rather_than_skipped() {
    let error = refuse_event("e 0 gamepad left down");
    assert_eq!(
        error.kind(),
        &TraceErrorKind::UnknownEventKind {
            kind: "gamepad".to_string()
        }
    );
    assert!(
        error.to_string().contains("refused rather than skipped"),
        "{error}"
    );
}

#[test]
fn an_event_line_that_stops_before_its_tick_is_refused() {
    assert_eq!(
        refuse_event("e").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "the tick"
        }
    );
}

#[test]
fn an_event_line_that_stops_before_its_kind_is_refused() {
    assert_eq!(
        refuse_event("e 0").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "the kind of event"
        }
    );
}

#[test]
fn a_tick_that_is_not_digits_is_refused() {
    assert_eq!(
        refuse_event("e -1 close").kind(),
        &TraceErrorKind::NotADecimalInteger {
            field: "the tick",
            text: "-1".to_string(),
        }
    );
}

#[test]
fn a_tick_too_large_for_its_width_is_refused() {
    assert_eq!(
        refuse_event("e 99999999999999999999 close").kind(),
        &TraceErrorKind::IntegerTooLarge {
            field: "the tick",
            text: "99999999999999999999".to_string(),
        }
    );
}

/// Ticks never decrease. Equal ones are legal, so only a genuine
/// decrease is refused, and it is refused against its own line.
#[test]
fn a_tick_earlier_than_the_one_before_it_is_refused() {
    let error = refuse(&format!("{HEADER}\ne 7 redraw\ne 7 redraw\ne 3 close\n"));
    assert_eq!(error.line(), 4);
    assert_eq!(
        error.kind(),
        &TraceErrorKind::TickWentBackwards {
            tick: 3,
            previous: 7
        }
    );
}

/// One past the end is refused; the end itself is not, and the message
/// says so rather than leaving someone to discover it.
#[test]
fn a_tick_past_the_end_of_the_run_is_refused() {
    let error = refuse_event("e 31 close");
    assert_eq!(error.line(), 2);
    assert_eq!(
        error.kind(),
        &TraceErrorKind::TickBeyondHeader {
            tick: 31,
            ticks: 30
        }
    );
    assert!(
        error.to_string().contains("the last legal tick is 30"),
        "{error}"
    );
}

#[test]
fn an_unknown_key_name_is_refused() {
    let error = refuse_event("e 0 key meta down");
    assert_eq!(
        error.kind(),
        &TraceErrorKind::UnknownKey {
            name: "meta".to_string()
        }
    );
    assert!(error.to_string().contains("arrow-right"), "{error}");
}

/// Key names, probed one character away in both directions: `spaceship`
/// begins with `space` and `spac` is begun by it. A lookup comparing
/// prefixes either way would answer `Space` to both, and a trace would
/// replay a key nobody pressed.
#[test]
fn a_key_name_one_character_from_a_known_one_is_unknown() {
    for name in ["spaceship", "spac", "arrow-rights", "arrow-righ"] {
        assert_eq!(
            refuse_event(&format!("e 0 key {name} down")).kind(),
            &TraceErrorKind::UnknownKey {
                name: name.to_string()
            },
            "{name} was read as a known key"
        );
    }
}

/// The same refusal serves keys and buttons, and says so: both are
/// `down` or `up`, and a message naming only keys would be wrong on half
/// the lines that can produce it.
#[test]
fn a_state_that_is_neither_down_nor_up_is_refused() {
    let key = refuse_event("e 0 key space pressed");
    assert_eq!(
        key.kind(),
        &TraceErrorKind::NotAPressedState {
            text: "pressed".to_string()
        }
    );
    assert_eq!(
        refuse_event("e 0 button left sideways").kind(),
        &TraceErrorKind::NotAPressedState {
            text: "sideways".to_string()
        }
    );
    assert!(key.to_string().contains("a key or a button"), "{key}");
}

#[test]
fn a_line_that_stops_before_its_state_is_refused() {
    for line in ["e 0 key space", "e 0 button left"] {
        assert_eq!(
            refuse_event(line).kind(),
            &TraceErrorKind::LineEndsEarly {
                expected: "`down` or `up`"
            }
        );
    }
}

#[test]
fn a_key_line_that_stops_before_its_name_is_refused() {
    assert_eq!(
        refuse_event("e 0 key").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "the key name"
        }
    );
}

#[test]
fn a_wheel_line_that_stops_before_a_delta_is_refused() {
    assert_eq!(
        refuse_event("e 0 wheel").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "the wheel's horizontal delta"
        }
    );
    assert_eq!(
        refuse_event("e 0 wheel 0x00000000").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "the wheel's vertical delta"
        }
    );
}

/// The repeat flag is present or absent. A word meaning "no" would be a
/// second spelling of an absent flag.
#[test]
fn anything_but_the_repeat_flag_after_a_key_state_is_refused() {
    assert_eq!(
        refuse_event("e 0 key space down norepeat").kind(),
        &TraceErrorKind::NotTheRepeatFlag {
            text: "norepeat".to_string()
        }
    );
}

#[test]
fn a_doubled_space_on_a_key_line_is_refused() {
    assert_eq!(
        refuse_event("e 0 key space down  repeat").kind(),
        &TraceErrorKind::BlankField
    );
}

#[test]
fn text_after_a_complete_line_is_refused() {
    assert_eq!(
        refuse_event("e 0 close now").kind(),
        &TraceErrorKind::TrailingText {
            text: "now".to_string()
        }
    );
    assert_eq!(
        refuse_event("e 0 key space down repeat twice").kind(),
        &TraceErrorKind::TrailingText {
            text: "twice".to_string()
        }
    );
}

#[test]
fn a_trailing_space_on_an_event_line_is_refused() {
    assert_eq!(
        refuse_event("e 0 close ").kind(),
        &TraceErrorKind::BlankField
    );
}

#[test]
fn a_doubled_space_before_an_event_kind_is_refused() {
    assert_eq!(
        refuse_event("e 0  close").kind(),
        &TraceErrorKind::BlankField
    );
}

/// One spelling per file: an uppercase digit, or an uppercase prefix, is
/// a second way to write a number that already has one.
#[test]
fn an_uppercase_bit_pattern_is_refused() {
    assert_eq!(
        refuse_event("e 0 scale 0x400000000000000A").kind(),
        &TraceErrorKind::NotAHexPattern {
            field: "the scale factor",
            text: "0x400000000000000A".to_string(),
            digits: 16,
        }
    );
    assert_eq!(
        refuse_event("e 0 scale 0X4000000000000000").kind(),
        &TraceErrorKind::NotAHexPattern {
            field: "the scale factor",
            text: "0X4000000000000000".to_string(),
            digits: 16,
        }
    );
}

#[test]
fn a_bit_pattern_of_the_wrong_width_is_refused() {
    assert_eq!(
        refuse_event("e 0 scale 0x4").kind(),
        &TraceErrorKind::NotAHexPattern {
            field: "the scale factor",
            text: "0x4".to_string(),
            digits: 16,
        }
    );
    assert_eq!(
        refuse_event("e 0 wheel 0x000000000 0x00000000").kind(),
        &TraceErrorKind::NotAHexPattern {
            field: "the wheel's horizontal delta",
            text: "0x000000000".to_string(),
            digits: 8,
        }
    );
}

#[test]
fn a_bit_pattern_without_its_prefix_is_refused() {
    assert_eq!(
        refuse_event("e 0 pointer 4000000000000000 0x0000000000000000").kind(),
        &TraceErrorKind::NotAHexPattern {
            field: "the pointer's x coordinate",
            text: "4000000000000000".to_string(),
            digits: 16,
        }
    );
}

#[test]
fn a_pointer_line_that_stops_before_its_second_coordinate_is_refused() {
    assert_eq!(
        refuse_event("e 0 pointer 0x0000000000000000").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "the pointer's y coordinate"
        }
    );
}

/// A trace carries finite values only, in both widths.
#[test]
fn a_non_finite_bit_pattern_is_refused() {
    assert_eq!(
        refuse_event("e 0 scale 0x7ff0000000000000").kind(),
        &TraceErrorKind::NonFinite {
            field: "the scale factor",
            text: "0x7ff0000000000000".to_string(),
        }
    );
    assert_eq!(
        refuse_event("e 0 pointer 0x0000000000000000 0xfff8000000000000").kind(),
        &TraceErrorKind::NonFinite {
            field: "the pointer's y coordinate",
            text: "0xfff8000000000000".to_string(),
        }
    );
    assert_eq!(
        refuse_event("e 0 wheel 0xff800000 0x00000000").kind(),
        &TraceErrorKind::NonFinite {
            field: "the wheel's horizontal delta",
            text: "0xff800000".to_string(),
        }
    );
    assert_eq!(
        refuse_event("e 0 wheel 0x00000000 0x7f800001").kind(),
        &TraceErrorKind::NonFinite {
            field: "the wheel's vertical delta",
            text: "0x7f800001".to_string(),
        }
    );
}

#[test]
fn an_unknown_pointer_button_is_refused() {
    let error = refuse_event("e 0 button thumb down");
    assert_eq!(
        error.kind(),
        &TraceErrorKind::UnknownButton {
            name: "thumb".to_string()
        }
    );
    assert!(error.to_string().contains("other:<index>"), "{error}");
}

/// Button names, probed the same way as key names — including
/// `otherwise`, which begins with the native-index prefix without being
/// it.
#[test]
fn a_button_name_one_character_from_a_known_one_is_unknown() {
    for name in ["leftmost", "lef", "middles", "otherwise"] {
        assert_eq!(
            refuse_event(&format!("e 0 button {name} down")).kind(),
            &TraceErrorKind::UnknownButton {
                name: name.to_string()
            },
            "{name} was read as a known button"
        );
    }
}

#[test]
fn a_native_button_index_that_is_not_digits_is_refused() {
    assert_eq!(
        refuse_event("e 0 button other:x down").kind(),
        &TraceErrorKind::NotADecimalInteger {
            field: "the native button index (u16)",
            text: "x".to_string(),
        }
    );
    assert_eq!(
        refuse_event("e 0 button other: down").kind(),
        &TraceErrorKind::NotADecimalInteger {
            field: "the native button index (u16)",
            text: String::new(),
        }
    );
}

#[test]
fn a_native_button_index_too_large_for_its_width_is_refused() {
    assert_eq!(
        refuse_event("e 0 button other:65536 down").kind(),
        &TraceErrorKind::IntegerTooLarge {
            field: "the native button index (u16)",
            text: "65536".to_string(),
        }
    );
}

#[test]
fn a_button_line_that_stops_before_its_button_is_refused() {
    assert_eq!(
        refuse_event("e 0 button").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "the pointer button"
        }
    );
}

#[test]
fn a_focus_state_that_is_neither_in_nor_out_is_refused() {
    assert_eq!(
        refuse_event("e 0 focus yes").kind(),
        &TraceErrorKind::NotAFocusState {
            text: "yes".to_string()
        }
    );
    assert_eq!(
        refuse_event("e 0 focus").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "`in` or `out`"
        }
    );
}

#[test]
fn a_resize_that_is_not_two_numbers_is_refused() {
    assert_eq!(
        refuse_event("e 0 resize wide 720").kind(),
        &TraceErrorKind::NotADecimalInteger {
            field: "the width (u32)",
            text: "wide".to_string(),
        }
    );
    assert_eq!(
        refuse_event("e 0 resize 1280").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "the height (u32)"
        }
    );
    assert_eq!(
        refuse_event("e 0 resize 4294967296 720").kind(),
        &TraceErrorKind::IntegerTooLarge {
            field: "the width (u32)",
            text: "4294967296".to_string(),
        }
    );
}

/// The touch phase table, probed to the file's own doctrine: a stranger,
/// a word one character short of a legal one, a word one character past
/// one, a case variant, and a word from a neighbouring table. An
/// exact-match reader refuses all five; a prefix-matching one would
/// accept `startx`, and that is the reader these near misses exist to
/// unmask.
#[test]
fn a_touch_phase_is_refused_by_exact_match_not_by_distance() {
    for text in ["hover", "star", "startx", "Start", "down"] {
        let error = refuse_event(&format!(
            "e 0 touch 1 {text} 0x0000000000000000 0x0000000000000000"
        ));
        assert_eq!(error.line(), 2, "{text}");
        assert_eq!(
            error.kind(),
            &TraceErrorKind::NotATouchPhase {
                text: text.to_string(),
            },
            "{text}"
        );
    }
    let shown = refuse_event("e 0 touch 1 hover 0x0000000000000000 0x0000000000000000");
    assert_eq!(
        shown.to_string(),
        "line 2: a touch phase is `start`, `move`, `end` or `cancel`, found `hover`"
    );
}

/// A touch line's fields are as mandatory as anyone else's: the line
/// stops early by name, a finger is digits, a coordinate is a bit
/// pattern, and nothing may follow.
#[test]
fn a_touch_line_is_held_to_its_grammar() {
    assert_eq!(
        refuse_event("e 0 touch").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "the finger id"
        }
    );
    assert_eq!(
        refuse_event("e 0 touch 1").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "`start`, `move`, `end` or `cancel`"
        }
    );
    assert_eq!(
        refuse_event("e 0 touch 1 start").kind(),
        &TraceErrorKind::LineEndsEarly {
            expected: "the touch's x coordinate"
        }
    );
    assert_eq!(
        refuse_event("e 0 touch one start 0x0000000000000000 0x0000000000000000").kind(),
        &TraceErrorKind::NotADecimalInteger {
            field: "the finger id",
            text: "one".to_string(),
        }
    );
    assert_eq!(
        refuse_event("e 0 touch 18446744073709551616 start 0x0000000000000000 0x0000000000000000")
            .kind(),
        &TraceErrorKind::IntegerTooLarge {
            field: "the finger id",
            text: "18446744073709551616".to_string(),
        }
    );
    assert_eq!(
        refuse_event("e 0 touch 1 start 1.5 0x0000000000000000").kind(),
        &TraceErrorKind::NotAHexPattern {
            field: "the touch's x coordinate",
            text: "1.5".to_string(),
            digits: 16,
        }
    );
    assert_eq!(
        refuse_event("e 0 touch 1 start 0x0000000000000000 0x0000000000000000 extra").kind(),
        &TraceErrorKind::TrailingText {
            text: "extra".to_string(),
        }
    );
}

/// A file that claims a version older than a word it uses is refused on
/// the word's line, naming both versions: laundering the claim into a
/// canonical newer header would forge a file the producer never wrote.
#[test]
fn a_touch_line_under_a_disclaiming_header_is_refused_by_version() {
    let error = refuse(
        "renew-trace 1 sample=input_echo ticks=30 timestep_ns=16666667 budget=5\n\
         e 0 touch 1 start 0x0000000000000000 0x0000000000000000\n",
    );
    assert_eq!(error.line(), 2);
    assert_eq!(
        error.kind(),
        &TraceErrorKind::EventFromANewerFormat {
            kind: "touch".to_string(),
            introduced: 2,
            declared: 1,
        }
    );
    assert!(error.to_string().contains("claims version 1"), "{error}");
}

/// The motion line's payloads are held to the same rules as the
/// pointer's — pinned separately because for two format versions no
/// malformed motion line existed anywhere in this suite, and the
/// refusal edges a suite never takes are the ones free to rot.
#[test]
fn a_motion_line_is_held_to_its_grammar() {
    assert_eq!(
        refuse_event("e 0 motion 1.5 0x0000000000000000").kind(),
        &TraceErrorKind::NotAHexPattern {
            field: "the pointer's rightward movement",
            text: "1.5".to_string(),
            digits: 16,
        }
    );
    assert_eq!(
        refuse_event("e 0 motion 0x0000000000000000 0xfff0000000000000").kind(),
        &TraceErrorKind::NonFinite {
            field: "the pointer's downward movement",
            text: "0xfff0000000000000".to_string(),
        }
    );
}

/// The touch line's second coordinate is as guarded as its first: a
/// probe that stopped at x would leave y's refusal edge untaken.
#[test]
fn a_touch_line_with_a_bad_second_coordinate_is_refused() {
    assert_eq!(
        refuse_event("e 0 touch 1 start 0x0000000000000000 nope").kind(),
        &TraceErrorKind::NotAHexPattern {
            field: "the touch's y coordinate",
            text: "nope".to_string(),
            digits: 16,
        }
    );
}

/// A typed character that is not even a number is a number refusal,
/// not a text refusal — the field's two gates fire in order.
#[test]
fn a_typed_character_that_is_not_a_number_is_refused_as_one() {
    assert_eq!(
        refuse_event("e 0 text abc").kind(),
        &TraceErrorKind::NotADecimalInteger {
            field: "the typed character",
            text: "abc".to_string(),
        }
    );
}
