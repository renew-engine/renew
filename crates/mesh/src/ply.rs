//! The PLY reader: an ASCII header describing a body that may not be.
//!
//! # The format
//!
//! A PLY file opens with a header that is always ASCII, however the body
//! is written:
//!
//! ```text
//! ply
//! format ascii 1.0
//! comment made by something
//! element vertex 8
//! property float x
//! property float y
//! property float z
//! element face 12
//! property list uchar int vertex_indices
//! end_header
//! ```
//!
//! Then the elements, in the order the header declared them, each with
//! the properties the header gave it. **The header is a schema and the
//! body is rows against it**, which is what makes PLY worth reading: a
//! file describes its own layout, so a reader that follows the header
//! reads files nobody anticipated.
//!
//! It is also what makes PLY worth being careful about. The header is
//! attacker-controlled arithmetic: a count, a property list and a type
//! width multiply into an offset, and a reader that trusts the product
//! reads wherever the file asks it to.
//!
//! # What this reader takes and leaves
//!
//! It reads the `vertex` element's `x`, `y` and `z`, and the `face`
//! element's index list. **Everything else in the file is skipped by
//! width rather than parsed**, which is the whole benefit of the header
//! being a schema: colours, confidence values, texture coordinates and
//! elements this reader has never heard of cost their own size in bytes
//! and nothing else.
//!
//! Normals are not read even where a file has them, and that is a
//! decision rather than an omission: PLY stores them per vertex, and
//! [`Mesh`] carries one per triangle because that is what the formats
//! reaching it so far provide. Reading them would mean choosing which
//! of a triangle's three to keep.
//!
//! # What it will not do
//!
//! **Triangulate anything but a fan.** A face with more than three
//! corners becomes triangles `(0, i, i+1)`, which is correct for a
//! convex polygon and wrong for a concave one. Real PLY files are
//! overwhelmingly triangles and quads from a subdivision surface, both
//! convex; a general tessellation needs an ear-clipping pass and a
//! decision about what to do with a self-intersecting face, which is
//! more than a reader should decide alone.
//!
//! [`Mesh`]: crate::Mesh

use crate::Mesh;
use crate::error::{MeshError, quoted};
use crate::refuse_over_ceiling;

/// How the body after the header is written.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Ascii,
    BinaryLittle,
    BinaryBig,
}

/// A scalar type a property can have, and the width it occupies.
///
/// **The names are doubled because the format doubles them.** PLY's own
/// specification uses `float`/`float32`, `uchar`/`uint8` and so on
/// interchangeably, and real files use both spellings, sometimes in the
/// same header.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "char" | "int8" => Self::I8,
            "uchar" | "uint8" => Self::U8,
            "short" | "int16" => Self::I16,
            "ushort" | "uint16" => Self::U16,
            "int" | "int32" => Self::I32,
            "uint" | "uint32" => Self::U32,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            // A type this reader does not know is refused rather than
            // guessed at: guessing its width would misalign every
            // property after it in a binary body.
            _ => return None,
        })
    }

    fn width(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }
}

/// One property of one element: either a scalar or a counted list.
#[derive(Clone)]
struct Property {
    name: String,
    kind: PropertyKind,
}

#[derive(Clone, Copy)]
enum PropertyKind {
    Scalar(Scalar),
    /// A list, whose length is read as `count` and whose entries are
    /// `item`.
    List {
        count: Scalar,
        item: Scalar,
    },
}

/// One element: a name, how many rows of it there are, and its schema.
struct Element {
    name: String,
    count: u64,
    properties: Vec<Property>,
}

