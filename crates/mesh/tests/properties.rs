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
use renew_math::{Mat4, Quat, Vec3};
use renew_mesh::data_uri::{self, DataUriError};
use renew_mesh::{Mesh, MeshError, blob, place, stl};

// The encoder the seeds and the fuzz target use, shared rather than
// copied so the round trip below is the same round trip they make.
#[path = "shared/base64_encode.rs"]
mod base64_encode;

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

// ---------------------------------------------------------------------
// Placing geometry, and joining it.
//
// **Properties rather than a fuzz target, and the choice is the rule's
// rather than convenience.** The testing table sends math to
// property-based tests and sends parsers to fuzz targets. Placement
// parses nothing: it takes a mesh some reader already validated and a
// matrix the document layer will supply, and every loop bound comes from
// a vector's own length. There is no offset computed from an untrusted
// number, which is the class of fault a fuzzer finds and a property does
// not. What can go wrong here is a value being wrong, and a fuzzer has
// no oracle for a wrong normal.
//
// The path by which a hostile matrix arrives — sixteen numbers read out
// of a document — is a parser, and it is fuzzed where it lives.
// ---------------------------------------------------------------------

/// A coordinate a bounding box can hold, and small enough that a
/// transform of it stays finite.
fn placed_coordinate() -> impl Strategy<Value = f32> {
    -1e3f32..1e3f32
}

fn point() -> impl Strategy<Value = [f32; 3]> {
    (
        placed_coordinate(),
        placed_coordinate(),
        placed_coordinate(),
    )
        .prop_map(|(x, y, z)| [x, y, z])
}

