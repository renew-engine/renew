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

pub use renew_mesh::accessor::BufferView;
use renew_mesh::accessor::{Accessor, AccessorError, Component, Shape};

/// The fixed head: parameters, then the buffer.
///
/// **Thirteen bytes, and it was nine.** The last four carry a buffer
/// view, so one corpus reaches both layers: a seed can hand its bytes to
/// an accessor directly, or resolve a region out of them first and hand
/// over that. Widening the head reshuffles every committed seed, which
/// costs nothing here because every one of them is generated.
pub const HEAD: usize = 13;

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
