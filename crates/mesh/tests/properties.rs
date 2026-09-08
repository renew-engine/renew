//! Properties of the STL reader and the blob codec, over inputs nobody
//! chose.
//!
//! The suite beside this one names cases. These five say something about
//! *every* input of a shape, which is the half a named case cannot
//! reach — and between them they are what stands behind the claim in the
//! crate's own documentation that a reader answers for every byte string
//! it can be handed.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` — which apply to `#[cfg(test)]` modules —
// do not reach it. A fixture this file built and then could not read
// back is a broken test rather than a condition to recover from.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use proptest::prelude::*;
use renew_mesh::{Mesh, blob, stl};

/// A binary STL over `triangles`, built the way an exporter would.
fn binary(triangles: &[([f32; 3], [[f32; 3]; 3])]) -> Vec<u8> {
    let mut out = vec![0u8; 80];
    out.extend_from_slice(&u32::try_from(triangles.len()).unwrap_or(0).to_le_bytes());
    for (normal, corners) in triangles {
        for value in normal.iter().chain(corners.iter().flatten()) {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}

/// A text STL over the same, written the way a text exporter would.
fn text(triangles: &[([f32; 3], [[f32; 3]; 3])]) -> String {
    use core::fmt::Write as _;
    let mut out = String::from("solid generated\n");
    for (normal, corners) in triangles {
        let _ = writeln!(
            out,
            "  facet normal {:e} {:e} {:e}\n    outer loop",
            normal[0], normal[1], normal[2]
        );
        for corner in corners {
            let _ = writeln!(
                out,
                "      vertex {:e} {:e} {:e}",
                corner[0], corner[1], corner[2]
            );
        }
        out.push_str("    endloop\n  endfacet\n");
    }
    out.push_str("endsolid generated\n");
    out
}

/// Coordinates a real model holds: finite, and across a wide range of
/// magnitudes rather than clustered around one.
fn coordinate() -> impl Strategy<Value = f32> {
    prop_oneof![
        (-1000.0f32..1000.0),
        (-1.0f32..1.0),
        (-1e12f32..1e12),
        (-1e-12f32..1e-12),
    ]
}

fn vector() -> impl Strategy<Value = [f32; 3]> {
    [coordinate(), coordinate(), coordinate()]
}

fn triangles() -> impl Strategy<Value = Vec<([f32; 3], [[f32; 3]; 3])>> {
    proptest::collection::vec((vector(), [vector(), vector(), vector()]), 1..12)
}

/// Coordinates for the blob, with both zeros deliberately in the mix.
///
/// **The ranges above will not produce a negative zero**, and negative
/// zero is the value that makes the difference between the two ways of
/// asking whether a round trip was lossless. `-0.0 == 0.0` is true, so a
/// codec that turned one into the other would satisfy an equality check
/// while having thrown information away. Generating it, and comparing
/// bits below, is what turns that from a claim into a test.
fn blob_coordinate() -> impl Strategy<Value = f32> {
    prop_oneof![
        4 => coordinate(),
        1 => Just(0.0f32),
        1 => Just(-0.0f32),
    ]
}

fn blob_vector() -> impl Strategy<Value = [f32; 3]> {
    [blob_coordinate(), blob_coordinate(), blob_coordinate()]
}

/// A mesh of the shape every reader in this crate produces: whole
/// triangles, at least one, and each optional array either empty or
/// exactly its full length. Those are `blob::write`'s stated contract,
/// so a generator that broke them would be testing an assertion rather
/// than the codec.
fn mesh() -> impl Strategy<Value = Mesh> {
    (1usize..8, any::<bool>(), any::<bool>(), any::<bool>()).prop_flat_map(
        |(faces, has_face, has_corner, has_uv)| {
            let corners = faces * 3;
            (
                proptest::collection::vec(blob_vector(), corners),
                proptest::collection::vec(blob_vector(), if has_face { faces } else { 0 }),
                proptest::collection::vec(blob_vector(), if has_corner { corners } else { 0 }),
                proptest::collection::vec(
                    [blob_coordinate(), blob_coordinate()],
                    if has_uv { corners } else { 0 },
                ),
            )
                .prop_map(
                    |(positions, face_normals, corner_normals, corner_texcoords)| Mesh {
                        positions,
                        face_normals,
                        corner_normals,
                        corner_texcoords,
                    },
                )
        },
    )
}

/// Every float of a mesh, in one order, as the bits it is stored as.
fn bits(mesh: &Mesh) -> Vec<u32> {
    mesh.positions
        .iter()
        .chain(&mesh.face_normals)
        .chain(&mesh.corner_normals)
        .flatten()
        .chain(mesh.corner_texcoords.iter().flatten())
        .map(|value| value.to_bits())
        .collect()
}

proptest! {
    /// **What a writer wrote, the reader gives back.**
    ///
    /// Bit for bit, because a binary STL stores `f32`s in native width
    /// and this reader does no arithmetic on them on the way through.
    /// Anything less than equality here would mean a coordinate had been
    /// rounded, reordered, or quietly normalised, and a mesh that came
    /// back *nearly* where it was written is the kind of defect that
    /// surfaces as a seam between two parts of a model.
    #[expect(
        clippy::float_cmp,
        reason = "the claim is that the bytes came back unchanged, so equality with what was written is exactly what must hold; a tolerance here would pass a reader that rounded"
    )]
    #[test]
    fn what_a_writer_wrote_the_binary_reader_gives_back(source in triangles()) {
        let mesh = stl::read(&binary(&source)).expect("a file this test built");
        prop_assert_eq!(mesh.triangles(), source.len());
        for (index, (normal, corners)) in source.iter().enumerate() {
            prop_assert_eq!(mesh.face_normals[index], *normal);
            for (offset, corner) in corners.iter().enumerate() {
                prop_assert_eq!(mesh.positions[index * 3 + offset], *corner);
            }
        }
    }

    /// The text dialect round-trips too, to the precision its own
    /// spelling of a float carries.
    ///
    /// **Not bit-exact, and the reason is the format rather than the
    /// reader**: a text STL stores a decimal rendering, so the round trip
    /// is only as exact as that rendering. `{:e}` in Rust prints enough
    /// digits to recover an `f32` exactly, so this asserts equality and
    /// would catch a writer that printed fewer — but the claim belongs to
    /// the formatting, not to the parse, and saying so is why this is a
    /// separate property from the one above.
    #[expect(
        clippy::float_cmp,
        reason = "the claim is that the bytes came back unchanged, so equality with what was written is exactly what must hold; a tolerance here would pass a reader that rounded"
    )]
    #[test]
    fn a_text_file_round_trips_through_its_own_spelling(source in triangles()) {
        let mesh = stl::read(text(&source).as_bytes()).expect("a file this test built");
        prop_assert_eq!(mesh.triangles(), source.len());
        for (index, (normal, corners)) in source.iter().enumerate() {
            prop_assert_eq!(mesh.face_normals[index], *normal);
            for (offset, corner) in corners.iter().enumerate() {
                prop_assert_eq!(mesh.positions[index * 3 + offset], *corner);
            }
        }
    }

    /// **Every byte string gets an answer.**
    ///
    /// Not a claim about which answer — a claim that there is one, with
    /// no panic and no read past the end, for input generated with no
    /// regard for the format at all. This is the property the fuzz target
    /// attacks with coverage guidance; here it runs on every merge.
    #[test]
    fn every_byte_string_gets_an_answer(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        // The invariants a returned mesh carries, so a wrong answer is
        // caught as well as a missing one.
        if let Ok(mesh) = stl::read(&bytes) {
            prop_assert_eq!(mesh.positions.len() % 3, 0);
            prop_assert!(!mesh.is_empty());
            prop_assert!(mesh.face_normals.is_empty() || mesh.face_normals.len() == mesh.triangles());
            for value in mesh.positions.iter().chain(&mesh.face_normals).flatten() {
                prop_assert!(value.is_finite());
            }
        }
    }

    /// **No proper prefix of a binary file reads at all.**
    ///
    /// A reader that answered `Ok` to half a file would be handing a
    /// caller a mesh with triangles missing and no way to know it — the
    /// worst of the three possible answers, because it is the one nothing
    /// downstream can detect. Truncation is the commonest corruption
    /// there is, so it gets a property rather than a case.
    ///
    /// **The assertion used to be that a prefix must not equal the whole,
    /// and that could not fail.** A prefix accepted with triangles
    /// missing has positions that differ from the whole file's, so it
    /// *passed* — for precisely the outcome the paragraph above calls the
    /// worst. Proved by returning the records that did arrive from a
    /// truncated read: the property stayed green.
    ///
    /// The true claim is stronger and simpler. A binary file's length is
    /// fixed by its own count, so no shorter prefix can satisfy it, and
    /// there is no honest reading of half an STL. It must refuse.
    #[test]
    fn no_proper_prefix_of_a_binary_file_reads(
        source in triangles(),
        cut in 0usize..1_000,
    ) {
        let whole = binary(&source);
        // A cut point as a thousandth of the file, in integers, so the
        // sweep is the same everywhere and nothing has to be rounded.
        let at = whole.len() * cut / 1_000;
        prop_assume!(at < whole.len());
        prop_assert!(
            stl::read(&whole[..at]).is_err(),
            "{at} of {} bytes was accepted as a mesh",
            whole.len()
        );
    }

    /// **What `write` wrote, `read` gives back — bit for bit.**
    ///
    /// Compared as bits rather than as floats, and the difference is not
    /// pedantry. `-0.0 == 0.0` is true, so an equality check passes a
    /// codec that normalised a negative zero away; `to_bits` does not.
    /// The generator above puts both zeros in deliberately so this
    /// distinction is exercised rather than merely available.
    ///
    /// The blob is the one format here this repository also writes, so
    /// it is the only one where a round trip is a claim about a pair of
    /// functions rather than about a file somebody else produced — and
    /// a lossy canonical form is worse than a lossy reader, because
    /// everything downstream trusts it to be canonical.
    #[test]
    fn a_mesh_written_as_a_blob_comes_back_bit_for_bit(source in mesh()) {
        let bytes = blob::write(&source);
        let read = blob::read(&bytes).expect("a blob this test wrote");

        prop_assert_eq!(read.positions.len(), source.positions.len());
        prop_assert_eq!(read.face_normals.len(), source.face_normals.len());
        prop_assert_eq!(read.corner_normals.len(), source.corner_normals.len());
        prop_assert_eq!(read.corner_texcoords.len(), source.corner_texcoords.len());
        prop_assert_eq!(bits(&read), bits(&source));

        // And the form is canonical: the same mesh written twice is the
        // same bytes, which is what every cache above a blob assumes.
        prop_assert_eq!(blob::write(&read), bytes);
    }
}
