//! Which reader owns which bytes.
//!
//! **These tests are the reason this lives in the crate.** The order
//! `detect` tries formats in is a fact about the formats, and it depends
//! on what each reader accepts — so a change to `mtl::looks_like` or to
//! PLY's magic must be seen by tests that sit beside those readers, not
//! by a tool that happens to call them.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` — which apply to `#[cfg(test)]` modules —
// do not reach it. A fixture this file wrote and then could not read
// back is a broken test rather than a condition to recover from.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use renew_mesh::format::{self, Format};
use renew_mesh::{Mesh, blob};

const AN_OBJ: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
const A_PLY: &str = "ply\nformat ascii 1.0\nelement vertex 3\n\
                     property float x\nproperty float y\nproperty float z\n\
                     element face 1\nproperty list uchar int vertex_indices\n\
                     end_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n";
const A_TEXT_STL: &str = "solid one\nfacet normal 0 0 1\n  outer loop\n\
                          vertex 0 0 0\n vertex 1 0 0\n vertex 0 1 0\n\
                          endloop\nendfacet\nendsolid one\n";
const AN_MTL: &str = "newmtl steel\nKd 0.4 0.4 0.45\n";

fn a_blob() -> Vec<u8> {
    blob::write(&Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        ..Mesh::default()
    })
}

/// **Every format this crate reads is identified as itself.**
#[test]
fn each_format_is_recognised() {
    assert_eq!(format::detect(AN_OBJ.as_bytes()), Format::Obj);
    assert_eq!(format::detect(A_PLY.as_bytes()), Format::Ply);
    assert_eq!(format::detect(A_TEXT_STL.as_bytes()), Format::Stl);
    assert_eq!(format::detect(AN_MTL.as_bytes()), Format::Mtl);
    assert_eq!(format::detect(&a_blob()), Format::Blob);
}

/// **A blob is recognised as itself and not as a truncated STL.**
///
/// It has an eight-byte magic, so this is the one identification here
/// that is certain rather than a guess. Before this existed, feeding a
/// blob back to the tool that wrote it fell through to the STL fallback
/// and was reported as a file too short to hold an 84-byte header —
/// true of an STL, and no help at all.
///
/// Probed by removing the blob arm: red, and the message a user gets
/// talks about a header the file was never meant to have.
#[test]
fn a_blob_is_not_mistaken_for_a_truncated_stl() {
    let bytes = a_blob();
    assert_eq!(format::detect(&bytes), Format::Blob);
    let read = Format::Blob
        .read(&bytes)
        .expect("a blob carries geometry")
        .expect("and this one is well-formed");
    assert_eq!(read.triangles(), 1);
}

/// **A material library is recognised before the fallback.**
///
/// Otherwise it reaches the STL reader and is refused as a truncated
/// mesh — a true sentence that sends its reader nowhere useful.
///
/// Probed by removing the MTL arm: red, it comes back as `Stl`.
#[test]
fn a_material_library_is_recognised_and_carries_no_geometry() {
    assert_eq!(format::detect(AN_MTL.as_bytes()), Format::Mtl);
    assert!(!Format::Mtl.carries_geometry());
    assert!(
        Format::Mtl.read(AN_MTL.as_bytes()).is_none(),
        "there is no geometry to read, which is an answer rather than a refusal"
    );
    for other in [Format::Obj, Format::Ply, Format::Stl, Format::Blob] {
        assert!(other.carries_geometry(), "{} does", other.name());
    }
}

/// **Anything unrecognised falls to STL, because STL cannot answer for
/// itself.**
///
/// The format has no magic number, so "these are not STL bytes" and
/// "these are STL bytes cut short" are the same observation. Making it
/// the fallback is what lets a truncated STL reach the reader whose
/// refusals describe it.
#[test]
fn what_nothing_claims_is_offered_to_stl() {
    for bytes in [
        &b""[..],
        &b"nonsense"[..],
        &[0xFF, 0xFE, 0x00][..],
        // A binary STL's eighty-byte header is arbitrary bytes, so it
        // claims nothing and must land here.
        &[0u8; 84][..],
    ] {
        assert_eq!(format::detect(bytes), Format::Stl, "{bytes:?}");
    }
}

