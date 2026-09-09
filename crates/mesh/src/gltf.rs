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
//! # What is decoded, and what is refused rather than fetched
//!
//! A buffer may name a `uri`, and the two things that can be are treated
//! differently. **A `data:` URI is decoded in place** through
//! [`crate::data_uri`], which is how a document that travels as one file
//! carries its geometry. **Anything else names a second file**, and
//! opening one is not something this crate does -- so it is refused by
//! name, and the caller knows to convert the file rather than wondering
//! what it got.
//!
//! A buffer with no `uri` at all is the container's own binary chunk,
//! and only the first buffer may be: the specification leaves any other
//! sourceless buffer undefined, and undefined is refused here.

use renew_json::{Json, JsonError, Value};
use renew_math::{Mat4, Quat, Vec3, Vec4};

use std::borrow::Cow;

use crate::accessor::{Accessor, AccessorError, BufferView, Component, Indices, Shape, View};
use crate::data_uri::{self, DataUriError};
use crate::error::MeshError;
use crate::glb::GlbError;
use crate::pbr::{Alpha, Material, NormalTexture, OcclusionTexture, TextureRef};
use crate::primitive::{self, Mode, Primitive};
use crate::{Mesh, glb, place};

/// The two media types a buffer's embedded payload may declare.
///
/// The specification names exactly these two and no others, which is why
/// a third is a refusal that can say what it found rather than a shrug.
const GLTF_BUFFER: &str = "application/gltf-buffer";
const OCTET_STREAM: &str = "application/octet-stream";

/// Whether these bytes are a glTF document rather than a container.
///
/// **This parses, and that is the point.** Every other format here is
/// recognised by a magic number or a keyword, and a JSON document has
/// neither: "starts with `{`" would claim every configuration file in
/// the world. So the question asked is the one the format answers -- a
/// glTF document **must** carry an `asset` object with a `version`
/// string, and nothing that is not one will have that where this looks.
///
/// The cost is a parse of the whole document before anything reads it,
/// which is real and is the right trade: the alternative is a confident
/// answer about the wrong format, which is the defect a prefix check
/// produced here once already.
#[must_use]
pub fn looks_like(bytes: &[u8]) -> bool {
    // **Bounded before it is thorough.** The parse below reads the whole
    // input, and every other arm of the dispatch this feeds is
    // deliberately bounded and says so -- so without this line a fifty
    // megabyte OBJ pays a full scan to be told it is not JSON, and pays
    // it before the cheap check that would have recognised it.
    //
    // The two answers are the same. A document's root is an object, so
    // when the first byte that is not whitespace is not `{`, either the
    // parse fails or the root is not an object, and asking a non-object
    // for a member answers `None` either way.
    let Some(first) = bytes.iter().position(|byte| !byte.is_ascii_whitespace()) else {
        return false;
    };
    if bytes.get(first) != Some(&b'{') {
        return false;
    }

    Json::parse(bytes).is_ok_and(|json| {
        json.root()
            .get("asset")
            .and_then(|asset| asset.get("version"))
            .is_some_and(|version| version.as_str().is_ok())
    })
}

