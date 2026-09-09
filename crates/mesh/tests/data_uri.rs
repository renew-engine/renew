//! `data:` URIs: what decodes, what does not, and where it says so.
//!
//! The refusals are the point. A decoder that only ever sees payloads
//! written by an encoder passes its own suite on the day it is written
//! and every day after, which is why most of what follows is input no
//! encoder would produce.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` — which apply to `#[cfg(test)]` modules — do
// not reach it. A fixture this file built and then could not read back
// is a broken test rather than a condition to recover from.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use renew_mesh::data_uri::{self, DataUriError};

// The encoder is `shared/base64_encode.rs`, included here and by three
// other targets. It is deliberately not the crate's: nothing in the
// engine writes a `data:` URI, and a round trip through one body of code
// proves only that the code agrees with itself.
#[path = "shared/base64_encode.rs"]
mod base64_encode;
use base64_encode::encode;

/// The prefix every URI here shares, so offsets in the assertions below
/// are computed rather than counted by hand.
const HEAD: &str = "data:application/octet-stream;base64,";

fn bytes(uri: &str) -> Vec<u8> {
    data_uri::read(uri)
        .unwrap_or_else(|refused| panic!("{uri} was refused: {refused}"))
        .bytes
}

fn refused(uri: &str) -> DataUriError {
    match data_uri::read(uri) {
        Err(refusal) => refusal,
        Ok(read) => panic!("{uri} decoded to {:?} and should not have", read.bytes),
    }
}

/// The vectors from RFC 4648, which cover every padding case there is.
#[test]
fn the_published_vectors_decode_to_their_bytes() {
    for (encoded, plain) in [
        ("", ""),
        ("Zg==", "f"),
        ("Zm8=", "fo"),
        ("Zm9v", "foo"),
        ("Zm9vYg==", "foob"),
        ("Zm9vYmE=", "fooba"),
        ("Zm9vYmFy", "foobar"),
    ] {
        let uri = HEAD.to_owned() + encoded;
        assert_eq!(
            bytes(&uri),
            plain.as_bytes(),
            "`{encoded}` should have been `{plain}`"
        );
    }
}

/// Every byte value, in one payload, so no alphabet entry is untested.
#[test]
fn all_two_hundred_and_fifty_six_byte_values_survive_a_round_trip() {
    let all: Vec<u8> = (0..=255).collect();
    let uri = HEAD.to_owned() + &encode(&all);
    assert_eq!(bytes(&uri), all);
}

#[test]
fn an_empty_payload_is_no_bytes_rather_than_a_refusal() {
    // A URI may legitimately carry nothing. Whether nothing is
    // acceptable is a question for whoever wanted the resource.
    assert!(bytes(HEAD).is_empty());
}

#[test]
fn a_media_type_is_reported_and_its_parameters_kept_separate() {
    let read = data_uri::read("data:image/png;quality=fine;base64,Zm9v").expect("a valid URI");
    assert_eq!(read.media_type, "image/png");
    assert_eq!(read.parameters, "quality=fine");
    assert_eq!(read.bytes, b"foo");
}

#[test]
fn a_type_may_be_absent_while_parameters_are_present() {
    // RFC 2397 spells this one out, and it is the case a reader written
    // against examples never sees.
    let read = data_uri::read("data:;charset=UTF-8;base64,Zm9v").expect("a valid URI");
    assert_eq!(read.media_type, "");
    assert_eq!(read.parameters, "charset=UTF-8");
}

#[test]
fn a_type_may_be_absent_altogether() {
    let read = data_uri::read("data:;base64,Zm9v").expect("a valid URI");
    assert_eq!(read.media_type, "");
    assert_eq!(read.parameters, "");
}

#[test]
fn the_marker_is_read_in_any_letter_case() {
    // Forgiven because no spelling of the marker changes an output byte.
    for uri in [
        "data:text/plain;base64,Zm9v",
        "data:text/plain;BASE64,Zm9v",
        "data:text/plain;Base64,Zm9v",
        "DATA:text/plain;base64,Zm9v",
    ] {
        assert_eq!(bytes(uri), b"foo", "{uri}");
    }
}