impl Element {
    /// The fewest bytes one row of this element can occupy.
    ///
    /// **The number that makes a row count checkable.** A count is the
    /// third attacker-controlled number in a PLY header, after the
    /// element and property counts, and it was the one with no ceiling
    /// — so a header could declare eighteen quintillion rows of an
    /// element carrying no properties, each row consuming nothing, and
    /// the reader would spin forever on a two-hundred-byte file. A row
    /// cannot be narrower than this, so a count multiplied by it and
    /// compared against the bytes actually present refuses that file
    /// before a single row is read.
    ///
    /// **Zero is the answer that matters.** An element with no
    /// properties has a row width of nothing, so the bound below says
    /// the body can supply no rows of it, and any count at all is
    /// refused. That is the case that hung.
    ///
    /// In a binary body a scalar occupies its declared width and a list
    /// occupies at least its count field. In a text body every property
    /// needs at least one character, so the property count is the floor.
    fn least_row_bytes(&self, binary: bool) -> usize {
        if !binary {
            return self.properties.len();
        }
        self.properties
            .iter()
            .map(|property| match property.kind {
                PropertyKind::Scalar(scalar) => scalar.width(),
                PropertyKind::List { count, .. } => count.width(),
            })
            .sum()
    }
}

/// Refuse an element declaring more rows than the bytes could hold.
///
/// This is the pack reader's rule applied one level in: **a count is
/// reported by the file and believed by nobody**, and the bytes the
/// caller already holds are what bound it. It is also the only thing
/// standing between this reader and a header that asks it to run
/// forever.
fn refuse_impossible_count(element: &Element, body: usize, binary: bool) -> Result<(), MeshError> {
    let least = element.least_row_bytes(binary);
    // `None` when a row is zero bytes wide, which is the element that
    // hung: no quantity of nothing is supplied by any body, so it
    // reads as a capacity of zero and every count is refused.
    let could_supply = body.checked_div(least).unwrap_or(0);
    if element.count > could_supply as u64 {
        return Err(MeshError::CountMismatch {
            declared: element.count.saturating_mul(least as u64),
            actual: body,
            count: u32::try_from(element.count).unwrap_or(u32::MAX),
        });
    }
    Ok(())
}

/// The most elements, properties or list entries a header may declare.
///
/// **A header is a schema an attacker writes.** Without a ceiling, a
/// twenty-byte header can declare four billion properties and a reader
/// that reserved for them is out of memory before it has read a row.
/// The bound is generous against real files — the largest PLY schemas in
/// the wild run to a few dozen properties — and stingy against a file
/// whose only purpose is to be large.
const MAX_SCHEMA: usize = 1024;

/// The most corners a single face may name.
///
/// Same reasoning one level down: a face's corner count is read from the
/// body, so it is also attacker-controlled, and a fan over four billion
/// corners is four billion triangles from one row.
const MAX_FACE_CORNERS: u16 = 1024;

/// Read a PLY file.
///
/// # Errors
///
/// Every way the bytes can fail to be a PLY this reader can use, by
/// name — see [`MeshError`]. A file that reads has at least one
/// triangle, a position count that is a multiple of three, and no
/// coordinate that is not a finite number.
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    let (elements, encoding, body_at) = header(bytes)?;
    let body = bytes.get(body_at..).unwrap_or(&[]);
    match encoding {
        Encoding::Ascii => ascii_body(&elements, body),
        Encoding::BinaryLittle => binary_body(&elements, body, false),
        Encoding::BinaryBig => binary_body(&elements, body, true),
    }
}

/// Whether these bytes open the way a PLY does.
///
/// The magic word and nothing more. PLY has one, which is what lets this
/// reader say "not a PLY" where the STL reader cannot.
#[must_use]
pub fn looks_like(bytes: &[u8]) -> bool {
    let mut cursor = bytes;
    while cursor.first().is_some_and(u8::is_ascii_whitespace) {
        cursor = cursor.get(1..).unwrap_or(&[]);
    }
    cursor.starts_with(b"ply")
}

/// Read the header, returning the schema, the encoding, and where the
/// body starts.
fn header(bytes: &[u8]) -> Result<(Vec<Element>, Encoding, usize), MeshError> {
    // The header is ASCII by definition, so it is found by searching the
    // bytes rather than by decoding the file: a body full of binary
    // floats is not text, and decoding the whole file to find where the
    // text stops would refuse every binary PLY there is.
    let end = header_end(bytes).ok_or(MeshError::ExpectedKeyword {
        expected: "end_header",
        found: String::new(),
        line: 1,
    })?;
    let after = bytes
        .get(end + b"end_header".len()..)
        .unwrap_or(&[])
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |at| end + b"end_header".len() + at + 1);

    let text = core::str::from_utf8(bytes.get(..end).unwrap_or(&[])).map_err(|_| {
        MeshError::ExpectedKeyword {
            expected: "ply",
            found: String::new(),
            line: 1,
        }
    })?;

    let (elements, encoding) = parse_schema(text)?;
    Ok((elements, encoding, after))
}

