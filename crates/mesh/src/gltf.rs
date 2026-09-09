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
    /// which have no materials in them at all. The message says the value
    /// left its range; **the member is what says which range that was**,
    /// because they differ -- the factors are bounded at both ends and
    /// the alpha cutoff only below.
    ///
    /// Refused rather than clamped, unlike the material library's
    /// specular exponent, and the two differ because the formats do: that
    /// range is a convention files exceed, this one is stated by the
    /// schema.
    FactorOutOfRange {
        /// Which member, spelled as the document spells it.
        field: &'static str,
    },

    /// An image naming both a `uri` and a view, or neither.
    ///
    /// **The format states exactly one.** Its schema is a `oneOf` over
    /// the two, so an image with both sources is a document that
    /// contradicts itself and one with neither is a document that
    /// describes nothing — and a reader that picked, or that returned
    /// an image with no bytes, would be answering a question the document
    /// did not settle.
    ImageSource {
        /// Whether it named both. False means it named neither.
        ///
        /// **A bool rather than a count**, because the count could only
        /// ever be zero or two and a field with two thirds of its values
        /// unreachable is a shape this crate argues against everywhere
        /// else.
        both: bool,
    },

    /// An alpha mode this format does not define.
    ///
    /// **Its own refusal rather than [`Unsupported`](Self::Unsupported),
    /// because that one would say something false.** `Unsupported` means
    /// the construct is in the format and not in this reader, and tells
    /// a caller to convert the file; an alpha mode the format does not
    /// define is the other way round — the reader knows the member,
    /// the document's value is not one of the three, and the fix is a
    /// repair rather than a conversion.
    UnknownAlphaMode {
        /// What the document spelled, so the message can show it.
        found: Box<str>,
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
            Self::UnknownAlphaMode { .. } => "UnknownAlphaMode",
            Self::ImageSource { .. } => "ImageSource",
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
            Self::ImageSource { both } => write!(
                f,
                "an image names {} of `uri` and `bufferView`, and the format states exactly one",
                if *both { "both" } else { "neither" }
            ),
            Self::UnknownAlphaMode { found } => write!(
                f,
                "`{found}` is not one of the three alpha modes this format defines"
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

        // **The schema states a minimum of one.** A zero-length view
        // used to be somebody else's problem: every accessor over an
        // empty region is refused by the layer below, so nothing needed
        // the rule here. An image is the first reader with no accessor
        // over its bytes, and a zero-byte image with a media type is a
        // thing this reader would otherwise hand back.
        let byte_length = required(view, "byteLength")?.as_u32()?;
        if byte_length == 0 {
            return Err(GltfError::FactorOutOfRange {
                field: "byteLength",
            });
        }
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

/// A number the format bounds, checked at the width the document wrote
/// it in.
///
/// **Narrowing first would clamp where this crate promises to refuse.**
/// A `f64` one unit in the last place above the bound narrows to exactly
/// the bound in `f32`, so a check after the conversion cannot see it: the
/// document's value is out of range and the caller is handed the limit
/// with no word said. Checked wide, then narrowed.
fn bounded(
    object: Value<'_>,
    key: &'static str,
    default: f32,
    low: f64,
    high: f64,
) -> Result<f32, GltfError> {
    let Some(value) = object.get(key) else {
        return Ok(default);
    };
    let found = value.as_f64()?;
    if !(low..=high).contains(&found) {
        return Err(GltfError::FactorOutOfRange { field: key });
    }
    // The layer below refuses a number that is not finite, so anything
    // that reaches here narrows to a real value.
    Ok(value.as_f32()?)
}

/// A factor in `0..=1`, which is the range the schema states for all of
/// them.
fn factor(object: Value<'_>, key: &'static str, default: f32) -> Result<f32, GltfError> {
    bounded(object, key, default, 0.0, 1.0)
}

/// A fixed-length array of factors, each inside the stated range.
///
/// **The length is exact, not a minimum.** The schema states `minItems`
/// and `maxItems` as the same number, so a longer array is a document
/// that does not conform — and reading the first few and dropping the
/// rest would hide whatever the tail said, including a value the reader
/// would have refused.
fn factors<const N: usize>(
    object: Value<'_>,
    key: &'static str,
    default: [f32; N],
) -> Result<[f32; N], GltfError> {
    let Some(array) = object.get(key) else {
        return Ok(default);
    };
    let found = array.elements().map_err(GltfError::Document)?;
    let mut out = default;
    let mut seen = 0;
    for value in found {
        let Some(slot) = out.get_mut(seen) else {
            return Err(GltfError::FactorOutOfRange { field: key });
        };
        let wide = value.as_f64()?;
        if !(0.0..=1.0).contains(&wide) {
            return Err(GltfError::FactorOutOfRange { field: key });
        }
        *slot = value.as_f32()?;
        seen += 1;
    }
    if seen != N {
        return Err(GltfError::FactorOutOfRange { field: key });
    }
    Ok(out)
}

/// One texture reference: which texture, and which coordinate set.
///
/// **The index is not resolved here.** This reader does not read the
/// `textures` table, so the number is checked against the table's length
/// and no further — which is the most that can be said about it
/// without a reader for what it points at.
fn texture_ref(
    root: Value<'_>,
    object: Value<'_>,
    key: &str,
) -> Result<Option<TextureRef>, GltfError> {
    let Some(info) = object.get(key) else {
        return Ok(None);
    };
    Ok(Some(named(root, info)?))
}

/// The same, for an info object the caller already holds.
fn named(root: Value<'_>, info: Value<'_>) -> Result<TextureRef, GltfError> {
    info.entries().map_err(GltfError::Document)?;

    // **`index` is required on every one of these.** The normal and
    // occlusion kinds inherit it rather than restating it, which is a
    // schema arrangement and not a licence to leave it out.
    let texture = required(info, "index")?.as_u32()?;
    let rows = root.get("textures").map_or(0, Value::len);
    if texture as usize >= rows {
        return Err(GltfError::NoSuchEntry {
            table: "textures",
            index: texture as usize,
            count: rows,
        });
    }

    Ok(TextureRef {
        texture,
        uv_set: number_or(info, "texCoord", 0)?,
    })
}

/// One image, and what the document says its bytes are.
///
/// Borrowed where the bytes are a range of a buffer, owned where a
/// payload had to be decoded into them — the same split the buffers
/// table makes, for the same reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image<'a> {
    /// What the document called it, if it called it anything.
    pub name: Option<String>,
    /// The media type the document states for these bytes, if it
    /// states one.
    ///
    /// **`mimeType` wins when it is there.** An image may state its type
    /// beside itself, in its payload's URI, or in both. The format makes
    /// `mimeType` mandatory where it is the only statement there can be,
    /// and its rule about the other one is that a payload's media type
    /// must match its *content* — not that the two labels must match
    /// each other. So the labels are not compared: `mimeType` is the
    /// document's answer, and the URI's is what is left when it gives
    /// none.
    ///
    /// **`None` is the document having said nothing**, which is not the
    /// same as `Some("")` — a URI may carry an empty media type, and
    /// the decoder below reports that rather than applying RFC 2397's
    /// default, so that a caller needing an explicit type can see there
    /// was none.
    ///
    /// **Reported, never judged.** Which types are readable is a fact
    /// about what the caller is doing with the bytes, and this layer
    /// decodes nothing.
    pub media_type: Option<String>,
    /// The bytes themselves.
    pub bytes: Cow<'a, [u8]>,
}

/// Read the document's images to bytes and the type stated for them.
///
/// **Exactly one source each.** The format's schema is a `oneOf` over
/// `uri` and `bufferView`, so an image names one or the other: both is a
/// contradiction and neither describes nothing. A `uri` is a payload this
/// reader decodes or a second file it will not open, which is the same
/// pair of answers a buffer's `uri` gets.
///
/// **No image is decoded here.** An image comes back as the bytes a
/// document carried and the name it gave them; what those bytes are is
/// the caller's question, and answering it would mean an image decoder
/// this layer has no need of.
///
/// **The tables come first, which couples this to them.** A `Source`
/// is built from the whole buffer table, so a document whose *geometry*
/// lives in a second file cannot have its embedded images read even
/// though they need no buffer at all. That is a real limit rather than
/// an oversight, and the fix -- resolving a buffer only when something
/// asks for it -- is a change to how the tables are held rather than to
/// this function.
///
/// # Errors
///
/// A [`GltfError`]: `ImageSource` when an image names both sources or
/// neither; `MissingField` when a view carries no `mimeType`, which the
/// format requires there; `NoSuchEntry` when it names a view or buffer
/// the document does not have; `Accessor` when the view does not fit its
/// buffer; `Unsupported` for a view that declares a stride, which the
/// format forbids for anything but vertex and index data;
/// `ExternalResource` for a second file; `Payload` when an embedded one
/// will not decode; and `Document` for a member of the wrong kind.
pub fn images<'s>(root: Value<'_>, source: &'s Source<'_>) -> Result<Vec<Image<'s>>, GltfError> {
    let Some(table) = root.get("images") else {
        return Ok(Vec::new());
    };

    // **Nothing is reserved, because the length is the attacker's
    // number.** `with_capacity(table.len())` reserves 72 bytes for every
    // array element before one of them has been looked at, and an
    // element can be the two bytes `0,` -- measured at 36 times the
    // document, 302 MB reserved from an 8 MB input that is then refused
    // outright. Growing instead costs a handful of reallocations against
    // a per-image cost measured in hundreds of nanoseconds, and the
    // amplification that survives is over images the document really
    // named: each needs a source spelled out, so roughly 72 bytes held
    // per twenty read.
    let mut out = Vec::new();
    for entry in table.elements().map_err(GltfError::Document)? {
        // **An image is an object.** Every member of a number answers
        // absent, so without this an image that is `5` would read as one
        // naming no source at all and be refused for the wrong reason.
        entry.entries().map_err(GltfError::Document)?;

        // Stated beside a view because the format requires it there,
        // and allowed to be absent beside a URI because the payload
        // carries one of its own.
        // **An empty statement is an absence.** The decoder below
        // already reports a payload's missing type as an empty string
        // rather than applying RFC 2397's default, and `mimeType` may be
        // written `""` because the format's own schema ends in a
        // permissive `string`. Both are a document saying nothing about
        // its bytes, and the two sides normalising differently is how a
        // reader ends up reporting `Some("")` -- a type nobody can name,
        // which every caller then has to know to treat as absent.
        let declared = match entry.get("mimeType") {
            None => None,
            Some(value) => {
                let stated = value.as_str()?.decode();
                (!stated.is_empty()).then_some(stated)
            }
        };

        // **The format's `oneOf` is this match.** Written as a count and
        // a guard instead, one arm would hold a source the guard had
        // already proved was there, and would reach for it through an
        // unwrap that cannot fire -- which is the shape this crate keeps
        // finding in its own defensive code.
        let (media_type, bytes) = match (entry.get("uri"), entry.get("bufferView")) {
            (Some(_), Some(_)) => return Err(GltfError::ImageSource { both: true }),
            (None, None) => return Err(GltfError::ImageSource { both: false }),

            (None, Some(index)) => {
                let Some(stated) = declared else {
                    return Err(GltfError::MissingField { path: "mimeType" });
                };
                let index = index.as_u32()? as usize;

                // **A stride is a rule about elements, and an image is
                // not elements.** The format says a view carrying
                // anything but vertex or index data must not define one,
                // and the layer below only refuses a stride wider than
                // the region it sits in -- a different rule that would
                // let this one through.
                //
                // The row is read here and again inside `view_bytes`,
                // which is deliberate: asking first is what lets this
                // refusal come before the accessor layer's, so a view
                // that is wrong in both ways is named by the rule an
                // image cares about. A row lookup measures under two
                // nanoseconds against three hundred for the image.
                let (_, view) = row(&source.views, "bufferViews", index)?;
                if view.byte_stride.is_some() {
                    return Err(GltfError::Unsupported {
                        found: "byteStride on an image",
                    });
                }

                (Some(stated), Cow::Borrowed(source.view_bytes(index)?))
            }

            (Some(uri), None) => {
                // **Escapes resolved only when there are any.** A URI
                // needs them resolved -- a payload carries `/` and a
                // document may spell that `\/` -- but base64 has no
                // backslash in its alphabet, so the copy is pure loss on
                // every conformant image. Measured on one 4 MB texture:
                // 1.2 ms of a 9.1 ms read, and 5.6 MB of the 9.8 MB it
                // asks the allocator for.
                let spelled = uri.as_str()?;
                let text = spelled
                    .as_plain()
                    .map_or_else(|| Cow::Owned(spelled.decode()), Cow::Borrowed);
                if !data_uri::looks_like(&text) {
                    return Err(GltfError::ExternalResource);
                }
                let payload = data_uri::read(&text).map_err(GltfError::Payload)?;

                // **`mimeType` is the document's answer when it gives
                // one.** The two labels are not compared: the format
                // relates neither to the other, and its rule about a
                // payload's own type is that it match the *content*,
                // which this layer cannot check because it decodes
                // nothing. Refusing a disagreement would refuse
                // documents the format permits -- an image labelled
                // `image/png` whose payload is carried as
                // `application/octet-stream` is ordinary and conformant.
                let media_type = declared.or_else(|| {
                    (!payload.media_type.is_empty()).then(|| payload.media_type.to_owned())
                });

                // Both sides normalised the same way, so the field is
                // never `Some("")` and a caller needing a type can test
                // for one rather than for two spellings of none.
                debug_assert_ne!(media_type.as_deref(), Some(""));
                (media_type, Cow::Owned(payload.bytes))
            }
        };

        out.push(Image {
            name: match entry.get("name") {
                None => None,
                Some(value) => Some(value.as_str()?.decode()),
            },
            media_type,
            bytes,
        });
    }
    Ok(out)
}

