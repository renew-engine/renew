//! The document's tables, and the refusals a table of indices needs.
//!
//! **Almost everything a document says is an index into an array**, so
//! almost everything that can be wrong with one is a number naming a row
//! that is not there. That refusal carries the table, the index and the
//! count, because a reader holding the file wants all three.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` do not reach it.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use renew_mesh::accessor::{Component, Shape};
use renew_mesh::gltf::{self, GltfError};
use renew_mesh::pbr::{Alpha, Material, TextureRef};

// The encoder the seeds and the URI suite share, included the way six
// other targets include it: a document that embeds its geometry has to
// spell it, and spelling it by hand in a fixture is how a fixture stops
// meaning what its name says.
#[path = "shared/base64_encode.rs"]
mod base64_encode;

/// Parse a document and hand back its root, panicking on text this test
/// wrote itself and cannot read back.
fn document(text: &str) -> renew_json::Json<'_> {
    gltf::parse(text.as_bytes()).expect("a fixture this file wrote")
}

/// One accessor, whole, for the cases that take a member away.
const WHOLE_ACCESSOR: &str =
    r#"{ "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 1, "type": "VEC3" }] }"#;

/// A document with one buffer view and one accessor over it.
const WHOLE: &str = r#"{
  "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }],
  "accessors": [{
    "bufferView": 0, "byteOffset": 0,
    "componentType": 5126, "count": 3, "type": "VEC3"
  }]
}"#;

/// The tables come back with the numbers the document spelled.
#[test]
fn the_tables_are_read_as_the_document_spells_them() {
    let json = document(WHOLE);
    let views = gltf::buffer_views(json.root()).expect("one view");
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].0, 0, "and which buffer it reads out of");
    assert_eq!(views[0].1.byte_offset, 0);
    assert_eq!(views[0].1.byte_length, 36);
    assert_eq!(views[0].1.byte_stride, None, "absent means tightly packed");

    let accessors = gltf::accessors(json.root()).expect("one accessor");
    assert_eq!(accessors.len(), 1);
    let (view, accessor) = accessors[0];
    assert_eq!(view, 0, "and which view it reads through");
    assert_eq!(accessor.component, Component::F32);
    assert_eq!(accessor.shape, Shape::Vec3);
    assert_eq!(accessor.count, 3);
    assert!(!accessor.normalized);
}

/// **The format's defaults are applied, and they are the format's.**
///
/// `byteOffset` and `normalized` are optional; a document that omits
/// them means zero and false, not "missing".
#[test]
fn absent_optional_members_take_the_formats_defaults() {
    let json = document(
        r#"{
          "buffers": [{ "byteLength": 12 }],
          "bufferViews": [{"buffer": 0, "byteLength": 12 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 1, "type": "VEC3" }]
        }"#,
    );
    let views = gltf::buffer_views(json.root()).expect("one view");
    assert_eq!(views[0].1.byte_offset, 0, "an absent offset is zero");
    // `buffer` is absent too, and defaults to the only buffer there is.
    let accessors = gltf::accessors(json.root()).expect("one accessor");
    assert_eq!(accessors[0].1.byte_offset, 0);
    assert!(!accessors[0].1.normalized);
}

/// A stride on the view is read, because that is where the format puts
/// it.
#[test]
fn a_stride_is_read_from_the_view() {
    let json = document(
        r#"{ "buffers": [{ "byteLength": 96 }],
          "bufferViews": [{"buffer": 0, "byteLength": 96, "byteStride": 32 }] }"#,
    );
    let views = gltf::buffer_views(json.root()).expect("one view");
    assert_eq!(views[0].1.byte_stride, Some(32));
}

/// A document with no tables has no tables, which is not a refusal.
#[test]
fn a_document_with_no_tables_reads_as_empty() {
    let json = document(r#"{ "asset": { "version": "2.0" } }"#);
    assert!(
        gltf::buffer_views(json.root())
            .expect("no views")
            .is_empty()
    );
    assert!(
        gltf::accessors(json.root())
            .expect("no accessors")
            .is_empty()
    );
}

/// **A required member that is not there is named.**
#[test]
fn a_missing_required_member_is_named() {
    let json = document(r#"{ "bufferViews": [{"buffer": 0, "byteOffset": 4 }] }"#);
    assert_eq!(
        gltf::buffer_views(json.root()).expect_err("a view must say how long it is"),
        GltfError::MissingField { path: "byteLength" }
    );

    let json =
        document(r#"{ "accessors": [{ "componentType": 5126, "count": 1, "type": "VEC3" }] }"#);
    assert_eq!(
        gltf::accessors(json.root()).expect_err("an accessor must say what it reads"),
        GltfError::MissingField { path: "bufferView" }
    );

    for missing in ["componentType", "count", "type"] {
        let text = WHOLE_ACCESSOR.replace(&format!(r#""{missing}""#), r#""ignored""#);
        let json = document(&text);
        assert_eq!(
            gltf::accessors(json.root()).expect_err("a required member was renamed away"),
            GltfError::MissingField { path: missing }
        );
    }
}

/// **A buffer naming a second file is refused rather than fetched.**
///
/// This crate never opens anything. A relative path is what a document
/// beside its `.bin` looks like, and the honest answer is to say so.
#[test]
fn a_buffer_naming_a_second_file_is_refused_rather_than_fetched() {
    let json = document(r#"{ "buffers": [{ "byteLength": 12, "uri": "geometry.bin" }] }"#);
    assert_eq!(
        gltf::buffers(json.root(), None).expect_err("a second file is somewhere else"),
        GltfError::ExternalResource
    );
}

/// **Only the first buffer may be the container's own chunk.**
///
/// The specification leaves any other sourceless buffer undefined, and
/// undefined is refused here rather than guessed at -- the alternative
/// is handing a view the first buffer's bytes and calling the result
/// geometry.
#[test]
fn a_later_buffer_with_no_source_is_refused_by_name() {
    let json = document(r#"{ "buffers": [{ "byteLength": 4 }, { "byteLength": 4 }] }"#);
    assert_eq!(
        gltf::buffers(json.root(), Some(&[0, 1, 2, 3])).expect_err("buffer 1 has no source"),
        GltfError::BufferWithoutSource { buffer: 1 }
    );
}

/// **A document wanting the container's chunk when there is none.**
///
/// The ordinary way to meet this is a document read on its own: it has
/// no container, so there is no chunk for its first buffer to be.
#[test]
fn a_buffer_wanting_a_chunk_that_is_not_there_is_refused() {
    let json = document(r#"{ "buffers": [{ "byteLength": 4 }] }"#);
    assert_eq!(
        gltf::buffers(json.root(), None).expect_err("there is no chunk"),
        GltfError::NoBinaryChunk
    );
}

/// **A buffer is its resource cut to the length it declares.**
///
/// The specification allows the resource to be longer and says only the
/// first `byteLength` bytes belong to the buffer. The container's chunk
/// routinely *is* longer, because it is padded to a four-byte boundary,
/// so this is the rule that stops a view reaching into that padding.
#[test]
fn a_resource_longer_than_its_buffer_is_cut_to_the_buffer() {
    let json = document(r#"{ "buffers": [{ "byteLength": 4 }] }"#);
    let read = gltf::buffers(json.root(), Some(&[1, 2, 3, 4, 0, 0, 0, 0])).expect("a buffer");
    assert_eq!(read.len(), 1);
    assert_eq!(
        &*read[0],
        &[1, 2, 3, 4],
        "the padding is not part of the buffer"
    );
}

/// And a resource shorter than its buffer is a disagreement, not a cut.
#[test]
fn a_resource_shorter_than_its_buffer_is_refused() {
    let json = document(r#"{ "buffers": [{ "byteLength": 16 }] }"#);
    assert_eq!(
        gltf::buffers(json.root(), Some(&[1, 2, 3, 4])).expect_err("four bytes are not sixteen"),
        GltfError::BufferTooShort {
            buffer: 0,
            declared: 16,
            available: 4,
        }
    );
}

/// **A payload embedded in the document is decoded and cut.**
#[test]
fn an_embedded_payload_becomes_the_buffers_bytes() {
    // `AQIDBA==` is 0x01 0x02 0x03 0x04.
    let json = document(
        r#"{ "buffers": [{
          "byteLength": 4,
          "uri": "data:application/octet-stream;base64,AQIDBA=="
        }] }"#,
    );
    let read = gltf::buffers(json.root(), None).expect("an embedded buffer");
    assert_eq!(&*read[0], &[1, 2, 3, 4]);

    // The other media type a buffer may declare reads the same way.
    let other = document(
        r#"{ "buffers": [{
          "byteLength": 4,
          "uri": "data:application/gltf-buffer;base64,AQIDBA=="
        }] }"#,
    );
    assert_eq!(
        &*gltf::buffers(other.root(), None).expect("a buffer")[0],
        &[1, 2, 3, 4]
    );
}

/// **A payload whose media type is not one a buffer may declare.**
#[test]
fn a_payload_of_the_wrong_media_type_is_refused_naming_it() {
    let json = document(
        r#"{ "buffers": [{
          "byteLength": 4,
          "uri": "data:image/png;base64,AQIDBA=="
        }] }"#,
    );
    let refused = gltf::buffers(json.root(), None).expect_err("a buffer is not an image");
    assert_eq!(
        refused,
        GltfError::WrongMediaType {
            found: "image/png".into()
        }
    );
    assert!(
        refused.to_string().contains("image/png"),
        "the message shows what it found: {refused}"
    );
}

/// **A payload that will not decode says which rule it broke.**
#[test]
fn a_payload_that_will_not_decode_carries_the_decoders_refusal() {
    let json = document(
        r#"{ "buffers": [{
          "byteLength": 4,
          "uri": "data:application/octet-stream;base64,AQID!A=="
        }] }"#,
    );
    let refused = gltf::buffers(json.root(), None).expect_err("`!` is not base64");
    assert_eq!(refused.name(), "Payload");
    assert!(
        refused.to_string().contains("base64 character"),
        "the layer below names the rule: {refused}"
    );
}

