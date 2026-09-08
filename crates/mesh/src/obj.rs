//! Reader for Wavefront OBJ geometry.
//!
//! OBJ is a line-oriented text format with no header, no counts and no
//! magic word. A line's first word says what it is; everything after is
//! whitespace-separated. There is nothing to seek to and nothing to
//! trust in advance, so this reader is a single forward pass that
//! accumulates three vertex streams and then de-indexes faces against
//! them.
//!
//! # What it reads
//!
//! `v` positions, `vt` texture coordinates, `vn` normals, and `f` faces.
//! A face names one corner per word, each of the form `v`, `v/vt`,
//! `v//vn` or `v/vt/vn`, and a polygon is fanned from its first corner
//! exactly as [`crate::ply`] fans one.
//!
//! # What it skips, and why skipping is right here
//!
//! **Every other keyword is ignored rather than refused.** A real OBJ
//! carries object and group names, smoothing groups, material library
//! references, curve and surface statements, and vendor extensions that
//! were never standard. Refusing a file for containing any of them would
//! refuse most of the format as it is actually written, and none of them
//! changes the geometry this reader produces. That is the same rule
//! [`crate::ply`] applies to properties it does not know: cost the line
//! nothing, and carry on.
//!
//! The one thing skipping does NOT extend to is a line this reader
//! claims to understand. A `v` with two numbers is a malformed position,
//! not an unknown keyword, and it is refused.
//!
//! # Indices
//!
//! OBJ numbers its vertex streams from one, and **a negative index counts
//! back from whatever has been declared so far** — `-1` is the most
//! recent. Both spellings are read, because both are written: the
//! relative form is what an exporter emits when it is concatenating
//! objects and does not want to track a running base.
//!
//! That leaves zero, which is neither. It is refused by name rather than
//! folded into "out of range", because the two are usually different
//! bugs: an out-of-range index is a writer that computed the wrong
//! number, and a zero is very often a field nobody filled in.
//!
//! # Materials
//!
//! `mtllib` and `usemtl` are skipped by this function. A material
//! library is a *second file*, and this crate never opens one — see the
//! crate documentation for why that is a promise rather than an
//! omission.
//!
//! **The names come back from [`materials`], an entry point of its own.**
//! Not a second return value here: a caller that wants triangles should
//! not have to receive a list of filenames it has no intention of
//! resolving, and a caller that wants the material list should not have
//! to parse the geometry to get it.

use std::collections::BTreeSet;

use crate::error::{MeshError, quoted};
use crate::{Mesh, refuse_over_ceiling};

/// The most corners one face may name.
///
/// A polygon with more corners than this is not a surface anybody
/// modelled; it is a number chosen to make the fan below expensive. The
/// bound is on the count a *line* can name, so it costs nothing to
/// check and it is checked as the corners arrive rather than after.
const MAX_FACE_CORNERS: usize = 1024;

/// Whether these bytes look like an OBJ.
///
/// **This is a weak answer and says so.** OBJ has no magic word and no
/// header: the first line of a valid file may be a comment, a blank
/// line, or geometry. So the question this can honestly answer is "does
/// a line near the front begin with a keyword only OBJ uses".
///
/// **The budget counts lines that could have been a keyword, and a
/// comment is not one.** An earlier version took the first sixty-four
/// lines flat, which meant a file with a sixty-four-line licence banner
/// — an ordinary thing for an exporter to write — answered no and was
/// then read by whatever the caller fell back to. `read` accepted that
/// same file perfectly well; only this said otherwise. Skipping comments
/// and blank lines costs nothing on a real header and leaves the budget
/// doing its actual job, which is to stop a file that is not an OBJ from
/// being scanned to its end.
///
/// Reading line by line rather than validating the whole file first is
/// the same economy: a byte string that is not text stops at the first
/// line that is not, instead of after a pass over every byte of it.
///
/// Use it to choose between formats when a caller has no better hint,
/// never to decide that a file is safe to read. [`read`] answers for
/// every byte string either way.
#[must_use]
pub fn looks_like(bytes: &[u8]) -> bool {
    let mut considered = 0;
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(text) = core::str::from_utf8(line) else {
            // An OBJ is text, and `read` refuses one that is not.
            return false;
        };
        let Some(keyword) = text.split_ascii_whitespace().next() else {
            // Blank. Exporters pad, and padding is not evidence.
            continue;
        };
        if keyword.starts_with('#') {
            // A comment carries anything at all to the end of its line,
            // which is exactly why it says nothing about the format.
            continue;
        }
        if matches!(keyword, "v" | "vn" | "vt" | "f" | "mtllib" | "usemtl") {
            return true;
        }
        considered += 1;
        if considered >= CONSIDERED_LINES {
            return false;
        }
    }
    false
}

