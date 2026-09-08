//! The MTL reader, against libraries this test writes.
//!
//! Every fixture is authored here for the reason the other suites give:
//! a library downloaded beside somebody's model is a licence question,
//! and the rule in this repository is that fixtures are written rather
//! than borrowed.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` — which apply to `#[cfg(test)]` modules —
// do not reach it. A fixture this file wrote and then could not read
// back is a broken test rather than a condition to recover from.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use renew_mesh::MeshError;
use renew_mesh::mtl::{self, MapSlot};

/// The refusal a byte string provokes, or a panic naming what it read.
fn refusal(bytes: &[u8]) -> MeshError {
    match mtl::read(bytes) {
        Ok(library) => panic!("expected a refusal, read {} materials", library.len()),
        Err(refusal) => refusal,
    }
}

/// **The ordinary case: two materials, each with its own factors.**
#[test]
fn a_library_of_two_materials_reads() {
    let library = "# a library\n\
                   newmtl steel\n\
                   Ka 0.1 0.1 0.1\n\
                   Kd 0.4 0.4 0.45\n\
                   Ks 0.9 0.9 0.9\n\
                   Ns 250\n\
                   d 1\n\
                   \n\
                   newmtl glass\n\
                   Kd 0.8 0.9 1\n\
                   d 0.25\n";
    let read = mtl::read(library.as_bytes()).expect("an ordinary library");
    assert_eq!(read.len(), 2);
    assert_eq!(read[0].name, "steel");
    assert_eq!(read[0].ambient, Some([0.1, 0.1, 0.1]));
    assert_eq!(read[0].specular, Some([0.9, 0.9, 0.9]));
    assert_eq!(read[0].shininess, Some(250.0));
    assert_eq!(read[0].opacity, Some(1.0));
    assert_eq!(read[1].name, "glass");
    assert_eq!(read[1].diffuse, Some([0.8, 0.9, 1.0]));
    assert_eq!(read[1].opacity, Some(0.25));
    assert_eq!(
        read[1].ambient, None,
        "a file that says nothing about ambient has said nothing, and None is not a default"
    );
}

/// **A one-component colour is a grey, which is the format's own
/// spelling and common in hand-written libraries.**
///
/// Probed by requiring three components: red, an ordinary library stops
/// reading.
#[test]
fn a_colour_of_one_component_is_a_grey() {
    let grey = "newmtl paper\nKd 0.5\n";
    let read = mtl::read(grey.as_bytes()).expect("one component is a colour");
    assert_eq!(read[0].diffuse, Some([0.5, 0.5, 0.5]));
}

/// **`Tr` is the reciprocal spelling of `d`, and the last one written
/// wins.**
///
/// The module documents why the two are not reconciled: real exporters
/// write them inconsistently and no reader can tell which tool wrote a
/// file from the file alone.
///
/// Probed by reading `Tr` as opacity rather than one minus it: red, a
/// transparent material comes back opaque.
#[test]
fn transparency_is_the_reciprocal_of_opacity_and_the_last_word_wins() {
    let transparent = "newmtl glass\nTr 0.75\n";
    let read = mtl::read(transparent.as_bytes()).expect("`Tr` is a spelling of `d`");
    assert_eq!(read[0].opacity, Some(0.25));

    let both = "newmtl glass\nTr 0.75\nd 0.9\n";
    let read = mtl::read(both.as_bytes()).expect("both spellings are legal");
    assert_eq!(read[0].opacity, Some(0.9), "the last one written wins");

    let reversed = "newmtl glass\nd 0.9\nTr 0.75\n";
    let read = mtl::read(reversed.as_bytes()).expect("both spellings are legal");
    assert_eq!(read[0].opacity, Some(0.25), "and it wins either way round");
}