/// Turn the header's text into a schema and an encoding.
///
/// Split from locating the header because they are two jobs: one is
/// about where the text stops in a file that may not be text at all, and
/// this one is about what the text says.
fn parse_schema(text: &str) -> Result<(Vec<Element>, Encoding), MeshError> {
    let mut encoding = None;
    let mut elements: Vec<Element> = Vec::new();
    let mut saw_magic = false;

    for (number, source) in text.lines().enumerate() {
        // One-based, as an editor counts, and carried into every refusal
        // this loop can make.
        let line = u32::try_from(number + 1).unwrap_or(u32::MAX);
        let mut words = source.split_ascii_whitespace();
        let Some(keyword) = words.next() else {
            continue;
        };
        match keyword {
            "ply" => saw_magic = true,
            // Comments and the obsolete `obj_info` carry anything at all
            // to the end of their line, including bytes that would not
            // parse as anything else.
            "comment" | "obj_info" => {}
            "format" => {
                let word = words.next().unwrap_or("");
                encoding = Some(match word {
                    "ascii" => Encoding::Ascii,
                    "binary_little_endian" => Encoding::BinaryLittle,
                    "binary_big_endian" => Encoding::BinaryBig,
                    other => {
                        return Err(MeshError::ExpectedKeyword {
                            expected: "ascii, binary_little_endian or binary_big_endian",
                            found: quoted(other),
                            line,
                        });
                    }
                });
                // The version is read and checked rather than ignored:
                // every PLY in existence is 1.0, and a file claiming
                // otherwise is describing a format this reader has not
                // been written against.
                let version = words.next().unwrap_or("");
                if version != "1.0" {
                    return Err(MeshError::ExpectedKeyword {
                        expected: "1.0",
                        found: quoted(version),
                        line,
                    });
                }
            }
            "element" => {
                let name = words.next().unwrap_or("");
                let count = number_word(words.next().unwrap_or(""), line)?;
                if elements.len() >= MAX_SCHEMA {
                    return Err(MeshError::TooLarge {
                        field: "element count",
                        value: elements.len() as u64 + 1,
                    });
                }
                elements.push(Element {
                    name: name.to_owned(),
                    count,
                    properties: Vec::new(),
                });
            }
            "property" => {
                let Some(element) = elements.last_mut() else {
                    return Err(MeshError::ExpectedKeyword {
                        expected: "element",
                        found: "property".to_owned(),
                        line,
                    });
                };
                if element.properties.len() >= MAX_SCHEMA {
                    return Err(MeshError::TooLarge {
                        field: "property count",
                        value: element.properties.len() as u64 + 1,
                    });
                }
                let first = words.next().unwrap_or("");
                let kind = if first == "list" {
                    let count = scalar_word(words.next().unwrap_or(""), line)?;
                    let item = scalar_word(words.next().unwrap_or(""), line)?;
                    PropertyKind::List { count, item }
                } else {
                    PropertyKind::Scalar(scalar_word(first, line)?)
                };
                element.properties.push(Property {
                    name: words.next().unwrap_or("").to_owned(),
                    kind,
                });
            }
            other => {
                return Err(MeshError::ExpectedKeyword {
                    expected: "ply, format, comment, element, property or end_header",
                    found: quoted(other),
                    line,
                });
            }
        }
    }

    if !saw_magic {
        return Err(MeshError::ExpectedKeyword {
            expected: "ply",
            found: String::new(),
            line: 1,
        });
    }
    let encoding = encoding.ok_or(MeshError::ExpectedKeyword {
        expected: "format",
        found: String::new(),
        line: 1,
    })?;
    Ok((elements, encoding))
}

