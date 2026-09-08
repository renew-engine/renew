//! The pure half: quads in, packed vertex bytes and indices out.
//!
//! Nothing here calls a device. That is what makes the arithmetic — the
//! packing, the winding, the index numbering — testable on a machine with
//! no adapter, which is most machines and every sanitizer lane.
//!
//! **This file goes further than that and names the rendering crate
//! nowhere at all** — it has no `use` statements. The property the 2D
//! sibling states for its own pure half is the weaker one, *no device
//! calls*, because `render2d/src/fill.rs` really does name `Extent`.
//! Both properties are worth having and they are not the same one; what
//! is written here is what this file does. The stride constant below is
//! the seam that keeps it so: it repeats a number the rendering crate
//! also knows rather than importing it, and `gpu.rs` is where the two are
//! checked against each other.
//!
//! # Push order is index order is draw order
//!
//! Quads are appended, and their indices are emitted in the order they
//! were pushed. There is no sort, no batching and no depth pre-pass, so
//! two scenes built by the same sequence of calls produce byte-identical
//! buffers — which is the whole of this crate's contribution to the
//! frame being reproducible. A caller who wants a different order pushes
//! in a different order.

/// Bytes in one vertex record: a three-float position and a four-float
/// colour, packed with no padding.
///
/// **Not a `#[repr(C)]` struct, and that is not a style choice.** The
/// maths crate's `Vec4` is `#[repr(C, align(16))]`; its `Vec3` is twelve
/// bytes at align four. A `#[repr(C)]` record of the two therefore pads
/// the `Vec3` out to the sixteen-byte boundary the `Vec4` demands and
/// occupies **thirty-two** bytes, not twenty-eight — the alignment of one
/// field, not of both, is what does it. The rendering crate asserts at
/// the moment a draw is recorded that a mesh's stride equals the stride
/// the pipeline's per-vertex layout packs to, so a padded record would
/// fail at the draw rather than here, a long way from the mistake.
/// Writing the bytes explicitly makes the layout the code's subject
/// rather than the compiler's.
///
/// The alignment claim is about a crate this one does not depend on, so
/// nothing compiles it. It is stated as the reason for a decision, not
/// relied on: what the code relies on is the assertion in `gpu.rs` that
/// this constant equals the packed width of the layout actually declared.
pub(crate) const VERTEX_STRIDE: u32 = 64;

/// The mapping [`Scene::quad`] and [`Scene::quad_shaded`] supply when the
/// caller says nothing: the four corners onto the four corners of the
/// unit square, in the order the corners come.
///
/// **A whole tile rather than a point.** Zero everywhere would collapse a
/// textured draw onto one texel — a quad in a single colour, which looks
/// like a working texture and is not one. Stretching the whole image
/// across the quad is the answer a caller who supplied no coordinates
/// most likely wanted, and it is visibly wrong rather than plausibly
/// wrong if they did not.
const WHOLE_TILE: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

/// The unit normal of the plane through three corners, in the winding
/// they are given.
///
/// **Computed rather than asked for, and that is a decision with a cost
/// worth naming.** Nothing in this crate has ever carried a normal, so
/// no caller has one to give; computing it means every existing quad
/// gains a correct normal with no caller change, and the alternative —
/// a zero placeholder — would put a value in the buffer that means
/// "no direction" and reads as a direction.
///
/// **A degenerate triangle has no plane, and this says so by returning
/// zero rather than by dividing by it.** Three collinear or coincident
/// corners give a zero cross product; normalising that is a NaN, which
/// would travel into a vertex buffer and out again as a lighting term
/// nobody can trace. Zero is not a direction either, but it is a value
/// a reader can test for, and a degenerate triangle covers no pixels
/// so nothing samples it.
fn face_normal(first: [f32; 3], second: [f32; 3], third: [f32; 3]) -> [f32; 3] {
    let edge = [
        second[0] - first[0],
        second[1] - first[1],
        second[2] - first[2],
    ];
    let other = [
        third[0] - first[0],
        third[1] - first[1],
        third[2] - first[2],
    ];
    let cross = [
        edge[1].mul_add(other[2], -(edge[2] * other[1])),
        edge[2].mul_add(other[0], -(edge[0] * other[2])),
        edge[0].mul_add(other[1], -(edge[1] * other[0])),
    ];
    let length = cross[0]
        .mul_add(cross[0], cross[1].mul_add(cross[1], cross[2] * cross[2]))
        .sqrt();
    if length > 0.0 && length.is_finite() {
        [cross[0] / length, cross[1] / length, cross[2] / length]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// The tangent frame of a surface: which way it faces, and which way its
/// texture's `u` axis runs across it.
///
/// **A normal alone is not enough to read a normal map.** A normal map
/// stores directions in the surface's own space, so a shader needs three
/// axes to move them into the world: the normal, a tangent along
/// increasing `u`, and a bitangent along increasing `v`. The third is not
/// stored, because it is `cross(normal, tangent) * handedness` and the
/// sign is the only part of it that is not already known $M which is
/// exactly what glTF's own `TANGENT` accessor does, and why this is four
/// floats rather than six.
///
/// **Why a type rather than two more parameters.** The two travel
/// together and are only correct together: a tangent orthogonalised
/// against one normal and written beside another describes a surface that
/// does not exist. Making them one value means an appender takes one
/// extra argument now and the same one argument if a third axis is ever
/// stored, and it gives a caller that already knows its frame somewhere
/// to say so.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// The unit normal.
    pub normal: [f32; 3],
    /// The unit tangent in `xyz`, and in `w` the sign the bitangent is
    /// reconstructed with: `cross(normal, tangent) * w`.
    pub tangent: [f32; 4],
}

impl Frame {
    /// The frame with no direction in it: what a surface that has no
    /// plane, or no usable texture mapping, gets.
    ///
    /// Zero rather than an arbitrary axis, for the reason
    /// [`face_normal`] returns zero: a value that is not a direction is
    /// testable, and a plausible direction is a lighting term nobody can
    /// trace back to the geometry that produced it.
    pub const NOWHERE: Self = Self {
        normal: [0.0, 0.0, 0.0],
        tangent: [0.0, 0.0, 0.0, 1.0],
    };

    /// The frame of the plane through three corners carrying those three
    /// texture coordinates.
    ///
    /// The normal is the plane's, in the winding given. The tangent is
    /// the direction `u` increases in, found by solving the two edge
    /// vectors against their coordinate deltas, then made perpendicular
    /// to the normal and unit length. The handedness is the sign of the
    /// bitangent that solve produced, so a mirrored mapping $M the same
    /// island flipped, which every atlas packer emits sooner or later $M
    /// keeps its lighting instead of inverting it.
    ///
    /// **Every way this can fail returns [`NOWHERE`] rather than a NaN.**
    /// Collinear corners have no plane; corners whose coordinates are
    /// collinear in `uv` space (all three equal, or a whole face mapped
    /// to one texel) give a zero determinant and no direction for `u`;
    /// a tangent parallel to the normal survives neither. A NaN here
    /// would reach a vertex buffer and come out as lighting, so each of
    /// those is a branch rather than a division.
    ///
    /// [`NOWHERE`]: Self::NOWHERE
    #[must_use]
    pub fn of_face(corners: [[f32; 3]; 3], uvs: [[f32; 2]; 3]) -> Self {
        let normal = face_normal(corners[0], corners[1], corners[2]);
        if normal == [0.0, 0.0, 0.0] {
            return Self::NOWHERE;
        }
        let edge = sub(corners[1], corners[0]);
        let other = sub(corners[2], corners[0]);
        let (du1, dv1) = (uvs[1][0] - uvs[0][0], uvs[1][1] - uvs[0][1]);
        let (du2, dv2) = (uvs[2][0] - uvs[0][0], uvs[2][1] - uvs[0][1]);
        // The determinant of the coordinate deltas. Zero means the three
        // coordinates lie on a line, which fixes no direction for `u`.
        //
        // **Deleting this check reddens nothing, which was measured
        // rather than assumed.** A zero determinant makes the reciprocal
        // below infinite, every component of `along_u` infinite or NaN,
        // and the guard in `unit` refuses it four operations later — so
        // the behaviour is identical either way. It stays because those
        // four operations are the difference between code that names the
        // condition it is refusing and code that relies on a NaN
        // arriving somewhere else intact, and because the two guards
        // refuse different things: this one a mapping with no direction
        // in it, that one a reciprocal too large to use.
        let det = du1.mul_add(dv2, -(du2 * dv1));
        if det == 0.0 || !det.is_finite() {
            return Self {
                normal,
                ..Self::NOWHERE
            };
        }
        let inverse = 1.0 / det;
        let along_u = [
            edge[0].mul_add(dv2, -(other[0] * dv1)) * inverse,
            edge[1].mul_add(dv2, -(other[1] * dv1)) * inverse,
            edge[2].mul_add(dv2, -(other[2] * dv1)) * inverse,
        ];
        let along_v = [
            other[0].mul_add(du1, -(edge[0] * du2)) * inverse,
            other[1].mul_add(du1, -(edge[1] * du2)) * inverse,
            other[2].mul_add(du1, -(edge[2] * du2)) * inverse,
        ];
        // **No Gram-Schmidt step, because there is nothing to correct.**
        // `along_u` is a linear combination of `edge` and `other`, both of
        // which lie in the plane, so it lies in the plane too and is
        // already perpendicular to the normal — by construction, not by
        // arithmetic. A projection was written here first and removed
        // when probing it changed no test and no byte: it is the step a
        // shader needs after interpolating two corner tangents, and this
        // crate stores one frame per face, so no interpolation has
        // happened yet when this runs.
        //
        // `unit` is what refuses the remaining case: a determinant small
        // enough that its reciprocal overflows leaves `along_u` infinite,
        // and an infinite length is not one this scales by.
        let Some(tangent) = unit(along_u) else {
            return Self {
                normal,
                ..Self::NOWHERE
            };
        };
        // Handedness: whether the bitangent the solve found agrees with
        // the one the stored pair reconstructs. A mirrored island
        // disagrees, and that sign is the whole reason `w` is stored.
        let handedness = if dot(cross(normal, tangent), along_v) < 0.0 {
            -1.0
        } else {
            1.0
        };
        Self {
            normal,
            tangent: [tangent[0], tangent[1], tangent[2], handedness],
        }
    }
}

fn sub(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0].mul_add(right[0], left[1].mul_add(right[1], left[2] * right[2]))
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1].mul_add(right[2], -(left[2] * right[1])),
        left[2].mul_add(right[0], -(left[0] * right[2])),
        left[0].mul_add(right[1], -(left[1] * right[0])),
    ]
}

