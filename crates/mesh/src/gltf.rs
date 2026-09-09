//! The document: a description of geometry, read into the shapes the
//! layers below already validate.
//!
//! Everything under this module works on numbers somebody has already
//! extracted — an accessor is six values and a mesh is four arrays.
//! **This is where those numbers come from**, and it is the only layer
//! that knows the format has a document at all.
//!
//! # Everything is addressed by index, so most refusals are one shape
//!
//! A document is a handful of parallel arrays and a great many indices
//! into them: a primitive names an accessor by number, an accessor names
//! a buffer view by number, a view names a buffer by number. **A number
//! naming a row that is not there is the commonest thing wrong with a
//! hand-edited or truncated document**, and it has one refusal here that
//! carries the table, the index and how many rows there were.
//!
//! # What is refused rather than fetched
//!
//! A buffer may name a `uri`, which is a second file or an embedded
//! payload. **This reader refuses both by name.** Reading a second file
//! would mean opening one, which this crate does not do and says so
//! everywhere else; decoding an embedded one needs a decoder the tree
//! does not have. Neither is a silent limitation: a document that wants
//! either is told which, so the caller knows whether to convert the file
//! or to wait for a reader that can.
//!
//! The one buffer this reads is the container's own binary chunk, which
//! is how a self-contained binary glTF stores its geometry.

use renew_json::{Json, JsonError, Value};
use renew_math::{Mat4, Quat, Vec3, Vec4};

use crate::accessor::{Accessor, AccessorError, BufferView, Component, Indices, Shape, View};
use crate::error::MeshError;
use crate::glb::GlbError;
use crate::primitive::{self, Mode, Primitive};
use crate::{Mesh, glb, place};

/// Every way a document can fail to describe geometry this can read.
///
/// **Four of these carry another layer's refusal**, and that is the
/// point of the type: a caller wants to know whether the container was
/// malformed, the JSON was, an accessor's arithmetic was, or the
/// geometry was — because those send them to four different places.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GltfError {
    /// The container the document came in.
    Container(GlbError),

    /// The document's own text.
    Document(JsonError),

    /// An accessor's claim about the bytes it addresses.
    Accessor(AccessorError),

    /// The geometry the document described.
    Geometry(MeshError),

    /// A member the format requires is not there.
    ///
    /// Carries the path in the document's own vocabulary, so it can be
    /// searched for in the file rather than guessed at.
    MissingField {
        /// Where it should have been, spelled as the document spells it.
        path: &'static str,
    },

    /// An index naming a row that is not in the table it names.
    ///
    /// **The commonest thing wrong with a hand-edited document**, and
    /// the one nothing downstream can catch: every layer below this one
    /// is handed the row rather than the index.
    NoSuchEntry {
        /// The array, as the document names it.
        table: &'static str,
        /// The index the document asked for.
        index: usize,
        /// How many rows there were.
        count: usize,
    },

    /// A buffer this reader will not go and get.
    ///
    /// Refused rather than ignored: a document whose geometry lives in a
    /// second file describes a model this cannot assemble, and returning
    /// what it *can* assemble would be returning half a model without
    /// saying so.
    ExternalResource,

    /// A node that is its own ancestor, or that two parents claim.
    ///
    /// **The one refusal here whose absence is a hang rather than a
    /// wrong answer.** A reader that followed parent links without
    /// remembering where it had been would walk a cycle forever, and a
    /// fuzzer cannot catch that: a hang is the one failure a harness has
    /// no way to report. So this is checked by construction — every node
    /// is entered at most once — and pinned by a test rather than by a
    /// corpus seed.
    ///
    /// It catches two faults at once, and both are invalid: a cycle, and
    /// a node reached from two parents. The format's hierarchy is a
    /// strict forest, so a second visit is wrong either way.
    NodeCycle {
        /// The node entered twice.
        node: usize,
    },

    /// A construct the format defines and this reader does not
    /// implement.
    ///
    /// Distinct from a malformed document: the file is fine and the
    /// reader is narrow, and the caller's next move is a conversion
    /// rather than a repair.
    Unsupported {
        /// What was found, in the document's own vocabulary.
        found: &'static str,
    },
}

