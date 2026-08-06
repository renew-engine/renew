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
pub(crate) const VERTEX_STRIDE: u32 = 28;

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
    /// other vector. This exists because the named consumer knows its
    /// face count before it starts, and an allocation per quad on a
    /// four-thousand-face world is a cost with no reason.
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
        // Recorded before the push, so the triangles below index the
        // corners this call adds rather than whatever came before.
        let base = self.vertex_count();
        for (corner, colour) in corners.into_iter().zip(colours) {
            self.push_vertex(corner, colour);
        }
        for offset in [0, 1, 2, 0, 2, 3] {
            self.indices.push(base + offset);
        }
    }

    /// One vertex record, packed exactly as [`VERTEX_STRIDE`] describes.
    fn push_vertex(&mut self, position: [f32; 3], colour: [f32; 4]) {
        for value in position {
            self.vertices.extend_from_slice(&value.to_ne_bytes());
        }
        for value in colour {
            self.vertices.extend_from_slice(&value.to_ne_bytes());
        }
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
        assert_eq!(VERTEX_STRIDE, 12 + 16, "a vec3 position and a vec4 colour");
    }

    /// The bytes are the floats a caller handed over, in order, with
    /// nothing between them — the property a shader reading this layout
    /// depends on.
    #[test]
    fn a_record_is_its_position_then_its_colour() {
        let mut scene = Scene::new();
        scene.quad(CORNERS, [0.25, 0.5, 0.75, 1.0]);
        let first = &scene.vertices()[..VERTEX_STRIDE as usize];
        let floats: Vec<f32> = first
            .chunks_exact(4)
            .map(|bytes| f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect();
        assert_eq!(floats, vec![-1.0, -1.0, 0.0, 0.25, 0.5, 0.75, 1.0]);
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
}