/// `vector` scaled to unit length, or `None` when it has none to scale.
fn unit(vector: [f32; 3]) -> Option<[f32; 3]> {
    let length = dot(vector, vector).sqrt();
    if length > 0.0 && length.is_finite() {
        Some([vector[0] / length, vector[1] / length, vector[2] / length])
    } else {
        None
    }
}

/// The place a packed vertex record names.
///
/// The first twelve bytes are the three position floats in native
/// order, exactly as [`Scene::push_vertex`] writes them; this is the
/// only reader of that layout outside the upload.
///
/// The record is a whole one by its type, so this cannot fail and does
/// not pretend it might: taking `[u8; VERTEX_STRIDE]` rather than a
/// slice moves the "is this a complete vertex" question to the split
/// that produced it, where it is answered once for the whole buffer
/// instead of re-asked, unanswerably, per corner.
///
/// Position is the first three words, and the zip stops there — the
/// colour and texture words that follow are not places.
fn position_of(record: &[u8; VERTEX_STRIDE as usize]) -> [f32; 3] {
    let (words, _) = record.as_chunks::<{ size_of::<f32>() }>();
    let mut at = [0.0_f32; 3];
    for (slot, word) in at.iter_mut().zip(words) {
        *slot = f32::from_ne_bytes(*word);
    }
    at
}

/// Geometry accumulated on the host, ready to be uploaded once.
///
/// Cheap to build and cheap to throw away: a scene owns two vectors and
/// nothing else, holds no device, and can be built on a machine with no
/// adapter at all.
#[derive(Debug, Clone, Default)]
pub struct Scene {
    vertices: Vec<u8>,
    indices: Vec<u32>,
}

impl Scene {
    /// An empty scene.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A scene sized for `quads` up front, so a caller that knows its
    /// count allocates once.
    ///
    /// A hint, not a limit: pushing past it grows the buffers like any
    /// other vector. What it saves is that growth — a vector that starts
    /// empty reaches four thousand faces in roughly a dozen doublings,
    /// each one copying everything written before it — and not one
    /// allocation per quad, which is what an earlier version of this
    /// sentence claimed and no vector has ever done.
    ///
    /// **Its only caller today is the benchmark that measures what it
    /// saves.** The voxel sample builds every scene with [`Scene::new`],
    /// including the two on its draw path. That is recorded here rather
    /// than left implied: a constructor whose documentation describes a
    /// caller it does not have is a claim about the tree, and this one
    /// was wrong for as long as it stood.
    ///
    /// # Panics
    ///
    /// If `quads` is large enough that the byte count overflows, or that
    /// the reservation itself cannot be made — the same conditions
    /// [`Vec::with_capacity`] panics on, reached through the multiply.
    /// A hint that cannot be honoured is a caller's arithmetic mistake,
    /// not a condition to thread a result type through the constructor
    /// for.
    #[must_use]
    pub fn with_capacity(quads: usize) -> Self {
        Self {
            vertices: Vec::with_capacity(quads * 4 * VERTEX_STRIDE as usize),
            indices: Vec::with_capacity(quads * 6),
        }
    }