/// How many lines that could have been a keyword are looked at before
/// answering no.
///
/// Small on purpose. A file whose first several statements say nothing
/// an OBJ says is not an OBJ, and every line spent past that is spent on
/// a file this is going to decline anyway — which for a text STL is a
/// scan of the whole thing.
const CONSIDERED_LINES: usize = 16;

/// Read an OBJ file's geometry.
///
/// # Errors
///
/// Returns the [`MeshError`] naming what the file got wrong: a number
/// that is not one, a coordinate that is not finite, an index that is
/// zero or points outside what it addresses, a face with too few corners
/// to be a surface, a face list this reader cannot represent, or no
/// geometry at all.
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    // OBJ is text by definition. Converting lossily would put characters
    // in a refusal's message that the file does not contain, and would
    // let a byte string that is not this format parse as if it were.
    let text = core::str::from_utf8(bytes).map_err(|_| MeshError::ExpectedKeyword {
        expected: "text",
        found: String::new(),
        line: 1,
    })?;

    let mut streams = Streams::default();
    let mut built = Built::default();

    for (number, source) in text.lines().enumerate() {
        // One-based, as an editor counts.
        let line = u32::try_from(number + 1).unwrap_or(u32::MAX);
        let mut words = source.split_ascii_whitespace();
        let Some(keyword) = words.next() else {
            continue;
        };
        match keyword {
            // Each stream numbers its own records, and the record a
            // refusal names is the one the file is on -- which is the
            // length so far, before the push.
            "v" => {
                let record = record_of(streams.positions.len());
                streams
                    .positions
                    .push(three(&mut words, "position", line, record)?);
            }
            "vn" => {
                let record = record_of(streams.normals.len());
                streams
                    .normals
                    .push(three(&mut words, "normal", line, record)?);
            }
            "vt" => {
                let record = record_of(streams.texcoords.len());
                streams
                    .texcoords
                    .push(two(&mut words, "texture coordinate", line, record)?);
            }
            "f" => face(&mut words, line, &streams, &mut built)?,
            // Comments, groups, objects, smoothing, materials, curves,
            // and whatever else a writer put here.
            _ => {}
        }
    }

    if built.positions.is_empty() {
        return Err(MeshError::NoGeometry);
    }
    Ok(Mesh {
        positions: built.positions,
        // OBJ states no normal for a face as a whole. What it carries is
        // per corner, and that is where it goes.
        face_normals: Vec::new(),
        corner_normals: built.corner_normals,
        corner_texcoords: built.corner_texcoords,
    })
}

/// The material references an OBJ makes, without its geometry.
///
/// Names exactly as the file wrote them. Resolving a library name to a
/// file is the caller's job and cannot be this crate's: see [`materials`]
/// for why that is a promise rather than a gap.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Materials {
    /// The library files the OBJ asks for, in first-appearance order.
    ///
    /// One `mtllib` line may name several, and the format says a later
    /// definition wins over an earlier one — which is why the order is
    /// kept rather than sorted.
    pub libraries: Vec<String>,
    /// The material names the file applies to its faces, in
    /// first-appearance order.
    ///
    /// **Which faces use which is not here.** That is a fact about the
    /// geometry, and a caller asking only "what does this file need"
    /// should not have to read the geometry to be told.
    pub used: Vec<String>,
}