#[test]
fn two_reads_of_one_uri_are_the_same_value() {
    // The struct is comparable, cloneable and printable because a caller
    // holding one wants all three -- and because the fuzz target's
    // "reading twice answers the same" check is worth nothing if the
    // comparison it makes is not the one a caller would make.
    let uri = HEAD.to_owned() + "Zm9v";
    let once = data_uri::read(&uri).expect("a valid URI");
    let again = data_uri::read(&uri).expect("and again");
    assert_eq!(once, again);
    assert_eq!(once.clone(), again);
    assert!(
        format!("{once:?}").contains("DataUri"),
        "a value that cannot be printed cannot be reported"
    );

    let different = HEAD.to_owned() + "Zm9u";
    let other = data_uri::read(&different).expect("a valid URI");
    assert_ne!(once, other, "different payloads are different values");
}

#[test]
fn a_relative_path_is_not_a_data_uri_and_says_so() {
    // The commonest non-fault there is: a document naming a second file.
    assert_eq!(refused("model.bin"), DataUriError::NotADataUri);
    assert!(!data_uri::looks_like("model.bin"));
    assert!(data_uri::looks_like(HEAD));
}

#[test]
fn a_multibyte_character_where_the_scheme_would_end_is_not_a_panic() {
    // Five bytes into `dat\u{e9}...` is the middle of a character, and a
    // reader comparing strings by slicing rather than by bytes ends the
    // process here instead of returning a refusal.
    assert_eq!(refused("dat\u{e9}foo"), DataUriError::NotADataUri);
    assert!(!data_uri::looks_like("dat\u{e9}foo"));

    // The same hazard at the other end: seven bytes back from the comma
    // lands mid-character in this one.
    assert_eq!(
        refused("data:text/\u{e9}\u{e9}\u{e9}\u{e9},Zm9v"),
        DataUriError::NotBase64
    );
}

#[test]
fn a_uri_with_no_comma_has_no_payload() {
    assert_eq!(
        refused("data:application/octet-stream;base64"),
        DataUriError::NoPayload
    );
}

#[test]
fn a_percent_encoded_payload_is_refused_as_this_readers_limit() {
    // Legal RFC 2397, not implemented here, and the message says which
    // of those two it is.
    let refusal = refused("data:text/plain,hello");
    assert_eq!(refusal, DataUriError::NotBase64);
    let said = refusal.to_string();
    assert!(
        said.contains("this reader"),
        "the message should own the limit rather than blame the document: {said}"
    );
}

#[test]
fn the_marker_has_to_be_a_parameter_rather_than_a_suffix() {
    // `base64` without its semicolon is part of the media type, not the
    // instruction, and reading it as the instruction would decode a
    // payload nobody said was encoded.
    assert_eq!(refused("data:base64,Zm9v"), DataUriError::NotBase64);
}

#[test]
fn whitespace_in_the_payload_is_a_bad_digit_rather_than_skipped() {
    // Skipping it is common and gives one payload two spellings.
    //
    // **The whitespace takes a character's place rather than being added
    // between characters**, because a payload with a space *inserted* is
    // no longer a whole number of groups and the length check answers
    // first -- correctly, and it is the next test that pins that order.
    for filler in [" ", "\n", "\r", "\t"] {
        let uri = HEAD.to_owned() + "Zm v".replace(' ', filler).as_str();
        let at = uri.find(filler).expect("the filler is in there");
        assert_eq!(at, HEAD.len() + 2, "{uri:?}");
        assert_eq!(
            refused(&uri),
            DataUriError::BadDigit {
                byte: filler.as_bytes()[0],
                at,
            },
            "{uri:?}"
        );
    }
}

#[test]
fn a_payload_of_the_wrong_length_is_measured_before_it_is_read() {
    // Two faults, one answer, and the cheap whole-payload one comes
    // first: counting characters needs no alphabet, and a reader that
    // reported the bad character instead would be describing group four
    // of a payload that has no group four.
    let uri = HEAD.to_owned() + "Zm 9v";
    assert_eq!(refused(&uri), DataUriError::NotWholeGroups { len: 5 });
}

#[test]
fn the_url_safe_alphabet_is_not_this_one() {
    // `-` and `_` are base64url. A URI payload uses `+` and `/`, and
    // accepting both alphabets would decode two texts to one resource.
    for wrong in ['-', '_'] {
        let uri = format!("{HEAD}Zm9{wrong}");
        assert_eq!(
            refused(&uri),
            DataUriError::BadDigit {
                byte: wrong as u8,
                at: HEAD.len() + 3,
            },
            "{uri}"
        );
    }
    assert_eq!(bytes(&format!("{HEAD}Zm9+")), [0x66, 0x6f, 0x7e]);
    assert_eq!(bytes(&format!("{HEAD}Zm9/")), [0x66, 0x6f, 0x7f]);
}

