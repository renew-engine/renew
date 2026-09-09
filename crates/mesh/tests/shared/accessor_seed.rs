//! One byte string, one accessor: the encoding three harnesses share.
//!
//! **This file is included by three targets in two crates**, with
//! `#[path]`, and it exists because none of them can reach the others'
//! code any other way:
//!
//! * `crates/mesh/examples/make_accessor_corpus.rs` writes the seeds.
//! * `crates/mesh/tests/corpus_replay.rs` replays them at every merge.
//! * `fuzz/fuzz_targets/accessor_view.rs` mutates them.
//!
//! Cargo compiles a `tests/` subdirectory for nobody, which is what
//! makes this a shared file rather than a fourth test target.
//!
//! # Why an encoding exists at all
//!
//! Every other fuzz target in this tree takes bytes that **are** a
//! format — a PNG, a WAV, a PLY, a document. An accessor is not a
//! format: it is a byte region *plus six parameters*, and a fuzzer that
//! could only vary the region would never reach a single one of the
//! refusals about the parameters. So the parameters are carried in a
//! fixed nine-byte head and the region is whatever follows.
//!
//! **The head is deliberately unhelpful.** It does no validation and
//! clamps nothing: a stride of 65535, an offset past the region, a
//! component code the format never defined, and a count of zero are all
//! reachable, because each of them is a refusal somebody has to be able
//! to provoke. Every value that reaches the reader reaches it as the
//! file's own claim.

#![allow(
    dead_code,
    reason = "each of the three includers uses a different half of this"
)]

use renew_mesh::Mesh;
pub use renew_mesh::accessor::BufferView;
use renew_mesh::accessor::{Accessor, AccessorError, Component, Shape};
use renew_mesh::primitive::{self, Mode, Primitive};

/// The fixed head: parameters, then the buffer.
///
/// **Seventeen bytes, grown twice from nine.** Four carry a buffer view
/// and four carry an index stream, so one corpus reaches three layers: a
/// seed can hand its bytes to an accessor directly, resolve a region out
/// of them first, or assemble positions and indices out of the same
/// buffer into geometry. Widening the head reshuffles every committed
/// seed, which costs nothing here because every one of them is
/// generated.
///
/// # What the assembly half deliberately does not reach
///
/// A seed that assembles forces three-component float positions and
/// triangles, and carries no normals or texture coordinates. **The
/// refusals it therefore cannot provoke — a stream of the wrong shape, a
/// mode this reader does not draw, two streams of different lengths —
/// are structural rather than arithmetic**, and the suite beside the
/// crate provokes every one of them deterministically. What a fuzzer
/// adds here is the de-indexing loop: index values are attacker-chosen
/// numbers used to address another stream, and that is the one place at
/// this layer where a wrong comparison reads out of bounds.
pub const HEAD: usize = 17;

/// The parameters as the head spells them, before anything judges them.
///
/// The component is kept as its **code** rather than as a
/// [`Component`], so that a code outside the format's table survives
/// this far and is refused by the reader rather than by the harness.
#[derive(Clone, Copy, Debug)]
pub struct Seed<'a> {
    /// The component type code, as the file would spell it.
    pub code: u32,
    /// The element shape.
    pub shape: Shape,
    /// How many elements the accessor claims.
    pub count: usize,
    /// Where the first one starts.
    pub byte_offset: usize,
    /// The declared stride, if the head sets its flag.
    pub byte_stride: Option<usize>,
    /// Whether integers are fractions of their own range.
    pub normalized: bool,
    /// The view to resolve out of the bytes first, when the head asks
    /// for one.
    ///
    /// `None` hands the bytes to the accessor directly, which is what
    /// every seed did before this field existed. `Some` puts the layer
    /// that decides whether a region is really inside its buffer in
    /// front of the layer that decides whether elements are really
    /// inside a region — the two claims catalogue entry 23 keeps apart.
    pub view: Option<BufferView>,
    /// The index stream to assemble geometry with, when the head asks
    /// for one.
    ///
    /// `Some(count)` builds a primitive out of the same buffer: the
    /// accessor's own parameters become the positions, and `count`
    /// unsigned sixteen-bit indices are read from `index_offset`.
    pub assemble: Option<usize>,
    /// Where the index stream starts, when there is one.
    pub index_offset: usize,
    /// Whether to read the region as element addresses rather than as
    /// attributes.
    ///
    /// **Both entry points or half the layer goes unfuzzed.** `indices`
    /// refuses two things `view` does not — a component that cannot
    /// address anything, and one marked normalised — and a corpus that
    /// only ever called `view` could not reach either.
    pub as_indices: bool,
    /// Everything after the head.
    pub region: &'a [u8],
}