/// **A URI spelled with an escape is still a URI.**
///
/// A base64 payload contains `/`, and a document may spell that `\/`.
/// A reader taking the borrowed fast path sees no plain string for
/// exactly the URIs it needs to read.
#[test]
fn a_payload_whose_uri_carries_an_escape_still_decodes() {
    // `Lw==` is a single `/`, and the URI spells its own slash escaped.
    let json = document(
        r#"{ "buffers": [{
          "byteLength": 1,
          "uri": "data:application\/octet-stream;base64,Lw=="
        }] }"#,
    );
    let read = gltf::buffers(json.root(), None).expect("an escaped URI is a URI");
    assert_eq!(&*read[0], b"/");
}

/// A sparse accessor is a different reader, and says so.
#[test]
fn a_sparse_accessor_is_refused_by_name() {
    let json = document(
        r#"{ "accessors": [{
            "bufferView": 0, "componentType": 5126, "count": 1, "type": "VEC3",
            "sparse": { "count": 1 }
        }] }"#,
    );
    let refused = gltf::accessors(json.root()).expect_err("sparse is not read");
    assert_eq!(refused, GltfError::Unsupported { found: "sparse" });
    assert!(refused.to_string().contains("not in this reader"));
}

/// **A component type or shape outside the table is refused by the layer
/// that owns the table**, and arrives here wrapped.
#[test]
fn an_unknown_type_is_refused_by_the_layer_that_owns_the_table() {
    let json = document(
        r#"{ "accessors": [{ "bufferView": 0, "componentType": 5124, "count": 1, "type": "VEC3" }] }"#,
    );
    let refused = gltf::accessors(json.root()).expect_err("5124 is not in the table");
    assert_eq!(
        refused.name(),
        "Accessor",
        "wrapped, and named for its layer"
    );
    assert!(
        refused.to_string().contains("5124"),
        "and the inner refusal's numbers survive: {refused}"
    );

    // A matrix shape is in the format and not in this reader, and it
    // reaches the shape table rather than a missing case.
    let json = document(
        r#"{ "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 1, "type": "MAT4" }] }"#,
    );
    let refused = gltf::accessors(json.root()).expect_err("matrices are not read");
    assert_eq!(refused.name(), "Accessor");
    assert!(refused.to_string().contains("SCALAR"), "{refused}");
}

/// **An escaped shape name is the shape it spells.**
///
/// `"\u0056EC3"` is `VEC3`, however strange it looks, and refusing it
/// for its spelling would be this reader inventing a rule the format
/// does not have.
///
/// **This test contained no escape when it was first written** — a
/// plain `VEC3` under a name that claimed otherwise, which is a test
/// that passes by duplicating one beside it.
#[test]
fn an_escaped_shape_name_is_the_shape_it_spells() {
    let json = document(
        r#"{ "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 1, "type": "\u0056EC3" }] }"#,
    );
    let accessors = gltf::accessors(json.root()).expect("an escaped VEC3 is a VEC3");
    assert_eq!(accessors[0].1.shape, Shape::Vec3);
}

/// A member of the wrong JSON type is the document reader's refusal, and
/// arrives wrapped.
#[test]
fn a_member_of_the_wrong_type_is_the_documents_refusal() {
    let json = document(r#"{ "bufferViews": [{"buffer": 0, "byteLength": "long" }] }"#);
    let refused = gltf::buffer_views(json.root()).expect_err("a length is a number");
    assert_eq!(
        refused.name(),
        "Document",
        "wrapped, and named for its layer"
    );
}

/// **Members of the wrong type, in the four places the reader reads
/// one.**
///
/// Each is a `Document` refusal, and each was an untravelled path until
/// the coverage gate named it: a normalisation flag that is not a
/// boolean, an attribute index that is not a number, a scene selector
/// that is not one, and a root node that is not one. **A reader that
/// took any of them on trust would be indexing a table with something
/// that was never an index.**
#[test]
fn a_member_of_the_wrong_type_is_refused_wherever_it_is_read() {
    let normalised = document(
        r#"{ "accessors": [{
            "bufferView": 0, "componentType": 5121, "count": 1, "type": "VEC3",
            "normalized": "yes"
        }] }"#,
    );
    assert_eq!(
        gltf::accessors(normalised.root())
            .expect_err("a flag is a boolean")
            .name(),
        "Document"
    );

    let attribute = document(
        r#"{
          "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{
            "attributes": { "POSITION": 0, "NORMAL": "one" }
          }] }]
        }"#,
    );
    let bytes = three_positions();
    let source = gltf::Source::of(attribute.root(), Some(&bytes)).expect("the tables read");
    assert_eq!(
        gltf::primitive(attribute.root(), &source, 0, 0)
            .expect_err("an attribute names an accessor by number")
            .name(),
        "Document"
    );

    let selector = container(
        &ONE_NODE.replace(r#""scenes""#, r#""scene": "first", "scenes""#),
        &three_positions(),
    );
    assert_eq!(
        gltf::read(&selector)
            .expect_err("a scene is chosen by number")
            .name(),
        "Document"
    );

    let root_node = container(
        &ONE_NODE.replace(r#""nodes": [0]"#, r#""nodes": ["zero"]"#),
        &three_positions(),
    );
    assert_eq!(
        gltf::read(&root_node)
            .expect_err("a scene names its roots by number")
            .name(),
        "Document"
    );
}

/// **An optional attribute naming a row that is not there is refused,
/// and so is a scene selector naming one.**
///
/// Both are the same fault one level apart, and both were paths nothing
/// travelled: a document may name any accessor for `NORMAL` and any
/// scene for `scene`, and neither number is checked by anything until it
/// is used.
#[test]
fn an_optional_attribute_or_a_scene_past_its_table_is_refused() {
    let json = document(
        r#"{
          "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{
            "attributes": { "POSITION": 0, "NORMAL": 9 }
          }] }]
        }"#,
    );
    let bytes = three_positions();
    let source = gltf::Source::of(json.root(), Some(&bytes)).expect("the tables read");
    assert_eq!(
        gltf::primitive(json.root(), &source, 0, 0).expect_err("accessor 9 of one"),
        GltfError::NoSuchEntry {
            table: "accessors",
            index: 9,
            count: 1,
        }
    );

    let selector = container(
        &ONE_NODE.replace(r#""scenes""#, r#""scene": 4, "scenes""#),
        &three_positions(),
    );
    assert_eq!(
        gltf::read(&selector).expect_err("scene 4 of one"),
        GltfError::NoSuchEntry {
            table: "scenes",
            index: 4,
            count: 1,
        }
    );
}