#[test]
fn a_payload_that_is_not_whole_groups_is_refused() {
    for short in ["Z", "Zm", "Zm9", "Zm9vZ"] {
        let uri = HEAD.to_owned() + short;
        assert_eq!(
            refused(&uri),
            DataUriError::NotWholeGroups { len: short.len() },
            "{uri}"
        );
    }
}

#[test]
fn padding_before_the_last_group_is_refused() {
    let uri = HEAD.to_owned() + "Zg==Zg==";
    assert_eq!(
        refused(&uri),
        DataUriError::BadPadding { at: HEAD.len() + 2 }
    );
}

#[test]
fn padding_in_the_first_half_of_a_group_is_refused() {
    // Two `=` are legal; four are not, and neither is a group that is
    // nothing but padding.
    for (payload, offset) in [("Zm9v====", 4), ("=m9v", 0), ("Z=9v", 1)] {
        let uri = HEAD.to_owned() + payload;
        assert_eq!(
            refused(&uri),
            DataUriError::BadPadding {
                at: HEAD.len() + offset
            },
            "{uri}"
        );
    }
}

#[test]
fn a_digit_after_padding_points_at_the_padding() {
    // The digit is what was found; the padding is what was wrong.
    let uri = HEAD.to_owned() + "Zm=v";
    assert_eq!(
        refused(&uri),
        DataUriError::BadPadding { at: HEAD.len() + 2 }
    );
}

#[test]
fn a_final_group_with_stray_bits_is_refused_though_it_would_decode() {
    // `QQ==` and `QR==` would both yield the single byte `A`: the last
    // four bits of the second character reach no output byte. Accepting
    // both means accepting two texts as one resource.
    assert_eq!(bytes(&(HEAD.to_owned() + "QQ==")), b"A");

    let uri = HEAD.to_owned() + "QR==";
    assert_eq!(
        refused(&uri),
        DataUriError::NonCanonical {
            at: HEAD.len() + 1,
            bits: 1,
        }
    );

    // One padding character leaves two bits rather than four, and the
    // carrier is the third character rather than the second.
    assert_eq!(bytes(&(HEAD.to_owned() + "Zm8=")), b"fo");
    let uri = HEAD.to_owned() + "Zm9=";
    assert_eq!(
        refused(&uri),
        DataUriError::NonCanonical {
            at: HEAD.len() + 2,
            bits: 0b01,
        }
    );
}

/// **Every variant, asked its name and its message.**
///
/// Not only the ones a provocation above reaches: a refusal a caller
/// cannot key on is half a refusal, and the whole list is asked once so
/// that a variant added later is covered by a test that already exists.
#[test]
fn every_refusal_names_itself_and_says_something() {
    let all = [
        DataUriError::NotADataUri,
        DataUriError::NoPayload,
        DataUriError::NotBase64,
        DataUriError::BadDigit { byte: b' ', at: 7 },
        DataUriError::NotWholeGroups { len: 3 },
        DataUriError::BadPadding { at: 9 },
        DataUriError::NonCanonical { at: 9, bits: 3 },
    ];

    let mut names = Vec::new();
    for refusal in all {
        let name = refusal.name();
        assert!(!name.is_empty(), "{refusal:?} has no name");
        assert!(!refusal.to_string().is_empty(), "{refusal:?} says nothing");
        assert!(
            !names.contains(&name),
            "{name} is two different refusals, and a caller keying on it cannot tell which"
        );
        names.push(name);
    }

    // **No variant here is unreachable from this module**, unlike the
    // geometry refusals five readers share. One reader, one enum, and
    // every entry above is provoked by a test in this file -- which is a
    // claim worth stating, because the day it stops being true is the day
    // a refusal became decoration.
    assert_eq!(names.len(), 7, "the count is part of the claim");
}

#[test]
fn the_fixture_encoder_agrees_with_the_published_vectors() {
    // The round trip is only evidence if the encoder is right, so it is
    // pinned against the same vectors rather than against the decoder.
    for (plain, encoded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(encode(plain.as_bytes()), encoded, "{plain}");
    }
}