/// Where the header's terminator begins.
///
/// **A keyword, not a substring.** This matched `end_header` anywhere in
/// the bytes, so a legal file carrying `comment written by the
/// end_header exporter` had its header cut off at the comment and was
/// then refused for lacking the schema that followed it. Exporters write
/// tool names and paths into comments, which is what comments are for.
///
/// A terminator sits alone on its line, so it has to start one and be
/// followed by the end of one.
fn header_end(bytes: &[u8]) -> Option<usize> {
    const KEYWORD: &[u8] = b"end_header";
    let mut at = 0;
    while let Some(found) = bytes.get(at..).and_then(|rest| {
        rest.windows(KEYWORD.len())
            .position(|window| window == KEYWORD)
    }) {
        let start = at + found;
        let opens_a_line = start == 0 || bytes.get(start - 1) == Some(&b'\n');
        let closes_one = bytes
            .get(start + KEYWORD.len())
            .is_none_or(u8::is_ascii_whitespace);
        if opens_a_line && closes_one {
            return Some(start);
        }
        at = start + 1;
    }
    None
}

fn scalar_word(word: &str, line: u32) -> Result<Scalar, MeshError> {
    Scalar::parse(word).ok_or_else(|| MeshError::ExpectedKeyword {
        expected: "a property type",
        found: quoted(word),
        line,
    })
}

fn number_word(word: &str, line: u32) -> Result<u64, MeshError> {
    word.parse::<u64>().map_err(|_| MeshError::NotANumber {
        found: quoted(word),
        line,
    })
}

/// Where the `x`, `y` and `z` properties sit in an element's schema.
struct Coordinates {
    at: [usize; 3],
}

impl Coordinates {
    /// Locate them, or say which one is missing.
    ///
    /// **By name and not by position.** A schema is allowed to put its
    /// coordinates anywhere and to interleave anything between them, and
    /// files from scanners routinely do — `x y z nx ny nz red green
    /// blue` is ordinary, and so is `x y z confidence intensity`.
    ///
    /// **Counted over scalars only, because that is what a row holds.**
    /// A list is consumed and — except the face element's — discarded, so
    /// it never reaches the row. Numbering these by their position in
    /// the schema instead meant a list declared before `x` shifted every
    /// coordinate after it, and the reader returned a neighbouring
    /// column as the position: `Ok`, with geometry the file does not
    /// describe, which is the worst of the three answers a reader can
    /// give. It only surfaced when enough trailing scalars existed to
    /// absorb the shift; otherwise it ran off the end and refused,
    /// which is how the comment claiming "the fallback is a refusal
    /// rather than a wrong coordinate" came to be written.
    fn locate(element: &Element) -> Result<Self, MeshError> {
        let mut at = [usize::MAX; 3];
        let mut scalars = 0usize;
        for property in &element.properties {
            // A coordinate is one number, so a list named `x` is not
            // one. Leaving it unmatched refuses the file below rather
            // than reading its first entry as a position.
            if !matches!(property.kind, PropertyKind::Scalar(_)) {
                continue;
            }
            let slot = match property.name.as_str() {
                "x" => Some(0),
                "y" => Some(1),
                "z" => Some(2),
                _ => None,
            };
            // The first wins: a schema naming `x` twice is describing
            // something this reader has no way to choose between, and
            // taking the earlier is at least deterministic.
            if let Some(slot) = slot
                && at[slot] == usize::MAX
            {
                at[slot] = scalars;
            }
            scalars += 1;
        }
        for (slot, wanted) in ["x", "y", "z"].into_iter().enumerate() {
            if at[slot] == usize::MAX {
                return Err(MeshError::Unsupported { wanted });
            }
        }
        Ok(Self { at })
    }
}

/// Which element is the vertices and which the faces.
fn geometry_elements(elements: &[Element]) -> Result<(usize, usize), MeshError> {
    let vertex = elements
        .iter()
        .position(|element| element.name == "vertex")
        .ok_or(MeshError::Unsupported { wanted: "vertex" })?;
    // `vertex_indices` is the spelling the specification uses and
    // `vertex_index` is the one half the tools in the world write, so a
    // face element is found by its own name rather than by its property.
    let face = elements
        .iter()
        .position(|element| element.name == "face")
        .ok_or(MeshError::Unsupported { wanted: "face" })?;
    Ok((vertex, face))
}

