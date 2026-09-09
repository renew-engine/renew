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
          "bufferViews": [{ "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{
            "attributes": { "POSITION": 0, "NORMAL": "one" }
          }] }]
        }"#,
    );
    let views = gltf::buffer_views(attribute.root()).expect("one view");
    let accessors = gltf::accessors(attribute.root()).expect("one accessor");
    assert_eq!(
        gltf::primitive(
            attribute.root(),
            &views,
            &accessors,
            &three_positions(),
            0,
            0
        )
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
        | GltfError::Unsupported { .. } => None,
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

    let mut wrong_magic = container(ONE_NODE, &three_positions());
    wrong_magic[0] = b'X';
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
          "bufferViews": [{ "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "attributes": { "POSITION": 7 } }] }]
        }"#,
    );

    let provocations: [(&str, GltfError); 9] = [
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
        (
            "Container",
            gltf::read(&wrong_magic).expect_err("not a container"),
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
                &gltf::buffer_views(past_a_table.root()).expect("one view"),
                &gltf::accessors(past_a_table.root()).expect("one accessor"),
                &three_positions(),
                0,
                0,
            )
            .expect_err("accessor 7 of one"),
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

    let mut named: Vec<&str> = Vec::new();
    for (name, _) in &provocations {
        assert!(!named.contains(name), "`{name}` is provoked twice");
        named.push(name);
    }
    assert_eq!(named.len(), 9, "one provocation per refusal");
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
    let views = gltf::buffer_views(json.root())?;
    let accessors = gltf::accessors(json.root())?;
    gltf::primitive(json.root(), &views, &accessors, binary, 0, 0)
}

/// A document naming one triangle produces one triangle.
#[test]
fn a_document_naming_one_triangle_produces_one() {
    let mesh = assemble(
        r#"{
          "bufferViews": [{ "byteLength": 36 }],
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
          "bufferViews": [{ "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "mode": 4, "attributes": { "POSITION": 0 } }] }]
        }"#,
        &three_positions(),
    )
    .expect("mode 4 is triangles");
    let without = assemble(
        r#"{
          "bufferViews": [{ "byteLength": 36 }],
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
          "bufferViews": [{ "byteLength": 36 }],
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
          "bufferViews": [{ "byteLength": 60, "byteStride": 24 }],
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
          "bufferViews": [
            { "byteLength": 36, "byteOffset": 0 },
            { "byteLength": 36, "byteOffset": 36 },
            { "byteLength": 24, "byteOffset": 72 },
            { "byteLength": 6, "byteOffset": 96 }
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
          "bufferViews": [{ "byteLength": 36 }],
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
          "bufferViews": [{ "byteLength": 36 }],
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
          "bufferViews": [{ "byteLength": 36 }],
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
          "bufferViews": [{ "byteLength": 36 }],
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

/// A view that runs past the chunk is caught before any element is read.
#[test]
fn a_view_past_the_chunk_is_refused() {
    let refused = assemble(
        r#"{
          "bufferViews": [{ "byteOffset": 24, "byteLength": 36 }],
          "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }],
          "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }]
        }"#,
        &three_positions(),
    )
    .expect_err("24 + 36 is past 36");
    assert_eq!(refused.name(), "Accessor");
    assert!(refused.to_string().contains("60"), "{refused}");
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
  "bufferViews": [{ "byteLength": 36 }]
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
      "bufferViews": [{ "byteLength": 36 }]
    }"#;
    assert_eq!(
        gltf::read(&container(json, &three_positions())).expect_err("no scene places anything"),
        GltfError::MissingField { path: "scenes" }
    );
}

/// A scene naming no geometry has none, and says so.
#[test]
fn a_scene_that_places_nothing_has_no_geometry() {
    let json = r#"{ "asset": { "version": "2.0" }, "scenes": [{ "nodes": [] }] }"#;
    let refused = gltf::read(&container(json, &[])).expect_err("an empty scene");
    assert_eq!(refused.name(), "Geometry");
    assert!(refused.to_string().contains("no geometry"), "{refused}");
}

/// **A container fault arrives as a container fault**, not as a
/// document one.
#[test]
fn a_malformed_container_is_a_container_refusal() {
    let mut bytes = container(ONE_NODE, &three_positions());
    bytes[0] = b'X';
    let refused = gltf::read(&bytes).expect_err("not a container");
    assert_eq!(refused.name(), "Container");
    assert!(refused.to_string().contains("glTF"), "{refused}");
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
      "bufferViews": [
        { "byteLength": 36, "byteOffset": 0 },
        { "byteLength": 36, "byteOffset": 36 }
      ]
    }"#;
    let refused = gltf::read(&container(json, &binary)).expect_err("no inverse to transpose");
    assert_eq!(refused.name(), "Geometry");
    assert!(refused.to_string().contains("flattens space"), "{refused}");
}