impl Image<'_> {
    /// Take ownership of the bytes, so the image outlives the document.
    ///
    /// **The only way out of the borrow, and it is a copy where the
    /// bytes came from a buffer.** An image read from a `bufferView`
    /// borrows the document's own memory; a caller that wants to hold
    /// it after the document is dropped -- to write it to a file, say --
    /// has to pay for that once. An image decoded from a payload already
    /// owns its bytes and pays nothing.
    #[must_use]
    pub fn into_owned(self) -> Image<'static> {
        Image {
            name: self.name,
            media_type: self.media_type,
            bytes: Cow::Owned(self.bytes.into_owned()),
        }
    }
}

/// What a document says beyond its geometry.
///
/// **Two tables that travel together because one caller wants both.**
/// A tool reporting what it imported needs the materials and the images
/// at once, and the alternative -- asking for each separately -- makes
/// the caller build the container dispatch and the buffer table twice.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tables {
    /// Every material, in the vocabulary the format states them in.
    pub materials: Vec<Material>,
    /// Every image, holding its own bytes.
    pub images: Vec<Image<'static>>,
}

/// Read what a document says beyond its geometry, in either shape.
///
/// **The sibling of [`read`], and it exists for the same reason.** A
/// binary glTF wraps its document in a container beside a chunk; a
/// `.gltf` is that document on its own. Every caller of these tables
/// would otherwise write that dispatch itself -- and the two that
/// already exist got it wrong in different ways before this function
/// did it once.
///
/// **The images own their bytes**, which a borrowed form could not: the
/// parsed document lives inside this call and cannot be handed back
/// beside things that point into it. A caller that wants to avoid the
/// copy has [`images`] and can hold the parse itself.
///
/// # Errors
///
/// A [`GltfError`] naming the layer that refused and carrying its
/// numbers.
pub fn tables(bytes: &[u8]) -> Result<Tables, GltfError> {
    let (document, chunk) = if glb::looks_like(bytes) {
        let container = glb::read(bytes).map_err(GltfError::Container)?;
        (container.json, container.binary)
    } else {
        (bytes, None)
    };

    let json = parse(document)?;
    let root = json.root();
    let source = Source::of(root, chunk)?;
    Ok(Tables {
        materials: materials(root)?,
        images: images(root, &source)?
            .into_iter()
            .map(Image::into_owned)
            .collect(),
    })
}