impl GltfError {
    /// The variant's own name, for a caller acting on which refusal this
    /// is rather than reading it.
    ///
    /// **A wrapping variant answers with its own name and not the
    /// inner's.** A caller keying on "Accessor" wants to know the fault
    /// was in an accessor; the inner refusal's name is one call away
    /// through the value, and folding the two would make the outer
    /// vocabulary unbounded.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Container(_) => "Container",
            Self::Document(_) => "Document",
            Self::Accessor(_) => "Accessor",
            Self::Geometry(_) => "Geometry",
            Self::MissingField { .. } => "MissingField",
            Self::NoSuchEntry { .. } => "NoSuchEntry",
            Self::ExternalResource => "ExternalResource",
            Self::NodeCycle { .. } => "NodeCycle",
            Self::Unsupported { .. } => "Unsupported",
        }
    }
}

impl core::fmt::Display for GltfError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Container(inner) => write!(f, "the container: {inner}"),
            Self::Document(inner) => write!(f, "the document: {inner}"),
            Self::Accessor(inner) => write!(f, "an accessor: {inner}"),
            Self::Geometry(inner) => write!(f, "the geometry: {inner}"),
            Self::MissingField { path } => write!(f, "`{path}` is required and is not there"),
            Self::NoSuchEntry {
                table,
                index,
                count,
            } => write!(f, "`{table}[{index}]` of a table holding {count}"),
            Self::ExternalResource => write!(
                f,
                "this document keeps its geometry somewhere else, and this reader takes bytes"
            ),
            Self::NodeCycle { node } => write!(
                f,
                "node {node} is reached twice, and this hierarchy is a tree"
            ),
            Self::Unsupported { found } => {
                write!(f, "`{found}` is in the format and not in this reader")
            }
        }
    }
}

impl core::error::Error for GltfError {}

impl From<AccessorError> for GltfError {
    fn from(inner: AccessorError) -> Self {
        Self::Accessor(inner)
    }
}

impl From<JsonError> for GltfError {
    fn from(inner: JsonError) -> Self {
        Self::Document(inner)
    }
}

impl From<MeshError> for GltfError {
    fn from(inner: MeshError) -> Self {
        Self::Geometry(inner)
    }
}

/// A member that must be there, by name.
fn required<'a>(object: Value<'a>, key: &'static str) -> Result<Value<'a>, GltfError> {
    object.get(key).ok_or(GltfError::MissingField { path: key })
}

/// A whole number member, or its default when the format gives one.
///
/// **The defaults are the format's and are written here once.** A reader
/// that spelled `byteOffset` as "zero if absent" at each of its three
/// call sites would be keeping three copies of one rule.
fn number_or(object: Value<'_>, key: &str, default: u32) -> Result<u32, GltfError> {
    match object.get(key) {
        None => Ok(default),
        Some(value) => Ok(value.as_u32()?),
    }
}

/// A row of a table, or the refusal that says the row is not there.
fn entry<'a>(
    table: Option<Value<'a>>,
    name: &'static str,
    index: usize,
) -> Result<Value<'a>, GltfError> {
    let Some(table) = table else {
        return Err(GltfError::NoSuchEntry {
            table: name,
            index,
            count: 0,
        });
    };
    table.index(index).ok_or(GltfError::NoSuchEntry {
        table: name,
        index,
        count: table.len(),
    })
}