/// A mesh of whole triangles, optionally carrying each of its optional
/// arrays.
fn placeable_mesh(with_normals: bool, with_texcoords: bool) -> impl Strategy<Value = Mesh> {
    proptest::collection::vec((point(), point(), point()), 1..6).prop_map(move |faces| {
        let mut out = Mesh::default();
        for (a, b, c) in faces {
            out.positions.extend_from_slice(&[a, b, c]);
            if with_normals {
                out.face_normals.push([0.0, 0.0, 1.0]);
                out.corner_normals.extend_from_slice(&[[0.0, 0.0, 1.0]; 3]);
            }
            if with_texcoords {
                out.corner_texcoords
                    .extend_from_slice(&[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
            }
        }
        out
    })
}

/// An invertible node transform: scale, then rotate, then translate.
fn transform() -> impl Strategy<Value = Mat4> {
    let axis = prop_oneof![-1e2f32..-1e-1f32, 1e-1f32..1e2f32];
    (
        point(),
        (
            placed_coordinate(),
            placed_coordinate(),
            placed_coordinate(),
        ),
        -3.0f32..3.0f32,
        (axis.clone(), axis.clone(), axis),
    )
        .prop_filter_map("needs an axis to turn about", |(t, a, angle, s)| {
            let axis = Vec3::new(a.0, a.1, a.2).try_normalize()?;
            Some(
                Mat4::from_translation(Vec3::new(t[0], t[1], t[2]))
                    * Mat4::from_quat(Quat::from_axis_angle(axis, angle))
                    * Mat4::from_scale(Vec3::new(s.0, s.1, s.2)),
            )
        })
}

proptest! {
    /// **Placement answers, and what it accepts is finite.**
    ///
    /// The claim a named case cannot make: over every mesh of this shape
    /// and every invertible transform, `place` either refuses or leaves
    /// a mesh whose every coordinate a bounding box can still hold.
    #[test]
    fn placing_geometry_leaves_it_finite_or_refuses(
        mut geometry in placeable_mesh(true, true),
        matrix in transform(),
    ) {
        let corners = geometry.positions.len();
        let faces = geometry.face_normals.len();
        // A refusal is an answer; what it accepts is the subject.
        if place::place(&mut geometry, matrix).is_ok() {
            {
                prop_assert_eq!(geometry.positions.len(), corners, "placement adds no geometry");
                prop_assert_eq!(geometry.face_normals.len(), faces);
                for value in geometry
                    .positions
                    .iter()
                    .chain(&geometry.face_normals)
                    .chain(&geometry.corner_normals)
                    .flatten()
                {
                    prop_assert!(value.is_finite(), "a placed coordinate nothing can bound");
                }
            }
        }
    }

    /// **Joining two pieces that carry the same arrays adds their
    /// lengths and nothing else.**
    #[test]
    fn joining_pieces_that_agree_adds_their_lengths(
        mut first in placeable_mesh(true, true),
        second in placeable_mesh(true, true),
    ) {
        let (corners, faces) = (first.positions.len(), first.face_normals.len());
        place::append(&mut first, &second).expect("both carry everything");
        prop_assert_eq!(first.positions.len(), corners + second.positions.len());
        prop_assert_eq!(first.face_normals.len(), faces + second.face_normals.len());
        prop_assert_eq!(first.positions.len() % 3, 0, "still whole triangles");
        prop_assert_eq!(first.corner_normals.len(), first.positions.len());
        prop_assert_eq!(first.corner_texcoords.len(), first.positions.len());
    }

    /// **Two pieces that disagree about an array are always refused**,
    /// whichever way round they are handed over.
    #[test]
    fn joining_pieces_that_disagree_always_refuses(
        furnished in placeable_mesh(true, true),
        mut plain in placeable_mesh(false, false),
    ) {
        let forwards = place::append(&mut furnished.clone(), &plain);
        let backwards = place::append(&mut plain, &furnished);
        prop_assert!(
            matches!(forwards, Err(MeshError::StreamLengthMismatch { .. })),
            "a piece with normals cannot take one without"
        );
        prop_assert!(
            matches!(backwards, Err(MeshError::StreamLengthMismatch { .. })),
            "and the disagreement is not about argument order"
        );
    }
}

// ---- `data:` URIs -------------------------------------------------

/// A URI carrying `bytes`, written the way an exporter would.
fn embedded(bytes: &[u8]) -> String {
    format!(
        "data:application/octet-stream;base64,{}",
        base64_encode::encode(bytes)
    )
}

proptest! {
    /// **Whatever an encoder writes, the reader gives back.**
    ///
    /// The named cases beside this cover the published vectors and the
    /// three padding states; this says it for every byte string, which is
    /// the half a named case cannot reach — and the payloads that matter
    /// most are the ones nobody would type, because `+` and `/` appear
    /// only when the bytes happen to land on them.
    #[test]
    fn what_an_encoder_wrote_the_reader_gives_back(
        bytes in proptest::collection::vec(any::<u8>(), 0..300),
    ) {
        let uri = embedded(&bytes);
        let read = data_uri::read(&uri).expect("what was encoded reads");
        prop_assert_eq!(read.bytes, bytes);
        prop_assert_eq!(read.media_type, "application/octet-stream");
        prop_assert_eq!(read.parameters, "");
    }

    /// **Text and bytes are one to one, in the direction that is easy to
    /// get wrong.**
    ///
    /// The property above says every encoding decodes. This says nothing
    /// else does: change any single character of a payload and either the
    /// reader refuses it, or it decodes to something *different*. A
    /// decoder that ignored the unused bits of a final group would fail
    /// this and pass everything else here, which is exactly how that bug
    /// survives in the wild.
    #[test]
    fn no_other_text_decodes_to_the_same_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 1..60),
        at in 0_usize..80,
        replacement in any::<u8>(),
    ) {
        let uri = embedded(&bytes);
        let payload_at = uri.find(',').expect("an encoded URI has a comma") + 1;
        let index = payload_at + at % (uri.len() - payload_at);
        let replacement = char::from(replacement);
        prop_assume!(replacement.is_ascii() && uri.as_bytes()[index] != replacement as u8);

        let mut altered = uri.clone();
        altered.replace_range(index..=index, &replacement.to_string());

        match data_uri::read(&altered) {
            Err(_) => {}
            Ok(other) => prop_assert_ne!(
                other.bytes,
                bytes,
                "{} is a second spelling of the same resource",
                altered
            ),
        }
    }

    /// **Every text gets an answer**, which is the claim the whole
    /// module rests on: a reader handed something no encoder wrote
    /// returns rather than looping, panicking, or deciding for itself.
    #[test]
    fn every_text_gets_an_answer(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let text = String::from_utf8_lossy(&bytes);
        match data_uri::read(&text) {
            Ok(read) => prop_assert_eq!(
                base64_encode::encode(&read.bytes),
                &text[text.find(',').expect("a URI that read has a comma") + 1..],
            ),
            Err(refusal) => {
                prop_assert!(!refusal.name().is_empty());
                prop_assert!(!refusal.to_string().is_empty());
            }
        }
    }

    /// **An offset a refusal reports is inside the text it is about.**
    ///
    /// A message pointing past the end of what the caller handed over is
    /// worse than no message: it sends whoever is debugging to a byte
    /// that is not there.
    #[test]
    fn a_reported_offset_is_inside_the_text(
        bytes in proptest::collection::vec(any::<u8>(), 0..400),
    ) {
        let text = String::from_utf8_lossy(&bytes);
        let Err(
            DataUriError::BadDigit { at, .. }
            | DataUriError::BadPadding { at }
            | DataUriError::NonCanonical { at, .. },
        ) = data_uri::read(&text)
        else {
            return Ok(());
        };
        prop_assert!(
            at < text.len(),
            "offset {} is past the end of a {}-byte text",
            at,
            text.len()
        );
    }
}