/// Read the document's materials, in the vocabulary glTF states them.
///
/// A material object has no required members, so an empty one is legal
/// and means every default — which is why this reads defaults rather
/// than refusing absence. It must still *be* an object: a number where a
/// material belongs is not a material that said nothing.
///
/// # Errors
///
/// A [`GltfError`]: `Document` when the table, a material, its shading
/// half or a map is the wrong kind; `MissingField` when a map names no
/// texture; `NoSuchEntry` when it names one the document does not have;
/// `UnknownAlphaMode` for a mode the format does not define;
/// `FactorOutOfRange` for a factor outside the range stated for it or a
/// colour with the wrong number of components; and `Geometry` carrying
/// [`MeshError::TooLarge`] when the table is past this reader's ceiling.
pub fn materials(root: Value<'_>) -> Result<Vec<Material>, GltfError> {
    let Some(table) = root.get("materials") else {
        return Ok(Vec::new());
    };

    // Sized from what was parsed, not from a number the document
    // declared -- a material is a wide row and doubling into it
    // copies twice the table before it settles.
    let mut out = Vec::with_capacity(table.len());
    for entry in table.elements().map_err(GltfError::Document)? {
        // **A material is an object.** `Value::get` answers `None` for
        // every member of a number, a string or an array, so a table of
        // those would read as a table of materials that each said
        // nothing -- and this reader's own rule would then hand back a
        // full default material for a document that described none.
        entry.entries().map_err(GltfError::Document)?;

        // **The ceiling this crate's amplification rule asks for.** A
        // two-byte array element is a whole material, so a document far
        // under the geometry ceiling can ask for more than it.
        crate::refuse_over_material_ceiling(out.len()).map_err(GltfError::Geometry)?;

        // **Read whatever the mode, kept only where it means something.**
        // The schema bounds the cutoff whether or not the mode uses it,
        // and forbids it outright when no mode is named -- so validating
        // it inside the masked arm alone would let two non-conformant
        // shapes through.
        let cutoff = bounded(entry, "alphaCutoff", 0.5, 0.0, f64::INFINITY)?;
        let alpha = match entry.get("alphaMode") {
            None => {
                if entry.get("alphaCutoff").is_some() {
                    return Err(GltfError::FactorOutOfRange {
                        field: "alphaCutoff",
                    });
                }
                Alpha::Opaque
            }
            Some(mode) => {
                // **Compared without building a string.** The text layer
                // answers `eq_str` against the document's own bytes,
                // escapes and all; decoding first would allocate a
                // `String` per material to compare three constants and
                // throw it away.
                let spelled = mode.as_str()?;
                if spelled.eq_str("OPAQUE") {
                    Alpha::Opaque
                } else if spelled.eq_str("BLEND") {
                    Alpha::Blend
                } else if spelled.eq_str("MASK") {
                    Alpha::Mask { cutoff }
                } else {
                    // Decoded only on the path that reports it.
                    return Err(GltfError::UnknownAlphaMode {
                        found: spelled.decode().into(),
                    });
                }
            }
        };

        let normal = entry.get("normalTexture");
        let occlusion = entry.get("occlusionTexture");

        let mut material = Material {
            name: match entry.get("name") {
                None => None,
                Some(value) => Some(value.as_str()?.decode()),
            },
            emissive: factors(entry, "emissiveFactor", [0.0; 3])?,
            alpha,
            double_sided: match entry.get("doubleSided") {
                None => false,
                Some(value) => value.as_bool()?,
            },
            // **Fetched once each.** A member lookup walks the whole
            // object, so asking for the same one twice -- as reading the
            // map and then its scale off it separately would -- pays for
            // the walk twice per material.
            normal_map: match normal {
                None => None,
                Some(info) => Some(NormalTexture {
                    map: named(root, info)?,
                    // Unbounded: the schema states no range for it.
                    scale: match info.get("scale") {
                        None => 1.0,
                        Some(value) => value.as_f32()?,
                    },
                }),
            },
            occlusion_map: match occlusion {
                None => None,
                Some(info) => Some(OcclusionTexture {
                    map: named(root, info)?,
                    strength: factor(info, "strength", 1.0)?,
                }),
            },
            emissive_map: texture_ref(root, entry, "emissiveTexture")?,
            ..Material::default()
        };

        // **The shading half, when there is one, and it must be an
        // object too.** Absent, every member of it keeps the default the
        // type already carries -- which is why this is one branch rather
        // than a guard repeated per member.
        if let Some(shading) = entry.get("pbrMetallicRoughness") {
            shading.entries().map_err(GltfError::Document)?;
            material.base_color = factors(shading, "baseColorFactor", [1.0; 4])?;
            material.metallic = factor(shading, "metallicFactor", 1.0)?;
            material.roughness = factor(shading, "roughnessFactor", 1.0)?;
            material.base_color_map = texture_ref(root, shading, "baseColorTexture")?;
            material.metallic_roughness_map =
                texture_ref(root, shading, "metallicRoughnessTexture")?;
        }

        out.push(material);
    }
    Ok(out)
}