/// Read the document's buffer views.
///
/// # Errors
///
/// A [`GltfError`] naming which view and which member.
pub fn buffer_views(root: Value<'_>) -> Result<Vec<BufferView>, GltfError> {
    let Some(table) = root.get("bufferViews") else {
        // A document with no views has no geometry to point at, which
        // the caller finds out when it asks for a primitive.
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(table.len());
    for index in 0..table.len() {
        let view = entry(Some(table), "bufferViews", index)?;

        // **Only the container's own chunk.** A document with more than
        // one buffer keeps geometry somewhere this cannot reach, and
        // saying so is better than reading the one buffer it can and
        // returning part of a model.
        if number_or(view, "buffer", 0)? != 0 {
            return Err(GltfError::ExternalResource);
        }

        let byte_length = required(view, "byteLength")?.as_u32()?;
        out.push(BufferView {
            byte_offset: number_or(view, "byteOffset", 0)? as usize,
            byte_length: byte_length as usize,
            byte_stride: match view.get("byteStride") {
                None => None,
                Some(stride) => Some(stride.as_u32()? as usize),
            },
        });
    }
    Ok(out)
}

/// Read the document's accessors.
///
/// # Errors
///
/// A [`GltfError`] naming which accessor and which member, or wrapping
/// the accessor layer's own refusal for a component type outside the
/// format's table.
pub fn accessors(root: Value<'_>) -> Result<Vec<(usize, Accessor)>, GltfError> {
    let Some(table) = root.get("accessors") else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(table.len());
    for index in 0..table.len() {
        let accessor = entry(Some(table), "accessors", index)?;

        if accessor.get("sparse").is_some() {
            // A sparse accessor overlays some of its elements from a
            // second pair of streams. Legal, and a different reader.
            return Err(GltfError::Unsupported { found: "sparse" });
        }
        // An accessor with no buffer view reads as zeros, by the format's
        // own rule. That is a real thing a document can say and not one
        // this reads, and the two are told apart rather than folded.
        let view = required(accessor, "bufferView")?.as_u32()? as usize;

        let component = Component::from_code(required(accessor, "componentType")?.as_u32()?)?;
        // **Decoded rather than compared raw.** A shape name is legal
        // JSON with escapes in it, and `"VEC3"` is `VEC3` however
        // strange it looks; refusing it for its spelling would be this
        // reader inventing a rule the format does not have. One
        // allocation per accessor, on a path that is already building a
        // table.
        let name = required(accessor, "type")?.as_str()?.decode();
        let shape = Shape::from_name(&name)?;
        let count = required(accessor, "count")?.as_u32()? as usize;
        let normalized = match accessor.get("normalized") {
            None => false,
            Some(value) => value.as_bool()?,
        };

        out.push((
            view,
            Accessor {
                component,
                shape,
                count,
                byte_offset: number_or(accessor, "byteOffset", 0)? as usize,
                // **Filled in from the view, not from here.** The format
                // puts the stride on the view, and copying it at this
                // one place is what keeps the two from disagreeing.
                byte_stride: None,
                normalized,
            },
        ));
    }
    Ok(out)
}

/// A row of a table read out of a slice, by the index the document
/// wrote.
fn row<T: Copy>(table: &[T], name: &'static str, index: usize) -> Result<T, GltfError> {
    table.get(index).copied().ok_or(GltfError::NoSuchEntry {
        table: name,
        index,
        count: table.len(),
    })
}

/// Resolve an accessor index into the bytes it addresses.
///
/// **This is the one place the stride crosses from a view to an
/// accessor**, and it is a function rather than three call sites for
/// that reason: the format puts `byteStride` on the view because
/// interleaved attributes share it, and the arithmetic wants it on the
/// accessor. A reader that copied it at each attribute would have three
/// chances to forget.
fn resolved<'a>(
    views: &[BufferView],
    accessors: &[(usize, Accessor)],
    binary: &'a [u8],
    index: usize,
) -> Result<(Accessor, &'a [u8]), GltfError> {
    let (which, accessor) = row(accessors, "accessors", index)?;
    let view = row(views, "bufferViews", which)?;
    let region = view.resolve(binary)?;
    Ok((
        Accessor {
            byte_stride: view.byte_stride,
            ..accessor
        },
        region,
    ))
}

/// An attribute stream, validated against the bytes it addresses.
fn stream<'a>(
    views: &[BufferView],
    accessors: &[(usize, Accessor)],
    binary: &'a [u8],
    index: usize,
) -> Result<View<'a>, GltfError> {
    let (accessor, region) = resolved(views, accessors, binary, index)?;
    Ok(accessor.view(region)?)
}