/// **A normalised attribute travels the whole way**, from the flag in
/// the document to the fraction the accessor layer produces.
#[test]
fn a_normalised_attribute_is_read_as_a_fraction() {
    // Three unsigned bytes per position, at their ceiling: normalised,
    // that is 1.0 in each axis.
    let binary = vec![255u8; 9];
    let json = ONE_NODE
        .replace(
            r#""componentType": 5126, "count": 3, "type": "VEC3""#,
            r#""componentType": 5121, "count": 3, "type": "VEC3", "normalized": true"#,
        )
        .replace(r#""byteLength": 36"#, r#""byteLength": 9"#);
    let mesh = gltf::read(&container(&json, &binary)).expect("normalised bytes");
    same(
        &mesh.positions[0],
        &[1.0, 1.0, 1.0],
        "255 of 255 is one, and the flag is what says so",
    );
}

/// **Every refusal this layer can make is reachable, and it took the
/// whole reader to make that true.**
///
/// This census was written when only the tables existed, and four of its
/// arms said why a refusal could not be reached from them — the
/// container's, the geometry's, the cycle's, and an index past a table.
/// **Every one of those reasons went stale the moment `read` existed**,
/// and the wildcard-free match is what said so: adding `NodeCycle`
/// stopped this file compiling until the list was looked at again.
///
/// No wildcard arm, so the next refusal added does the same.
fn gltf_cannot_reach(refusal: &GltfError) -> Option<&'static str> {
    match refusal {
        GltfError::Container(_)
        | GltfError::Document(_)
        | GltfError::Accessor(_)
        | GltfError::Geometry(_)
        | GltfError::MissingField { .. }
        | GltfError::NoSuchEntry { .. }
        | GltfError::ExternalResource
        | GltfError::NodeCycle { .. }
        | GltfError::Unsupported { .. }
        | GltfError::Payload(_)
        | GltfError::BufferWithoutSource { .. }
        | GltfError::NoBinaryChunk
        | GltfError::WrongMediaType { .. }
        | GltfError::BufferTooShort { .. }
        | GltfError::FactorOutOfRange { .. } => None,
    }
}

/// The buffer layer's refusals, each provoked by a document.
///
/// Split out of the census below rather than listed inside it: five
/// refusals arrived at once when a document learned to read more than
/// the container's own chunk, and one function naming every refusal in
/// the reader had grown past what anybody reads in one go.
fn buffer_provocations() -> Vec<(&'static str, GltfError)> {
    vec![
        (
            "BufferWithoutSource",
            gltf::buffers(
                document(r#"{ "buffers": [{ "byteLength": 4 }, { "byteLength": 4 }] }"#).root(),
                Some(&[0, 1, 2, 3]),
            )
            .expect_err("only the first may be the chunk"),
        ),
        (
            "NoBinaryChunk",
            gltf::buffers(
                document(r#"{ "buffers": [{ "byteLength": 4 }] }"#).root(),
                None,
            )
            .expect_err("there is no chunk"),
        ),
        (
            "WrongMediaType",
            gltf::buffers(
                document(
                    r#"{ "buffers": [{ "byteLength": 4,
                       "uri": "data:image/png;base64,AQIDBA==" }] }"#,
                )
                .root(),
                None,
            )
            .expect_err("a buffer is not an image"),
        ),
        (
            "BufferTooShort",
            gltf::buffers(
                document(r#"{ "buffers": [{ "byteLength": 16 }] }"#).root(),
                Some(&[1, 2, 3, 4]),
            )
            .expect_err("four bytes are not sixteen"),
        ),
        (
            "FactorOutOfRange",
            gltf::materials(
                document(r#"{ "materials": [{ "emissiveFactor": [0.0, 0.0, 4.0] }] }"#).root(),
            )
            .expect_err("four is outside zero to one"),
        ),
        (
            "Payload",
            gltf::buffers(
                document(
                    r#"{ "buffers": [{ "byteLength": 4,
                       "uri": "data:application/octet-stream;base64,AQID!A==" }] }"#,
                )
                .root(),
                None,
            )
            .expect_err("`!` is not base64"),
        ),
    ]
}

/// **A document read on its own, with its geometry embedded.**
///
/// The self-contained form of the same asset: no container, no chunk,
/// and the buffer carrying its bytes as a payload the document spells
/// out. Everything below the buffers table reads it identically.
#[test]
fn a_document_on_its_own_reads_its_embedded_geometry() {
    // The same triangle the container fixtures use, as base64.
    let payload = base64_encode::encode(&three_positions());
    let text = format!(
        r#"{{"asset":{{"version":"2.0"}},"scenes":[{{"nodes":[0]}}],
          "nodes":[{{"mesh":0}}],
          "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}}}}]}}],
          "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}}],
          "buffers":[{{"byteLength":36,
            "uri":"data:application/octet-stream;base64,{payload}"}}],
          "bufferViews":[{{"buffer":0,"byteLength":36}}]}}"#
    );

    let mesh = gltf::read(text.as_bytes()).expect("a document that carries its own geometry");
    assert_eq!(mesh.triangles(), 1);
    assert_eq!(mesh.positions.len(), 3);

    // **The same document in a container reads to the same geometry.**
    // Its buffer carries its own payload, so the chunk beside it is
    // beside the point -- which is the claim worth pinning: the shape
    // the asset arrived in does not change what it means.
    let packed = container(&text, &[]);
    let wrapped = gltf::read(&packed).expect("the same document, wrapped");
    assert_eq!(wrapped.positions, mesh.positions);
    assert_eq!(wrapped.triangles(), mesh.triangles());
}

/// **A view reads the buffer it names, and not the first one.**
///
/// This is what the whole table is for, and until this test nothing
/// asserted it: every fixture and every seed pointed its views at buffer
/// zero, so a reader that dropped the index and always used the first
/// buffer would have passed the entire suite. It nearly was that reader
/// -- `buffer` was being defaulted to zero, which is a member the format
/// requires and gives no default.
#[test]
fn a_view_reads_the_buffer_it_names() {
    // Two buffers whose contents cannot be confused: one triangle at the
    // origin, one shifted a long way along x.
    let near = base64_encode::encode(&three_positions());
    let far = base64_encode::encode(&{
        let mut bytes = Vec::new();
        for corner in [100.0_f32, 101.0, 102.0] {
            for value in [corner, 0.0, 0.0] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes
    });

    let text = format!(
        r#"{{"asset":{{"version":"2.0"}},"scenes":[{{"nodes":[0]}}],
          "nodes":[{{"mesh":0}}],
          "meshes":[{{"primitives":[
            {{"attributes":{{"POSITION":0}}}},
            {{"attributes":{{"POSITION":1}}}}]}}],
          "accessors":[
            {{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},
            {{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}}],
          "buffers":[
            {{"byteLength":36,"uri":"data:application/octet-stream;base64,{near}"}},
            {{"byteLength":36,"uri":"data:application/octet-stream;base64,{far}"}}],
          "bufferViews":[
            {{"buffer":0,"byteLength":36}},
            {{"buffer":1,"byteLength":36}}]}}"#
    );

    let mesh = gltf::read(text.as_bytes()).expect("two buffers, two primitives");
    assert_eq!(mesh.triangles(), 2);

    // The second primitive's corners came out of the second buffer, so
    // they are the far ones. A reader ignoring the index would give six
    // corners at the origin.
    let far_corners = mesh.positions.iter().filter(|p| p[0] >= 100.0).count();
    assert_eq!(
        far_corners, 3,
        "three corners must come from buffer 1: {:?}",
        mesh.positions
    );
}

