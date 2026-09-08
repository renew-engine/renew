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
//! **The names are not returned anywhere yet.** When they are, it will be
//! through an entry point of their own rather than as a second return
//! value here: a caller that wants triangles should not have to receive
//! a list of filenames it has no intention of resolving, and a caller
//! that wants the material list should not have to parse the geometry to
//! get it.

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
/// a line here begin with a keyword that only OBJ uses", and a file that
/// opens with a thousand comment lines answers no.
///
/// Use it to choose between formats when a caller has no better hint,
/// never to decide that a file is safe to read. [`read`] answers for
/// every byte string either way.
#[must_use]
pub fn looks_like(bytes: &[u8]) -> bool {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return false;
    };
    text.lines().take(64).any(|line| {
        let mut words = line.split_ascii_whitespace();
        matches!(
            words.next(),
            Some("v" | "vn" | "vt" | "f" | "mtllib" | "usemtl")
        )
    })
}

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
            "v" => streams.positions.push(three(&mut words, "position", line)?),
            "vn" => streams.normals.push(three(&mut words, "normal", line)?),
            "vt" => streams
                .texcoords
                .push(two(&mut words, "texture coordinate", line)?),
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
) -> Result<[f32; 3], MeshError> {
    let mut out = [0.0; 3];
    for (at, slot) in out.iter_mut().enumerate() {
        let word = words.next().ok_or(MeshError::NotANumber {
            found: String::new(),
            line,
        })?;
        *slot = number(word, field, line, at)?;
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
) -> Result<[f32; 2], MeshError> {
    let first = words.next().ok_or(MeshError::NotANumber {
        found: String::new(),
        line,
    })?;
    let u = number(first, field, line, 0)?;
    let v = match words.next() {
        Some(word) => number(word, field, line, 1)?,
        None => 0.0,
    };
    Ok([u, v])
}

/// One float, parsed and checked for being a number at all.
fn number(word: &str, field: &'static str, line: u32, at: usize) -> Result<f32, MeshError> {
    let value: f32 = word.parse().map_err(|_| MeshError::NotANumber {
        found: quoted(word),
        line,
    })?;
    if !value.is_finite() {
        return Err(MeshError::NotFinite {
            field,
            index: u32::try_from(at).unwrap_or(u32::MAX),
        });
    }
    Ok(value)
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
    let declared = i64::try_from(count).unwrap_or(i64::MAX);
    // Positive counts from one; negative counts back from the end of
    // what has been declared, so `-1` is the most recent.
    let at = if raw > 0 { raw - 1 } else { declared + raw };
    if at < 0 || at >= declared {
        return Err(MeshError::IndexOutOfRange {
            index: raw,
            count,
            face,
        });
    }
    usize::try_from(at).map_err(|_| MeshError::IndexOutOfRange {
        index: raw,
        count,
        face,
    })
}