/// Read the material names an OBJ refers to, without reading its
/// geometry.
///
/// **An entry point of its own, rather than a second return value from
/// [`read`].** The two questions have different callers: one wants
/// triangles and has no interest in a list of filenames it will not
/// resolve, and the other wants to know what a file depends on before
/// deciding whether to load it at all. Answering both from one call
/// would make each pay for the other.
///
/// **This crate cannot follow a `mtllib` and does not pretend to.** A
/// material library is a second file; a reader that never opens one can
/// only hand back the name. That is the same promise the crate makes
/// everywhere else, and here it is visible in the return type.
///
/// # On bounds
///
/// There is no ceiling on how many names come back, and that is
/// deliberate rather than an oversight. Every name is a run of bytes
/// copied out of the input, so the total returned is bounded by the
/// input's own length: a file cannot ask for more memory than it spends.
/// The ceilings elsewhere in this crate exist where a small number in a
/// file multiplies into a large allocation, and nothing here multiplies.
///
/// # Errors
///
/// Returns [`MeshError::ExpectedKeyword`] when the bytes are not text.
/// Nothing else: a file naming no materials names none, which is an
/// answer rather than a refusal, and a library this crate cannot open is
/// not a library this crate can complain about.
pub fn materials(bytes: &[u8]) -> Result<Materials, MeshError> {
    let text = core::str::from_utf8(bytes).map_err(|_| MeshError::ExpectedKeyword {
        expected: "text",
        found: String::new(),
        line: 1,
    })?;

    let mut found = Materials::default();
    let mut seen_libraries = BTreeSet::new();
    let mut seen_used = BTreeSet::new();

    for source in text.lines() {
        let mut words = source.split_ascii_whitespace();
        match words.next() {
            // A `mtllib` line may name several libraries at once.
            Some("mtllib") => {
                for name in words {
                    if seen_libraries.insert(name.to_owned()) {
                        found.libraries.push(name.to_owned());
                    }
                }
            }
            // `usemtl` names one. A bare `usemtl` with nothing after it
            // is how a writer says "no material from here on", which
            // names nothing and so contributes nothing.
            Some("usemtl") => {
                if let Some(name) = words.next()
                    && seen_used.insert(name.to_owned())
                {
                    found.used.push(name.to_owned());
                }
            }
            _ => {}
        }
    }
    Ok(found)
}

/// The three indexed streams a file declares, in declaration order.
#[derive(Default)]
struct Streams {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    texcoords: Vec<[f32; 2]>,
}

/// The de-indexed mesh as it is being built, plus what the faces so far
/// have committed the file to.
#[derive(Default)]
struct Built {
    positions: Vec<[f32; 3]>,
    corner_normals: Vec<[f32; 3]>,
    corner_texcoords: Vec<[f32; 2]>,
    /// Which face is being read, zero-based, for refusals to name.
    faces: u32,
    /// What the first corner of the first face carried. Every corner
    /// after it must carry the same, because the arrays are per corner
    /// and a gap in one cannot be filled with anything true.
    shape: Option<Shape>,
}

/// Which of the optional streams a corner names.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Shape {
    texcoord: bool,
    normal: bool,
}

/// One corner of a face, resolved against the streams declared so far.
#[derive(Clone, Copy)]
struct Corner {
    position: usize,
    texcoord: Option<usize>,
    normal: Option<usize>,
}

/// Read three finite floats, refusing anything else.
///
/// Extra words on the line are ignored: `v` legally carries a fourth
/// component, and writers in the wild also put per-vertex colours there.
/// Neither changes the point, and refusing them would refuse files that
/// every other reader accepts.
fn three<'a>(
    words: &mut impl Iterator<Item = &'a str>,
    field: &'static str,
    line: u32,
    record: u32,
) -> Result<[f32; 3], MeshError> {
    let mut out = [0.0; 3];
    for slot in &mut out {
        let word = words.next().ok_or(MeshError::NotANumber {
            found: String::new(),
            line,
        })?;
        *slot = number(word, field, line, record)?;
    }
    Ok(out)
}

/// Read one or two finite floats, the second defaulting to zero.
///
/// `vt` carries one, two or three components. A one-component
/// coordinate is a position along a one-dimensional texture, and zero is
/// what the second axis of such a coordinate means.
fn two<'a>(
    words: &mut impl Iterator<Item = &'a str>,
    field: &'static str,
    line: u32,
    record: u32,
) -> Result<[f32; 2], MeshError> {
    let first = words.next().ok_or(MeshError::NotANumber {
        found: String::new(),
        line,
    })?;
    let u = number(first, field, line, record)?;
    let v = match words.next() {
        Some(word) => number(word, field, line, record)?,
        None => 0.0,
    };
    Ok([u, v])
}

/// One float, parsed and checked for being a number at all.
///
/// `record` is which record of its stream the value belongs to, which is
/// what [`MeshError::NotFinite`] reports. It is deliberately not the
/// component within the record: a caller told "record 2" goes and looks
/// at the third `v` line, and telling it the third *component* instead
/// would send it to a record that may not exist.
fn number(word: &str, field: &'static str, line: u32, record: u32) -> Result<f32, MeshError> {
    let value: f32 = word.parse().map_err(|_| MeshError::NotANumber {
        found: quoted(word),
        line,
    })?;
    if !value.is_finite() {
        return Err(MeshError::NotFinite {
            field,
            index: record,
        });
    }
    Ok(value)
}

/// Which record of a stream is being read, given how many it already
/// holds. Saturating, because a file with four billion vertices has a
/// bigger problem than an imprecise refusal.
fn record_of(so_far: usize) -> u32 {
    u32::try_from(so_far).unwrap_or(u32::MAX)
}