/// **Every map slot the reader knows, and the name taken from the end of
/// the line.**
///
/// A map line may carry options before the file name, which is why the
/// name is the last word rather than the second: `-s 1 1 1 wood.png`
/// names `wood.png`, and a reader taking the second word would try to
/// open `-s`.
///
/// Probed by taking the second word: red, the option is read as the file
/// name.
#[test]
fn a_map_names_the_file_at_the_end_of_its_line() {
    let mapped = "newmtl wall\n\
                  map_Ka ao.png\n\
                  map_Kd -s 1 1 1 brick.png\n\
                  map_Ks spec.png\n\
                  map_Ns gloss.png\n\
                  map_d cutout.png\n\
                  map_bump height.png\n\
                  norm normal.png\n";
    let read = mtl::read(mapped.as_bytes()).expect("a mapped material");
    let maps = &read[0].maps;
    assert_eq!(maps.len(), 7, "every slot the reader knows");
    assert_eq!(maps[0].slot, MapSlot::Ambient);
    assert_eq!(maps[1].slot, MapSlot::Diffuse);
    assert_eq!(
        maps[1].name, "brick.png",
        "the name is the last word, past the options"
    );
    assert_eq!(maps[5].slot, MapSlot::Bump);
    assert_eq!(
        maps[6].slot,
        MapSlot::Normal,
        "a normal map replaces the normal where a height map perturbs it, \
         so folding them would hand a caller the wrong thing to sample"
    );
}

/// **A `map_*` line with nothing after it names no file and adds no
/// map.**
///
/// The keyword is understood and the line is empty of a name; inventing
/// one would put a file in the material that the library never
/// mentioned.
///
/// Probed by pushing a map with an empty name: red, a material comes
/// back carrying a texture nothing asked for.
#[test]
fn a_map_line_with_no_name_adds_no_map() {
    let bare = "newmtl wall
map_Kd
Kd 0.5
";
    let read = mtl::read(bare.as_bytes()).expect("a bare map keyword is not a refusal");
    assert!(read[0].maps.is_empty(), "no name, no map");
    assert_eq!(
        read[0].diffuse,
        Some([0.5, 0.5, 0.5]),
        "and the line after it is still read"
    );
}

/// **Keywords this reader does not implement cost the file nothing.**
///
/// A real library is full of them, and none changes the factors above.
///
/// Probed by refusing an unknown keyword: red, an ordinary library stops
/// being readable.
#[test]
fn keywords_this_reader_does_not_know_are_skipped() {
    let extended = "newmtl modern\n\
                    illum 2\n\
                    Pr 0.4\n\
                    Pm 0.0\n\
                    aniso 0.1\n\
                    Kd 0.5 0.5 0.5\n";
    let read = mtl::read(extended.as_bytes()).expect("the factors are still in there");
    assert_eq!(read[0].diffuse, Some([0.5, 0.5, 0.5]));
}

/// **A property before any `newmtl` has no material to belong to.**
///
/// It is a file whose first material is missing, not a value to attach
/// to nothing. The refusal names the keyword that arrived too early and
/// the line it was on.
///
/// Probed by attaching it to a material created on demand: red, a file
/// missing its `newmtl` reads as if it had one.
#[test]
fn a_property_before_any_material_is_refused() {
    let headless = "# a library\nKd 0.5 0.5 0.5\nnewmtl steel\n";
    let MeshError::ExpectedKeyword {
        expected,
        found,
        line,
    } = refusal(headless.as_bytes())
    else {
        panic!("a property needs a material");
    };
    assert_eq!(expected, "newmtl");
    assert!(found.contains("Kd"), "it names what arrived early: {found}");
    assert_eq!(line, 2);
}

/// **A library that declares no material is refused by name.**
///
/// Distinct from a malformed file: this one is well-formed and has
/// nothing in it this reader can use, and the caller's next move is to
/// find the right library rather than to re-export this one.
#[test]
fn a_library_with_no_material_is_refused() {
    let empty = "# nothing here\nillum 2\n";
    let MeshError::Unsupported { wanted } = refusal(empty.as_bytes()) else {
        panic!("a library with no material has nothing to give");
    };
    assert_eq!(wanted, "newmtl");
}