/// Read the head, or `None` when there is not one.
///
/// **The only reason this answers `None`.** Every other way the bytes
/// can be wrong is a claim for the reader to refuse, which is the whole
/// point of the split.
pub fn decode(bytes: &[u8]) -> Option<Seed<'_>> {
    let head = bytes.get(..HEAD)?;
    let region = bytes.get(HEAD..)?;

    // Eight codes across a table of six, so the two the format leaves
    // out — 5124 and 5127 — are as reachable as the ones it defines.
    let code = 5120 + u32::from(head[0] % 8);
    let shape = match head[1] % 4 {
        0 => Shape::Scalar,
        1 => Shape::Vec2,
        2 => Shape::Vec3,
        _ => Shape::Vec4,
    };
    let count = usize::from(u16::from_le_bytes([head[2], head[3]]));
    let byte_offset = usize::from(u16::from_le_bytes([head[4], head[5]]));
    let stride = usize::from(u16::from_le_bytes([head[6], head[7]]));
    let flags = head[8];
    let view_offset = usize::from(u16::from_le_bytes([head[9], head[10]]));
    let view_length = usize::from(u16::from_le_bytes([head[11], head[12]]));
    let index_count = usize::from(u16::from_le_bytes([head[13], head[14]]));
    let index_offset = usize::from(u16::from_le_bytes([head[15], head[16]]));
    let byte_stride = (flags & 1 != 0).then_some(stride);

    Some(Seed {
        code,
        shape,
        count,
        byte_offset,
        byte_stride,
        // The view shares the accessor's stride, because that is where
        // the format puts it and where a caller assembling from a
        // document would copy it from.
        view: (flags & 8 != 0).then_some(BufferView {
            byte_offset: view_offset,
            byte_length: view_length,
            byte_stride,
        }),
        assemble: (flags & 16 != 0).then_some(index_count),
        index_offset,
        normalized: flags & 2 != 0,
        as_indices: flags & 4 != 0,
        region,
    })
}