/// An index stream, validated against the bytes it addresses.
fn order<'a>(
    views: &[BufferView],
    accessors: &[(usize, Accessor)],
    binary: &'a [u8],
    index: usize,
) -> Result<Indices<'a>, GltfError> {
    let (accessor, region) = resolved(views, accessors, binary, index)?;
    Ok(accessor.indices(region)?)
}

/// An optional attribute, by the name the document spells it with.
fn optional_stream<'a>(
    attributes: Value<'_>,
    views: &[BufferView],
    accessors: &[(usize, Accessor)],
    binary: &'a [u8],
    name: &str,
) -> Result<Option<View<'a>>, GltfError> {
    match attributes.get(name) {
        None => Ok(None),
        Some(value) => Ok(Some(stream(
            views,
            accessors,
            binary,
            value.as_u32()? as usize,
        )?)),
    }
}

/// Assemble one primitive of one mesh.
///
/// # Errors
///
/// A [`GltfError`] naming which table an index missed, which member was
/// absent, or wrapping the refusal of whichever layer below found the
/// fault.
pub fn primitive(
    root: Value<'_>,
    views: &[BufferView],
    accessors: &[(usize, Accessor)],
    binary: &[u8],
    mesh: usize,
    index: usize,
) -> Result<Mesh, GltfError> {
    let meshes = root.get("meshes");
    let entry_row = entry(meshes, "meshes", mesh)?;
    let primitives = entry_row.get("primitives");
    let found = entry(primitives, "primitives", index)?;

    let attributes = required(found, "attributes")?;
    let positions = stream(
        views,
        accessors,
        binary,
        required(attributes, "POSITION")?.as_u32()? as usize,
    )?;

    // **The default is triangles and it is the format's**, not this
    // reader's convenience: a primitive with no `mode` is a triangle
    // list, and a reader that refused one would reject most of the files
    // in the world.
    let mode = Mode::from_code(number_or(found, "mode", 4)?)?;

    let indices = match found.get("indices") {
        None => None,
        Some(value) => Some(order(views, accessors, binary, value.as_u32()? as usize)?),
    };

    Ok(primitive::build(&Primitive {
        mode,
        positions,
        normals: optional_stream(attributes, views, accessors, binary, "NORMAL")?,
        texcoords: optional_stream(attributes, views, accessors, binary, "TEXCOORD_0")?,
        indices,
    })?)
}

/// A fixed-length array of numbers, or the default the format gives.
fn numbers<const N: usize>(
    object: Value<'_>,
    key: &'static str,
    default: [f32; N],
) -> Result<[f32; N], GltfError> {
    let Some(array) = object.get(key) else {
        return Ok(default);
    };
    let mut out = default;
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = entry(Some(array), key, index)?.as_f32()?;
    }
    Ok(out)
}

/// One node's own transform, before its parent's is applied.
///
/// **Either a matrix or the three parts, never both halves of each.**
/// The format allows a node to give a matrix or to give translation,
/// rotation and scale; when it gives a matrix that matrix is already the
/// composition, and when it gives the parts they compose as
/// `translation * rotation * scale` — the order the format states, and
/// the order that scales a vertex before it is turned.
fn node_transform(node: Value<'_>) -> Result<Mat4, GltfError> {
    if node.get("matrix").is_some() {
        // Column-major, sixteen numbers, in the order the file stores
        // them.
        let m = numbers::<16>(node, "matrix", [0.0; 16])?;
        return Ok(Mat4::from_cols(
            Vec4::new(m[0], m[1], m[2], m[3]),
            Vec4::new(m[4], m[5], m[6], m[7]),
            Vec4::new(m[8], m[9], m[10], m[11]),
            Vec4::new(m[12], m[13], m[14], m[15]),
        ));
    }

    let t = numbers::<3>(node, "translation", [0.0; 3])?;
    let r = numbers::<4>(node, "rotation", [0.0, 0.0, 0.0, 1.0])?;
    let s = numbers::<3>(node, "scale", [1.0; 3])?;
    Ok(Mat4::from_translation(Vec3::new(t[0], t[1], t[2]))
        * Mat4::from_quat(Quat::new(r[0], r[1], r[2], r[3]))
        * Mat4::from_scale(Vec3::new(s[0], s[1], s[2])))
}