/// The index list of a face element, located by name.
fn face_list(element: &Element) -> Result<usize, MeshError> {
    element
        .properties
        .iter()
        .position(|property| {
            matches!(property.kind, PropertyKind::List { .. })
                && (property.name == "vertex_indices" || property.name == "vertex_index")
        })
        .ok_or(MeshError::Unsupported {
            wanted: "vertex_indices",
        })
}

/// Turn vertices and faces into triangles.
///
/// The one place the two halves meet, so the index check that separates
/// a mesh from a read past the end lives here rather than in each
/// encoding's reader.
fn assemble(vertices: &[[f32; 3]], faces: &[Vec<u64>]) -> Result<Mesh, MeshError> {
    let mut positions = Vec::new();
    for (number, corners) in faces.iter().enumerate() {
        let face = u32::try_from(number).unwrap_or(u32::MAX);
        if corners.len() < 3 {
            return Err(MeshError::NotAFace {
                face,
                corners: corners.len(),
            });
        }
        for corner in corners {
            let at = usize::try_from(*corner).unwrap_or(usize::MAX);
            if at >= vertices.len() {
                return Err(MeshError::IndexOutOfRange {
                    // PLY's indices are unsigned; the refusal is shared
                    // with a format whose are not, and every value this
                    // reader can produce fits.
                    index: i64::try_from(*corner).unwrap_or(i64::MAX),
                    count: vertices.len(),
                    face,
                });
            }
        }
        // Checked as the fan emits rather than after it, so the refusal
        // arrives before the memory does — which is the difference
        // between a policy ceiling and a representation limit.
        let fanned = (corners.len() - 2) * 3;
        refuse_over_ceiling(positions.len(), fanned)?;
        // A fan from the first corner. Correct for a convex polygon,
        // which is what a triangle and a quad from a subdivision surface
        // both are; see this module's own documentation for why nothing
        // more general is attempted here.
        for step in 1..corners.len() - 1 {
            for corner in [corners[0], corners[step], corners[step + 1]] {
                let at = usize::try_from(corner).unwrap_or(usize::MAX);
                positions.push(*vertices.get(at).unwrap_or(&[0.0; 3]));
            }
        }
    }
    if positions.is_empty() {
        return Err(MeshError::NoGeometry);
    }
    Ok(Mesh {
        positions,
        // PLY stores normals per vertex where it stores them at all, so
        // there is no face normal here to report: choosing which of a
        // triangle's three to keep would be a decision about a model
        // rather than about a file.
        face_normals: Vec::new(),
        // `corner_normals` is where a per-vertex stream belongs and this
        // reader does not yet fill it. The schema is already parsed, so
        // what is missing is reading `nx`/`ny`/`nz` beside the
        // coordinates and emitting one per corner through the fan.
        ..Mesh::default()
    })
}

/// Read an ASCII body: whitespace-separated numbers, in schema order.
fn ascii_body(elements: &[Element], body: &[u8]) -> Result<Mesh, MeshError> {
    let (vertex_at, face_at) = geometry_elements(elements)?;
    let coordinates = Coordinates::locate(&elements[vertex_at])?;
    let list_at = face_list(&elements[face_at])?;

    let text = core::str::from_utf8(body).map_err(|_| MeshError::ExpectedKeyword {
        expected: "an ascii body",
        found: String::new(),
        line: 1,
    })?;
    let mut words = text.split_ascii_whitespace();
    let mut vertices = Vec::new();
    let mut faces = Vec::new();

    // The elements are read in the order the header declared them,
    // because that is the order the body is written in. An element this
    // reader has no use for is still read: its rows have to be consumed
    // to reach the ones that follow.
    for (index, element) in elements.iter().enumerate() {
        refuse_impossible_count(element, body.len(), false)?;
        for _ in 0..element.count {
            let mut row: Vec<f64> = Vec::new();
            let mut list: Vec<u64> = Vec::new();
            for (slot, property) in element.properties.iter().enumerate() {
                match property.kind {
                    PropertyKind::Scalar(_) => {
                        let word = words.next().ok_or(MeshError::NoGeometry)?;
                        row.push(word.parse::<f64>().map_err(|_| MeshError::NotANumber {
                            found: quoted(word),
                            line: 0,
                        })?);
                    }
                    PropertyKind::List { .. } => {
                        let word = words.next().ok_or(MeshError::NoGeometry)?;
                        let count = word.parse::<usize>().map_err(|_| MeshError::NotANumber {
                            found: quoted(word),
                            line: 0,
                        })?;
                        if count > usize::from(MAX_FACE_CORNERS) {
                            return Err(MeshError::TooLarge {
                                field: "face corner count",
                                value: count as u64,
                            });
                        }
                        let mut entries = Vec::with_capacity(count);
                        for _ in 0..count {
                            let word = words.next().ok_or(MeshError::NoGeometry)?;
                            entries.push(word.parse::<u64>().map_err(|_| {
                                MeshError::NotANumber {
                                    found: quoted(word),
                                    line: 0,
                                }
                            })?);
                        }
                        if index == face_at && slot == list_at {
                            list = entries;
                        }
                    }
                }
            }
            if index == vertex_at {
                vertices.push(read_position(&row, &coordinates, vertices.len())?);
            } else if index == face_at {
                faces.push(list);
            }
        }
    }
    assemble(&vertices, &faces)
}