/// **A view naming a buffer the document does not have.**
#[test]
fn a_view_naming_a_buffer_that_is_not_there_is_refused() {
    let text = r#"{"asset":{"version":"2.0"},"scenes":[{"nodes":[0]}],
      "nodes":[{"mesh":0}],
      "meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],
      "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}],
      "buffers":[{"byteLength":36,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}],
      "bufferViews":[{"buffer":7,"byteLength":36}]}"#;
    let refused = gltf::read(text.as_bytes()).expect_err("there is no buffer 7");
    assert_eq!(
        refused,
        GltfError::NoSuchEntry {
            table: "buffers",
            index: 7,
            count: 1,
        }
    );
}

/// **A view with no `buffer` at all is refused rather than defaulted.**
///
/// The format requires the member and gives it no default, unlike
/// `byteOffset` beside it. Inventing one would hand a view the first
/// buffer's bytes whenever a document forgot to say.
#[test]
fn a_view_that_names_no_buffer_is_refused() {
    let json = document(r#"{ "bufferViews": [{ "byteLength": 36 }] }"#);
    assert_eq!(
        gltf::buffer_views(json.root()).expect_err("`buffer` is required"),
        GltfError::MissingField { path: "buffer" }
    );
}

/// **An embedded payload longer than its buffer is cut too.**
///
/// The chunk path had a test for this and the payload path did not: every
/// data-URI fixture happened to decode to exactly `byteLength`, so the
/// owned half of the cut was never taken with anything to remove.
#[test]
fn an_embedded_payload_longer_than_its_buffer_is_cut() {
    // `AQIDBAUGBwg=` is eight bytes; the buffer declares four.
    let json = document(
        r#"{ "buffers": [{
          "byteLength": 4,
          "uri": "data:application/octet-stream;base64,AQIDBAUGBwg="
        }] }"#,
    );
    let read = gltf::buffers(json.root(), None).expect("an embedded buffer");
    assert_eq!(
        &*read[0],
        &[1, 2, 3, 4],
        "the tail is not part of the buffer"
    );
}

/// **A media type is compared without case, as the format's own URIs
/// permit.**
#[test]
fn a_media_type_is_read_without_regard_to_case() {
    for spelling in [
        "application/octet-stream",
        "Application/Octet-Stream",
        "APPLICATION/OCTET-STREAM",
        "application/gltf-buffer",
        "application/GLTF-Buffer",
    ] {
        let text = format!(
            r#"{{ "buffers": [{{ "byteLength": 4,
               "uri": "data:{spelling};base64,AQIDBA==" }}] }}"#
        );
        let json = document(&text);
        assert!(
            gltf::buffers(json.root(), None).is_ok(),
            "`{spelling}` is one of the two types a buffer may declare"
        );
    }
}

/// **A document with views and no buffers table is refused.**
///
/// It used to read: every view was assumed to point into the container's
/// chunk, so the table could be absent and nothing noticed. A view's
/// `buffer` indexes that table, so a document without one is not a
/// document, and this pins the answer rather than leaving the change
/// visible only as a fixture edit.
#[test]
fn a_document_with_views_and_no_buffers_is_refused() {
    let packed = container(
        r#"{"asset":{"version":"2.0"},"scenes":[{"nodes":[0]}],
          "nodes":[{"mesh":0}],
          "meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],
          "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}],
          "bufferViews":[{"buffer":0,"byteLength":36}]}"#,
        &three_positions(),
    );
    assert_eq!(
        gltf::read(&packed).expect_err("no buffers table"),
        GltfError::NoSuchEntry {
            table: "buffers",
            index: 0,
            count: 0,
        }
    );
}

/// **The document detector, asked directly.**
///
/// Every other detector in this crate has a test of its own; this one was
/// reachable only through `detect`, which answers on the container magic
/// first -- so its answer for anything opening with `glTF` was observed
/// by nothing.
#[test]
fn the_document_detector_answers_for_itself() {
    assert!(gltf::looks_like(br#"{"asset":{"version":"2.0"}}"#));
    assert!(
        gltf::looks_like(b"  \n\t {\"asset\":{\"version\":\"2.0\"}}"),
        "leading whitespace is not a reason to decline"
    );
    assert!(!gltf::looks_like(b""));
    assert!(!gltf::looks_like(b"   "));
    assert!(
        !gltf::looks_like(br#"glTF {"asset":{"version":"2.0"}}"#),
        "a container is not a document, whatever follows its magic"
    );
    assert!(
        !gltf::looks_like(br#"[{"asset":{"version":"2.0"}}]"#),
        "a document's root is an object"
    );
    assert!(!gltf::looks_like(
        b"# a comment
v 0 0 0
"
    ));
}

/// **A document with no container and a buffer wanting one is refused.**
///
/// This is the ordinary way to meet `NoBinaryChunk`: a `.gltf` saved
/// beside a `.bin` that the exporter forgot to write into the buffer.
#[test]
fn a_document_on_its_own_whose_buffer_wants_a_chunk_is_refused() {
    let text = r#"{"asset":{"version":"2.0"},"scenes":[{"nodes":[0]}],
      "nodes":[{"mesh":0}],
      "meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],
      "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}],
      "buffers":[{"byteLength":36}],
      "bufferViews":[{"buffer":0,"byteLength":36}]}"#;
    assert_eq!(
        gltf::read(text.as_bytes()).expect_err("no container, no chunk"),
        GltfError::NoBinaryChunk
    );
}

/// **Bytes that are neither a container nor a document are refused as a
/// document**, because that is what they most nearly are.
#[test]
fn bytes_that_are_neither_shape_are_refused_by_the_document_layer() {
    let refused = gltf::read(b"not a container and not a document").expect_err("neither shape");
    assert_eq!(refused.name(), "Document");
}

// ---------------------------------------------------------------------
// Materials.
//
// The vocabulary is glTF's own, and every member of it has a default, so
// most of what follows is about what a document that says nothing means.

/// **An empty material is every default, because the format says so.**
///
/// A material object has no required members at all. A reader that
/// refused one would be refusing a conformant document, and a reader that
/// invented values would be reporting a material nobody wrote -- the
/// defaults are the format's, which is what makes returning them honest.
#[test]
fn a_material_that_says_nothing_is_every_default() {
    let json = document(r#"{ "materials": [{}] }"#);
    let read = gltf::materials(json.root()).expect("an empty material is a material");
    assert_eq!(read.len(), 1);
    assert_eq!(read[0], Material::default());
}

/// A document with no material table has no materials, which is not a
/// refusal.
#[test]
fn a_document_with_no_materials_has_none() {
    let json = document(r#"{ "asset": { "version": "2.0" } }"#);
    assert!(
        gltf::materials(json.root())
            .expect("no materials")
            .is_empty()
    );
}

/// **Every member the format states, read into the vocabulary it states
/// them in.**
#[expect(
    clippy::float_cmp,
    reason = "the claim is that the document's own numbers came back unchanged, and no arithmetic happens between the file and the assertion, so equality with what the file says is exactly what must hold; a tolerance would pass a reader that rounded a factor or read the wrong member"
)]
#[test]
fn a_material_reads_every_member_the_format_states() {
    let json = document(
        r#"{ "materials": [{
          "name": "brushed",
          "pbrMetallicRoughness": {
            "baseColorFactor": [0.5, 0.25, 0.125, 1.0],
            "metallicFactor": 0.75,
            "roughnessFactor": 0.25,
            "baseColorTexture": { "index": 3, "texCoord": 1 },
            "metallicRoughnessTexture": { "index": 4 }
          },
          "normalTexture": { "index": 5, "scale": 2.5 },
          "occlusionTexture": { "index": 6, "strength": 0.5 },
          "emissiveTexture": { "index": 7 },
          "emissiveFactor": [0.1, 0.2, 0.3],
          "alphaMode": "MASK",
          "alphaCutoff": 0.25,
          "doubleSided": true
        }] }"#,
    );
    let read = gltf::materials(json.root()).expect("a full material");
    let material = &read[0];

    assert_eq!(material.name.as_deref(), Some("brushed"));
    assert_eq!(material.base_color, [0.5, 0.25, 0.125, 1.0]);
    assert_eq!(material.metallic, 0.75);
    assert_eq!(material.roughness, 0.25);
    assert_eq!(material.emissive, [0.1, 0.2, 0.3]);
    assert_eq!(material.alpha, Alpha::Mask { cutoff: 0.25 });
    assert!(material.double_sided);

    assert_eq!(
        material.base_color_map,
        Some(TextureRef {
            texture: 3,
            uv_set: 1
        })
    );
    assert_eq!(
        material.metallic_roughness_map,
        Some(TextureRef {
            texture: 4,
            uv_set: 0
        }),
        "an absent texCoord is the first set, which is the format's default"
    );
    let normal = material.normal_map.expect("a normal map");
    assert_eq!(normal.map.texture, 5);
    assert_eq!(normal.scale, 2.5, "and its scale is unbounded");
    let occlusion = material.occlusion_map.expect("an occlusion map");
    assert_eq!(occlusion.map.texture, 6);
    assert_eq!(occlusion.strength, 0.5);
    assert_eq!(
        material.emissive_map,
        Some(TextureRef {
            texture: 7,
            uv_set: 0
        })
    );
}