/// Every way a document can fail to describe geometry this can read.
///
/// **Five of these carry another layer's refusal**, and that is the
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

    /// A payload embedded in the document that will not decode.
    ///
    /// The layer below names which rule the text broke and where.
    Payload(DataUriError),

    /// A buffer with no `uri` that is not the container's own chunk.
    ///
    /// **Only the first buffer may be the chunk.** The specification
    /// says of any other buffer with no source that its behaviour "is
    /// left undefined to accommodate future extensions", and undefined
    /// behaviour is a refusal here rather than a guess: the alternative
    /// is handing a view the wrong bytes and calling the result
    /// geometry.
    BufferWithoutSource {
        /// Which buffer, by its index in the document's own table.
        buffer: usize,
    },

    /// The document wants the container's chunk and there is none.
    ///
    /// A container whose document has a buffer with no `uri` **must**
    /// carry a binary chunk. A document read on its own never can, which
    /// is the ordinary way to meet this.
    NoBinaryChunk,

    /// A `data:` URI whose media type is not one a buffer may declare.
    WrongMediaType {
        /// What the URI said, so the message can show it.
        found: Box<str>,
    },

    /// A resource shorter than the buffer that names it.
    ///
    /// The specification allows a resource to be **longer** — only
    /// the first `byteLength` bytes belong to the buffer — and
    /// requires it to be at least that long. Shorter means the document
    /// and its payload disagree about what is there.
    BufferTooShort {
        /// Which buffer, by its index in the document's own table.
        buffer: usize,
        /// What the document said the buffer holds.
        declared: usize,
        /// What the resource actually holds.
        available: usize,
    },

    /// A material factor outside the range the format states for it.
    ///
    /// **The member is named and the value is not carried**, which is a
    /// trade rather than an oversight: a factor is a float, this
    /// vocabulary is compared for equality, and a float would cost every
    /// refusal in it that property — including the geometry ones,
    /// which have no materials in them at all. The message states the
    /// range, and the member names where to look.
    ///
    /// Refused rather than clamped, unlike the material library's
    /// specular exponent, and the two differ because the formats do: that
    /// range is a convention files exceed, this one is stated by the
    /// schema.
    FactorOutOfRange {
        /// Which member, spelled as the document spells it.
        field: &'static str,
    },

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
            Self::Payload(_) => "Payload",
            Self::BufferWithoutSource { .. } => "BufferWithoutSource",
            Self::NoBinaryChunk => "NoBinaryChunk",
            Self::WrongMediaType { .. } => "WrongMediaType",
            Self::BufferTooShort { .. } => "BufferTooShort",
            Self::FactorOutOfRange { .. } => "FactorOutOfRange",
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
            Self::Payload(refusal) => write!(f, "an embedded payload will not decode: {refusal}"),
            Self::BufferWithoutSource { buffer } => write!(
                f,
                "buffer {buffer} names no source, and only the first buffer may be the \
                 container's own"
            ),
            Self::NoBinaryChunk => write!(
                f,
                "a buffer wants the container's binary chunk and there is no such chunk"
            ),
            Self::WrongMediaType { found } => write!(
                f,
                "a buffer's payload declares `{found}`, and a buffer may declare only \
                 `{GLTF_BUFFER}` or `{OCTET_STREAM}`"
            ),
            Self::BufferTooShort {
                buffer,
                declared,
                available,
            } => write!(
                f,
                "buffer {buffer} declares {declared} bytes and its resource holds {available}"
            ),
            Self::FactorOutOfRange { field } => {
                write!(f, "`{field}` is outside the range the format states for it")
            }
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
    // **`ok_or_else`, not `ok_or`.** `Value::len` counts the table's
    // children, so building the refusal eagerly counts every row of
    // every table on every *successful* lookup -- which is the larger
    // half of what made reading a table quadratic.
    table.index(index).ok_or_else(|| GltfError::NoSuchEntry {
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
pub fn buffer_views(root: Value<'_>) -> Result<Vec<(usize, BufferView)>, GltfError> {
    let Some(table) = root.get("bufferViews") else {
        // A document with no views has no geometry to point at, which
        // the caller finds out when it asks for a primitive.
        return Ok(Vec::new());
    };
    // **One pass, not one walk per row.** A `Value`'s index restarts
    // from the head of the array, and its own documentation says a layer
    // that wants a table it will hit thousands of times should build one
    // in a single pass. This is that layer, and this is that pass.
    let mut out = Vec::new();
    for view in table.elements().map_err(GltfError::Document)? {
        // **Required, and with no default of its own.** `byteOffset` may
        // be absent and mean zero because the format says so; `buffer`
        // may not. Defaulting it was harmless while every buffer but the
        // first was refused -- absent and zero named the same bytes --
        // and became a wrong answer the moment a second buffer could be
        // read, because a view naming none would silently be handed the
        // first one's.
        //
        // The buffer travels beside the view from here, the way an
        // accessor already carries the view it reads through.
        let buffer = required(view, "buffer")?.as_u32()? as usize;

        let byte_length = required(view, "byteLength")?.as_u32()?;
        out.push((
            buffer,
            BufferView {
                byte_offset: number_or(view, "byteOffset", 0)? as usize,
                byte_length: byte_length as usize,
                byte_stride: match view.get("byteStride") {
                    None => None,
                    Some(stride) => Some(stride.as_u32()? as usize),
                },
            },
        ));
    }
    Ok(out)
}

/// The bytes of every buffer the document names.
///
/// Borrowed where a buffer is the container's own chunk, owned where a
/// `data:` URI had to be decoded into one. Nothing here opens a file:
/// a buffer naming a second resource is refused, because fetching it is
/// the caller's business and this crate does not have one.
///
/// # A buffer is its resource, cut to the length it declares
///
/// The specification is explicit that a resource **may be longer** than
/// the buffer that names it, and that only the range from zero to
/// `byteLength` is referenced. That is not a technicality: the container
/// pads its binary chunk to a four-byte boundary, so the chunk is longer
/// than the buffer inside it **whenever that buffer's length is not a
/// multiple of four** -- which the corpus's fully furnished document
/// happens to be, at a hundred and two bytes in a hundred-and-four-byte
/// chunk. Cutting here is what stops a view reaching past the buffer
/// into that padding and being handed bytes the document never claimed.
///
/// # Errors
///
/// A [`GltfError`] naming what was wrong: a source this crate will not
/// fetch, or -- naming the buffer as well -- no source at all, no chunk
/// for it to be, a media type a buffer may not declare, a payload that
/// will not decode, or a resource shorter than the length declared for
/// it.
pub fn buffers<'a>(
    root: Value<'_>,
    binary: Option<&'a [u8]>,
) -> Result<Vec<Cow<'a, [u8]>>, GltfError> {
    let Some(table) = root.get("buffers") else {
        // A document with no buffers has no geometry to point at, which
        // the caller finds out when it asks for a primitive.
        return Ok(Vec::new());
    };

    let mut out: Vec<Cow<'a, [u8]>> = Vec::new();
    for (index, buffer) in table.elements().map_err(GltfError::Document)?.enumerate() {
        let declared = required(buffer, "byteLength")?.as_u32()? as usize;

        let resource: Cow<'a, [u8]> = match buffer.get("uri") {
            None => {
                // **Only the first buffer may be the container's own.**
                if index != 0 {
                    return Err(GltfError::BufferWithoutSource { buffer: index });
                }
                Cow::Borrowed(binary.ok_or(GltfError::NoBinaryChunk)?)
            }
            Some(uri) => {
                // **Escapes resolved first, not the borrowed fast path.**
                // A base64 payload contains `/`, and a document is free
                // to spell that `\/` -- so the plain form is absent for
                // exactly the URIs this needs to read.
                let text = uri.as_str()?.decode();
                if !data_uri::looks_like(&text) {
                    return Err(GltfError::ExternalResource);
                }
                let payload = data_uri::read(&text).map_err(GltfError::Payload)?;
                // **Compared without case.** RFC 2045 says a media type is
                // not case sensitive, and the decoder one file over
                // already forgives the marker's case under its own rule
                // that a difference which cannot change an output byte is
                // not a difference. Two readers of one URI should not
                // disagree about that.
                if !payload.media_type.eq_ignore_ascii_case(GLTF_BUFFER)
                    && !payload.media_type.eq_ignore_ascii_case(OCTET_STREAM)
                {
                    return Err(GltfError::WrongMediaType {
                        found: payload.media_type.into(),
                    });
                }
                Cow::Owned(payload.bytes)
            }
        };

        if resource.len() < declared {
            return Err(GltfError::BufferTooShort {
                buffer: index,
                declared,
                available: resource.len(),
            });
        }

        out.push(match resource {
            Cow::Borrowed(bytes) => Cow::Borrowed(&bytes[..declared]),
            Cow::Owned(mut bytes) => {
                bytes.truncate(declared);
                Cow::Owned(bytes)
            }
        });
    }
    Ok(out)
}