/// The three coordinates out of one row, checked for being numbers a
/// bounding box can hold.
fn read_position(
    row: &[f64],
    coordinates: &Coordinates,
    index: usize,
) -> Result<[f32; 3], MeshError> {
    let index = u32::try_from(index).unwrap_or(u32::MAX);
    let mut out = [0.0f32; 3];
    for (slot, value) in out.iter_mut().enumerate() {
        // A scalar property sits at its own index only because lists are
        // not counted into `row`. A vertex element carrying a list would
        // break that; no such file exists, and the fallback is a refusal
        // rather than a wrong coordinate.
        let found = row
            .get(coordinates.at[slot])
            .copied()
            .ok_or(MeshError::NotFinite {
                field: "position",
                index,
            })?;
        // Narrowed once, here, so the finite check below is on the value
        // that actually reaches a caller rather than on a wider one that
        // survived it. A PLY may store f64 and the record holds f32.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "narrowing to the record's width IS the conversion, and what it produces is checked for being finite on the next line"
        )]
        let narrowed = found as f32;
        if !narrowed.is_finite() {
            return Err(MeshError::NotFinite {
                field: "position",
                index,
            });
        }
        *value = narrowed;
    }
    Ok(out)
}

/// Read a binary body, little- or big-endian.
///
/// **Every offset is bounds-checked as it is taken, never computed
/// ahead.** A header declares counts, widths and list lengths, all of
/// which an attacker writes, and their product is exactly the number a
/// reader must not trust. The cursor here only ever moves forward by a
/// width it has just confirmed is there.
fn binary_body(elements: &[Element], body: &[u8], big: bool) -> Result<Mesh, MeshError> {
    let (vertex_at, face_at) = geometry_elements(elements)?;
    let coordinates = Coordinates::locate(&elements[vertex_at])?;
    let list_at = face_list(&elements[face_at])?;

    let mut cursor = Cursor { bytes: body, at: 0 };
    let mut vertices = Vec::new();
    let mut faces = Vec::new();

    for (index, element) in elements.iter().enumerate() {
        refuse_impossible_count(element, body.len(), true)?;
        for _ in 0..element.count {
            let mut row: Vec<f64> = Vec::new();
            let mut list: Vec<u64> = Vec::new();
            for (slot, property) in element.properties.iter().enumerate() {
                match property.kind {
                    PropertyKind::Scalar(scalar) => row.push(cursor.scalar(scalar, big)?),
                    PropertyKind::List { count, item } => {
                        let declared = cursor.scalar(count, big)?;
                        // A negative length and an enormous one are the
                        // same refusal: neither is a number of corners.
                        // `f64::from` a `u16` is exact, so the bound
                        // itself needs no conversion to get wrong.
                        if !(0.0..=f64::from(MAX_FACE_CORNERS)).contains(&declared) {
                            #[expect(
                                clippy::cast_possible_truncation,
                                clippy::cast_sign_loss,
                                reason = "clamped into a u32 range on this line, so the conversion is exact; the number is being reported back to a caller, never used to size anything"
                            )]
                            let reported = declared.clamp(0.0, f64::from(u32::MAX)) as u64;
                            return Err(MeshError::TooLarge {
                                field: "face corner count",
                                value: reported,
                            });
                        }
                        #[expect(
                            clippy::cast_possible_truncation,
                            clippy::cast_sign_loss,
                            reason = "the range check on the line above is what makes this exact: the value is between zero and the corner ceiling"
                        )]
                        let corners = declared as usize;
                        let mut entries =
                            Vec::with_capacity(corners.min(usize::from(MAX_FACE_CORNERS)));
                        for _ in 0..corners {
                            let entry = cursor.scalar(item, big)?;
                            // **Refused, not coerced.** This was
                            // `entry.max(0.0) as u64`, and the reason
                            // beside it said an index out of range is
                            // caught against the vertex count later. It
                            // is not: `max` had already turned a
                            // negative index into vertex zero, and
                            // `NaN.max(0.0)` is `0.0`, so a file naming
                            // vertex -1 or vertex NaN got vertex zero
                            // and a caller got geometry the file does
                            // not describe. A fractional index was
                            // truncated the same way, silently.
                            if !entry.is_finite() || entry < 0.0 || entry.fract() != 0.0 {
                                return Err(MeshError::NotANumber {
                                    found: quoted(&entry.to_string()),
                                    line: 0,
                                });
                            }
                            #[expect(
                                clippy::cast_possible_truncation,
                                clippy::cast_sign_loss,
                                reason = "the check above admits only a non-negative whole number, and one of that shape converts exactly; whether it names a vertex that exists is `assemble`'s question"
                            )]
                            let index = entry as u64;
                            entries.push(index);
                        }
                        if index == face_at && slot == list_at {
                            list = entries;
                        }
                    }
                }
            }
            if index == vertex_at {
                vertices.push(read_position(&row, &coordinates, vertices.len())?);
            } else if index == face_at {
                faces.push(list);
            }
        }
    }
    assemble(&vertices, &faces)
}