    /// Append one quad in `colour`, as two triangles.
    ///
    /// Corners are in clip space and are taken in order — the two
    /// triangles are `0,1,2` and `0,2,3`, so a caller listing its corners
    /// around the perimeter gets a quad and one listing them crosswise
    /// gets a bow tie. That is the caller's arithmetic, not this crate's
    /// to second-guess: there is no winding check here, because the
    /// pipeline culls nothing in v0 and a quad wound either way draws.
    ///
    /// **Clip space, because v0 has no camera.** Positions go to the
    /// vertex stage unmodified. A projection is a later step, and until
    /// it exists a caller drawing a world transforms on its own side.
    pub fn quad(&mut self, corners: [[f32; 3]; 4], colour: [f32; 4]) {
        self.quad_shaded(corners, [colour; 4]);
    }

    /// The same quad with a colour for each corner, interpolated across
    /// it by the rasterizer.
    ///
    /// **The vertex format always allowed this**; [`Self::quad`] simply
    /// did not offer it, and a caller that wanted corner-varying colour
    /// had to push vertices itself and get the winding right.
    ///
    /// What it is for: shading that belongs to the *geometry* rather than
    /// to the surface. Corner darkening where blocks meet is the obvious
    /// case — a flat-coloured world has no cue at all for an inner
    /// corner, because two faces of the same colour meeting at one is
    /// indistinguishable from one flat face.
    ///
    /// Corners are in the same order as the positions: the colour at
    /// index `i` belongs to the corner at index `i`. The two triangles
    /// share the diagonal from corner 0 to corner 2, so a quad whose
    /// corner colours disagree is shaded slightly differently on either
    /// side of that diagonal. That is inherent to drawing a quad as two
    /// triangles and is not hidden here.
    pub fn quad_shaded(&mut self, corners: [[f32; 3]; 4], colours: [[f32; 4]; 4]) {
        self.quad_uv(corners, colours, WHOLE_TILE);
    }

    /// The same quad with a texture coordinate for each corner.
    ///
    /// **The pair is per-vertex because that is the only place it can
    /// be.** A voxel face's six neighbours want six different tiles from
    /// one atlas, and a per-draw uniform would mean a draw per face.
    ///
    /// Corners, colours and coordinates are all in the same order: index
    /// `i` of each belongs to corner `i`. What the coordinates *mean* —
    /// which atlas, laid out how — is the caller's business entirely;
    /// this crate packs two floats and says nothing about them.
    ///
    /// The colour is not replaced by the texture: they multiply in the
    /// shader that samples, so corner darkening and a tile survive each
    /// other.
    pub fn quad_uv(&mut self, corners: [[f32; 3]; 4], colours: [[f32; 4]; 4], uvs: [[f32; 2]; 4]) {
        // Recorded before the push, so the triangles below index the
        // corners this call adds rather than whatever came before.
        // One frame for the whole quad: its four corners are coplanar
        // by construction here, and a caller that wants them not to be
        // is drawing two triangles rather than a quad.
        let frame = Frame::of_face(
            [corners[0], corners[1], corners[2]],
            [uvs[0], uvs[1], uvs[2]],
        );
        self.quad_uv_with_frame(corners, colours, uvs, frame);
    }

    /// The same quad with its tangent frame supplied rather than derived.
    ///
    /// **For the caller that already knows.** [`quad_uv`] spends a cross
    /// product, two square roots and a handful of divides per face
    /// recovering a frame from the corners; a caller that built those
    /// corners from an axis basis, or read the frame out of a file that
    /// carries one, is paying to be told what it already knew. The voxel
    /// sample in this repository is the first of those and a glTF
    /// importer is the second, which is why this exists now rather than
    /// when a second caller appeared.
    ///
    /// The frame is written to all four corners unchanged. Nothing
    /// normalises it or checks it against the corners — a caller passing
    /// this is asserting it knows better, and a crate that re-derived the
    /// answer to check would cost exactly what this call is for.
    ///
    /// [`quad_uv`]: Self::quad_uv
    pub fn quad_uv_with_frame(
        &mut self,
        corners: [[f32; 3]; 4],
        colours: [[f32; 4]; 4],
        uvs: [[f32; 2]; 4],
        frame: Frame,
    ) {
        // Recorded before the push, so the triangles below index the
        // corners this call adds rather than whatever came before.
        let base = self.vertex_count();
        for ((corner, colour), uv) in corners.into_iter().zip(colours).zip(uvs) {
            self.push_vertex(corner, colour, uv, frame);
        }
        for offset in [0, 1, 2, 0, 2, 3] {
            self.indices.push(base + offset);
        }
    }

    /// Append one triangle, with a colour and a texture coordinate for
    /// each of its three corners.
    ///
    /// **Every mesh format in the world emits triangles, and until this
    /// existed there was no way to give one to a scene.** The quad
    /// family above is what a voxel world wants and what this crate was
    /// built for; a face imported from a file is a triangle, and
    /// decomposing it into a degenerate quad would put a fourth vertex
    /// and two extra indices into the buffer to describe geometry that
    /// has three of each.
    ///
    /// Corners, colours and coordinates are in the same order: index `i`
    /// of each belongs to corner `i`. The winding is the order given,
    /// unchanged — the pipeline culls nothing, so a triangle wound
    /// either way draws, and a caller importing a file keeps whatever
    /// its source said.
    ///
    /// The normal is computed from the three corners rather than taken
    /// from the caller. A file that carries its own per-vertex normals
    /// is not yet expressible; when it is, this gains a sibling rather
    /// than a parameter, because a caller that has real normals wants
    /// all three and a caller that has none wants zero.
    pub fn triangle(&mut self, corners: [[f32; 3]; 3], colours: [[f32; 4]; 3], uvs: [[f32; 2]; 3]) {
        self.triangle_with_frame(corners, colours, uvs, Frame::of_face(corners, uvs));
    }

    /// The same triangle with its tangent frame supplied rather than
    /// derived — [`quad_uv_with_frame`] for three corners, and the call a
    /// file importer wants, because a format that carries normals and
    /// tangents carries them per vertex and has no use for a crate that
    /// recomputes a face's.
    ///
    /// [`quad_uv_with_frame`]: Self::quad_uv_with_frame
    pub fn triangle_with_frame(
        &mut self,
        corners: [[f32; 3]; 3],
        colours: [[f32; 4]; 3],
        uvs: [[f32; 2]; 3],
        frame: Frame,
    ) {
        let base = self.vertex_count();
        for ((corner, colour), uv) in corners.into_iter().zip(colours).zip(uvs) {
            self.push_vertex(corner, colour, uv, frame);
        }
        for offset in [0, 1, 2] {
            self.indices.push(base + offset);
        }
    }