/// A number that must sit inside the range the format states.
fn factor(object: Value<'_>, key: &'static str, default: f32) -> Result<f32, GltfError> {
    let Some(value) = object.get(key) else {
        return Ok(default);
    };
    let found = value.as_f32()?;
    if !(0.0..=1.0).contains(&found) {
        return Err(GltfError::FactorOutOfRange { field: key });
    }
    Ok(found)
}

/// A fixed-length array of factors, each inside the stated range.
fn factors<const N: usize>(
    object: Value<'_>,
    key: &'static str,
    default: [f32; N],
) -> Result<[f32; N], GltfError> {
    let read = numbers::<N>(object, key, default)?;
    for component in read {
        if !(0.0..=1.0).contains(&component) {
            return Err(GltfError::FactorOutOfRange { field: key });
        }
    }
    Ok(read)
}

/// One texture reference: which texture, and which coordinate set.
fn texture_ref(object: Value<'_>, key: &str) -> Result<Option<TextureRef>, GltfError> {
    let Some(info) = object.get(key) else {
        return Ok(None);
    };
    // **`index` is required on every one of these.** The normal and
    // occlusion kinds inherit it rather than restating it, which is a
    // schema arrangement and not a licence to leave it out.
    Ok(Some(TextureRef {
        texture: required(info, "index")?.as_u32()? as usize,
        uv_set: number_or(info, "texCoord", 0)? as usize,
    }))
}