/// **A cutoff reaches the caller only on the mode that uses one.**
#[test]
fn only_a_masked_material_carries_its_cutoff() {
    for (mode, expected) in [
        (r#""alphaMode": "OPAQUE","#, Alpha::Opaque),
        (r#""alphaMode": "BLEND","#, Alpha::Blend),
        ("", Alpha::Opaque),
    ] {
        // The cutoff is written in every case; only one mode reports it.
        let text = format!(r#"{{ "materials": [{{ {mode} "alphaCutoff": 0.75 }}] }}"#);
        let json = document(&text);
        let read = gltf::materials(json.root()).expect("a material");
        assert_eq!(read[0].alpha, expected, "{mode}");
    }

    let json = document(r#"{ "materials": [{ "alphaMode": "MASK" }] }"#);
    let read = gltf::materials(json.root()).expect("a masked material");
    assert_eq!(
        read[0].alpha,
        Alpha::Mask { cutoff: 0.5 },
        "an absent cutoff is a half, which is the format's default"
    );
}

/// An alpha mode this format does not have is refused by name.
#[test]
fn an_alpha_mode_the_format_does_not_have_is_refused() {
    let json = document(r#"{ "materials": [{ "alphaMode": "DITHER" }] }"#);
    let refused = gltf::materials(json.root()).expect_err("there are three modes");
    assert_eq!(refused.name(), "Unsupported");
}

/// **A factor outside the range the schema states is refused, and the
/// refusal names the member.**
///
/// Refused rather than clamped: clamping would report a material the
/// document does not contain. The material library's specular exponent
/// *is* left unclamped, and the two differ because that range is a
/// convention files exceed while this one is stated by the schema.
#[test]
fn a_factor_outside_its_stated_range_is_refused_naming_it() {
    for (member, text) in [
        (
            "baseColorFactor",
            r#"{ "materials": [{ "pbrMetallicRoughness": {
              "baseColorFactor": [1.5, 0.0, 0.0, 1.0] } }] }"#,
        ),
        (
            "metallicFactor",
            r#"{ "materials": [{ "pbrMetallicRoughness": {
              "metallicFactor": -0.5 } }] }"#,
        ),
        (
            "roughnessFactor",
            r#"{ "materials": [{ "pbrMetallicRoughness": {
              "roughnessFactor": 2.0 } }] }"#,
        ),
        (
            "emissiveFactor",
            r#"{ "materials": [{ "emissiveFactor": [0.0, 0.0, 4.0] }] }"#,
        ),
        (
            "strength",
            r#"{ "materials": [{ "occlusionTexture": {
              "index": 0, "strength": 3.0 } }] }"#,
        ),
    ] {
        let json = document(text);
        assert_eq!(
            gltf::materials(json.root()).expect_err("outside the stated range"),
            GltfError::FactorOutOfRange { field: member },
            "{member}"
        );
    }
}

/// **A cutoff is bounded below and not above**, which is what the schema
/// states and not a guess about what a renderer wants.
#[test]
fn a_cutoff_is_bounded_below_and_not_above() {
    let json = document(r#"{ "materials": [{ "alphaMode": "MASK", "alphaCutoff": 4.0 }] }"#);
    let read = gltf::materials(json.root()).expect("no upper bound is stated");
    assert_eq!(read[0].alpha, Alpha::Mask { cutoff: 4.0 });

    let json = document(r#"{ "materials": [{ "alphaMode": "MASK", "alphaCutoff": -1.0 }] }"#);
    assert_eq!(
        gltf::materials(json.root()).expect_err("a minimum is stated"),
        GltfError::FactorOutOfRange {
            field: "alphaCutoff"
        }
    );
}

/// **A map that names no texture is not a map.**
///
/// `index` is the one required member of a texture reference, and the
/// normal and occlusion kinds inherit the requirement rather than
/// restating it -- which is a schema arrangement, not a licence to leave
/// it out.
#[test]
fn a_map_that_names_no_texture_is_refused() {
    for text in [
        r#"{ "materials": [{ "pbrMetallicRoughness": {
          "baseColorTexture": { "texCoord": 1 } } }] }"#,
        r#"{ "materials": [{ "normalTexture": { "scale": 1.0 } }] }"#,
        r#"{ "materials": [{ "occlusionTexture": { "strength": 1.0 } }] }"#,
        r#"{ "materials": [{ "emissiveTexture": {} }] }"#,
    ] {
        let json = document(text);
        assert_eq!(
            gltf::materials(json.root()).expect_err("a map names a texture"),
            GltfError::MissingField { path: "index" },
            "{text}"
        );
    }
}

/// **A primitive's material is reported, not stored.**
///
/// Geometry and the surface it wears are two facts, and the canonical
/// form carries one of them. A caller that wants the pairing asks for it;
/// putting the index into the geometry would change the stored form for a
/// value nothing here can yet use.
#[test]
fn a_primitives_material_is_reported_by_index() {
    let json = document(
        r#"{ "meshes": [{ "primitives": [
          { "attributes": { "POSITION": 0 }, "material": 2 },
          { "attributes": { "POSITION": 0 } }
        ] }] }"#,
    );
    assert_eq!(
        gltf::primitive_material(json.root(), 0, 0).expect("a primitive"),
        Some(2)
    );
    assert_eq!(
        gltf::primitive_material(json.root(), 0, 1).expect("a primitive"),
        None,
        "a primitive naming no material has none, which is the default material"
    );
    assert_eq!(
        gltf::primitive_material(json.root(), 0, 7).expect_err("there are two"),
        GltfError::NoSuchEntry {
            table: "primitives",
            index: 7,
            count: 2,
        }
    );
}

