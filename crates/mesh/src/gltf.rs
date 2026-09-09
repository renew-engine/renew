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

use crate::accessor::{Accessor, AccessorError, BufferView, Component, Shape};
use crate::error::MeshError;
use crate::glb::GlbError;

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

/// Parse the document out of a container's JSON chunk.
///
/// # Errors
///
/// A [`GltfError`] wrapping whatever the JSON reader refused.
pub fn parse(json: &[u8]) -> Result<Json<'_>, GltfError> {
    Json::parse(json).map_err(GltfError::Document)
}