/// Read the document's materials, in the vocabulary glTF states them.
///
/// A material object has no required members, so an empty one is legal
/// and means every default — which is why this reads defaults rather
/// than refusing absence.
///
/// # Errors
///
/// A [`GltfError`] naming the member that was the wrong type, named a
/// texture without saying which, spelled an alpha mode this format does
/// not have, or carried a factor outside the range stated for it.
pub fn materials(root: Value<'_>) -> Result<Vec<Material>, GltfError> {
    let Some(table) = root.get("materials") else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for entry in table.elements().map_err(GltfError::Document)? {
        let pbr = entry.get("pbrMetallicRoughness");
        let shading = pbr.unwrap_or(entry);

        // **The alpha mode carries its cutoff or it does not.** The
        // document states the number whatever the mode is; putting it on
        // the one variant that uses it is what stops a caller reading a
        // threshold that means nothing.
        let alpha = match entry.get("alphaMode") {
            None => Alpha::Opaque,
            Some(mode) => {
                let spelled = mode.as_str()?.decode();
                match spelled.as_str() {
                    "OPAQUE" => Alpha::Opaque,
                    "BLEND" => Alpha::Blend,
                    "MASK" => Alpha::Mask {
                        // Bounded below by zero and above by nothing,
                        // which is what the schema states.
                        cutoff: match entry.get("alphaCutoff") {
                            None => 0.5,
                            Some(value) => {
                                let found = value.as_f32()?;
                                if found < 0.0 || !found.is_finite() {
                                    return Err(GltfError::FactorOutOfRange {
                                        field: "alphaCutoff",
                                    });
                                }
                                found
                            }
                        },
                    },
                    _ => {
                        return Err(GltfError::Unsupported { found: "alphaMode" });
                    }
                }
            }
        };

        out.push(Material {
            name: match entry.get("name") {
                None => None,
                Some(value) => Some(value.as_str()?.decode()),
            },
            base_color: if pbr.is_some() {
                factors(shading, "baseColorFactor", [1.0; 4])?
            } else {
                [1.0; 4]
            },
            metallic: if pbr.is_some() {
                factor(shading, "metallicFactor", 1.0)?
            } else {
                1.0
            },
            roughness: if pbr.is_some() {
                factor(shading, "roughnessFactor", 1.0)?
            } else {
                1.0
            },
            emissive: factors(entry, "emissiveFactor", [0.0; 3])?,
            alpha,
            double_sided: match entry.get("doubleSided") {
                None => false,
                Some(value) => value.as_bool()?,
            },
            base_color_map: if pbr.is_some() {
                texture_ref(shading, "baseColorTexture")?
            } else {
                None
            },
            metallic_roughness_map: if pbr.is_some() {
                texture_ref(shading, "metallicRoughnessTexture")?
            } else {
                None
            },
            normal_map: match texture_ref(entry, "normalTexture")? {
                None => None,
                Some(map) => Some(NormalTexture {
                    map,
                    // Unbounded: the schema states no range for it.
                    scale: match entry
                        .get("normalTexture")
                        .and_then(|info| info.get("scale"))
                    {
                        None => 1.0,
                        Some(value) => value.as_f32()?,
                    },
                }),
            },
            occlusion_map: match texture_ref(entry, "occlusionTexture")? {
                None => None,
                Some(map) => Some(OcclusionTexture {
                    map,
                    strength: match entry.get("occlusionTexture") {
                        None => 1.0,
                        Some(info) => factor(info, "strength", 1.0)?,
                    },
                }),
            },
            emissive_map: texture_ref(entry, "emissiveTexture")?,
        });
    }
    Ok(out)
}