/// Read one face and fan it into triangles.
fn face<'a>(
    words: &mut impl Iterator<Item = &'a str>,
    line: u32,
    streams: &Streams,
    built: &mut Built,
) -> Result<(), MeshError> {
    let at = built.faces;
    built.faces = built.faces.saturating_add(1);

    let mut corners = Vec::new();
    for word in words {
        if corners.len() >= MAX_FACE_CORNERS {
            return Err(MeshError::TooLarge {
                field: "face corner count",
                value: corners.len() as u64 + 1,
            });
        }
        corners.push(corner(word, line, at, streams, built)?);
    }
    if corners.len() < 3 {
        return Err(MeshError::NotAFace {
            face: at,
            corners: corners.len(),
        });
    }

    // A fan from the first corner, correct for a convex polygon, which
    // is what the quads and triangles a modeller exports are.
    for step in 1..corners.len() - 1 {
        refuse_over_ceiling(built.positions.len(), 3)?;
        for corner in [corners[0], corners[step], corners[step + 1]] {
            built.positions.push(streams.positions[corner.position]);
            if let Some(index) = corner.texcoord {
                built.corner_texcoords.push(streams.texcoords[index]);
            }
            if let Some(index) = corner.normal {
                built.corner_normals.push(streams.normals[index]);
            }
        }
    }
    Ok(())
}

/// Resolve one `v`, `v/vt`, `v//vn` or `v/vt/vn` corner.
fn corner(
    word: &str,
    line: u32,
    face: u32,
    streams: &Streams,
    built: &mut Built,
) -> Result<Corner, MeshError> {
    let mut parts = word.split('/');
    // `split` on a non-empty string always yields at least one piece, so
    // the position is always present as a string — it may still be
    // empty, which `resolve` refuses as not a number.
    let position = resolve(
        parts.next().unwrap_or(""),
        streams.positions.len(),
        line,
        face,
    )?;
    let texcoord = match parts.next() {
        // `v//vn` writes the texture slot as nothing at all.
        None | Some("") => None,
        Some(word) => Some(resolve(word, streams.texcoords.len(), line, face)?),
    };
    let normal = match parts.next() {
        None | Some("") => None,
        Some(word) => Some(resolve(word, streams.normals.len(), line, face)?),
    };

    // A per-corner array is either empty or exactly as long as the
    // positions, so a file where some corners name a normal and others
    // do not has no representation here that is not an invention.
    let shape = Shape {
        texcoord: texcoord.is_some(),
        normal: normal.is_some(),
    };
    match built.shape {
        None => built.shape = Some(shape),
        Some(first) if first == shape => {}
        Some(first) => {
            // Which stream disagreed, so the message names the one to
            // go and look at. Matched rather than compared: these are
            // two flags, and asking whether one differs from the other
            // reads worse than saying which pairs mean what.
            let wanted = match (first.normal, shape.normal) {
                (true, false) | (false, true) => "a normal on every corner, or on none",
                _ => "a texture coordinate on every corner, or on none",
            };
            return Err(MeshError::Unsupported { wanted });
        }
    }

    Ok(Corner {
        position,
        texcoord,
        normal,
    })
}

/// Turn one of OBJ's signed, one-based indices into an offset.
fn resolve(word: &str, count: usize, line: u32, face: u32) -> Result<usize, MeshError> {
    let raw: i64 = word.parse().map_err(|_| MeshError::NotANumber {
        found: quoted(word),
        line,
    })?;
    if raw == 0 {
        return Err(MeshError::IndexZero { line });
    }
    // Positive counts from one; negative counts back from the end of
    // what has been declared, so `-1` is the most recent. Both arms end
    // in a `usize` that is in range or in nothing at all.
    //
    // **There was a second `IndexOutOfRange` here, for the conversion
    // from the signed index to a `usize` failing.** It could not fire:
    // the range check above had already bounded the value by `count`,
    // which came out of a `usize` to begin with. Doing the arithmetic in
    // `usize` from the start removes the arm rather than leaving a
    // refusal nothing can reach.
    let at = if raw > 0 {
        usize::try_from(raw - 1).ok().filter(|at| *at < count)
    } else {
        // `unsigned_abs` rather than negation, because `i64::MIN` has no
        // positive counterpart to negate to.
        usize::try_from(raw.unsigned_abs())
            .ok()
            .and_then(|back| count.checked_sub(back))
    };
    at.ok_or(MeshError::IndexOutOfRange {
        index: raw,
        count,
        face,
    })
}