    /// One vertex record, packed exactly as [`VERTEX_STRIDE`] describes.
    ///
    /// **Assembled whole, then appended once.** This wrote each float
    /// with its own `extend_from_slice` until the record reached sixteen
    /// of them, at which point that was sixteen capacity checks and
    /// sixteen length updates per vertex to move sixty-four bytes. The
    /// measurement is in this repository's mesh-build benchmark and it
    /// is not small — see the ladder recorded with this change.
    ///
    /// The array is the layout, in the order `renew_rhi::builtin::MESH_LAYOUT` is
    /// declared in, and it is the only place that order is written down
    /// in this crate.
    fn push_vertex(&mut self, position: [f32; 3], colour: [f32; 4], uv: [f32; 2], frame: Frame) {
        let floats = [
            position[0],
            position[1],
            position[2],
            colour[0],
            colour[1],
            colour[2],
            colour[3],
            uv[0],
            uv[1],
            frame.normal[0],
            frame.normal[1],
            frame.normal[2],
            frame.tangent[0],
            frame.tangent[1],
            frame.tangent[2],
            frame.tangent[3],
        ];
        let mut record = [0u8; VERTEX_STRIDE as usize];
        for (slot, value) in record.as_chunks_mut::<4>().0.iter_mut().zip(floats) {
            *slot = value.to_ne_bytes();
        }
        self.vertices.extend_from_slice(&record);
    }

    /// Whole vertex records pushed so far.
    ///
    /// # Panics
    ///
    /// In dev builds, if the count has passed what a `u32` holds. See the
    /// assertion's own note: the release behaviour is a saturating floor,
    /// which is wrong rather than merely imprecise, so the dev build says
    /// so instead of continuing.
    #[must_use]
    pub fn vertex_count(&self) -> u32 {
        // Every push adds exactly one stride, so the division is exact.
        let records = self.vertices.len() / VERTEX_STRIDE as usize;
        // **Asserted rather than argued away.** A scene of more than a
        // `u32` of records needs 2^32 * 28 bytes, about 120 GiB — beyond
        // anything this engine will build on the host, but well inside
        // what a 64-bit host can address, so "impossible" would be a
        // claim rather than a fact. It matters which: saturating here
        // would make `quad` number its corners from `u32::MAX`, and those
        // indices wrap into the low, *valid* range, where the in-range
        // scan the rendering crate runs cannot see them. A wrong picture
        // that passes every check is the one failure worth a dev-build
        // abort. Release keeps the floor: a scene this size fails at
        // upload regardless, where the refusal is an ordinary error.
        debug_assert!(
            u32::try_from(records).is_ok(),
            "a scene of {records} vertex records has outgrown the u32 an index carries"
        );
        u32::try_from(records).unwrap_or(u32::MAX)
    }

    /// Indices pushed so far — six per quad, which is what an indexed
    /// draw counts.
    #[must_use]
    pub fn index_count(&self) -> u32 {
        u32::try_from(self.indices.len()).unwrap_or(u32::MAX)
    }