/// Which material a primitive names, if it names one.
///
/// **Reported rather than stored.** A [`Mesh`] carries geometry and has
/// nowhere to put a material index; giving it one would change the
/// canonical form, its version question and everything that reads it, for
/// a value nothing in this engine can yet use. A caller that wants the
/// pairing asks for it here.
///
/// # Errors
///
/// A [`GltfError`] naming the table an index missed, or the member that
/// was the wrong type.
pub fn primitive_material(
    root: Value<'_>,
    mesh: usize,
    index: usize,
) -> Result<Option<usize>, GltfError> {
    let entry_row = entry(root.get("meshes"), "meshes", mesh)?;
    let found = entry(entry_row.get("primitives"), "primitives", index)?;
    match found.get("material") {
        None => Ok(None),
        Some(value) => Ok(Some(value.as_u32()? as usize)),
    }
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

/// Everything a primitive needs in order to find its bytes.
///
/// The three tables are always read together and always in the same
/// order — an accessor names a view, a view names a buffer — so
/// they travel as one value rather than as three arguments threaded
/// through every function that only passes them along.
pub struct Source<'a> {
    /// Every view, each beside the buffer it points at.
    pub views: Vec<(usize, BufferView)>,
    /// Every accessor, each beside the view it points at.
    pub accessors: Vec<(usize, Accessor)>,
    /// Every buffer's bytes, cut to the length it declared.
    pub buffers: Vec<Cow<'a, [u8]>>,
}

impl<'a> Source<'a> {
    /// Read all three tables out of a document.
    ///
    /// `binary` is the container's chunk when there is one. A document
    /// read on its own has none, and any buffer that wanted it is
    /// refused by name rather than silently given nothing.
    ///
    /// # Errors
    ///
    /// A [`GltfError`] from whichever table was wrong first.
    pub fn of(root: Value<'_>, binary: Option<&'a [u8]>) -> Result<Self, GltfError> {
        Ok(Self {
            views: buffer_views(root)?,
            accessors: accessors(root)?,
            buffers: buffers(root, binary)?,
        })
    }

    /// The bytes of one buffer.
    fn bytes(&self, index: usize) -> Result<&[u8], GltfError> {
        self.buffers
            .get(index)
            .map(|buffer| &**buffer)
            .ok_or(GltfError::NoSuchEntry {
                table: "buffers",
                index,
                count: self.buffers.len(),
            })
    }

    /// One accessor and the bytes it addresses.
    ///
    /// **The single place the three tables cross.** An accessor names a
    /// view, the view names a buffer and a region of it, and the stride
    /// belongs to the view rather than to the accessor — so this is
    /// also the one place that stride is filled in.
    fn resolved(&self, index: usize) -> Result<(Accessor, &[u8]), GltfError> {
        let (which, accessor) = row(&self.accessors, "accessors", index)?;
        let (buffer, view) = row(&self.views, "bufferViews", which)?;
        let region = view.resolve(self.bytes(buffer)?)?;
        Ok((
            Accessor {
                byte_stride: view.byte_stride,
                ..accessor
            },
            region,
        ))
    }