/// The census and the documents agree, and every refusal says something.
#[test]
fn the_census_and_the_documents_agree() {
    let bad_type = document(r#"{ "bufferViews": [{"buffer": 0, "byteLength": "long" }] }"#);
    let unknown = document(
        r#"{ "accessors": [{ "bufferView": 0, "componentType": 5124, "count": 1, "type": "VEC3" }] }"#,
    );
    let missing = document(r#"{ "bufferViews": [{"buffer": 0, "byteOffset": 4 }] }"#);
    let elsewhere = document(r#"{ "buffers": [{ "byteLength": 12, "uri": "geometry.bin" }] }"#);
    let sparse = document(
        r#"{ "accessors": [{
            "bufferView": 0, "componentType": 5126, "count": 1, "type": "VEC3",
            "sparse": {}
        }] }"#,
    );

    // The magic stays intact: bytes that do not announce themselves as a
    // container are not one, and are tried as a document instead.
    let mut bad_version = container(ONE_NODE, &three_positions());
    bad_version[4] = 9;
    let cycle = container(
        &ONE_NODE.replace(
            r#""nodes": [{ "mesh": 0 }]"#,
            r#""nodes": [{ "children": [0], "mesh": 0 }]"#,
        ),
        &three_positions(),
    );
    let empty_scene = container(
        r#"{ "asset": { "version": "2.0" }, "scenes": [{ "nodes": [] }] }"#,
        &[],
    );
    let past_a_table = document(
        r#"{
          "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "attributes": { "POSITION": 7 } }] }]
        }"#,
    );

    let mut provocations: Vec<(&str, GltfError)> = vec![
        (
            "Document",
            gltf::buffer_views(bad_type.root()).expect_err("a length is a number"),
        ),
        (
            "Accessor",
            gltf::accessors(unknown.root()).expect_err("5124 is not a component type"),
        ),
        (
            "MissingField",
            gltf::buffer_views(missing.root()).expect_err("no length"),
        ),
        (
            "ExternalResource",
            gltf::buffers(elsewhere.root(), None).expect_err("a second file"),
        ),
        (
            "Unsupported",
            gltf::accessors(sparse.root()).expect_err("sparse"),
        ),
        (
            "Container",
            gltf::read(&bad_version).expect_err("version 9 is not version 2"),
        ),
        (
            "NodeCycle",
            gltf::read(&cycle).expect_err("0 is its own child"),
        ),
        (
            "Geometry",
            gltf::read(&empty_scene).expect_err("a scene placing nothing"),
        ),
        (
            "NoSuchEntry",
            gltf::primitive(
                past_a_table.root(),
                &gltf::Source::of(past_a_table.root(), Some(&three_positions()))
                    .expect("the tables read"),
                0,
                0,
            )
            .expect_err("accessor 7 of one"),
        ),
    ];

    provocations.extend(buffer_provocations());

    for (name, refused) in &provocations {
        assert!(
            gltf_cannot_reach(refused).is_none(),
            "`{name}` is provoked here and the census calls it unreachable"
        );
        assert_eq!(refused.name(), *name);
        assert!(!refused.to_string().is_empty(), "{refused:?} says nothing");
    }

    let mut named: Vec<&str> = Vec::new();

    for (name, _) in &provocations {
        assert!(!named.contains(name), "`{name}` is provoked twice");
        named.push(name);
    }
    assert_eq!(named.len(), 15, "one provocation per refusal");
}

// ---------------------------------------------------------------------
// Meshes and primitives.
//
// The tables above are numbers; these turn a document's indices into
// streams and hand them to the layer that assembles geometry. **The
// interesting cases are the crossings**: an index into a table that is
// too short, and the stride travelling from a view to the accessor that
// reads through it.
// ---------------------------------------------------------------------

/// Compare coordinates by bits.
///
/// **The honest comparison here, not a way around the lint.** Reading a
/// document does no arithmetic on a coordinate: it takes four bytes out
/// of the chunk and puts them in an array. Anything but an exact match
/// is a value that came from somewhere other than where the fixture put
/// it, and a tolerance would hide exactly that.
fn same(got: &[f32], want: &[f32], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: different lengths");
    for (index, (left, right)) in got.iter().zip(want).enumerate() {
        assert_eq!(
            left.to_bits(),
            right.to_bits(),
            "{what}: component {index} is {left}, not {right}"
        );
    }
}

/// Three positions, tightly packed, as a binary chunk would store them.
fn three_positions() -> Vec<u8> {
    [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Read the tables and one primitive out of a document and a chunk.
fn assemble(text: &str, binary: &[u8]) -> Result<renew_mesh::Mesh, GltfError> {
    let json = document(text);
    let source = gltf::Source::of(json.root(), Some(binary))?;
    gltf::primitive(json.root(), &source, 0, 0)
}

/// A document naming one triangle produces one triangle.
#[test]
fn a_document_naming_one_triangle_produces_one() {
    let mesh = assemble(
        r#"{
          "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }]
        }"#,
        &three_positions(),
    )
    .expect("one unindexed triangle");
    assert_eq!(mesh.triangles(), 1);
    assert_eq!(mesh.positions.len(), 3);
    assert!(
        mesh.corner_normals.is_empty(),
        "the document carried no normals"
    );
}

/// **A primitive with no `mode` is a triangle list, by the format's own
/// default.**
///
/// A reader that refused one would reject most of the files in the
/// world, which is why the default is written down rather than left to
/// whichever branch happens to run.
#[test]
fn a_primitive_with_no_mode_is_triangles() {
    let with = assemble(
        r#"{
          "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "mode": 4, "attributes": { "POSITION": 0 } }] }]
        }"#,
        &three_positions(),
    )
    .expect("mode 4 is triangles");
    let without = assemble(
        r#"{
          "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }]
        }"#,
        &three_positions(),
    )
    .expect("and so is no mode at all");
    same(
        &with.positions.concat(),
        &without.positions.concat(),
        "an absent mode and mode 4",
    );
}

/// A mode this reader does not draw arrives wrapped as a geometry
/// refusal.
#[test]
fn a_mode_this_reader_does_not_draw_is_a_geometry_refusal() {
    let refused = assemble(
        r#"{
          "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "mode": 5, "attributes": { "POSITION": 0 } }] }]
        }"#,
        &three_positions(),
    )
    .expect_err("a triangle strip is not assembled");
    assert_eq!(refused.name(), "Geometry");
    assert!(
        refused.to_string().contains("triangle strip"),
        "the inner refusal names which mode: {refused}"
    );
}

/// **The stride travels from the view to the accessor that reads through
/// it**, which is the one place that number crosses layers.
///
/// Six floats at a stride of twenty-four: two positions whose second
/// starts a whole stride in, with a neighbour's bytes between them. A
/// reader that dropped the stride would read the neighbour as a
/// coordinate and would not notice.
#[test]
fn the_stride_crosses_from_the_view_to_the_accessor() {
    let mut binary: Vec<u8> = Vec::new();
    for value in [1.0f32, 2.0, 3.0] {
        binary.extend_from_slice(&value.to_le_bytes());
    }
    binary.extend_from_slice(&[0xFF; 12]); // A neighbour's bytes.
    for value in [4.0f32, 5.0, 6.0] {
        binary.extend_from_slice(&value.to_le_bytes());
    }
    binary.extend_from_slice(&[0xFF; 12]);
    for value in [7.0f32, 8.0, 9.0] {
        binary.extend_from_slice(&value.to_le_bytes());
    }

    let mesh = assemble(
        r#"{
          "buffers": [{ "byteLength": 60 }],
          "bufferViews": [{"buffer": 0, "byteLength": 60, "byteStride": 24 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }]
        }"#,
        &binary,
    )
    .expect("three interleaved positions");
    same(&mesh.positions[0], &[1.0, 2.0, 3.0], "the first position");
    same(
        &mesh.positions[1],
        &[4.0, 5.0, 6.0],
        "the neighbour's bytes were stepped over",
    );
    same(&mesh.positions[2], &[7.0, 8.0, 9.0], "the third position");
}