    /// Whether anything has been pushed.
    ///
    /// Worth asking before an upload: an empty scene is ordinary data —
    /// an all-air world, a fully culled mesh — and the upload refuses it
    /// rather than handing it to a layer that treats it as a caller bug.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// The smallest box holding every corner pushed so far, as
    /// `(lowest, highest)` — or nothing at all if no quad has been.
    ///
    /// **A scene could say how much it held and never where.**
    /// [`Self::vertex_count`] answers a question about size in memory;
    /// this answers one about size in the world, and a caller had no way
    /// to ask it. Three of them want to: framing a camera on what was
    /// just built, deciding whether a scene is worth submitting at all,
    /// and — the case that prompted this — checking that geometry
    /// assembled from a scale factor came out the size it was meant to.
    /// That last one is a test's question, and without an extent the
    /// only available answer is to look at a picture and believe it.
    ///
    /// **Nothing rather than a zero box when empty**, because a box at
    /// the origin is a real answer a caller would act on: it would frame
    /// a camera on a point, or report a body of no size as one correctly
    /// placed at nothing. An empty scene has no extent, and saying so is
    /// the only honest option.
    ///
    /// Positions only. A vertex record carries colour and texture
    /// coordinates after its position and neither is a place, so neither
    /// is asked — which is a property worth a test rather than a
    /// comment, and has one.
    ///
    /// Whatever floats were pushed are the floats measured: a scene does
    /// not screen its geometry, so a corner pushed as NaN puts NaN in
    /// the answer. That is reported rather than repaired, because a box
    /// silently shrunk to exclude a bad corner is the same wrong answer
    /// with the evidence removed.
    #[must_use]
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        // The remainder is empty by construction - every push writes a
        // whole record, which the stride property test holds to - so a
        // partial tail here would be corruption upstream, not geometry
        // this could meaningfully report on.
        let (records, _) = self.vertices.as_chunks::<{ VERTEX_STRIDE as usize }>();
        let (first, rest) = records.split_first()?;
        let first = position_of(first);
        let (mut low, mut high) = (first, first);
        for record in rest {
            let corner = position_of(record);
            for ((lowest, highest), value) in low.iter_mut().zip(high.iter_mut()).zip(corner) {
                *lowest = lowest.min(value);
                *highest = highest.max(value);
            }
        }
        Some((low, high))
    }

    /// Forget every quad, keeping the allocation for the next build.
    ///
    /// Explicit rather than folded into a build call, for the reason the
    /// sibling crate gives: a caller that never clears is accumulating a
    /// static scene deliberately, and one that clears and pushes nothing
    /// has built a legitimately empty one.
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
    }

    /// The packed vertex bytes.
    ///
    /// Crate-visible: the upload is the only reader, and it applies the
    /// stride itself. A caller with its own use for the bytes would be
    /// building a mesh this crate did not describe, which is a request to
    /// widen this deliberately rather than a gap to leave open.
    #[must_use]
    pub(crate) fn vertices(&self) -> &[u8] {
        &self.vertices
    }

    /// The indices, in push order.
    #[must_use]
    pub(crate) fn indices(&self) -> &[u32] {
        &self.indices
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shaded quad is the same geometry with four colours instead of
    /// one, and a flat one is the case where the four agree.
    #[test]
    fn a_flat_quad_is_a_shaded_one_whose_corners_agree() {
        let corners = [
            [-1.0, -1.0, 0.5],
            [1.0, -1.0, 0.5],
            [1.0, 1.0, 0.5],
            [-1.0, 1.0, 0.5],
        ];
        let colour = [0.25, 0.5, 0.75, 1.0];

        let mut flat = Scene::new();
        flat.quad(corners, colour);

        let mut shaded = Scene::new();
        shaded.quad_shaded(corners, [colour; 4]);

        assert_eq!(
            flat.vertices(),
            shaded.vertices(),
            "the flat call must be the shaded one with four equal corners"
        );
        assert_eq!(flat.indices(), shaded.indices());
    }

    /// Corner colours land on their own corners, in order.
    #[test]
    fn each_corner_keeps_its_own_colour() {
        let corners = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let colours = [
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
            [1.0, 1.0, 0.0, 1.0],
        ];
        let mut scene = Scene::new();
        scene.quad_shaded(corners, colours);

        let bytes = scene.vertices();
        for (index, expected) in colours.iter().enumerate() {
            let at = index * VERTEX_STRIDE as usize + 12;
            for (channel, wanted) in expected.iter().enumerate() {
                let start = at + channel * 4;
                let found = f32::from_ne_bytes([
                    bytes[start],
                    bytes[start + 1],
                    bytes[start + 2],
                    bytes[start + 3],
                ]);
                assert!(
                    (found - wanted).abs() < f32::EPSILON,
                    "corner {index} channel {channel} is {found}, wanted {wanted}"
                );
            }
        }
    }

    const CORNERS: [[f32; 3]; 4] = [
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ];
    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    /// **The stride is the one number the rendering crate asserts on**,
    /// and a padded record would fail that assertion at the draw rather
    /// than here. Pinned on the bytes a real push produces, not on the
    /// constant, so the two cannot drift apart.
    #[test]
    fn a_vertex_record_packs_to_the_declared_stride() {
        let mut scene = Scene::new();
        scene.quad(CORNERS, WHITE);
        assert_eq!(
            scene.vertices().len(),
            4 * VERTEX_STRIDE as usize,
            "four corners at {VERTEX_STRIDE} bytes each"
        );
        assert_eq!(
            VERTEX_STRIDE,
            12 + 16 + 8 + 12 + 16,
            "a vec3 position, a vec4 colour, a vec2 coordinate, a vec3 normal \n             and a vec4 tangent"
        );
    }

    /// The bytes are the floats a caller handed over, in order, with
    /// nothing between them — the property a shader reading this layout
    /// depends on.
    #[test]
    fn a_record_is_its_position_then_colour_then_coordinate_then_frame() {
        let mut scene = Scene::new();
        scene.quad(CORNERS, [0.25, 0.5, 0.75, 1.0]);
        let first = &scene.vertices()[..VERTEX_STRIDE as usize];
        let floats: Vec<f32> = first
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect();
        // The two after the colour are the default mapping's first
        // corner, the origin of the tile. Then the face normal, which
        // these corners put on +Z: they wind counter-clockwise seen from
        // +Z, and (2,0,0) x (2,2,0) is (0,0,4). Then the tangent, which
        // the whole-tile mapping puts on +X — `u` runs from corner 0 to
        // corner 1, and that edge is +X — with a handedness of +1,
        // because `cross(+Z, +X)` is `+Y` and `v` does run along `+Y`
        // here. Every one of those five is a different axis of the same
        // frame, so a record that lost track of which is which would
        // have to move a number between two of them to pass.
        assert_eq!(
            floats,
            vec![
                -1.0, -1.0, 0.0, 0.25, 0.5, 0.75, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0
            ]
        );
    }

    /// **The tangent points the way `u` increases, and that is the whole
    /// contract.** Everything else about a tangent frame is derivable
    /// from it plus the normal; this is the part that has to come from
    /// the geometry and its mapping together.
    ///
    /// Checked on a face whose `u` axis is deliberately not the first
    /// edge: the corners lie in the XY plane, the first edge runs along
    /// `+X`, and `u` runs along `+Y` across them. A tangent that quietly
    /// returned the first edge, or `+X`, or the position delta, is a
    /// different vector from the right answer rather than accidentally
    /// equal to it.
    ///
    /// **The numbers are chosen so the obvious wrong solve is visible,
    /// and the first version of this test failed to do that.** With
    /// `du1 = 0` and `dv1 = dv2`, swapping the two `dv` terms leaves
    /// every expression numerically identical, so the mutant passed and
    /// the test proved nothing about the solve. Here `dv1` is 1 and
    /// `dv2` is 0, and the swap collapses the determinant from -1 to 0.
    ///
    /// Probed by swapping `dv2` and `dv1` throughout the solve: red, the
    /// determinant goes to zero and the frame comes back with no tangent
    /// at all.
    #[expect(
        clippy::float_cmp,
        reason = "the normal of an axis-aligned face is exact, and the handedness is a sign this code writes as a literal 1 or -1"
    )]
    #[test]
    fn the_tangent_runs_the_way_the_coordinate_does() {
        // A right triangle on the XY plane. Its edges are +X and +Y; the
        // mapping puts `u` along the second and `v` along the first.
        let frame = Frame::of_face(
            [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 3.0, 0.0]],
            [[0.0, 0.0], [0.0, 1.0], [1.0, 0.0]],
        );
        assert_eq!(frame.normal, [0.0, 0.0, 1.0], "the plane is the XY plane");
        let [x, y, z, w] = frame.tangent;
        assert!(
            (x - 0.0).abs() < 1e-6 && (y - 1.0).abs() < 1e-6 && (z - 0.0).abs() < 1e-6,
            "`u` increases along +Y here, so the tangent must: got {:?}",
            frame.tangent
        );
        assert_eq!(
            w, -1.0,
            "`u` along +Y and `v` along +X about a +Z normal is a \
             left-handed mapping, and the sign is where that is recorded"
        );
    }

    /// A mirrored mapping keeps its lighting, and the sign in `w` is how.
    ///
    /// **This is the reason the tangent is four floats.** Flip the
    /// coordinates of one face across `v` and the surface is unchanged,
    /// the normal is unchanged, and the way `u` runs is unchanged — only
    /// the bitangent turns over. A three-float tangent cannot say that,
    /// so a shader reconstructing `cross(n, t)` lights the mirrored
    /// island as though its normal map were inverted. Every atlas packer
    /// mirrors something eventually, so this is a case that arrives
    /// rather than one that might.
    ///
    /// Probed by returning a constant `1.0` for the handedness: red on
    /// the mirrored half.
    #[expect(
        clippy::float_cmp,
        reason = "the handedness is a sign written as a literal, and the two tangents are the same arithmetic on the same inputs, so equality is the claim rather than an approximation of it"
    )]
    #[test]
    fn a_mirrored_mapping_flips_the_handedness_and_nothing_else() {
        let corners = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]];
        let upright = Frame::of_face(corners, [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]);
        let mirrored = Frame::of_face(corners, [[0.0, 0.0], [1.0, 0.0], [1.0, -1.0]]);
        assert_eq!(
            upright.normal, mirrored.normal,
            "mirroring the mapping does not move the surface"
        );
        assert_eq!(
            [upright.tangent[0], upright.tangent[1], upright.tangent[2]],
            [
                mirrored.tangent[0],
                mirrored.tangent[1],
                mirrored.tangent[2]
            ],
            "`u` still runs the same way; only `v` was flipped"
        );
        assert_eq!(upright.tangent[3], 1.0);
        assert_eq!(
            mirrored.tangent[3], -1.0,
            "a mirrored island reconstructs its bitangent the other way"
        );
    }

    /// The stored tangent is perpendicular to the normal and of unit
    /// length, on a face where the raw solve gives neither.
    ///
    /// A sheared mapping — one whose `u` and `v` are not at right angles
    /// on the surface — produces a raw tangent of arbitrary length, so
    /// the unit-length half of this needs the normalisation to hold.
    ///
    /// **The perpendicular half has no mutant, and that is the finding
    /// rather than a gap.** A projection step was written here to enforce
    /// it; deleting it changed no test and no byte, because `along_u` is
    /// built from two edges of the face and therefore already lies in its
    /// plane. The step is gone and this assertion stays: it is the
    /// property every shader reading the record relies on, and the first
    /// change to store an interpolated or caller-derived tangent instead
    /// is the one that would break it.
    #[expect(
        clippy::float_cmp,
        reason = "only the handedness is compared exactly, and it is a sign written as a literal; the vectors are compared with a tolerance"
    )]
    #[test]
    fn the_stored_tangent_is_perpendicular_and_unit_length() {
        // A face tilted out of every coordinate plane, with a mapping
        // that is neither square nor axis-aligned on it.
        let frame = Frame::of_face(
            [[0.0, 0.0, 0.0], [2.0, 1.0, 0.5], [0.5, 2.0, 1.5]],
            [[0.0, 0.0], [1.0, 0.4], [0.3, 1.0]],
        );
        let [x, y, z, w] = frame.tangent;
        let length = (x * x + y * y + z * z).sqrt();
        assert!(
            (length - 1.0).abs() < 1e-5,
            "the tangent must be unit length, got {length}"
        );
        let leaning = x * frame.normal[0] + y * frame.normal[1] + z * frame.normal[2];
        assert!(
            leaning.abs() < 1e-5,
            "the tangent must lie in the plane, got a dot product of {leaning}"
        );
        assert!(w == 1.0 || w == -1.0, "handedness is a sign, got {w}");
    }

    /// **Every way a frame can fail to exist gives zero, never a NaN.**
    ///
    /// A NaN in a vertex buffer is the worst of these outcomes: it
    /// reaches the shader, contaminates whatever it touches, and shows up
    /// as geometry that vanishes at some angles. Zero is not a direction
    /// either, but it is a value a reader can test for and a shader can
    /// fall back from.
    ///
    /// The four ways in: no plane at all, a mapping with no direction in
    /// it, three coordinates on a line, and a mapping so small that the
    /// reciprocal of its determinant is not a number this type holds.
    ///
    /// **The last one is here because it is the only case the guard in
    /// `unit` catches on its own**, and a guard no input reaches is a
    /// guard nobody can trust. The first three are refused by the
    /// determinant check before that guard is asked anything, so with
    /// only those, replacing `unit`'s refusal with a fallback axis passes
    /// — measured, not assumed. A determinant of `1e-40` is a real `f32`
    /// and its reciprocal is not, which is how an atlas cell a hundred
    /// millionth of a texel across reaches this.
    ///
    /// Probed by replacing the `unit` refusal with a fallback axis: red
    /// on the fourth case. Probed by deleting the determinant check:
    /// green, and that is recorded at the check itself rather than here.
    #[expect(
        clippy::float_cmp,
        reason = "every value compared here is one this code writes as a literal zero or one: the point is that no arithmetic reached the output at all"
    )]
    #[test]
    fn a_frame_that_cannot_exist_is_zero_rather_than_a_nan() {
        let flat = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        // Collinear corners: no plane, so nothing downstream either.
        let collinear = Frame::of_face([[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [2.0, 2.0, 2.0]], flat);
        assert_eq!(collinear, Frame::NOWHERE);

        let corners = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        // A whole face mapped to one texel: a real plane, and no
        // direction for `u` anywhere on it.
        let one_texel = Frame::of_face(corners, [[0.5, 0.5], [0.5, 0.5], [0.5, 0.5]]);
        assert_eq!(
            one_texel.normal,
            [0.0, 0.0, 1.0],
            "the plane is still there"
        );
        assert_eq!(one_texel.tangent, Frame::NOWHERE.tangent);

        // Three coordinates on a line: a determinant of zero from the
        // other direction, and what a degenerate atlas cell gives.
        let collinear_uv = Frame::of_face(corners, [[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]]);
        assert_eq!(collinear_uv.tangent, Frame::NOWHERE.tangent);

        // A determinant that is a number and whose reciprocal is not:
        // 1e-20 squared is a subnormal `f32`, and one divided by it is
        // past what the type holds. Nothing here is zero, so the
        // determinant check passes it through.
        let vanishing = Frame::of_face(corners, [[0.0, 0.0], [1e-20, 0.0], [0.0, 1e-20]]);
        assert_eq!(
            vanishing.normal,
            [0.0, 0.0, 1.0],
            "the surface is unremarkable; only its mapping is not"
        );
        assert_eq!(vanishing.tangent, Frame::NOWHERE.tangent);

        for frame in [collinear, one_texel, collinear_uv, vanishing] {
            for value in frame.normal {
                assert!(value.is_finite(), "a normal must not be a NaN");
            }
            for value in frame.tangent {
                assert!(value.is_finite(), "a tangent must not be a NaN");
            }
        }
    }

    /// A supplied frame reaches the buffer exactly as given, and a
    /// derived one is what the deriving call would have produced.
    ///
    /// **The two halves are the whole point of the pair existing.** The
    /// first is what a caller that already knows its frame is buying: no
    /// re-derivation, and no silent correction of a value it asserted.
    /// The second is that adding the sibling did not change what the
    /// original call writes — every existing caller keeps its bytes.
    #[test]
    fn a_supplied_frame_is_written_and_a_derived_one_matches_it() {
        let corners = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]];
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];

        let mut derived = Scene::new();
        derived.triangle(corners, [WHITE; 3], uvs);

        let mut supplied = Scene::new();
        supplied.triangle_with_frame(corners, [WHITE; 3], uvs, Frame::of_face(corners, uvs));
        assert_eq!(
            derived.vertices(),
            supplied.vertices(),
            "the deriving call is the supplying call with `of_face` in front"
        );

        // And a frame nothing would derive is written unchanged: this
        // crate does not check a caller's assertion, because checking it
        // costs exactly what the call exists to save.
        let claimed = Frame {
            normal: [0.0, 1.0, 0.0],
            tangent: [0.0, 0.0, 1.0, -1.0],
        };
        let mut asserted = Scene::new();
        asserted.triangle_with_frame(corners, [WHITE; 3], uvs, claimed);
        let record = &asserted.vertices()[..VERTEX_STRIDE as usize];
        let floats: Vec<f32> = record
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect();
        assert_eq!(&floats[9..12], &claimed.normal, "the normal as claimed");
        assert_eq!(&floats[12..16], &claimed.tangent, "the tangent as claimed");
    }

    /// **A quad with no coordinates given gets a whole tile**, not a
    /// point. Zero everywhere would collapse a textured draw onto one
    /// texel — a quad in a single colour, which looks like a working
    /// texture and is not one.
    #[test]
    fn a_quad_without_coordinates_spans_the_whole_tile() {
        let mut scene = Scene::new();
        scene.quad(CORNERS, WHITE);
        let bytes = scene.vertices();
        let uv_of = |corner: usize| {
            let at = corner * VERTEX_STRIDE as usize + 28;
            let read = |offset: usize| {
                let start = at + offset;
                f32::from_ne_bytes([
                    bytes[start],
                    bytes[start + 1],
                    bytes[start + 2],
                    bytes[start + 3],
                ])
            };
            [read(0), read(4)]
        };
        for (corner, wanted) in [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
            .into_iter()
            .enumerate()
        {
            let found = uv_of(corner);
            assert!(
                found
                    .iter()
                    .zip(wanted)
                    .all(|(a, b)| (a - b).abs() < f32::EPSILON),
                "corner {corner} maps to {found:?}, wanted {wanted:?}"
            );
        }
    }

    /// Coordinates given are the coordinates packed, corner for corner.
    #[test]
    fn each_corner_keeps_its_own_coordinate() {
        let uvs = [[0.1, 0.2], [0.3, 0.4], [0.5, 0.6], [0.7, 0.8]];
        let mut scene = Scene::new();
        scene.quad_uv(CORNERS, [WHITE; 4], uvs);
        let bytes = scene.vertices();
        for (corner, wanted) in uvs.iter().enumerate() {
            let at = corner * VERTEX_STRIDE as usize + 28;
            for (channel, expected) in wanted.iter().enumerate() {
                let start = at + channel * 4;
                let found = f32::from_ne_bytes([
                    bytes[start],
                    bytes[start + 1],
                    bytes[start + 2],
                    bytes[start + 3],
                ]);
                assert!(
                    (found - expected).abs() < f32::EPSILON,
                    "corner {corner} channel {channel} is {found}, wanted {expected}"
                );
            }
        }
    }

    /// Two triangles per quad, indexing the four corners this push added.
    #[test]
    fn a_quad_is_two_triangles_over_four_corners() {
        let mut scene = Scene::new();
        scene.quad(CORNERS, WHITE);
        assert_eq!(scene.vertex_count(), 4);
        assert_eq!(scene.index_count(), 6);
        assert_eq!(scene.indices(), &[0, 1, 2, 0, 2, 3]);
    }

    /// **Push order is index order**, which is the crate's whole claim
    /// about a reproducible frame. The second quad's indices continue
    /// from the first's vertices rather than restarting.
    #[test]
    fn a_second_quad_continues_the_first_ones_numbering() {
        let mut scene = Scene::new();
        scene.quad(CORNERS, WHITE);
        scene.quad(CORNERS, WHITE);
        assert_eq!(scene.vertex_count(), 8);
        assert_eq!(scene.indices()[6..], [4, 5, 6, 4, 6, 7]);
    }

    /// The same calls twice give the same bytes — stated as a test
    /// because it is the property the golden images rest on.
    #[test]
    fn the_same_pushes_produce_identical_bytes() {
        let build = || {
            let mut scene = Scene::new();
            scene.quad(CORNERS, [1.0, 0.0, 0.0, 1.0]);
            scene.quad(CORNERS, [0.0, 0.0, 1.0, 1.0]);
            scene
        };
        let first = build();
        let second = build();
        assert_eq!(first.vertices(), second.vertices());
        assert_eq!(first.indices(), second.indices());
    }

    /// Reversing the push order changes the buffers, so "push order is
    /// draw order" is a claim with observable content rather than a
    /// restatement.
    #[test]
    fn reversing_the_push_order_changes_the_bytes() {
        let mut forward = Scene::new();
        forward.quad(CORNERS, [1.0, 0.0, 0.0, 1.0]);
        forward.quad(CORNERS, [0.0, 0.0, 1.0, 1.0]);
        let mut backward = Scene::new();
        backward.quad(CORNERS, [0.0, 0.0, 1.0, 1.0]);
        backward.quad(CORNERS, [1.0, 0.0, 0.0, 1.0]);
        assert_ne!(forward.vertices(), backward.vertices());
        assert_eq!(
            forward.indices(),
            backward.indices(),
            "the numbering is positional, so only the vertex bytes differ"
        );
    }

    /// Empty is empty, and clearing returns to it.
    #[test]
    fn a_new_scene_is_empty_and_clearing_empties_one() {
        let mut scene = Scene::new();
        assert!(scene.is_empty());
        scene.quad(CORNERS, WHITE);
        assert!(!scene.is_empty());
        scene.clear();
        assert!(scene.is_empty());
        assert_eq!(scene.vertex_count(), 0);
        assert_eq!(scene.index_count(), 0);
    }

    /// An empty scene has no extent, and says so rather than reporting a
    /// box at the origin — which is a real answer a caller would act on.
    #[test]
    fn an_empty_scene_has_no_bounds() {
        assert_eq!(Scene::new().bounds(), None);
        let mut cleared = Scene::new();
        cleared.quad(CORNERS, WHITE);
        cleared.clear();
        assert_eq!(
            cleared.bounds(),
            None,
            "a cleared scene still claimed an extent"
        );
    }

    /// One quad's bounds are its own corners; a second quad widens them
    /// to the union rather than replacing them.
    #[test]
    fn bounds_are_the_union_of_every_corner() {
        let mut scene = Scene::new();
        scene.quad(
            [
                [-1.0, -2.0, 0.0],
                [1.0, -2.0, 0.0],
                [1.0, 2.0, 0.0],
                [-1.0, 2.0, 0.0],
            ],
            WHITE,
        );
        assert_eq!(scene.bounds(), Some(([-1.0, -2.0, 0.0], [1.0, 2.0, 0.0])));
        scene.quad(
            [
                [0.0, 0.0, 5.0],
                [3.0, 0.0, 5.0],
                [3.0, 1.0, 5.0],
                [0.0, 1.0, 5.0],
            ],
            WHITE,
        );
        assert_eq!(
            scene.bounds(),
            Some(([-1.0, -2.0, 0.0], [3.0, 2.0, 5.0])),
            "the second quad replaced the extent instead of widening it"
        );
    }

    /// **Positions only.** A vertex record carries colour and texture
    /// coordinates after its position, and neither is a place. Reading
    /// the wrong offset would fold them in — and colour and uv both sit
    /// in nought-to-one, so on ordinary geometry the mistake would look
    /// like a slightly wrong box rather than an obviously wrong one.
    ///
    /// So the colours here are far outside the corners: if either leaks
    /// into the answer, the extent is not the quad's.
    #[test]
    fn colour_and_texture_are_not_places() {
        let corners = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let mut scene = Scene::new();
        scene.quad_uv(
            corners,
            [[-50.0, 90.0, -70.0, 40.0]; 4],
            [[-30.0, 60.0], [-30.0, 60.0], [-30.0, 60.0], [-30.0, 60.0]],
        );
        assert_eq!(
            scene.bounds(),
            Some(([0.0, 0.0, 0.0], [1.0, 1.0, 0.0])),
            "something other than a position reached the extent"
        );
    }

    /// The capacity hint changes no output — it is a hint, and a test
    /// that only checked it allocated would not notice it corrupting the
    /// geometry.
    #[test]
    fn the_capacity_hint_changes_nothing_but_the_allocation() {
        let mut hinted = Scene::with_capacity(2);
        let mut plain = Scene::new();
        for scene in [&mut hinted, &mut plain] {
            scene.quad(CORNERS, WHITE);
            scene.quad(CORNERS, WHITE);
        }
        assert_eq!(hinted.vertices(), plain.vertices());
        assert_eq!(hinted.indices(), plain.indices());
    }

    proptest::proptest! {
        /// **The two invariants the layer below actually depends on, at
        /// counts no example test reaches.**
        ///
        /// `create_mesh` scans every index and refuses one that is not
        /// less than the vertex count, and it computes the vertex count
        /// from `vertices.len() / stride` — so a scene whose bytes are
        /// not a whole number of records, or whose indices point past its
        /// own corners, is refused at upload or, worse, draws the wrong
        /// corners. Both properties are arithmetic over the quad count,
        /// which is exactly the shape examples at one and two quads
        /// cannot speak for.
        #[test]
        fn any_number_of_quads_packs_to_whole_records_indexing_only_its_own_corners(
            count in 0_usize..400,
        ) {
            let mut scene = Scene::new();
            for _ in 0..count {
                scene.quad(CORNERS, WHITE);
            }

            let vertices = u32::try_from(count).unwrap_or(u32::MAX) * 4;
            proptest::prop_assert_eq!(scene.vertex_count(), vertices);
            proptest::prop_assert_eq!(scene.index_count(), u32::try_from(count).unwrap_or(u32::MAX) * 6);
            // Whole records: the division the layer below performs is
            // exact, so its vertex count is this one.
            proptest::prop_assert_eq!(
                scene.vertices().len(),
                count * 4 * VERTEX_STRIDE as usize
            );
            // Every index addresses a vertex this scene actually holds.
            // The failing direction matters: an index equal to the count
            // is past the last vertex, not at it.
            proptest::prop_assert!(
                scene.indices().iter().all(|&index| index < vertices),
                "an index reached past the last vertex"
            );
            // Emptiness is the condition `upload` refuses, and it has to
            // agree with both buffers or the guard reads one of them.
            proptest::prop_assert_eq!(scene.is_empty(), count == 0);
            proptest::prop_assert_eq!(scene.vertices().is_empty(), count == 0);
        }

        /// Clearing returns a scene to the state a new one is in, for any
        /// history — so a caller rebuilding geometry every frame cannot
        /// accumulate anything, and the indices restart from zero rather
        /// than from where the last build stopped.
        #[test]
        fn clearing_any_scene_leaves_it_indistinguishable_from_a_new_one(
            count in 0_usize..200,
        ) {
            let mut scene = Scene::new();
            for _ in 0..count {
                scene.quad(CORNERS, WHITE);
            }
            scene.clear();
            proptest::prop_assert!(scene.is_empty());
            proptest::prop_assert_eq!(scene.vertex_count(), 0);
            proptest::prop_assert_eq!(scene.index_count(), 0);

            scene.quad(CORNERS, WHITE);
            let fresh = {
                let mut other = Scene::new();
                other.quad(CORNERS, WHITE);
                other
            };
            proptest::prop_assert_eq!(scene.vertices(), fresh.vertices());
            proptest::prop_assert_eq!(scene.indices(), fresh.indices());
        }
    }

    /// **A triangle is three vertices and three indices**, not a quad
    /// with a corner folded onto another. Pinned because the cheap
    /// wrong implementation — forwarding to `quad_uv` with a repeated
    /// corner — produces a picture that looks identical and puts a
    /// fourth vertex and three extra indices in the buffer for every
    /// face a file imports.
    ///
    /// Probed by forwarding to `quad_uv` with `corners[2]` twice: the
    /// counts go to four and six and this says which.
    #[test]
    fn a_triangle_is_three_vertices_and_three_indices() {
        let mut scene = Scene::new();
        scene.triangle(
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [WHITE; 3],
            [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        );
        assert_eq!(scene.vertex_count(), 3, "three corners, three records");
        assert_eq!(scene.index_count(), 3, "one triangle, three indices");
    }

    /// **The winding a caller gives is the winding that is kept**, and
    /// the normal follows it. A file importer preserves whatever its
    /// source said, so reversing a triangle must reverse its normal
    /// rather than being silently corrected to face one way.
    ///
    /// Probed by sorting the corners before the cross product: both
    /// windings then report the same normal and the second assertion
    /// fails.
    #[expect(
        clippy::float_cmp,
        reason = "an axis-aligned normal is exact: the cross product of integral edges is integral, and its length divides it back to one component of exactly 1 or -1"
    )]
    #[test]
    fn reversing_a_triangle_reverses_its_normal() {
        let corners = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let reversed = [corners[0], corners[2], corners[1]];
        let normal_of = |corners: [[f32; 3]; 3]| {
            let mut scene = Scene::new();
            scene.triangle(corners, [WHITE; 3], [[0.0, 0.0]; 3]);
            let bytes = scene.vertices();
            let at = 36;
            let read = |offset: usize| {
                let start = at + offset;
                f32::from_ne_bytes([
                    bytes[start],
                    bytes[start + 1],
                    bytes[start + 2],
                    bytes[start + 3],
                ])
            };
            [read(0), read(4), read(8)]
        };
        assert_eq!(normal_of(corners), [0.0, 0.0, 1.0]);
        assert_eq!(normal_of(reversed), [0.0, 0.0, -1.0]);
    }

    /// **A degenerate triangle has no plane, and gets zero rather than
    /// a NaN.** Three collinear corners give a zero cross product;
    /// normalising that divides by zero, and the NaN would travel into
    /// a vertex buffer and out again as a lighting term nobody can
    /// trace back here.
    ///
    /// Probed by removing the length guard: every component comes back
    /// NaN and the assertion names it.
    #[test]
    fn a_degenerate_triangle_gets_a_zero_normal_and_not_a_nan() {
        for corners in [
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            [[1.0, 2.0, 3.0], [1.0, 2.0, 3.0], [1.0, 2.0, 3.0]],
        ] {
            let mut scene = Scene::new();
            scene.triangle(corners, [WHITE; 3], [[0.0, 0.0]; 3]);
            let bytes = scene.vertices();
            for offset in [36, 40, 44] {
                let value = f32::from_ne_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ]);
                assert!(
                    value == 0.0,
                    "a degenerate triangle produced {value}, which is not a direction"
                );
            }
        }
    }

    /// **Every quad already carried a plane; now it carries the normal
    /// of it.** The voxel world's faces are axis-aligned, so this is
    /// exact rather than approximate, and a caller that never asked for
    /// a normal gets a correct one without changing a line.
    #[expect(
        clippy::float_cmp,
        reason = "an axis-aligned normal is exact: the cross product of integral edges is integral, and its length divides it back to one component of exactly 1 or -1"
    )]
    #[test]
    fn a_quads_normal_is_the_plane_its_corners_lie_in() {
        let mut scene = Scene::new();
        // A face on the +X plane, wound so its normal points along +X.
        scene.quad(
            [
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0],
                [1.0, 0.0, 1.0],
            ],
            WHITE,
        );
        let bytes = scene.vertices();
        for corner in 0..4 {
            let at = corner * VERTEX_STRIDE as usize + 36;
            let read = |offset: usize| {
                let start = at + offset;
                f32::from_ne_bytes([
                    bytes[start],
                    bytes[start + 1],
                    bytes[start + 2],
                    bytes[start + 3],
                ])
            };
            assert_eq!(
                [read(0), read(4), read(8)],
                [1.0, 0.0, 0.0],
                "corner {corner} of an +X face"
            );
        }
    }
}
