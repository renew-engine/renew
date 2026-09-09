//! Reader for Wavefront MTL material libraries.
//!
//! The file an OBJ's `mtllib` names. Line-oriented text like OBJ, with
//! the same shape of grammar: a keyword, then whitespace-separated
//! values. `newmtl` opens a material and everything after it belongs to
//! that material until the next `newmtl`.
//!
//! # What it reads
//!
//! The factors every renderer has a use for — `Ka`, `Kd`, `Ks` and `Ke`
//! for ambient, diffuse, specular and emissive colour, `Ns` for
//! shininess, `d` and `Tr` for opacity — and the `map_*` lines that name
//! a texture for one of those slots.
//!
//! # What it skips
//!
//! **Everything else, for the reason [`crate::obj`] skips its unknowns.**
//! A real `.mtl` carries `illum` modes, physically-based extensions that
//! were never in the format (`Pr`, `Pm`, `Ke` predates them and is now
//! standard), and per-tool keywords nobody standardised. None of them
//! changes the factors above, and refusing a file for carrying one would
//! refuse most libraries in the world.
//!
//! # What it never does
//!
//! **Open a texture.** A `map_Kd` names a second file, and this crate
//! opens no files at all — see the crate documentation for why that is a
//! promise rather than an omission. The name comes back; resolving it is
//! the caller's, exactly as `mtllib` is in [`crate::obj::materials`].
//!
//! # Two spellings of one fact
//!
//! `d 0.5` and `Tr 0.5` both describe half-transparency, and they are
//! reciprocal: `Tr` is one minus `d`. **Real exporters write them
//! inconsistently**, some emitting `Tr` with the value `d` should have
//! had, and no reader can tell which tool wrote a file from the file
//! alone. So this one takes the file at its word in the order written,
//! the last of the two winning, and says so here rather than guessing at
//! provenance. A caller that knows its own pipeline can decide better
//! than a reader that does not.

use crate::error::{MeshError, quoted};

/// Which of a material's slots a texture fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapSlot {
    /// `map_Ka`.
    Ambient,
    /// `map_Kd`.
    Diffuse,
    /// `map_Ks`.
    Specular,
    /// `map_Ns`, a greyscale map scaling the shininess.
    Shininess,
    /// `map_d`, a greyscale map scaling the opacity.
    Opacity,
    /// `map_bump` or `bump`, a height map.
    Bump,
    /// `norm`, a tangent-space normal map.
    ///
    /// Distinct from [`MapSlot::Bump`] because they are read
    /// differently: a height map perturbs along the normal and a normal
    /// map replaces it. Files use both, and a reader that folded them
    /// would hand a caller a height map to sample as vectors.
    Normal,
}

/// A texture a material names for one of its slots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureMap {
    /// Which slot it fills.
    pub slot: MapSlot,
    /// The file name, exactly as written, unresolved.
    pub name: String,
}

/// One material from a library.
///
/// Every factor is optional because the format makes every one optional,
/// and `None` is not the same as a default: a file that says nothing
/// about specularity has said nothing, and what to do about that is the
/// renderer's convention rather than this reader's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Material {
    /// The name `newmtl` gave it, which is what an OBJ's `usemtl`
    /// refers to.
    pub name: String,
    /// `Ka`.
    pub ambient: Option<[f32; 3]>,
    /// `Kd`.
    pub diffuse: Option<[f32; 3]>,
    /// `Ks`.
    pub specular: Option<[f32; 3]>,
    /// `Ke`.
    pub emissive: Option<[f32; 3]>,
    /// `Ns`, the specular exponent.
    ///
    /// Not clamped. The format's usual range is 0 to 1000 and files
    /// exceed it; a reader that clamped would silently change a material
    /// rather than report one.
    pub shininess: Option<f32>,
    /// `d`, or one minus `Tr`, whichever the file wrote last.
    ///
    /// See this module's documentation for why the two spellings are not
    /// reconciled.
    pub opacity: Option<f32>,
    /// The textures it names, in the order it named them.
    pub maps: Vec<TextureMap>,
}

