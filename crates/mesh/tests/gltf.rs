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
    assert_eq!(views[0].byte_offset, 0);
    assert_eq!(views[0].byte_length, 36);
    assert_eq!(views[0].byte_stride, None, "absent means tightly packed");

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
          "bufferViews": [{ "byteLength": 12 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 1, "type": "VEC3" }]
        }"#,
    );
    let views = gltf::buffer_views(json.root()).expect("one view");
    assert_eq!(views[0].byte_offset, 0, "an absent offset is zero");
    // `buffer` is absent too, and defaults to the only buffer there is.
    let accessors = gltf::accessors(json.root()).expect("one accessor");
    assert_eq!(accessors[0].1.byte_offset, 0);
    assert!(!accessors[0].1.normalized);
}

/// A stride on the view is read, because that is where the format puts
/// it.
#[test]
fn a_stride_is_read_from_the_view() {
    let json = document(r#"{ "bufferViews": [{ "byteLength": 96, "byteStride": 32 }] }"#);
    let views = gltf::buffer_views(json.root()).expect("one view");
    assert_eq!(views[0].byte_stride, Some(32));
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
    let json = document(r#"{ "bufferViews": [{ "byteOffset": 4 }] }"#);
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

/// **A buffer that is not the container's own is refused rather than
/// fetched.**
#[test]
fn a_second_buffer_is_refused_rather_than_fetched() {
    let json = document(r#"{ "bufferViews": [{ "buffer": 1, "byteLength": 12 }] }"#);
    assert_eq!(
        gltf::buffer_views(json.root()).expect_err("buffer 1 is somewhere else"),
        GltfError::ExternalResource
    );
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
    let json = document(r#"{ "bufferViews": [{ "byteLength": "long" }] }"#);
    let refused = gltf::buffer_views(json.root()).expect_err("a length is a number");
    assert_eq!(
        refused.name(),
        "Document",
        "wrapped, and named for its layer"
    );
}

/// **Every refusal this layer can make is reachable, and every one it
/// cannot make says why.**
fn gltf_cannot_reach(refusal: &GltfError) -> Option<&'static str> {
    match refusal {
        GltfError::Document(_)
        | GltfError::Accessor(_)
        | GltfError::MissingField { .. }
        | GltfError::ExternalResource
        | GltfError::Unsupported { .. } => None,
        GltfError::Container(_) => Some(
            "the tables are read from a document that has already been taken out of its \
             container; the container's own refusals belong to whatever opened it",
        ),
        GltfError::Geometry(_) => Some(
            "no geometry is built here: these functions produce the numbers a later step \
             assembles from",
        ),
        GltfError::NoSuchEntry { .. } => Some(
            "these functions walk their own tables from zero, so an index past the end is \
             unreachable until something follows a number the document wrote",
        ),
    }
}

/// The census and the documents agree, and every refusal says something.
#[test]
fn the_census_and_the_documents_agree() {
    let bad_type = document(r#"{ "bufferViews": [{ "byteLength": "long" }] }"#);
    let unknown = document(
        r#"{ "accessors": [{ "bufferView": 0, "componentType": 5124, "count": 1, "type": "VEC3" }] }"#,
    );
    let missing = document(r#"{ "bufferViews": [{ "byteOffset": 4 }] }"#);
    let elsewhere = document(r#"{ "bufferViews": [{ "buffer": 1, "byteLength": 12 }] }"#);
    let sparse = document(
        r#"{ "accessors": [{
            "bufferView": 0, "componentType": 5126, "count": 1, "type": "VEC3",
            "sparse": {}
        }] }"#,
    );

    let provocations: [(&str, GltfError); 5] = [
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
            gltf::buffer_views(elsewhere.root()).expect_err("buffer 1"),
        ),
        (
            "Unsupported",
            gltf::accessors(sparse.root()).expect_err("sparse"),
        ),
    ];

    for (name, refused) in &provocations {
        assert!(
            gltf_cannot_reach(refused).is_none(),
            "`{name}` is provoked here and the census calls it unreachable"
        );
        assert_eq!(refused.name(), *name);
        assert!(!refused.to_string().is_empty(), "{refused:?} says nothing");
    }

    // The three the census calls unreachable still say something, and
    // still answer to their names, because a later step provokes them.
    for refusal in [
        GltfError::Container(renew_mesh::glb::GlbError::NoChunks),
        GltfError::Geometry(renew_mesh::MeshError::NoGeometry),
        GltfError::NoSuchEntry {
            table: "accessors",
            index: 7,
            count: 2,
        },
    ] {
        assert!(gltf_cannot_reach(&refusal).is_some());
        assert!(!refusal.name().is_empty());
        assert!(!refusal.to_string().is_empty());
    }
}
