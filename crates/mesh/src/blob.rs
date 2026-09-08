//! The canonical form a read mesh is stored in.
//!
//! Every other module here turns somebody else's format into a [`Mesh`].
//! This one turns a `Mesh` into bytes and back, so a model read once can
//! be kept, put in a pack, and loaded without parsing the original file
//! again.
//!
//! # The shape
//!
//! ```text
//! magic     8 bytes   RENEWMSH
//! version   u32       1
//! corners   u32       how many positions, which is three per triangle
//! present   u32       a bit per optional array
//! body                positions, face normals, corner normals, corner
//!                     coordinates, in that order, each little-endian f32
//! ```
//!
//! **Little-endian on every target, not native.** A blob written on one
//! machine and read on another has to be the same blob; native order
//! would make the format a property of its writer.
//!
//! # Why the optional arrays are flagged and not counted
//!
//! Each of them is either empty or exactly as long as the geometry
//! requires — one normal per triangle, one coordinate per corner — so
//! storing a length would store a number the corner count already
//! determines.
//!
//! **The first draft of this format stored all four counts**, on the
//! argument that a redundant field lets a reader refuse a contradiction
//! instead of assuming its way past one. That argument is a good one for
//! the formats beside this module, and it does not apply here, because
//! **those formats are written by other people's tools and this one is
//! written by the function below.** A redundancy that catches a foreign
//! writer's bug catches, here, only our own — which the round-trip
//! property beside it already catches, at build time, for nothing.
//!
//! What this reader actually has to survive is a byte string somebody
//! chose, and against that, fewer independently settable numbers is
//! strictly better: every redundant field is one more thing that can be
//! made to disagree and one more branch that has to be right about it.
//! Deriving the lengths means a corrupted blob is a byte-length
//! mismatch, which is one refusal with one meaning.
//!
//! # What it does not hold
//!
//! Materials, indices, transforms and node trees. See the crate
//! documentation: a `Mesh` is de-indexed triangles and the attributes
//! their corners carry, and this is exactly that, written down.

use crate::error::MeshError;
use crate::{Mesh, refuse_over_ceiling};

/// The eight bytes a blob opens with.
pub const MAGIC: [u8; 8] = *b"RENEWMSH";

/// The version this build writes and the only one it reads.
pub const VERSION: u32 = 1;

/// Magic, version, corner count and the presence bits.
const HEADER: usize = 20;

/// A face normal is stated for the whole triangle.
const HAS_FACE_NORMALS: u32 = 1 << 0;
/// A normal is stated per corner.
const HAS_CORNER_NORMALS: u32 = 1 << 1;
/// A texture coordinate is stated per corner.
const HAS_CORNER_TEXCOORDS: u32 = 1 << 2;
/// Every bit this version defines. Anything outside it belongs to a
/// version this build does not implement.
const KNOWN_FLAGS: u32 = HAS_FACE_NORMALS | HAS_CORNER_NORMALS | HAS_CORNER_TEXCOORDS;

/// Bytes one three-component vector occupies.
const VEC3: usize = 12;
/// Bytes one two-component vector occupies.
const VEC2: usize = 8;