/// Normals, texture coordinates and an index stream are all read when
/// the document names them.
#[test]
fn the_optional_streams_are_read_when_named() {
    let mut binary = three_positions();
    // Normals at 36, texture coordinates at 72, indices at 96.
    for value in [0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0] {
        binary.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0] {
        binary.extend_from_slice(&value.to_le_bytes());
    }
    for index in [0u16, 1, 2] {
        binary.extend_from_slice(&index.to_le_bytes());
    }

    let mesh = assemble(
        r#"{
          "buffers": [{ "byteLength": 102 }],
          "bufferViews": [
            {"buffer": 0, "byteLength": 36, "byteOffset": 0 },
            {"buffer": 0, "byteLength": 36, "byteOffset": 36 },
            {"buffer": 0, "byteLength": 24, "byteOffset": 72 },
            {"buffer": 0, "byteLength": 6, "byteOffset": 96 }
          ],
          "accessors": [
            { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" },
            { "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" },
            { "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" },
            { "bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR" }
          ],
          "meshes": [{ "primitives": [{
            "attributes": { "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 },
            "indices": 3
          }] }]
        }"#,
        &binary,
    )
    .expect("everything named is read");
    assert_eq!(mesh.triangles(), 1);
    assert_eq!(mesh.corner_normals.len(), 3);
    assert_eq!(mesh.corner_texcoords.len(), 3);
    same(
        &mesh.corner_normals[0],
        &[0.0, 0.0, 1.0],
        "the first normal",
    );
}

/// **An index naming a row that is not there carries the table, the
/// index and the count.**
#[test]
fn an_index_past_a_table_names_all_three() {
    let refused = assemble(
        r#"{
          "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "attributes": { "POSITION": 7 } }] }]
        }"#,
        &three_positions(),
    )
    .expect_err("accessor 7 of one");
    assert_eq!(
        refused,
        GltfError::NoSuchEntry {
            table: "accessors",
            index: 7,
            count: 1,
        }
    );

    // And a view index past the views, which is the same shape one
    // level down.
    let refused = assemble(
        r#"{
          "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteLength": 36 }],
          "accessors": [{ "bufferView": 3, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }]
        }"#,
        &three_positions(),
    )
    .expect_err("view 3 of one");
    assert_eq!(
        refused,
        GltfError::NoSuchEntry {
            table: "bufferViews",
            index: 3,
            count: 1,
        }
    );

    // And a mesh that is not there at all.
    let refused = assemble(r#"{ "asset": { "version": "2.0" } }"#, &three_positions())
        .expect_err("no meshes at all");
    assert_eq!(
        refused,
        GltfError::NoSuchEntry {
            table: "meshes",
            index: 0,
            count: 0,
        }
    );
}

/// A primitive with no positions is not geometry, and says which member
/// is missing.
#[test]
fn a_primitive_with_no_positions_is_refused() {
    let refused = assemble(
        r#"{
          "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "attributes": { "NORMAL": 0 } }] }]
        }"#,
        &three_positions(),
    )
    .expect_err("a primitive without positions describes nothing");
    assert_eq!(refused, GltfError::MissingField { path: "POSITION" });
}

/// **An accessor that does not fit the chunk is refused by the layer
/// that does the arithmetic**, and arrives wrapped.
#[test]
fn an_accessor_past_the_chunk_is_an_accessor_refusal() {
    let refused = assemble(
        r#"{
          "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 99, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }]
        }"#,
        &three_positions(),
    )
    .expect_err("ninety-nine positions in thirty-six bytes");
    assert_eq!(refused.name(), "Accessor");
    assert!(
        refused.to_string().contains("1188"),
        "the inner refusal's numbers survive: {refused}"
    );
}

/// **A view that runs past its buffer is caught before any element is
/// read**, and the buffer layer has nothing to say about it: the buffer
/// is exactly as long as it claimed, and the view inside it is not.
#[test]
fn a_view_past_its_buffer_is_refused() {
    let refused = assemble(
        r#"{
          "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteOffset": 24, "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }]
        }"#,
        &three_positions(),
    )
    .expect_err("24 + 36 is past 36");
    assert_eq!(refused.name(), "Accessor");
    assert!(refused.to_string().contains("36"), "{refused}");
}

/// **A buffer longer than the resource behind it is a different fault,
/// one layer down**, and the two are worth telling apart: this document
/// and its chunk disagree about how much is there, where the test above
/// has a document that disagrees with itself.
#[test]
fn a_buffer_longer_than_its_chunk_is_refused() {
    let refused = assemble(
        r#"{
          "buffers": [{ "byteLength": 60 }],
          "bufferViews": [{"buffer": 0, "byteOffset": 24, "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }]
        }"#,
        &three_positions(),
    )
    .expect_err("a sixty-byte buffer over a thirty-six-byte chunk");
    assert_eq!(
        refused,
        GltfError::BufferTooShort {
            buffer: 0,
            declared: 60,
            available: 36,
        }
    );
}

// ---------------------------------------------------------------------
// Whole containers, read end to end.
//
// **The test this section exists for is the cycle**, and it is the one
// that cannot be a corpus seed. A reader that followed parent links
// without remembering where it had been would walk forever, and a hang
// is the one failure a fuzz harness has no way to report — so the guard
// is checked by construction and pinned here.
// ---------------------------------------------------------------------

/// Wrap a document and a binary payload in a container.
fn container(json: &str, binary: &[u8]) -> Vec<u8> {
    let mut document = json.as_bytes().to_vec();
    while !document.len().is_multiple_of(4) {
        document.push(b' ');
    }
    let mut payload = binary.to_vec();
    while !payload.len().is_multiple_of(4) {
        payload.push(0);
    }

    let mut out = b"glTF".to_vec();
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(document.len())
            .expect("a fixture is small")
            .to_le_bytes(),
    );
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&document);
    if !payload.is_empty() {
        out.extend_from_slice(&u32::try_from(payload.len()).expect("small").to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        out.extend_from_slice(&payload);
    }
    let total = u32::try_from(out.len()).expect("a fixture is small");
    out[8..12].copy_from_slice(&total.to_le_bytes());
    out
}

/// One triangle, one node, one scene.
const ONE_NODE: &str = r#"{
  "asset": { "version": "2.0" },
  "scenes": [{ "nodes": [0] }],
  "nodes": [{ "mesh": 0 }],
  "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }],
  "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
  "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteLength": 36 }]
}"#;

/// **A container reads end to end.**
#[test]
fn a_container_reads_into_geometry() {
    let bytes = container(ONE_NODE, &three_positions());
    let mesh = gltf::read(&bytes).expect("one triangle under one node");
    assert_eq!(mesh.triangles(), 1);
    same(&mesh.positions[1], &[1.0, 0.0, 0.0], "the second corner");
}

/// **A node's transform reaches its geometry**, by the composition order
/// the format states.
#[test]
fn a_nodes_transform_moves_its_geometry() {
    let json = ONE_NODE.replace(
        r#"{ "mesh": 0 }"#,
        r#"{ "mesh": 0, "translation": [10.0, 0.0, 0.0], "scale": [2.0, 1.0, 1.0] }"#,
    );
    let mesh = gltf::read(&container(&json, &three_positions())).expect("a placed triangle");
    // Scale first, then translate, which is the order the format names.
    same(&mesh.positions[0], &[10.0, 0.0, 0.0], "the origin corner");
    same(&mesh.positions[1], &[12.0, 0.0, 0.0], "scaled then moved");
}