/// **A value that is not a number, and one that is not finite.**
#[test]
fn a_value_that_is_not_a_finite_number_is_refused() {
    let word = "newmtl steel\nNs shiny\n";
    let MeshError::NotANumber { found, line } = refusal(word.as_bytes()) else {
        panic!("`shiny` is not a number");
    };
    assert!(found.contains("shiny"), "the refusal quotes it: {found}");
    assert_eq!(line, 2);

    let infinite = "newmtl steel\nKd 1 1 inf\n";
    let MeshError::NotFinite { field, .. } = refusal(infinite.as_bytes()) else {
        panic!("an infinite colour is not a colour");
    };
    assert_eq!(field, "diffuse");

    let short = "newmtl steel\nKd 1 1\n";
    assert!(
        matches!(refusal(short.as_bytes()), MeshError::NotANumber { .. }),
        "two components is neither a grey nor a colour"
    );
}

/// **A refusal names the material it was in, not the component within a
/// colour.**
///
/// `NotFinite` carries `index`, documented as the record as the file
/// stores them and rendered as `record {index}`. For a library a record
/// is a material, and the first version of this reader passed the colour
/// component instead MM which named a material that need not exist.
///
/// Probed by passing the component index again: red.
#[test]
fn a_refusal_names_the_material_and_not_the_component() {
    // The second material, and the THIRD component of its colour, so
    // the two numbers cannot be confused for each other.
    let library = "newmtl steel\nKd 1 1 1\nnewmtl broken\nKd 1 1 inf\n";
    let MeshError::NotFinite { field, index } = refusal(library.as_bytes()) else {
        panic!("an infinite colour is refused");
    };
    assert_eq!(field, "diffuse");
    assert_eq!(
        index, 1,
        "the second material is record 1; the second component is not"
    );
}

/// **A name declared twice is kept twice, in order.**
///
/// The format's own rule is that a later definition wins, so the order
/// is what carries the meaning. Collapsing them here would answer, on
/// the caller's behalf, a question the file already answers.
///
/// Probed by keeping only the first: red, the definition the format says
/// wins is the one thrown away.
#[test]
fn a_name_declared_twice_is_kept_twice() {
    let redefined = "newmtl steel\nKd 1 0 0\nnewmtl steel\nKd 0 1 0\n";
    let read = mtl::read(redefined.as_bytes()).expect("redefinition is legal");
    assert_eq!(read.len(), 2);
    assert_eq!(read[0].diffuse, Some([1.0, 0.0, 0.0]));
    assert_eq!(
        read[1].diffuse,
        Some([0.0, 1.0, 0.0]),
        "the later definition is present, and it is the one the format says wins"
    );
}

/// **Bytes that are not text are refused rather than lossily
/// converted.**
#[test]
fn bytes_that_are_not_text_are_refused() {
    let mut bytes = b"newmtl steel\n".to_vec();
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    let MeshError::ExpectedKeyword { expected, .. } = refusal(&bytes) else {
        panic!("a file that is not text is not this format");
    };
    assert_eq!(expected, "text");
}

/// **Every byte string gets an answer.**
///
/// No input panics, no input hangs, and an accepted one holds materials
/// whose numbers are all finite.
#[test]
fn every_byte_string_gets_an_answer() {
    let mut seed = 0x853C_49E6_748F_EA9B_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let alphabet = b"newmtlKadKsNrTbump_ 0123456789-.#\ne";
    let span = u64::try_from(alphabet.len()).unwrap_or(1);
    for _ in 0..2000 {
        let length = usize::try_from(next() % 70).unwrap_or(0);
        let bytes: Vec<u8> = (0..length)
            .map(|_| alphabet[usize::try_from(next() % span).unwrap_or(0)])
            .collect();
        let Ok(library) = mtl::read(&bytes) else {
            continue;
        };
        assert!(!library.is_empty(), "an accepted library has a material");
        for material in &library {
            for colour in [
                material.ambient,
                material.diffuse,
                material.specular,
                material.emissive,
            ]
            .into_iter()
            .flatten()
            {
                for value in colour {
                    assert!(value.is_finite(), "a colour nothing can bound");
                }
            }
            for value in [material.shininess, material.opacity].into_iter().flatten() {
                assert!(value.is_finite(), "a factor nothing can bound");
            }
        }
    }
}

