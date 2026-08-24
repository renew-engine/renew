//! World space to clip space, for a fixed isometric view.
//!
//! Pure, like [`crate::mesh`], and for the same reason: the arithmetic
//! that decides where a face lands should be testable without a GPU.
//!
//! # The three conventions, and what each costs if it is wrong
//!
//! All three were read out of this repository's own rendering code rather
//! than recalled, because each produces a *plausible* wrong picture.
//!
//! **Clip `y` points down.** The viewport is built with a positive height
//! and no flip, so nothing inverts `y` for this crate. World up is `+y`,
//! screen up is `-y`, and the projection carries an explicit minus. Omit
//! it and the arena renders upside down — which reads as a perfectly
//! ordinary picture of a corner, because the box is nearly symmetric. The
//! tell is the mound hanging off the ceiling.
//!
//! **Clip `z` runs `[0, 1]` REVERSED, and nearer must be larger.** Depth
//! clears to zero and the compare is `GREATER_OR_EQUAL`, so the largest
//! `z` survives — the engine's single depth convention. Flip the sign
//! and the depth test keeps the *furthest* surface: you see the far wall
//! through everything in front of it, which again looks like a real
//! render rather than a bug.
//!
//! **No trigonometry.** The rotation is a true isometric one — a 45°
//! turn and a 35.264° tilt — and every entry of that basis is expressible
//! in square roots. That matters because the picture this produces is
//! committed and compared across three platforms: `sqrt` is required by
//! IEEE 754 to be correctly rounded and therefore identical everywhere,
//! while `sin` and `cos` are not, and differ between platform maths
//! libraries in the last bits. A basis built from `f32::sin` would make
//! the golden image a coin toss.

/// A fixed isometric view of a world-space box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    /// Screen right, in world space.
    right: [f32; 3],
    /// Screen up, in world space.
    up: [f32; 3],
    /// The direction the viewer looks, away from the eye and into the
    /// scene, so that a larger distance along it is further away.
    forward: [f32; 3],
    /// The point that lands in the middle of the picture.
    centre: [f32; 3],
    /// Half the view box, along each of the three axes above.
    half: [f32; 3],
}

/// How much empty space to leave around the world, as a fraction.
///
/// Without it the outermost corner sits exactly on the clip boundary,
/// where a rounding difference of one bit decides whether it is drawn.
const MARGIN: f32 = 0.05;

impl Projection {
    /// A view of the axis-aligned box from `min` to `max`, looking down
    /// the `(-1, -1, -1)` diagonal.
    ///
    /// The eye is in the `+x +y +z` octant, so the picture shows the
    /// world from above and to one side — the angle that makes a voxel
    /// world readable, and the reason it is worth having no camera yet.
    #[must_use]
    pub fn isometric(min: [f32; 3], max: [f32; 3]) -> Self {
        // The true-isometric basis, in radicals. Orthonormal by
        // construction: `right` is the horizontal perpendicular to the
        // view diagonal, and `up` completes the frame.
        let (r2, r3, r6) = (2.0f32.sqrt(), 3.0f32.sqrt(), 6.0f32.sqrt());
        let right = [1.0 / r2, 0.0, -1.0 / r2];
        let up = [-1.0 / r6, 2.0 / r6, -1.0 / r6];
        let forward = [-1.0 / r3, -1.0 / r3, -1.0 / r3];

        let centre = [
            min[0].midpoint(max[0]),
            min[1].midpoint(max[1]),
            min[2].midpoint(max[2]),
        ];

        // The box's extent along each view axis is the sum of its
        // half-extents projected onto that axis — the support function of
        // a box, which is exact rather than a sampling of its corners.
        let world_half = [
            (max[0] - min[0]) * 0.5,
            (max[1] - min[1]) * 0.5,
            (max[2] - min[2]) * 0.5,
        ];
        let extent = |axis: [f32; 3]| {
            (world_half[0] * axis[0].abs()
                + world_half[1] * axis[1].abs()
                + world_half[2] * axis[2].abs())
                * (1.0 + MARGIN)
        };

        Self {
            right,
            up,
            forward,
            centre,
            half: [extent(right), extent(up), extent(forward)],
        }
    }