/// Every primitive of one mesh, placed and joined.
fn mesh_at(
    root: Value<'_>,
    views: &[BufferView],
    accessors: &[(usize, Accessor)],
    binary: &[u8],
    index: usize,
    world: Mat4,
    out: &mut Mesh,
) -> Result<(), GltfError> {
    let count = entry(root.get("meshes"), "meshes", index)?
        .get("primitives")
        .map_or(0, Value::len);
    for which in 0..count {
        let mut piece = primitive(root, views, accessors, binary, index, which)?;
        place::place(&mut piece, world)?;
        if out.positions.is_empty() {
            *out = piece;
        } else {
            place::append(out, &piece)?;
        }
    }
    Ok(())
}

/// Read a whole binary glTF into one mesh.
///
/// # The walk cannot hang, by construction
///
/// Every node is entered at most once, which is checked before its
/// children are pushed. That is what makes a cycle a refusal rather than
/// a loop — and it matters more than it looks, because a hang is the one
/// failure a fuzz harness cannot report, so this guard has to be right
/// without a fuzzer's help.
///
/// # Errors
///
/// A [`GltfError`] naming the layer that refused and carrying its
/// numbers.
pub fn read(bytes: &[u8]) -> Result<Mesh, GltfError> {
    let container = glb::read(bytes).map_err(GltfError::Container)?;
    let json = parse(container.json)?;
    let root = json.root();
    let binary = container.binary.unwrap_or_default();

    let views = buffer_views(root)?;
    let accessors = accessors(root)?;

    // **A document with no scenes is a library rather than a model**,
    // which is the format's own reading of it, and a caller asking for
    // geometry is asking the wrong question of it.
    let scenes = root
        .get("scenes")
        .ok_or(GltfError::MissingField { path: "scenes" })?;
    // `scene` says which one to show and is optional; when it is absent
    // a client may choose, and this chooses the first.
    let scene = entry(
        Some(scenes),
        "scenes",
        number_or(root, "scene", 0)? as usize,
    )?;

    let nodes = root.get("nodes");
    let node_count = nodes.map_or(0, Value::len);
    let mut seen = vec![false; node_count];
    let mut stack: Vec<(usize, Mat4)> = Vec::new();

    if let Some(roots) = scene.get("nodes") {
        for index in (0..roots.len()).rev() {
            stack.push((
                entry(Some(roots), "nodes", index)?.as_u32()? as usize,
                Mat4::IDENTITY,
            ));
        }
    }

    let mut out = Mesh::default();
    while let Some((index, parent)) = stack.pop() {
        let node = entry(nodes, "nodes", index)?;
        // Checked before the children are pushed, so a cycle is a
        // refusal rather than a walk that never ends.
        if *seen.get(index).unwrap_or(&true) {
            return Err(GltfError::NodeCycle { node: index });
        }
        seen[index] = true;

        let world = parent * node_transform(node)?;
        if let Some(mesh) = node.get("mesh") {
            mesh_at(
                root,
                &views,
                &accessors,
                binary,
                mesh.as_u32()? as usize,
                world,
                &mut out,
            )?;
        }
        if let Some(children) = node.get("children") {
            for child in (0..children.len()).rev() {
                stack.push((
                    entry(Some(children), "children", child)?.as_u32()? as usize,
                    world,
                ));
            }
        }
    }

    if out.positions.is_empty() {
        return Err(GltfError::Geometry(MeshError::NoGeometry));
    }
    Ok(out)
}

/// Parse the document out of a container's JSON chunk.
///
/// # Errors
///
/// A [`GltfError`] wrapping whatever the JSON reader refused.
pub fn parse(json: &[u8]) -> Result<Json<'_>, GltfError> {
    Json::parse(json).map_err(GltfError::Document)
}