/// **Every refusal this reader can make is reachable, and every one it
/// cannot make says why.**
///
/// No wildcard arm, so a variant added later stops this file compiling
/// until somebody decides which it is.
fn mtl_cannot_reach(refusal: &MeshError) -> Option<&'static str> {
    match refusal {
        // Reachable, and each is provoked by a file in this suite.
        MeshError::ExpectedKeyword { .. }
        | MeshError::NotANumber { .. }
        | MeshError::NotFinite { .. }
        | MeshError::Unsupported { .. } => None,
        MeshError::TooShortForHeader { .. } => {
            Some("this format has no header, so there is no least size it can be")
        }
        MeshError::CountMismatch { .. } => {
            Some("this format declares no counts for a file to contradict")
        }
        MeshError::TooLarge { .. } => Some(
            "nothing here multiplies: every value is one number on one line and every \
             name is bytes copied out of the input, so there is no product to bound",
        ),
        MeshError::IndexOutOfRange { .. } | MeshError::IndexZero { .. } => {
            Some("this format indexes nothing; a material is named, never numbered")
        }
        MeshError::NotAFace { .. } => Some("this format describes surfaces, not their shape"),
        MeshError::NoGeometry => {
            Some("this format carries no geometry; a library with no material is `Unsupported`")
        }
    }
}

/// The census above and the files here agree.
#[test]
fn the_census_and_the_files_agree() {
    let provocations: [(&str, Vec<u8>); 4] = [
        ("ExpectedKeyword", b"Kd 1 1 1\n".to_vec()),
        ("NotANumber", b"newmtl steel\nNs shiny\n".to_vec()),
        ("NotFinite", b"newmtl steel\nKd 1 1 inf\n".to_vec()),
        ("Unsupported", b"# nothing here\n".to_vec()),
    ];
    for (name, bytes) in &provocations {
        let got = refusal(bytes);
        assert!(
            mtl_cannot_reach(&got).is_none(),
            "`{name}` is provoked by a file here, and the census calls it unreachable"
        );
        assert_eq!(
            variant(&got),
            *name,
            "the file meant to provoke `{name}` provoked something else"
        );
    }

    for refusal in [
        MeshError::TooShortForHeader { needs: 84, len: 3 },
        MeshError::CountMismatch {
            declared: 1,
            actual: 0,
            count: 1,
        },
        MeshError::TooLarge {
            field: "element count",
            value: 1,
        },
        MeshError::IndexOutOfRange {
            index: 1,
            count: 0,
            face: 0,
        },
        MeshError::IndexZero { line: 1 },
        MeshError::NotAFace {
            face: 0,
            corners: 2,
        },
        MeshError::NoGeometry,
    ] {
        assert!(
            mtl_cannot_reach(&refusal).is_some(),
            "{refusal:?} is claimed reachable and nothing here provokes it"
        );
    }
}

/// A refusal's variant, as a name, with no wildcard arm.
fn variant(refusal: &MeshError) -> &'static str {
    match refusal {
        MeshError::TooShortForHeader { .. } => "TooShortForHeader",
        MeshError::CountMismatch { .. } => "CountMismatch",
        MeshError::TooLarge { .. } => "TooLarge",
        MeshError::ExpectedKeyword { .. } => "ExpectedKeyword",
        MeshError::NotANumber { .. } => "NotANumber",
        MeshError::NotFinite { .. } => "NotFinite",
        MeshError::IndexOutOfRange { .. } => "IndexOutOfRange",
        MeshError::IndexZero { .. } => "IndexZero",
        MeshError::NotAFace { .. } => "NotAFace",
        MeshError::Unsupported { .. } => "Unsupported",
        MeshError::NoGeometry => "NoGeometry",
    }
}