    /// A world-space point in clip space.
    #[must_use]
    pub fn project(&self, point: [f32; 3]) -> [f32; 3] {
        let offset = [
            point[0] - self.centre[0],
            point[1] - self.centre[1],
            point[2] - self.centre[2],
        ];
        let along =
            |axis: [f32; 3]| offset[0] * axis[0] + offset[1] * axis[1] + offset[2] * axis[2];
        [
            along(self.right) / self.half[0],
            // Negated: screen y grows downward, world y grows up.
            -along(self.up) / self.half[1],
            // Depth into [0, 1] REVERSED: further along the view
            // direction is smaller, so the larger value the compare
            // keeps is nearer.
            (-0.5f32).mul_add(along(self.forward) / self.half[2], 0.5),
        ]
    }

    /// Whether a face with this outward normal is turned toward the eye.
    ///
    /// **Needed because the arena is a closed box.** Every face the
    /// mesher emits points inward, so a view from outside would show the
    /// underside of the near wall filling the frame — technically a
    /// correct render of the world, and useless as a picture. Nothing
    /// culls it: the pipeline draws both sides. Dropping the faces that
    /// point away cuts the near walls off and leaves a view *into* the
    /// room, which is what a person wants to look at.
    ///
    /// This lives here rather than in the mesher on purpose: how many
    /// faces the world has is a fact about the world, and which of them
    /// you can see is a fact about where you are standing.
    #[must_use]
    pub fn faces_viewer(&self, normal: [f32; 3]) -> bool {
        let facing =
            normal[0] * self.forward[0] + normal[1] * self.forward[1] + normal[2] * self.forward[2];
        facing < 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box roughly the shape of the arena.
    fn arena_box() -> Projection {
        Projection::isometric([-20.5, -0.5, -20.5], [20.5, 11.5, 20.5])
    }

    /// The basis is orthonormal, which is what makes the projection a
    /// rotation rather than a shear.
    #[test]
    fn the_view_basis_is_orthonormal() {
        let view = arena_box();
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        for (name, axis) in [
            ("right", view.right),
            ("up", view.up),
            ("forward", view.forward),
        ] {
            assert!(
                (dot(axis, axis) - 1.0).abs() < 1e-6,
                "{name} is not a unit vector"
            );
        }
        assert!(
            dot(view.right, view.up).abs() < 1e-6,
            "right and up must be square"
        );
        assert!(dot(view.right, view.forward).abs() < 1e-6);
        assert!(dot(view.up, view.forward).abs() < 1e-6);
    }

    /// The middle of the world lands in the middle of the picture, at the
    /// middle of the depth range.
    #[test]
    fn the_centre_of_the_world_is_the_centre_of_the_picture() {
        let view = arena_box();
        let clip = view.project([0.0, 5.5, 0.0]);
        assert!(clip[0].abs() < 1e-6, "x: {clip:?}");
        assert!(clip[1].abs() < 1e-6, "y: {clip:?}");
        assert!((clip[2] - 0.5).abs() < 1e-6, "z: {clip:?}");
    }

    /// **Up in the world is up on the screen.**
    ///
    /// Its own test with its own message, because the failure is a world
    /// rendered upside down — a picture of three shaded planes meeting at
    /// a corner, which is exactly what a correct render looks like. The
    /// only tell in the image itself is a mound hanging off the ceiling.
    #[test]
    fn a_higher_point_is_higher_on_the_screen() {
        let view = arena_box();
        let low = view.project([0.0, 1.0, 0.0]);
        let high = view.project([0.0, 9.0, 0.0]);
        assert!(
            high[1] < low[1],
            "screen y grows downward, so a higher point needs a smaller y; the world is upside \
             down: high {high:?} against low {low:?}"
        );
    }

    /// **Nearer is larger**, which is what the reversed compare keeps.
    ///
    /// Same shape of failure as the test above: get this backwards and
    /// the far wall draws over everything in front of it, which still
    /// looks like a rendered room.
    #[test]
    fn a_nearer_point_gets_a_larger_depth() {
        let view = arena_box();
        // The eye is toward +x +y +z, so that corner is the near one.
        let near = view.project([18.0, 9.0, 18.0]);
        let far = view.project([-18.0, 1.0, -18.0]);
        assert!(
            near[2] > far[2],
            "the reversed compare keeps the larger depth, so nearer must be larger: \
             near {near:?} against far {far:?}"
        );
        assert!(
            (0.0..=1.0).contains(&near[2]) && (0.0..=1.0).contains(&far[2]),
            "depth must land inside the unit range the viewport maps"
        );
    }

    /// The faces kept are the ones turned toward the eye.
    #[test]
    fn only_faces_turned_toward_the_eye_survive() {
        let view = arena_box();
        // The eye looks along -(1,1,1): a face pointing back along
        // +(1,1,1) faces it, one pointing away does not.
        assert!(view.faces_viewer([1.0, 0.0, 0.0]), "east faces the eye");
        assert!(view.faces_viewer([0.0, 1.0, 0.0]), "up faces the eye");
        assert!(view.faces_viewer([0.0, 0.0, 1.0]), "north faces the eye");
        assert!(!view.faces_viewer([-1.0, 0.0, 0.0]), "west points away");
        assert!(!view.faces_viewer([0.0, -1.0, 0.0]), "down points away");
        assert!(!view.faces_viewer([0.0, 0.0, -1.0]), "south points away");
    }

    /// No trigonometry reaches the basis, so the same source produces the
    /// same bytes on every platform.
    ///
    /// Asserted as exact equality against values built from `sqrt`, which
    /// IEEE 754 requires to be correctly rounded. `sin` and `cos` carry no
    /// such requirement and differ between platform maths libraries — and
    /// the picture this projection produces is committed and compared
    /// across three of them.
    #[test]
    fn the_basis_is_built_from_radicals_rather_than_trigonometry() {
        let view = arena_box();
        let (r2, r3, r6) = (2.0f32.sqrt(), 3.0f32.sqrt(), 6.0f32.sqrt());
        // Bit patterns rather than values: the claim is that these are
        // the *same computation*, not merely close, and the rendering
        // crates use the same idiom where exactness is the point.
        let bits = |axis: [f32; 3]| axis.map(f32::to_bits);
        assert_eq!(bits(view.right), bits([1.0 / r2, 0.0, -1.0 / r2]));
        assert_eq!(bits(view.up), bits([-1.0 / r6, 2.0 / r6, -1.0 / r6]));
        assert_eq!(bits(view.forward), bits([-1.0 / r3, -1.0 / r3, -1.0 / r3]));
    }

    proptest::proptest! {
        /// Every point inside the fitted box lands inside clip space.
        ///
        /// The property the margin exists for: a view box derived from
        /// the world's own bounds must not put any of that world outside
        /// the frame, at any point, not just at the corners somebody
        /// thought to type.
        #[test]
        fn any_point_in_the_world_lands_inside_the_picture(
            x in -20.5f32..20.5,
            y in -0.5f32..11.5,
            z in -20.5f32..20.5,
        ) {
            let clip = arena_box().project([x, y, z]);
            proptest::prop_assert!(
                (-1.0..=1.0).contains(&clip[0]),
                "x outside the frame: {:?}", clip
            );
            proptest::prop_assert!(
                (-1.0..=1.0).contains(&clip[1]),
                "y outside the frame: {:?}", clip
            );
            proptest::prop_assert!(
                (0.0..=1.0).contains(&clip[2]),
                "depth outside the unit range: {:?}", clip
            );
        }

        /// Moving away from the eye never increases depth — reversed,
        /// so away means smaller.
        ///
        /// Stated over arbitrary points because the depth test is the one
        /// thing separating a solid world from a soup of faces, and a
        /// projection that is monotone at the two points a person picked
        /// can still fold in the middle.
        #[test]
        fn moving_away_from_the_eye_never_comes_closer(
            x in -18.0f32..18.0,
            y in 0.0f32..11.0,
            z in -18.0f32..18.0,
            step in 0.1f32..8.0,
        ) {
            let view = arena_box();
            let here = view.project([x, y, z]);
            // One step along the view direction is one step away.
            let there = view.project([
                x + view.forward[0] * step,
                y + view.forward[1] * step,
                z + view.forward[2] * step,
            ]);
            proptest::prop_assert!(
                there[2] < here[2],
                "a step away should be smaller under reversed depth: {:?} then {:?}", here, there
            );
        }

        /// The projection is a pure function of its input.
        #[test]
        fn projecting_is_reproducible(
            x in -30.0f32..30.0,
            y in -30.0f32..30.0,
            z in -30.0f32..30.0,
        ) {
            let view = arena_box();
            proptest::prop_assert_eq!(
                view.project([x, y, z]).map(f32::to_bits),
                view.project([x, y, z]).map(f32::to_bits)
            );
        }
    }
}