/// A forward-only cursor over the body, which never reads past its end.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    /// One scalar, widened to `f64` so every type takes one path.
    ///
    /// **A widening, not a conversion.** The widest integer here is
    /// thirty-two bits and an `f64` carries fifty-three of mantissa, so
    /// every value of every type above survives exactly. Nothing is lost
    /// on the way in, which is what lets one function serve eight types
    /// without eight code paths to get wrong.
    fn scalar(&mut self, scalar: Scalar, big: bool) -> Result<f64, MeshError> {
        let width = scalar.width();
        let end = self.at.checked_add(width).ok_or(MeshError::CountMismatch {
            declared: u64::MAX,
            actual: self.bytes.len(),
            count: 0,
        })?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or(MeshError::CountMismatch {
                declared: end as u64,
                actual: self.bytes.len(),
                count: 0,
            })?;
        self.at = end;
        let mut buffer = [0u8; 8];
        buffer[..width].copy_from_slice(slice);
        if big {
            buffer[..width].reverse();
        }
        Ok(match scalar {
            #[expect(
                clippy::cast_possible_wrap,
                reason = "reinterpreting the byte as signed is the point: the schema said this column is a signed eight-bit integer"
            )]
            Scalar::I8 => f64::from(buffer[0] as i8),
            Scalar::U8 => f64::from(buffer[0]),
            Scalar::I16 => f64::from(i16::from_le_bytes([buffer[0], buffer[1]])),
            Scalar::U16 => f64::from(u16::from_le_bytes([buffer[0], buffer[1]])),
            Scalar::I32 => f64::from(i32::from_le_bytes([
                buffer[0], buffer[1], buffer[2], buffer[3],
            ])),
            Scalar::U32 => f64::from(u32::from_le_bytes([
                buffer[0], buffer[1], buffer[2], buffer[3],
            ])),
            Scalar::F32 => f64::from(f32::from_le_bytes([
                buffer[0], buffer[1], buffer[2], buffer[3],
            ])),
            Scalar::F64 => f64::from_le_bytes(buffer),
        })
    }
}