/// A matrix says the same thing as the three parts that compose it.
#[test]
fn a_matrix_and_its_parts_agree() {
    let parts = ONE_NODE.replace(
        r#"{ "mesh": 0 }"#,
        r#"{ "mesh": 0, "translation": [1.0, 2.0, 3.0], "scale": [2.0, 2.0, 2.0] }"#,
    );
    // The same transform, column-major, as the file stores it.
    let matrix = ONE_NODE.replace(
        r#"{ "mesh": 0 }"#,
        r#"{ "mesh": 0, "matrix": [2,0,0,0, 0,2,0,0, 0,0,2,0, 1,2,3,1] }"#,
    );
    let from_parts = gltf::read(&container(&parts, &three_positions())).expect("parts");
    let from_matrix = gltf::read(&container(&matrix, &three_positions())).expect("matrix");
    same(
        &from_parts.positions.concat(),
        &from_matrix.positions.concat(),
        "a matrix and the parts it composes",
    );
}

/// A parent's transform reaches a child's geometry.
#[test]
fn a_parents_transform_reaches_its_children() {
    let json = ONE_NODE.replace(
        r#""nodes": [{ "mesh": 0 }]"#,
        r#""nodes": [
              { "children": [1], "translation": [10.0, 0.0, 0.0] },
              { "mesh": 0, "translation": [0.0, 5.0, 0.0] }
            ]"#,
    );
    let mesh = gltf::read(&container(&json, &three_positions())).expect("a child under a parent");
    same(
        &mesh.positions[0],
        &[10.0, 5.0, 0.0],
        "the parent's translation composed with the child's",
    );
}

/// **A node that is its own ancestor is refused, and the walk does not
/// hang.**
///
/// The refusal this reader could not have got from a fuzzer: a corpus
/// seed carrying a cycle would wedge the harness rather than fail it, so
/// the guard is checked by construction and pinned here.
#[test]
fn a_cycle_in_the_hierarchy_is_refused() {
    let json = ONE_NODE.replace(
        r#""nodes": [{ "mesh": 0 }]"#,
        r#""nodes": [{ "children": [1] }, { "children": [0], "mesh": 0 }]"#,
    );
    assert_eq!(
        gltf::read(&container(&json, &three_positions())).expect_err("0 is its own grandparent"),
        GltfError::NodeCycle { node: 0 }
    );

    // A node that is its own child, which is the shortest cycle there
    // is and the one an off-by-one guard would miss.
    let json = ONE_NODE.replace(
        r#""nodes": [{ "mesh": 0 }]"#,
        r#""nodes": [{ "children": [0], "mesh": 0 }]"#,
    );
    assert_eq!(
        gltf::read(&container(&json, &three_positions())).expect_err("0 is its own child"),
        GltfError::NodeCycle { node: 0 }
    );
}

/// A node claimed by two parents is refused by the same guard.
#[test]
fn a_node_with_two_parents_is_refused() {
    let json = ONE_NODE
        .replace(r#""nodes": [0]"#, r#""nodes": [0, 1]"#)
        .replace(
            r#""nodes": [{ "mesh": 0 }]"#,
            r#""nodes": [
              { "children": [2] },
              { "children": [2] },
              { "mesh": 0 }
            ]"#,
        );
    assert_eq!(
        gltf::read(&container(&json, &three_positions())).expect_err("two parents claim node 2"),
        GltfError::NodeCycle { node: 2 }
    );
}

/// **A document with no scenes is a library, not a model.**
#[test]
fn a_document_with_no_scenes_is_refused() {
    let json = r#"{
      "asset": { "version": "2.0" },
      "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }],
      "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
      "buffers": [{ "byteLength": 36 }],
          "bufferViews": [{"buffer": 0, "byteLength": 36 }]
    }"#;
    assert_eq!(
        gltf::read(&container(json, &three_positions())).expect_err("no scene places anything"),
        GltfError::MissingField { path: "scenes" }
    );
}

/// **A scene naming no geometry has none, whether it says so with an
/// empty list or by not having one.**
///
/// Two different documents and one answer. `nodes` is optional on a
/// scene, so a scene object with no member at all is a legal thing to
/// write — and it was the shape no fixture had, which the coverage gate
/// noticed by naming the closing brace of the branch that reads it.
#[test]
fn a_scene_that_places_nothing_has_no_geometry() {
    for json in [
        r#"{ "asset": { "version": "2.0" }, "scenes": [{ "nodes": [] }] }"#,
        r#"{ "asset": { "version": "2.0" }, "scenes": [{}] }"#,
    ] {
        let refused = gltf::read(&container(json, &[])).expect_err("a scene placing nothing");
        assert_eq!(refused.name(), "Geometry", "for `{json}`");
        assert!(refused.to_string().contains("no geometry"), "{refused}");
    }
}

/// **A container fault arrives as a container fault**, not as a
/// document one.
///
/// The magic is left intact deliberately. These bytes announce
/// themselves as a container and then fail to be one, which is the case
/// this claim is about; bytes whose magic is *wrong* are not a container
/// at all, and the test below says what happens to those.
#[test]
fn a_malformed_container_is_a_container_refusal() {
    let mut bytes = container(ONE_NODE, &three_positions());
    bytes[4] = 9; // a container version this build does not read
    let refused = gltf::read(&bytes).expect_err("version 9 is not version 2");
    assert_eq!(refused.name(), "Container");
    assert!(refused.to_string().contains("version"), "{refused}");
}

/// **Bytes whose magic is wrong are tried as a document.**
///
/// A reader that takes two shapes has to choose, and the magic is what
/// chooses: four bytes that either say `glTF` or do not. Something that
/// does not say it is not a container, so the remaining question is
/// whether it is a document -- and when it is not one either, the
/// document layer is what refused, because it is what was asked.
#[test]
fn bytes_whose_magic_is_wrong_are_tried_as_a_document() {
    let mut bytes = container(ONE_NODE, &three_positions());
    bytes[0] = b'X';
    assert_eq!(
        gltf::read(&bytes).expect_err("neither shape").name(),
        "Document"
    );
}

/// Several primitives under one node are joined into one mesh.
#[test]
fn several_primitives_are_joined() {
    let json = ONE_NODE.replace(
        r#""primitives": [{ "attributes": { "POSITION": 0 } }]"#,
        r#""primitives": [
          { "attributes": { "POSITION": 0 } },
          { "attributes": { "POSITION": 0 } }
        ]"#,
    );
    let mesh = gltf::read(&container(&json, &three_positions())).expect("two primitives");
    assert_eq!(mesh.triangles(), 2, "joined rather than replaced");
}

/// **A transform that flattens geometry carrying normals is refused, and
/// the refusal comes from the placement layer.**
#[test]
fn a_flattening_transform_over_normals_is_a_geometry_refusal() {
    let mut binary = three_positions();
    for value in [0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0] {
        binary.extend_from_slice(&value.to_le_bytes());
    }
    let json = r#"{
      "asset": { "version": "2.0" },
      "scenes": [{ "nodes": [0] }],
      "nodes": [{ "mesh": 0, "scale": [1.0, 0.0, 1.0] }],
      "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0, "NORMAL": 1 } }] }],
      "accessors": [
        { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" },
        { "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" }
      ],
      "buffers": [{ "byteLength": 72 }],
          "bufferViews": [
        {"buffer": 0, "byteLength": 36, "byteOffset": 0 },
        {"buffer": 0, "byteLength": 36, "byteOffset": 36 }
      ]
    }"#;
    let refused = gltf::read(&container(json, &binary)).expect_err("no inverse to transpose");
    assert_eq!(refused.name(), "Geometry");
    assert!(refused.to_string().contains("flattens space"), "{refused}");
}