    /// An attribute stream, validated against the bytes it addresses.
    fn stream(&self, index: usize) -> Result<View<'_>, GltfError> {
        let (accessor, region) = self.resolved(index)?;
        Ok(accessor.view(region)?)
    }

    /// An index stream, validated against the bytes it addresses.
    fn order(&self, index: usize) -> Result<Indices<'_>, GltfError> {
        let (accessor, region) = self.resolved(index)?;
        Ok(accessor.indices(region)?)
    }

    /// An optional attribute, by the name the document spells it with.
    fn optional_stream(
        &self,
        attributes: Value<'_>,
        name: &str,
    ) -> Result<Option<View<'_>>, GltfError> {
        let Some(value) = attributes.get(name) else {
            return Ok(None);
        };
        let index = value.as_u32()? as usize;
        Ok(Some(self.stream(index)?))
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
    source: &Source<'_>,
    mesh: usize,
    index: usize,
) -> Result<Mesh, GltfError> {
    let meshes = root.get("meshes");
    let entry_row = entry(meshes, "meshes", mesh)?;
    let primitives = entry_row.get("primitives");
    let found = entry(primitives, "primitives", index)?;

    let attributes = required(found, "attributes")?;
    let positions = source.stream(required(attributes, "POSITION")?.as_u32()? as usize)?;

    // **The default is triangles and it is the format's**, not this
    // reader's convenience: a primitive with no `mode` is a triangle
    // list, and a reader that refused one would reject most of the files
    // in the world.
    let mode = Mode::from_code(number_or(found, "mode", 4)?)?;

    let indices = match found.get("indices") {
        None => None,
        Some(value) => Some(source.order(value.as_u32()? as usize)?),
    };

    Ok(primitive::build(&Primitive {
        mode,
        positions,
        normals: source.optional_stream(attributes, "NORMAL")?,
        texcoords: source.optional_stream(attributes, "TEXCOORD_0")?,
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
    source: &Source<'_>,
    index: usize,
    world: Mat4,
    out: &mut Mesh,
) -> Result<(), GltfError> {
    let count = entry(root.get("meshes"), "meshes", index)?
        .get("primitives")
        .map_or(0, Value::len);
    for which in 0..count {
        let mut piece = primitive(root, source, index, which)?;
        place::place(&mut piece, world)?;
        if out.positions.is_empty() {
            *out = piece;
        } else {
            place::append(out, &piece)?;
        }
    }
    Ok(())
}

/// Read a whole binary glTF, or a document on its own, into one mesh.
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
    // **Either shape of the same asset.** A binary glTF wraps its
    // document in a container beside a chunk of geometry; a `.gltf` is
    // that document on its own, carrying its geometry as embedded
    // payloads. The layers below this line cannot tell the difference
    // and do not need to: one of them has a chunk to offer and the
    // other has none.
    if glb::looks_like(bytes) {
        let container = glb::read(bytes).map_err(GltfError::Container)?;
        let json = parse(container.json)?;
        return document(json.root(), container.binary);
    }
    let json = parse(bytes)?;
    document(json.root(), None)
}

/// Read a parsed document, with the container's chunk if there was one.
///
/// # Errors
///
/// A [`GltfError`] naming the layer that refused and carrying its
/// numbers.
pub fn document(root: Value<'_>, binary: Option<&[u8]>) -> Result<Mesh, GltfError> {
    let source = Source::of(root, binary)?;

    // **A document with no scenes is a library rather than a model**,
    // which is the format's own reading of it, and a caller asking for
    // geometry is asking the wrong question of it.
    let scenes = root
        .get("scenes")
        .ok_or(GltfError::MissingField { path: "scenes" })?;
    // `scene` says which one to show and is optional; when it is absent
    // a client may choose, and this chooses the first.
    let which = number_or(root, "scene", 0)? as usize;
    let scene = entry(Some(scenes), "scenes", which)?;

    let nodes = root.get("nodes");
    let node_count = nodes.map_or(0, Value::len);
    let mut seen = vec![false; node_count];
    let mut stack: Vec<(usize, Mat4)> = Vec::new();

    if let Some(roots) = scene.get("nodes") {
        for index in (0..roots.len()).rev() {
            let node = entry(Some(roots), "nodes", index)?.as_u32()? as usize;
            stack.push((node, Mat4::IDENTITY));
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
            mesh_at(root, &source, mesh.as_u32()? as usize, world, &mut out)?;
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