/// **A licence banner is not evidence, and does not spend the budget.**
///
/// The window used to be the first sixty-four lines flat, so an OBJ with
/// a sixty-four-line header fell through to the STL fallback and was
/// refused with an unactionable count mismatch — while `obj::read`
/// accepted the very same file. Exporters write banners; the boundary
/// was measured at exactly 63 working and 64 failing.
///
/// Probed by counting every line rather than every keyword-shaped one:
/// red at 64.
#[test]
fn a_long_comment_header_does_not_hide_the_geometry() {
    for banner in [0, 63, 64, 500] {
        let mut source = String::new();
        for line in 0..banner {
            source.push_str("# exported by something, line ");
            source.push_str(&line.to_string());
            source.push('\n');
        }
        source.push_str(AN_OBJ);
        assert_eq!(
            format::detect(source.as_bytes()),
            Format::Obj,
            "{banner} comment lines before the geometry"
        );
        let read = Format::Obj
            .read(source.as_bytes())
            .expect("obj carries geometry")
            .unwrap_or_else(|error| panic!("{banner} lines: {error}"));
        assert_eq!(read.triangles(), 1);
    }
}

/// **Blank lines are not evidence either.**
///
/// Padding is the other thing exporters emit, and it was spending the
/// same budget.
#[test]
fn padding_does_not_hide_the_geometry() {
    let padded = format!("{}{AN_OBJ}", "\n".repeat(300));
    assert_eq!(format::detect(padded.as_bytes()), Format::Obj);
}

/// **The budget still binds on a file that is not an OBJ.**
///
/// It exists so a text STL is not scanned to its end before being
/// declined. Sixteen lines that say nothing an OBJ says is enough to
/// decline, and this checks the decline still happens rather than the
/// budget having been quietly widened to "the whole file".
///
/// Probed by removing the budget: red, a long file of non-OBJ keywords
/// is read to its end and still declined — slower, and the assertion
/// below is what notices.
#[test]
fn a_file_that_says_nothing_an_obj_says_is_declined() {
    // Longer than the budget, entirely keywords no OBJ uses, and with a
    // real `v` far past the end of it: if the budget were gone this
    // would come back `Obj`.
    let mut source = String::new();
    for _ in 0..64 {
        source.push_str("widget 1 2 3\n");
    }
    source.push_str(AN_OBJ);
    assert_eq!(
        format::detect(source.as_bytes()),
        Format::Stl,
        "sixteen non-comment lines that say nothing OBJ says is enough to decline"
    );
}

/// **The name is what a machine keys on, and every format has a
/// distinct one.**
///
/// This census listed five formats while the type had six. Nothing
/// failed, because a list written by hand agrees with itself: the sixth
/// was simply never asked its name, and the assertion about which names
/// exist was true of the five that were.
///
/// **So the list is no longer only a list.** The match below has no
/// wildcard, which means a seventh format stops this file compiling
/// until somebody says what it is called — the same trick the refusal
/// censuses use, and the same reason: a vocabulary check that can be
/// out of date is not a check.
///
/// **What that does and does not guarantee, precisely.** A new variant
/// cannot be added without touching this file, and it cannot be given a
/// name here without saying which. It *can* still be left out of the
/// array below, in which case it is never asked its name at run time —
/// the compiler forces the arm, not the membership. Closing that last
/// gap needs a count derived from the type, which stable Rust does not
/// offer without a derive, and a derive is a dependency. The residual
/// hole is one line wide and is written down here rather than left for
/// somebody to find, which is the same bargain the MTL census makes
/// when it writes a sentence per unreachable variant.
#[test]
fn every_format_has_its_own_stable_name() {
    let all = [
        Format::Obj,
        Format::Mtl,
        Format::Stl,
        Format::Ply,
        Format::Blob,
        Format::Glb,
    ];

    for format in all {
        // No wildcard, deliberately. A format added to the type and not
        // to the array above still fails here, because this arm list is
        // what the compiler checks for completeness.
        let expected = match format {
            Format::Obj => "obj",
            Format::Mtl => "mtl",
            Format::Stl => "stl",
            Format::Ply => "ply",
            Format::Blob => "blob",
            Format::Glb => "glb",
        };
        assert_eq!(
            format.name(),
            expected,
            "{format:?} answers to a name this census does not know"
        );
    }

    let mut names: Vec<&str> = all.iter().map(|format| format.name()).collect();
    names.sort_unstable();
    let distinct = names.len();
    names.dedup();
    assert_eq!(names.len(), distinct, "two formats share a name: {names:?}");
    assert_eq!(names, vec!["blob", "glb", "mtl", "obj", "ply", "stl"]);
}

