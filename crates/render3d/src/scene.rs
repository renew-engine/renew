//! The pure half: quads in, packed vertex bytes and indices out.
//!
//! Nothing here calls a device. That is what makes the arithmetic — the
//! packing, the winding, the index numbering — testable on a machine with
//! no adapter, which is most machines and every sanitizer lane. The
//! sibling 2D crate draws the same line in the same place, and its pure
//! half does still name the rendering crate's `Extent`: the property is
//! *no device calls*, not *no dependency*, and claiming the stronger one
//! would claim something no crate here actually does.
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
/// maths crate's vector types are sixteen-byte aligned, so a struct of a
/// `Vec3` and a `Vec4` occupies more than the twenty-eight bytes its
/// fields need — and the rendering crate asserts, at the moment a draw is
/// recorded, that a mesh's stride equals the stride the pipeline's
/// per-vertex layout packs to. A padded record would fail that assertion
/// at the draw rather than here, which is a long way from the mistake.
/// Writing the bytes explicitly makes the layout the code's subject
/// rather than the compiler's.
pub const VERTEX_STRIDE: u32 = 28;

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
        // Recorded before the push, so the triangles below index the
        // corners this call adds rather than whatever came before.
        let base = self.vertex_count();
        for corner in corners {
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
    #[must_use]
    pub fn vertex_count(&self) -> u32 {
        // Every push adds exactly one stride, so the division is exact.
        // The cast cannot lose: a scene that overflowed a `u32` of
        // records would need more bytes than the host can address.
        u32::try_from(self.vertices.len() / VERTEX_STRIDE as usize).unwrap_or(u32::MAX)
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
    #[must_use]
    pub fn vertices(&self) -> &[u8] {
        &self.vertices
    }

    /// The indices, in push order.
    #[must_use]
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