/// Whether these bytes look like a material library.
///
/// **A keyword scan, not a parse.** An earlier caller answered this
/// question by running [`read`] and discarding the result, which cost a
/// full parse of every file that was not something else — measured at
/// twenty-three per cent of the time to import a text STL, and growing
/// with the file. The question "is this an MTL" does not need the
/// answer to "what is in it".
///
/// `newmtl` is what it looks for, because a library that declares no
/// material is refused by [`read`] anyway, so a file without one is not
/// a library this crate can use whatever else it contains.
///
/// Like every `looks_like` here this is a guess and says so: it answers
/// from the opening of the file, and a library preceded by a very long
/// comment banner answers no. [`read`] answers for every byte string
/// either way.
#[must_use]
pub fn looks_like(bytes: &[u8]) -> bool {
    let Ok(text) = core::str::from_utf8(bytes) else {
        // A material library is text. Bytes that are not text are not
        // one, and this is the cheap half of what `read` would say.
        return false;
    };
    text.lines()
        .take(SCANNED_LINES)
        .any(|line| line.split_ascii_whitespace().next() == Some("newmtl"))
}

/// How far into a file `looks_like` reads before answering.
///
/// Far enough to clear the licence banners real exporters emit, and
/// bounded so the question costs the same on a file of any size.
const SCANNED_LINES: usize = 256;

/// Read a material library.
///
/// Materials come back in the order the file declares them. **A name may
/// appear twice**: the format's own rule is that a later definition wins
/// over an earlier one, so both are kept and the order is what carries
/// that meaning. Collapsing them here would decide, on the caller's
/// behalf, a question the file already answers.
///
/// # Errors
///
/// Returns the [`MeshError`] naming what the file got wrong: bytes that
/// are not text, a value that is not a number or is not finite, a
/// property stated before any `newmtl` opened a material to hold it, or
/// a library that declares no material at all.
pub fn read(bytes: &[u8]) -> Result<Vec<Material>, MeshError> {
    let text = core::str::from_utf8(bytes).map_err(|_| MeshError::ExpectedKeyword {
        expected: "text",
        found: String::new(),
        line: 1,
    })?;

    let mut library: Vec<Material> = Vec::new();

    for (number, source) in text.lines().enumerate() {
        // One-based, as an editor counts.
        let line = u32::try_from(number + 1).unwrap_or(u32::MAX);
        let mut words = source.split_ascii_whitespace();
        let Some(keyword) = words.next() else {
            continue;
        };

        if keyword == "newmtl" {
            library.push(Material {
                // A `newmtl` with no name opens a material nothing can
                // refer to, which is a file to read rather than refuse:
                // its factors are still there and a caller matching by
                // name simply never matches it.
                name: words.next().unwrap_or("").to_owned(),
                ..Material::default()
            });
            continue;
        }
        // Looked up once. An earlier version asked `is_property` here
        // and then asked `map_slot` again inside the match, which left
        // an arm for a keyword that was a map on the first question and
        // not on the second — a branch no input could take.
        let Some(property) = property_of(keyword) else {
            // Anything this reader does not implement, which is most of
            // what a real library carries.
            continue;
        };

        // A property needs a material to belong to. Stating one before
        // any `newmtl` is a file whose first material is missing rather
        // than a value to attach to nothing.
        // Which material a refusal names: the one being filled, as the
        // file stores them. `NotFinite` reports a record, and here a
        // record is a material.
        let record = u32::try_from(library.len().saturating_sub(1)).unwrap_or(u32::MAX);
        let Some(material) = library.last_mut() else {
            return Err(MeshError::ExpectedKeyword {
                expected: "newmtl",
                found: quoted(keyword),
                line,
            });
        };

        match property {
            Property::Ambient => {
                material.ambient = Some(colour(&mut words, "ambient", line, record)?);
            }
            Property::Diffuse => {
                material.diffuse = Some(colour(&mut words, "diffuse", line, record)?);
            }
            Property::Specular => {
                material.specular = Some(colour(&mut words, "specular", line, record)?);
            }
            Property::Emissive => {
                material.emissive = Some(colour(&mut words, "emissive", line, record)?);
            }
            Property::Shininess => {
                material.shininess = Some(scalar(&mut words, "shininess", line, record)?);
            }
            Property::Opacity => {
                material.opacity = Some(scalar(&mut words, "opacity", line, record)?);
            }
            // The reciprocal spelling. See this module's documentation
            // for why the last one written wins rather than the two
            // being reconciled.
            Property::Transparency => {
                material.opacity = Some(1.0 - scalar(&mut words, "opacity", line, record)?);
            }
            Property::Map(slot) => {
                // A map line may carry options before the file name
                // (`-s 1 1 1 wood.png`), so the name is the last word
                // rather than the second.
                //
                // **That is a heuristic and not a parse.** A line whose
                // options run to the end (`map_Kd -bm 0.2`) has no file
                // name in it, and this takes the option's value for one.
                // The alternative is to implement the option grammar,
                // which is per-tool and undocumented; the cost of
                // guessing here is a name that fails to resolve, which
                // the caller was going to have to handle anyway, since
                // this crate cannot tell it whether any name resolves.
                if let Some(name) = words.last() {
                    material.maps.push(TextureMap {
                        slot,
                        name: name.to_owned(),
                    });
                }
            }
        }
    }

    if library.is_empty() {
        return Err(MeshError::Unsupported { wanted: "newmtl" });
    }
    Ok(library)
}