/// Write a seed back out as bytes `decode` reads.
///
/// Used only by the generator, and kept beside `decode` so the two
/// cannot drift: a round-trip test in the replay gate holds them
/// together.
#[must_use]
pub fn encode(seed: &Seed<'_>) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEAD + seed.region.len());
    let selector = u8::try_from(seed.code.saturating_sub(5120)).unwrap_or(0);
    out.push(selector);
    out.push(match seed.shape {
        Shape::Scalar => 0,
        Shape::Vec2 => 1,
        Shape::Vec3 => 2,
        Shape::Vec4 => 3,
    });
    out.extend_from_slice(&u16::try_from(seed.count).unwrap_or(u16::MAX).to_le_bytes());
    out.extend_from_slice(
        &u16::try_from(seed.byte_offset)
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(
        &u16::try_from(seed.byte_stride.unwrap_or(0))
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    let mut flags = 0u8;
    if seed.byte_stride.is_some() {
        flags |= 1;
    }
    if seed.normalized {
        flags |= 2;
    }
    if seed.as_indices {
        flags |= 4;
    }
    if seed.view.is_some() {
        flags |= 8;
    }
    if seed.assemble.is_some() {
        flags |= 16;
    }
    out.push(flags);
    let view = seed.view.unwrap_or(BufferView {
        byte_offset: 0,
        byte_length: 0,
        byte_stride: None,
    });
    out.extend_from_slice(
        &u16::try_from(view.byte_offset)
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(
        &u16::try_from(view.byte_length)
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(
        &u16::try_from(seed.assemble.unwrap_or(0))
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(
        &u16::try_from(seed.index_offset)
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(seed.region);
    out
}

impl Seed<'_> {
    /// The accessor this seed claims, or the refusal its component code
    /// earns.
    ///
    /// # Errors
    ///
    /// [`AccessorError::UnknownComponentType`] for a code outside the
    /// format's table. **Asked through the reader's own function**, so
    /// the harness never keeps a second copy of that table.
    pub fn accessor(&self) -> Result<Accessor, AccessorError> {
        Ok(Accessor {
            component: Component::from_code(self.code)?,
            shape: self.shape,
            count: self.count,
            byte_offset: self.byte_offset,
            byte_stride: self.byte_stride,
            normalized: self.normalized,
        })
    }
}

impl<'a> Seed<'a> {
    /// The bytes the accessor is checked against: the whole region, or
    /// the part of it this seed's view resolves to.
    ///
    /// # Errors
    ///
    /// Whatever [`BufferView::resolve`] refuses, when there is a view.
    pub fn bytes(&self) -> Result<&'a [u8], AccessorError> {
        match self.view {
            Some(view) => view.resolve(self.region),
            None => Ok(self.region),
        }
    }
}

impl Seed<'_> {
    /// Assemble this seed's buffer into geometry.
    ///
    /// Positions are forced to three-component floats and the mode to
    /// triangles, so the fuzzer spends its time on index values rather
    /// than bouncing off a shape refusal it can reach in one byte.
    ///
    /// # Errors
    ///
    /// The **name** of whatever the accessor, the index accessor or the
    /// assembly refuses. A name rather than a refusal because the three
    /// layers speak two different vocabularies on purpose, and inventing
    /// a conversion between them so that a harness could hold one value
    /// would put a harness's convenience into the crate's public API.
    pub fn assembled(&self, region: &[u8]) -> Result<Mesh, &'static str> {
        let count = self.assemble.unwrap_or_default();
        let positions = Accessor {
            component: Component::F32,
            shape: Shape::Vec3,
            count: self.count,
            byte_offset: self.byte_offset,
            byte_stride: self.byte_stride,
            normalized: false,
        }
        .view(region)
        .map_err(AccessorError::name)?;

        let order = Accessor {
            component: Component::U16,
            shape: Shape::Scalar,
            count,
            byte_offset: self.index_offset,
            byte_stride: None,
            normalized: false,
        }
        .indices(region)
        .map_err(AccessorError::name)?;

        primitive::build(&Primitive {
            mode: Mode::Triangles,
            positions,
            normals: None,
            texcoords: None,
            indices: Some(order),
        })
        .map_err(|refusal| refusal.name())
    }
}

/// The answer a seed gets, as a name.
///
/// `Ok` for a claim the reader accepts, `NoHead` for bytes too short to
/// carry parameters at all, and the refusal's own name otherwise —
/// **including a view's**, which is the layer in front of the accessor
/// rather than a different kind of answer.
#[must_use]
pub fn outcome(bytes: &[u8]) -> &'static str {
    let Some(seed) = decode(bytes) else {
        return "NoHead";
    };
    let region = match seed.bytes() {
        Ok(region) => region,
        Err(refusal) => return refusal.name(),
    };
    if seed.assemble.is_some() {
        return match seed.assembled(region) {
            Err(name) => name,
            Ok(_) => "Ok",
        };
    }
    match seed.accessor() {
        Err(refusal) => refusal.name(),
        Ok(accessor) if seed.as_indices => match accessor.indices(region) {
            Err(refusal) => refusal.name(),
            Ok(_) => "Ok",
        },
        Ok(accessor) => match accessor.view(region) {
            Err(refusal) => refusal.name(),
            Ok(_) => "Ok",
        },
    }
}
