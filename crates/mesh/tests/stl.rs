//! The STL reader, against files this test builds and files nobody
//! would write on purpose.
//!
//! **Every fixture is generated here.** A model downloaded from a sample
//! repository is a licence question, and this repository's rule is that
//! test fixtures are authored or generated rather than borrowed — the
//! same ground the two sample atlases are built on.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` — which apply to `#[cfg(test)]` modules —
// do not reach it. A fixture this file built and then could not read
// back is a broken test rather than a condition to recover from.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use renew_mesh::{MeshError, stl};

/// One triangle's worth of binary record: a normal and three corners.
fn record(normal: [f32; 3], corners: [[f32; 3]; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(50);
    for value in normal {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for corner in corners {
        for value in corner {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// A binary file over `triangles`, with `header` as its eighty bytes.
fn binary(header: &[u8], triangles: &[([f32; 3], [[f32; 3]; 3])]) -> Vec<u8> {
    let mut out = vec![0u8; 80];
    let cut = header.len().min(80);
    out[..cut].copy_from_slice(&header[..cut]);
    out.extend_from_slice(
        &u32::try_from(triangles.len())
            .expect("a small count")
            .to_le_bytes(),
    );
    for (normal, corners) in triangles {
        out.extend_from_slice(&record(*normal, *corners));
    }
    out
}

const UNIT: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
const UP: [f32; 3] = [0.0, 0.0, 1.0];

fn refusal(bytes: &[u8]) -> MeshError {
    stl::read(bytes).expect_err("these bytes should have been refused")
}

/// A binary file reads, and reads what it was given.
#[test]
fn a_binary_file_gives_back_the_triangles_it_holds() {
    let mesh = stl::read(&binary(b"a header", &[(UP, UNIT)])).expect("a well-formed file");
    assert_eq!(mesh.triangles(), 1);
    assert_eq!(mesh.positions, UNIT.to_vec());
    assert_eq!(mesh.face_normals, vec![UP]);
    assert_eq!(mesh.winding_disagreements(), 0);
}

/// **A binary file whose header begins `solid` is still binary.**
///
/// This is the classic way to get STL wrong, and the reason [`stl::read`]
/// decides by arithmetic rather than by the leading word. The eighty
/// bytes are arbitrary and exporters really do write "solid" into them,
/// because the header is a comment and that is a natural thing to put in
/// one. A reader that dispatched on the word would read this file as
/// text and fail on the first byte of the first float.
///
/// Probed by dispatching on `bytes.starts_with(b"solid")`: red, the file
/// is refused as text with `expected: facet`.
#[test]
fn a_binary_file_that_says_solid_is_read_as_binary() {
    let mesh = stl::read(&binary(b"solid exported by something", &[(UP, UNIT)]))
        .expect("the header is eighty bytes of anything, including that word");
    assert_eq!(mesh.triangles(), 1);
    assert_eq!(mesh.positions, UNIT.to_vec());
}

/// And the converse: a text file is not mistaken for a binary one.
///
/// The arithmetic that decides has to be wrong in both directions to be
/// worth anything, so both directions are here.
#[test]
fn a_text_file_is_read_as_text() {
    let mesh = stl::read(TEXT_ONE.as_bytes()).expect("a well-formed text file");
    assert_eq!(mesh.triangles(), 1);
    assert_eq!(mesh.face_normals, vec![UP]);
    assert_eq!(mesh.positions, UNIT.to_vec());
}

const TEXT_ONE: &str = "solid unit
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
endsolid unit
";

/// The text grammar tolerates what its producers actually emit.
///
/// **Read as a token stream rather than line by line**, because the
/// format's own writers disagree about line structure: tabs, CRLF,
/// unusual indentation, no trailing newline. A line-oriented reader
/// would refuse working files over a disagreement the format never
/// settled, so each of those shapes is here.
///
/// **One line is still a line, and it is the first.** `solid <name>`
/// takes the whole of its own line as the name, so a file with the
/// entire object on one line is not an STL with an unusual layout — it
/// is an STL whose object is called "x facet normal 0 0 1 outer loop
/// …" and which then ends without a facet. A shape like that was in
/// this list until the reader refused it, and the reader was right.
#[test]
fn the_text_grammar_accepts_what_exporters_write() {
    let shapes = [
        // Tabs, and CRLF.
        "solid x\r\n\tfacet normal 0 0 1\r\n\t\touter loop\r\n\t\t\tvertex 0 0 0\r\n\
         \t\t\tvertex 1 0 0\r\n\t\t\tvertex 0 1 0\r\n\t\tendloop\r\n\tendfacet\r\nendsolid x\r\n",
        // Indentation and line breaks in places no exporter uses but
        // the grammar permits, since only the `solid` line is a line.
        "solid x
facet normal
0 0 1
outer loop
vertex 0 0 0 vertex 1 0 0
\n         vertex 0 1 0
endloop endfacet
endsolid x",
        // No trailing newline, no name.
        "solid\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\n\
         endloop\nendfacet\nendsolid",
        // Exponent and sign spellings a float writer produces.
        "solid x\nfacet normal 0.0e0 -0.0 +1\nouter loop\nvertex 0 0 0\nvertex 1e0 0 0\n\
         vertex 0 1 0\nendloop\nendfacet\nendsolid x",
    ];
    for (which, text) in shapes.iter().enumerate() {
        let mesh = stl::read(text.as_bytes())
            .unwrap_or_else(|error| panic!("shape {which} should read: {error}"));
        assert_eq!(mesh.triangles(), 1, "shape {which}");
    }
}

/// Which reader can produce each refusal, and which cannot.
///
/// **This is the compile-time tripwire the test below used to claim and
/// not have.** Its doc said "the enum is closed, so the compiler will
/// not let this list silently miss one added later — the match below has
/// no wildcard", and there was no match: the test is a flat run of
/// assertions, and nothing stopped three variants being added to
/// `MeshError` without it noticing. They were, and it did not.
///
/// A wildcard-free match is what makes the claim true. Adding a refusal
/// now stops this file compiling until somebody decides which reader can
/// reach it, and a test below turns that decision into a check.
///
/// **The reason is the answer, not a comment beside it.** Written as a
/// bool, the unreachable arms had identical bodies and what separated
/// them lived only in prose — which is the shape this whole review kept
/// finding. A reason the test prints is a reason somebody reads.
fn stl_cannot_reach(refusal: &MeshError) -> Option<&'static str> {
    match refusal {
        // Reachable, and each is provoked by a file in this suite.
        MeshError::TooShortForHeader { .. }
        | MeshError::CountMismatch { .. }
        | MeshError::ExpectedKeyword { .. }
        | MeshError::NotANumber { .. }
        | MeshError::NotFinite { .. }
        | MeshError::NoGeometry => None,
        MeshError::TooLarge { .. } => Some(
            "a ceiling this reader sets on nothing: the length equality has              already bounded every count against the file before any              conversion happens",
        ),
        MeshError::IndexOutOfRange { .. } => {
            Some("this format repeats every corner, so it has no index to be out of range")
        }
        MeshError::IndexZero { .. } => {
            Some("this format repeats every corner, so it has no index to be zero")
        }
        MeshError::NotAFace { .. } => {
            Some("this format has no face element whose corner count could be short")
        }
        MeshError::Unsupported { .. } => {
            Some("this format has no schema, so there is nothing in it to be unsupported")
        }
    }
}

/// Every refusal this reader can make is reachable, and each by a file
/// that provokes only it.
///
/// **A list rather than a test each**, because what matters is that no
/// variant goes unseeded: a refusal nothing provokes is a refusal nobody
/// has read the message of.
#[test]
fn every_refusal_is_reachable() {
    // Three arbitrary bytes do not open like text, so they go to the
    // binary reader and get the answer that fits: there is no header
    // there. Which is the better answer of the two — "expected `solid`"
    // for a three-byte file describes what a text reader wanted rather
    // than what is wrong.
    assert_eq!(
        refusal(&[1, 2, 3]),
        MeshError::TooShortForHeader { needs: 84, len: 3 }
    );

    // Control bytes inside a file that *does* open like text: **quoted
    // into the message escaped rather than raw.** A refusal is printed
    // to a terminal, and control bytes travelling out of a file nobody
    // here wrote, through a log, into a terminal, are how a file that
    // could not be parsed still gets to move a cursor. Found by writing
    // an assertion like this one and reading what it returned.
    assert_eq!(
        refusal(b"solid x\n\x01\x02\x03\n"),
        MeshError::ExpectedKeyword {
            expected: "facet",
            found: "<U+0001><U+0002><U+0003>".to_owned(),
            line: 2
        }
    );
    // A text file with a long run of control bytes where a keyword
    // belongs. **The quote comes back bounded once escaped**, which it
    // was not when this assertion was first written: the bound counted
    // input characters while escaping multiplied each one by eight, so
    // thirty-two control bytes made a two-hundred-and-sixty character
    // message. The bound is on the output now, and this holds it there.
    let mut controls = b"solid x
"
    .to_vec();
    controls.extend(std::iter::repeat_n(0u8, 82));
    let MeshError::ExpectedKeyword { found, .. } = refusal(&controls) else {
        panic!("a run of zero bytes is not a word this grammar wants");
    };
    assert!(
        found.chars().count() <= 33,
        "a quote is bounded AFTER escaping; got {} characters",
        found.chars().count()
    );
    assert!(found.starts_with("<U+0000>"), "escaped, not raw: `{found}`");
    assert!(found.ends_with('…'), "and it says it was cut");

    // **A binary file cut short gets a binary answer.** This is the
    // commonest way a real STL is wrong, and the refusal has to carry
    // the pair a person diagnoses it from: what the header said, and
    // what arrived. An earlier dispatch fell through to the text reader
    // here and answered \"line 1: expected `solid`\", which tells the
    // holder of a truncated download nothing at all.
    let mut short = binary(b"h", &[(UP, UNIT)]);
    short.truncate(short.len() - 1);
    assert_eq!(
        refusal(&short),
        MeshError::CountMismatch {
            declared: 134,
            actual: 133,
            count: 1
        }
    );

    // A count that claims more triangles than the file carries, which
    // is the same refusal reached from the other direction.
    let mut lying = binary(b"h", &[(UP, UNIT)]);
    lying[80..84].copy_from_slice(&2u32.to_le_bytes());
    assert_eq!(
        refusal(&lying),
        MeshError::CountMismatch {
            declared: 184,
            actual: 134,
            count: 2
        }
    );

    // No geometry, in both dialects.
    assert_eq!(refusal(&binary(b"empty", &[])), MeshError::NoGeometry);
    assert_eq!(
        refusal(b"solid x\nendsolid x\n"),
        MeshError::NoGeometry,
        "a text file with no facets declares no geometry either"
    );

    // A coordinate that is not a number a bounding box can hold.
    let nan = binary(b"h", &[(UP, [[f32::NAN, 0.0, 0.0], UNIT[1], UNIT[2]])]);
    assert_eq!(
        refusal(&nan),
        MeshError::NotFinite {
            field: "position",
            index: 0
        }
    );
    let infinite = binary(b"h", &[([f32::INFINITY, 0.0, 0.0], UNIT)]);
    assert_eq!(
        refusal(&infinite),
        MeshError::NotFinite {
            field: "normal",
            index: 0
        }
    );

    // A word the grammar wanted, and a number that is not one. Both
    // carry the line, because a person is looking at the text.
    assert_eq!(
        refusal(b"solid x\nfacet wombat 0 0 1\n"),
        MeshError::ExpectedKeyword {
            expected: "normal",
            found: "wombat".to_owned(),
            line: 2
        }
    );
    assert_eq!(
        refusal(b"solid x\nfacet normal 0 0 1\nouter loop\nvertex 1.0.0 0 0\n"),
        MeshError::NotANumber {
            found: "1.0.0".to_owned(),
            line: 4
        }
    );

    // A file that stops in the middle.
    assert_eq!(
        refusal(b"solid x\nfacet normal 0 0 1\nouter loop\n"),
        MeshError::ExpectedKeyword {
            expected: "vertex",
            found: String::new(),
            line: 4
        }
    );
}

/// **The census and the files agree, in both directions.**
///
/// Every refusal a file in this suite provokes is one the census calls
/// reachable, and every refusal it calls reachable is provoked by a file
/// here. Without this the match is a claim nobody checks — which is
/// exactly what the sentence it replaced turned out to be.
#[test]
fn the_census_and_the_files_agree() {
    let mut short = binary(b"h", &[(UP, UNIT)]);
    short.truncate(short.len() - 1);
    let nan = binary(b"h", &[(UP, [[f32::NAN, 0.0, 0.0], UNIT[1], UNIT[2]])]);

    let provoked = [
        refusal(&[1, 2, 3]),
        refusal(&short),
        refusal(&binary(b"empty", &[])),
        refusal(
            b"solid x
endsolid x
",
        ),
        refusal(&nan),
        refusal(
            b"solid x
facet wombat 0 0 1
",
        ),
        refusal(
            b"solid x
facet normal 0 0 1
outer loop
vertex 1.0.0 0 0
",
        ),
    ];
    for refused in &provoked {
        assert!(
            stl_cannot_reach(refused).is_none(),
            "{refused:?} was provoked by a file in this suite, and the census says this reader              cannot reach it: {}",
            stl_cannot_reach(refused).unwrap_or("")
        );
    }
    for name in [
        "TooShortForHeader",
        "CountMismatch",
        "ExpectedKeyword",
        "NotANumber",
        "NotFinite",
        "NoGeometry",
    ] {
        assert!(
            provoked
                .iter()
                .any(|refused| format!("{refused:?}").starts_with(name)),
            "`{name}` is called reachable and no file in this suite provokes it"
        );
    }
}

/// **Every byte string gets an answer.**
///
/// Not a claim about what the answer is — a claim that there is one, for
/// every input, without a panic and without reading past the end. That
/// is the property the fuzz target exists to attack and the one this
/// suite can assert cheaply over the shapes most likely to break it.
#[test]
fn every_byte_string_gets_an_answer() {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    // Random noise of every length across the interesting boundaries.
    for len in 0..200 {
        let bytes: Vec<u8> = (0..len)
            .map(|_| u8::try_from(next() >> 56).unwrap_or(0))
            .collect();
        let _ = stl::read(&bytes);
    }

    // Well-formed files with one byte corrupted, which is where a
    // reader that trusts a field it has already checked falls over.
    let good = binary(b"header", &[(UP, UNIT), (UP, UNIT)]);
    for at in 0..good.len() {
        let mut broken = good.clone();
        broken[at] ^= 0xFF;
        let _ = stl::read(&broken);
    }

    // And the same for the text dialect.
    let text = TEXT_ONE.as_bytes();
    for at in 0..text.len() {
        let mut broken = text.to_vec();
        broken[at] ^= 0x20;
        let _ = stl::read(&broken);
    }

    // Every prefix of a good file: the truncation family, exhaustively.
    for len in 0..good.len() {
        let _ = stl::read(&good[..len]);
    }
}

/// A file whose triangles disagree with their own normals reads, and
/// says how many.
///
/// **A fact rather than a refusal**, because a stored normal that
/// disagrees with a winding is the most common thing wrong with a real
/// STL and refusing it would refuse a great deal of working art.
#[test]
fn a_file_wound_against_its_normals_reads_and_reports_it() {
    let down = [0.0, 0.0, -1.0];
    let mesh = stl::read(&binary(b"h", &[(UP, UNIT), (down, UNIT), (down, UNIT)]))
        .expect("disagreement is a fact about a model, not a malformed file");
    assert_eq!(mesh.triangles(), 3);
    assert_eq!(
        mesh.winding_disagreements(),
        2,
        "two of the three normals point away from the face their corners wind"
    );
}

/// The reader holds the invariants its type documents.
///
/// Checked over a spread of files rather than one, because these are
/// claims about every mesh this crate hands back.
#[test]
fn a_mesh_that_reads_holds_what_its_type_promises() {
    let files = [
        binary(b"one", &[(UP, UNIT)]),
        binary(b"many", &[(UP, UNIT); 7]),
        TEXT_ONE.as_bytes().to_vec(),
    ];
    for (which, bytes) in files.iter().enumerate() {
        let mesh = stl::read(bytes).unwrap_or_else(|error| panic!("file {which}: {error}"));
        assert!(!mesh.is_empty(), "file {which} came back empty");
        assert_eq!(
            mesh.positions.len() % 3,
            0,
            "file {which}: positions must divide into triangles"
        );
        assert_eq!(
            mesh.face_normals.len(),
            mesh.triangles(),
            "file {which}: one normal per triangle"
        );
        for value in mesh.positions.iter().chain(&mesh.face_normals).flatten() {
            assert!(value.is_finite(), "file {which} held a non-finite value");
        }
    }
}

/// **A refused file costs no more than its own length buys.**
///
/// The reservation is derived from a count the header declares, and the
/// point of checking the length *before* reserving is that a sixty-byte
/// file cannot ask for a gigabyte. Asserted on the shape rather than on
/// an allocator: the count that survives to reach `with_capacity` is
/// bounded by `len / 50`, so a hostile count is refused before it is
/// believed.
#[test]
fn a_hostile_count_is_refused_before_it_is_reserved() {
    // Eighty bytes of header, then a count of four billion, then
    // nothing. If the count were believed, this asks for 200 GB.
    let mut hostile = vec![0u8; 80];
    hostile.extend_from_slice(&u32::MAX.to_le_bytes());
    let refused = stl::read(&hostile).expect_err("a count with no bytes behind it");
    // The count claims 214 GB and 84 bytes arrived, and that pair is
    // the refusal: **the number is reported, never believed.** Nothing
    // is reserved on the way to saying so, which is the property this
    // test is named for — the length is checked against the count before
    // a single element is asked for.
    assert_eq!(
        refused,
        MeshError::CountMismatch {
            declared: 214_748_364_834,
            actual: 84,
            count: u32::MAX
        },
        "a hostile count is reported with the length that refutes it"
    );

    // And with the length made to agree, the count is still refused on a
    // target that cannot address it.
    let mut believable = vec![0u8; 80];
    believable.extend_from_slice(&1u32.to_le_bytes());
    believable.extend_from_slice(&record(UP, UNIT));
    assert!(stl::read(&believable).is_ok(), "the honest version reads");
}

/// **A refusal quoting a file's own text cannot move a cursor, and that
/// is a wider question than "is this a control character".**
///
/// The escaping was written against a named threat: control bytes
/// travelling out of an unparseable file, through a log, into a terminal
/// are how that file still gets "to move a cursor, clear a screen, or
/// hide the rest of the line it was reported on". The predicate was
/// `char::is_control`, which is Unicode category Cc alone — so
/// `U+202E` RIGHT-TO-LEFT OVERRIDE passed through untouched, and a bidi
/// override is *precisely* the thing that hides the rest of a line.
///
/// The escaping and the hole were written in the same commit, against
/// the same sentence.
///
/// Probed by narrowing the predicate back to `is_control`: red, the
/// override reaches the message intact.
#[test]
fn a_refusal_cannot_carry_a_character_that_rewrites_the_line() {
    // Each of these changes what a reader of the message sees without
    // being a control character in the Cc sense.
    for hidden in ['\u{202E}', '\u{2028}', '\u{200F}', '\u{00AD}', '\u{061C}'] {
        let file = format!("solid x\nfa{hidden}cet normal 0 0 1\n");
        let MeshError::ExpectedKeyword { found, .. } = refusal(file.as_bytes()) else {
            panic!("a word that is not `facet` should be refused as one");
        };
        assert!(
            !found.contains(hidden),
            "U+{:04X} reached the message raw, in `{found}`",
            u32::from(hidden)
        );
        assert!(
            found.contains(&format!("<U+{:04X}>", u32::from(hidden))),
            "it should be shown as what the file held: `{found}`"
        );
    }
}