/// What a keyword attaches to a material.
///
/// **One lookup, so the dispatch cannot disagree with the guard.** The
/// question "does this keyword belong to a material" and the question
/// "which slot does it fill" have to be answered together or an arm
/// exists for a keyword that answers yes to the first and no to the
/// second — which is a branch no input can take and a line no test can
/// cover.
enum Property {
    /// `Ka`.
    Ambient,
    /// `Kd`.
    Diffuse,
    /// `Ks`.
    Specular,
    /// `Ke`.
    Emissive,
    /// `Ns`.
    Shininess,
    /// `d`.
    Opacity,
    /// `Tr`, the reciprocal of `d`.
    Transparency,
    /// One of the `map_*` lines, and which slot it fills.
    Map(MapSlot),
}

/// Which property a keyword names, or nothing if this reader does not
/// implement it.
fn property_of(keyword: &str) -> Option<Property> {
    match keyword {
        "Ka" => Some(Property::Ambient),
        "Kd" => Some(Property::Diffuse),
        "Ks" => Some(Property::Specular),
        "Ke" => Some(Property::Emissive),
        "Ns" => Some(Property::Shininess),
        "d" => Some(Property::Opacity),
        "Tr" => Some(Property::Transparency),
        other => map_slot(other).map(Property::Map),
    }
}

/// Which slot a `map_*` keyword fills, if any.
fn map_slot(keyword: &str) -> Option<MapSlot> {
    match keyword {
        "map_Ka" => Some(MapSlot::Ambient),
        "map_Kd" => Some(MapSlot::Diffuse),
        "map_Ks" => Some(MapSlot::Specular),
        "map_Ns" => Some(MapSlot::Shininess),
        "map_d" => Some(MapSlot::Opacity),
        "map_bump" | "map_Bump" | "bump" => Some(MapSlot::Bump),
        "norm" => Some(MapSlot::Normal),
        _ => None,
    }
}

/// Read a colour: three components, or one meaning all three.
///
/// `Kd 0.5` is the format's spelling of a grey, and it is common enough
/// in hand-written libraries that refusing it would refuse working files.
/// Anything between one and three components is read; a fourth is
/// ignored, as an extra component is everywhere else here.
fn colour<'a>(
    words: &mut impl Iterator<Item = &'a str>,
    field: &'static str,
    line: u32,
    record: u32,
) -> Result<[f32; 3], MeshError> {
    let first = scalar_word(words.next(), field, line, record)?;
    let Some(second) = words.next() else {
        return Ok([first; 3]);
    };
    let second = scalar_word(Some(second), field, line, record)?;
    let third = scalar_word(words.next(), field, line, record)?;
    Ok([first, second, third])
}

/// Read one number.
fn scalar<'a>(
    words: &mut impl Iterator<Item = &'a str>,
    field: &'static str,
    line: u32,
    record: u32,
) -> Result<f32, MeshError> {
    scalar_word(words.next(), field, line, record)
}

/// Parse one word as a finite number, or say which way it was not one.
/// `record` is which material the value belongs to, which is what
/// [`MeshError::NotFinite`] reports. Not the component within a colour:
/// a caller told "record 2" goes and looks at the third material, and
/// naming the third component instead would send it to one that may not
/// be there.
fn scalar_word(
    word: Option<&str>,
    field: &'static str,
    line: u32,
    record: u32,
) -> Result<f32, MeshError> {
    let word = word.ok_or(MeshError::NotANumber {
        found: String::new(),
        line,
    })?;
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