/// Which material each primitive of one mesh names.
///
/// **Reported rather than stored.** A [`Mesh`] carries geometry and has
/// nowhere to put a material index; giving it one would change the
/// canonical form, its version question and everything that reads it, for
/// a value nothing in this engine can yet use. A caller that wants the
/// pairing asks for it here.
///
/// **Every primitive in one pass**, rather than one lookup per
/// primitive. An index into a document array walks that array from its
/// head, so asking per primitive -- which is what a caller pairing a mesh
/// with its surfaces does -- costs the square of the count. This is the
/// same shape `buffer_views` and `accessors` are read in, for the same
/// reason.
///
/// # Errors
///
/// A [`GltfError`] naming the table an index missed, the member that was
/// the wrong type, or the material the document does not have.
pub fn primitive_materials(root: Value<'_>, mesh: usize) -> Result<Vec<Option<u32>>, GltfError> {
    let row = entry(root.get("meshes"), "meshes", mesh)?;
    let Some(primitives) = row.get("primitives") else {
        return Ok(Vec::new());
    };
    let rows = root.get("materials").map_or(0, Value::len);

    let mut out = Vec::new();
    for found in primitives.elements().map_err(GltfError::Document)? {
        out.push(match found.get("material") {
            None => None,
            Some(value) => {
                let named = value.as_u32()?;
                if named as usize >= rows {
                    return Err(GltfError::NoSuchEntry {
                        table: "materials",
                        index: named as usize,
                        count: rows,
                    });
                }
                Some(named)
            }
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

    /// The bytes one view addresses, with no accessor over them.
    ///
    /// An image stored in the document is a view and a media type: the
    /// bytes are a whole file rather than a typed stream, so nothing
    /// here reads elements out of them.
    ///
    /// # Errors
    ///
    /// A [`GltfError`] naming the table an index missed, or the accessor
    /// layer's refusal when the view does not fit its buffer.
    pub fn view_bytes(&self, index: usize) -> Result<&[u8], GltfError> {
        let (buffer, view) = row(&self.views, "bufferViews", index)?;
        Ok(view.resolve(self.bytes(buffer)?)?)
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