/// **Whether a format carries geometry is asked of every one of them.**
///
/// The other half of the same gap: a format added to the type inherits
/// an answer here from whichever arm its variant falls into, and nothing
/// says whether that answer was chosen or inherited. A material library
/// is the only one that carries none, and that is worth stating in a
/// place that breaks when it stops being true.
#[test]
fn only_a_material_library_carries_no_geometry() {
    for format in [
        Format::Obj,
        Format::Mtl,
        Format::Stl,
        Format::Ply,
        Format::Blob,
        Format::Glb,
    ] {
        let expected = match format {
            Format::Mtl => false,
            Format::Obj | Format::Stl | Format::Ply | Format::Blob | Format::Glb => true,
        };
        assert_eq!(
            format.carries_geometry(),
            expected,
            "{format:?} disagrees with what this census says it carries"
        );
    }
}

/// **`detect` answers for every byte string, and what it names can be
/// read or is honestly empty.**
///
/// The sweep that matters for a function standing in front of five
/// readers: no input makes it panic, and whatever it names either reads,
/// refuses, or carries no geometry.
#[test]
fn every_byte_string_gets_a_format() {
    let mut seed = 0x2545_F491_4F6C_DD1D_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let alphabet = b"plyendhaformtscivnwmKd 0123456789./#RENWMSH\n";
    let span = u64::try_from(alphabet.len()).unwrap_or(1);
    for _ in 0..2000 {
        let length = usize::try_from(next() % 90).unwrap_or(0);
        let bytes: Vec<u8> = (0..length)
            .map(|_| alphabet[usize::try_from(next() % span).unwrap_or(0)])
            .collect();
        let found = format::detect(&bytes);
        match found.read(&bytes) {
            None => assert_eq!(found, Format::Mtl, "only a library carries no geometry"),
            Some(Ok(mesh)) => {
                assert_eq!(
                    mesh.positions.len() % 3,
                    0,
                    "{found:?} gave whole triangles"
                );
                assert!(!mesh.is_empty());
            }
            Some(Err(_)) => {}
        }
    }
}

/// **A binary STL's header is eighty bytes of whatever its exporter felt
/// like**, and one of the things an exporter writes there is the name of
/// the thing it exported.
///
/// `ply` is a real English prefix — plywood, plywood-panel, plyboard —
/// and detection asked only whether the file *starts with* those three
/// bytes. So a valid binary STL named after plywood was handed to the PLY
/// reader, which refused it for having no `end_header`, and the tool
/// reported a PLY refusal about a file that was never a PLY.
///
/// **The reader was already stricter than the detector.** `ply::read`
/// tokenises the first line and matches the whole word, and `header_end`
/// requires `end_header` to both open and close a line. Only
/// `looks_like` took a prefix for a magic word — so the two disagreed
/// about the same bytes, which is the one thing a detector must never do.
#[test]
fn a_binary_stl_whose_header_begins_with_the_ply_magic_is_still_an_stl() {
    for header in [
        &b"plywood test model"[..],
        &b"ply"[..],
        &b"plyboard, 4mm"[..],
        &b"ply-1"[..],
    ] {
        let mut padded = [b' '; 80];
        padded[..header.len()].copy_from_slice(header);

        let mut file = padded.to_vec();
        file.extend_from_slice(&1u32.to_le_bytes());
        for value in [0.0f32, 0.0, 1.0] {
            file.extend_from_slice(&value.to_le_bytes());
        }
        for corner in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for value in corner {
                file.extend_from_slice(&value.to_le_bytes());
            }
        }
        file.extend_from_slice(&0u16.to_le_bytes());

        let shown = String::from_utf8_lossy(header).to_string();
        assert_eq!(
            format::detect(&file),
            Format::Stl,
            "a binary STL whose header opens {shown:?} is an STL"
        );
        let read = Format::Stl
            .read(&file)
            .expect("stl carries geometry")
            .unwrap_or_else(|error| panic!("{shown:?}: {error}"));
        assert_eq!(read.triangles(), 1, "{shown:?}");
    }
}

/// **And the magic still has to be the magic.**
///
/// The fix above is a place where tightening a check can go one step too
/// far and start declining real files, so this pins the other side: a
/// PLY is still a PLY with the line endings and the trailing whitespace
/// real files carry, and the leading blank lines the detector already
/// skipped.
#[test]
fn the_ply_magic_is_still_recognised_however_its_line_ends() {
    for opening in ["ply\n", "ply\r\n", "ply   \n", "\n\nply\n", "  ply\n"] {
        let source = format!("{opening}{}", A_PLY.trim_start_matches("ply\n"));
        assert_eq!(
            format::detect(source.as_bytes()),
            Format::Ply,
            "opening {opening:?}"
        );
    }
}