/// Write a mesh as a blob.
///
/// The output is a function of the mesh alone: the same mesh gives the
/// same bytes on every target, which is what lets a blob be compared,
/// cached and digested by whatever stores it.
///
/// # Contract
///
/// The mesh must hold the invariants [`Mesh`] documents and every reader
/// here guarantees: whole triangles, at least one, each optional array
/// empty or exactly its full length, every float finite, and a corner
/// count inside the ceiling. **A mesh from any reader in this crate
/// satisfies all five**; a mesh built by hand is the caller's to vouch
/// for, which is what `Mesh`'s own documentation says.
///
/// **Violating it is a defect and asserts in a development build**
/// rather than being handled, because there is no handling that helps.
/// The lengths are what tell the reader where each array begins, so a
/// mesh whose arrays disagree with its corner count does not fail to
/// write MM it writes a *different, well-formed mesh*. One with a single
/// corner normal and six texture coordinates comes back with three of
/// each, silently, and nothing downstream can tell. A refusal would be
/// kinder than that and an assertion is kinder still, because it fires
/// where the mistake is rather than where it is noticed.
#[must_use]
pub fn write(mesh: &Mesh) -> Vec<u8> {
    debug_assert!(
        !mesh.positions.is_empty() && mesh.positions.len().is_multiple_of(3),
        "a mesh is whole triangles and at least one: {} positions",
        mesh.positions.len()
    );
    debug_assert!(
        mesh.face_normals.is_empty() || mesh.face_normals.len() * 3 == mesh.positions.len(),
        "a face normal per triangle or none: {} normals for {} positions",
        mesh.face_normals.len(),
        mesh.positions.len()
    );
    debug_assert!(
        mesh.corner_normals.is_empty() || mesh.corner_normals.len() == mesh.positions.len(),
        "a normal per corner or none: {} normals for {} corners",
        mesh.corner_normals.len(),
        mesh.positions.len()
    );
    debug_assert!(
        mesh.corner_texcoords.is_empty() || mesh.corner_texcoords.len() == mesh.positions.len(),
        "a coordinate per corner or none: {} coordinates for {} corners",
        mesh.corner_texcoords.len(),
        mesh.positions.len()
    );
    debug_assert!(
        mesh.positions
            .iter()
            .chain(&mesh.face_normals)
            .chain(&mesh.corner_normals)
            .flatten()
            .chain(mesh.corner_texcoords.iter().flatten())
            .all(|value| value.is_finite()),
        "every float in a mesh is finite, and a reader will refuse one that is not"
    );
    debug_assert!(
        refuse_over_ceiling(0, mesh.positions.len()).is_ok(),
        "a mesh past the ceiling writes a blob no reader here will take back"
    );
    let mut present = 0;
    if !mesh.face_normals.is_empty() {
        present |= HAS_FACE_NORMALS;
    }
    if !mesh.corner_normals.is_empty() {
        present |= HAS_CORNER_NORMALS;
    }
    if !mesh.corner_texcoords.is_empty() {
        present |= HAS_CORNER_TEXCOORDS;
    }

    // The ceiling asserted above is far below `u32::MAX`, so this is
    // exact for every mesh that satisfies the contract. Saturating is
    // what a release build does with one that does not, and it is
    // deliberately not silent: the count it writes will not match the
    // body, so `read` refuses with `CountMismatch` rather than handing
    // anybody a mesh that was never written.
    let corners = u32::try_from(mesh.positions.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(HEADER + mesh.positions.len() * VEC3);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&corners.to_le_bytes());
    out.extend_from_slice(&present.to_le_bytes());

    for value in mesh.positions.iter().flatten() {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in mesh.face_normals.iter().flatten() {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in mesh.corner_normals.iter().flatten() {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in mesh.corner_texcoords.iter().flatten() {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// Read a blob back into a mesh.
///
/// # Errors
///
/// Returns the [`MeshError`] naming what the bytes got wrong: too short
/// to hold a header, an opening that is not this format, a version or a
/// flag this build does not implement, a corner count that is not whole
/// triangles, is past the ceiling, or accounts for a different number of
/// bytes than are present, a coordinate that is not finite, or no
/// geometry at all.
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    if bytes.len() < HEADER {
        return Err(MeshError::TooShortForHeader {
            needs: HEADER,
            len: bytes.len(),
        });
    }
    if bytes.get(..MAGIC.len()) != Some(&MAGIC[..]) {
        return Err(MeshError::ExpectedKeyword {
            expected: "RENEWMSH",
            found: String::new(),
            line: 1,
        });
    }

    let version = u32_at(bytes, 8);
    if version != VERSION {
        return Err(MeshError::Unsupported {
            wanted: "a blob of version 1",
        });
    }
    let present = u32_at(bytes, 16);
    if present & !KNOWN_FLAGS != 0 {
        // A bit outside this version's vocabulary is a blob a later
        // build wrote, which is well-formed and unusable here rather
        // than malformed.
        return Err(MeshError::Unsupported {
            wanted: "a blob using only the arrays this version defines",
        });
    }

    let corners = usize::try_from(u32_at(bytes, 12)).unwrap_or(usize::MAX);
    if corners == 0 {
        return Err(MeshError::NoGeometry);
    }
    if corners % 3 != 0 {
        // The last face named one or two corners, and neither covers any
        // area.
        return Err(MeshError::NotAFace {
            face: u32::try_from(corners / 3).unwrap_or(u32::MAX),
            corners: corners % 3,
        });
    }
    // **This is an overflow guard, and the comment here used to call it
    // an amplification guard, which it is not.**
    //
    // There is no amplification to stop. The length check below requires
    // the byte total the header implies to EQUAL the file's own length,
    // so the bytes this reader allocates are the bytes it was handed
    // minus a twenty-byte header: the factor is exactly one, and the
    // worst case from a twenty-byte file is nothing at all. Swept, not
    // argued: every multiple-of-three count the ceiling admits, in a
    // header-only file with every presence bit set, refuses before it
    // reserves anything.
    //
    // What the ceiling really does is run BEFORE `wanted` is computed,
    // so `corners * VEC3` cannot overflow on a target whose pointers are
    // narrower than the count. That is worth keeping and worth naming.
    refuse_over_ceiling(0, corners)?;

    let triangles = corners / 3;
    let mut wanted = HEADER + corners * VEC3;
    if present & HAS_FACE_NORMALS != 0 {
        wanted += triangles * VEC3;
    }
    if present & HAS_CORNER_NORMALS != 0 {
        wanted += corners * VEC3;
    }
    if present & HAS_CORNER_TEXCOORDS != 0 {
        wanted += corners * VEC2;
    }
    if wanted != bytes.len() {
        return Err(MeshError::CountMismatch {
            declared: wanted as u64,
            actual: bytes.len(),
            count: u32::try_from(corners).unwrap_or(u32::MAX),
        });
    }

    let mut at = HEADER;
    let positions = triples(bytes, &mut at, corners, "position")?;
    let face_normals = if present & HAS_FACE_NORMALS != 0 {
        triples(bytes, &mut at, triangles, "normal")?
    } else {
        Vec::new()
    };
    let corner_normals = if present & HAS_CORNER_NORMALS != 0 {
        triples(bytes, &mut at, corners, "normal")?
    } else {
        Vec::new()
    };
    let corner_texcoords = if present & HAS_CORNER_TEXCOORDS != 0 {
        pairs(bytes, &mut at, corners, "texture coordinate")?
    } else {
        Vec::new()
    };

    Ok(Mesh {
        positions,
        face_normals,
        corner_normals,
        corner_texcoords,
    })
}

/// Four bytes as a little-endian `u32`.
///
/// The caller has already checked the header's length, and every offset
/// this is asked for is inside it.
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    let mut word = [0; 4];
    word.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(word)
}

/// One little-endian `f32`, refused if it is not a finite number.
fn float(bytes: &[u8], at: usize, field: &'static str, record: usize) -> Result<f32, MeshError> {
    let mut word = [0; 4];
    word.copy_from_slice(&bytes[at..at + 4]);
    let value = f32::from_le_bytes(word);
    if value.is_finite() {
        Ok(value)
    } else {
        Err(MeshError::NotFinite {
            field,
            index: u32::try_from(record).unwrap_or(u32::MAX),
        })
    }
}

/// `count` three-component vectors, advancing the cursor past them.
///
/// The length check above has already proved these bytes are there, so
/// the reads below are inside the slice by arithmetic rather than by
/// hope. Indexing rather than `get` says so: a bound that a preceding
/// check has already established does not want a second, unreachable
/// arm asking about it again.
fn triples(
    bytes: &[u8],
    at: &mut usize,
    count: usize,
    field: &'static str,
) -> Result<Vec<[f32; 3]>, MeshError> {
    let mut out = Vec::with_capacity(count);
    for record in 0..count {
        out.push([
            float(bytes, *at, field, record)?,
            float(bytes, *at + 4, field, record)?,
            float(bytes, *at + 8, field, record)?,
        ]);
        *at += VEC3;
    }
    Ok(out)
}

/// `count` two-component vectors, advancing the cursor past them.
fn pairs(
    bytes: &[u8],
    at: &mut usize,
    count: usize,
    field: &'static str,
) -> Result<Vec<[f32; 2]>, MeshError> {
    let mut out = Vec::with_capacity(count);
    for record in 0..count {
        out.push([
            float(bytes, *at, field, record)?,
            float(bytes, *at + 4, field, record)?,
        ]);
        *at += VEC2;
    }
    Ok(out)
}
