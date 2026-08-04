//! World-space extents: how far a shape reaches along each axis.

use crate::shape::{Shape, Transform};
use renew_fixed::{Fixed, Vec2};

/// An axis-aligned box in world space, given by its lowest and highest corner.
///
/// Half-open in neither direction: a body whose maximum equals another's
/// minimum *is* touching, and the broadphase reports the pair so narrowphase
/// can decide. Under-reporting loses a contact silently; over-reporting costs
/// one narrowphase test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aabb {
    /// Lowest corner.
    pub min: Vec2,
    /// Highest corner.
    pub max: Vec2,
}

impl Aabb {
    /// The box containing exactly this point.
    #[must_use]
    pub const fn point(at: Vec2) -> Self {
        Self { min: at, max: at }
    }

    /// Grown by `margin` in every direction.
    ///
    /// The broadphase inflates by the contact tolerance so that a pair
    /// separated by less than it still reaches narrowphase — otherwise a
    /// contact the vocabulary requires to be reported at depth zero would
    /// never be generated at all.
    #[must_use]
    pub fn expanded(self, margin: Fixed) -> Self {
        let by = Vec2::new(margin, margin);
        Self {
            min: self.min - by,
            max: self.max + by,
        }
    }

    /// Whether two boxes share any point, touching included.
    #[must_use]
    pub fn overlaps(self, other: Self) -> bool {
        self.min.x <= other.max.x
            && other.min.x <= self.max.x
            && self.min.y <= other.max.y
            && other.min.y <= self.max.y
    }

    /// Whether a point is inside or on the boundary.
    #[must_use]
    pub fn contains(self, point: Vec2) -> bool {
        self.min.x <= point.x
            && point.x <= self.max.x
            && self.min.y <= point.y
            && point.y <= self.max.y
    }
}

impl Shape {
    /// The world-space box this shape occupies under `transform`.
    ///
    /// **Exact, not conservative.** A bounding circle around a rotated box
    /// would be cheaper and would change which pairs the broadphase reports —
    /// and the candidate set is observable through the reported counts, so a
    /// looser bound is a different answer rather than a slower one.
    ///
    /// The extents come from the rotation's sine and cosine directly. For a
    /// box with half-extents `(hx, hy)` turned by θ, the reach along *x* is
    /// `hx·|cos θ| + hy·|sin θ|` — the two edge directions each contributing
    /// their projection — and along *y* the same with the two swapped.
    #[must_use]
    pub fn world_bounds(self, transform: Transform) -> Aabb {
        let (sin, cos) = transform.rotation.sin_cos();
        let (sin, cos) = (sin.abs(), cos.abs());
        let reach = match self {
            Self::Circle { radius } => Vec2::new(radius, radius),
            Self::Box { half_extents } => Vec2::new(
                half_extents.x.saturating_mul(cos) + half_extents.y.saturating_mul(sin),
                half_extents.x.saturating_mul(sin) + half_extents.y.saturating_mul(cos),
            ),
            // The core segment runs along local *y*, so after a turn of θ its
            // direction is (−sin θ, cos θ) and the radius is added in both
            // directions because a capsule is a segment swept by a circle.
            Self::Capsule {
                radius,
                half_height,
            } => Vec2::new(
                half_height.saturating_mul(sin) + radius,
                half_height.saturating_mul(cos) + radius,
            ),
        };
        Aabb {
            min: transform.translation - reach,
            max: transform.translation + reach,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Aabb;
    use crate::shape::{Shape, Transform};
    use renew_fixed::{Angle, Fixed, Vec2};

    fn v(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    fn circle(units: i32) -> Shape {
        Shape::Circle {
            radius: Fixed::from_int(units),
        }
    }

    #[test]
    fn a_circle_has_the_same_bounds_at_every_angle() {
        let shape = circle(2);
        let upright = shape.world_bounds(Transform::at(v(5, 5)));
        for eighths in 0..8 {
            let turned =
                shape.world_bounds(Transform::new(v(5, 5), Angle::from_turn_ratio(eighths, 8)));
            assert_eq!(turned, upright, "a circle is rotation-invariant");
        }
        assert_eq!(upright.min, v(3, 3));
        assert_eq!(upright.max, v(7, 7));
    }

    #[test]
    fn an_unrotated_box_bounds_exactly_its_half_extents() {
        let bounds = Shape::Box {
            half_extents: v(3, 1),
        }
        .world_bounds(Transform::at(v(10, 20)));
        assert_eq!(bounds.min, v(7, 19));
        assert_eq!(bounds.max, v(13, 21));
    }

    /// A quarter turn swaps a box's reach, which is the cheapest check that
    /// the shape is in body space rather than world space.
    #[test]
    fn a_quarter_turn_swaps_a_box_s_extents() {
        let shape = Shape::Box {
            half_extents: v(3, 1),
        };
        let turned = shape.world_bounds(Transform::new(Vec2::ZERO, Angle::from_turn_ratio(1, 4)));
        // Rounding in the sine table can leave a raw unit or two.
        assert!((turned.max.x - Fixed::from_int(1)).to_bits().abs() <= 2);
        assert!((turned.max.y - Fixed::from_int(3)).to_bits().abs() <= 2);
    }

    /// The diagonal is where a conservative bound would differ most from the
    /// exact one, so it is the case worth pinning.
    #[test]
    fn a_box_on_the_diagonal_reaches_its_projected_extent() {
        let shape = Shape::Box {
            half_extents: v(1, 1),
        };
        let turned = shape.world_bounds(Transform::new(Vec2::ZERO, Angle::from_turn_ratio(1, 8)));
        // At an eighth turn a unit square reaches sqrt(2) along each axis,
        // which is 92682 raw units.
        let expected = 92_682i64;
        let reached = turned.max.x.to_bits();
        assert!((reached - expected).abs() <= 4, "reached {reached} raw");
    }

    #[test]
    fn a_capsule_reaches_its_radius_across_and_its_height_along() {
        let shape = Shape::Capsule {
            radius: Fixed::from_int(1),
            half_height: Fixed::from_int(4),
        };
        let upright = shape.world_bounds(Transform::at(Vec2::ZERO));
        assert_eq!(upright.max, v(1, 5));
        let sideways = shape.world_bounds(Transform::new(Vec2::ZERO, Angle::from_turn_ratio(1, 4)));
        assert!((sideways.max.x - Fixed::from_int(5)).to_bits().abs() <= 2);
        assert!((sideways.max.y - Fixed::from_int(1)).to_bits().abs() <= 2);
    }

    /// Touching counts as overlapping. Under-reporting here loses a contact
    /// silently; over-reporting costs one narrowphase test.
    #[test]
    fn boxes_that_merely_touch_overlap() {
        let left = Aabb {
            min: v(0, 0),
            max: v(1, 1),
        };
        let right = Aabb {
            min: v(1, 0),
            max: v(2, 1),
        };
        assert!(left.overlaps(right));
        assert!(right.overlaps(left));

        let apart = Aabb {
            min: v(2, 0),
            max: v(3, 1),
        };
        assert!(!left.overlaps(apart));
        // Separated on y alone is still separated.
        let above = Aabb {
            min: v(0, 2),
            max: v(1, 3),
        };
        assert!(!left.overlaps(above));
    }

    #[test]
    fn expanding_by_the_tolerance_makes_near_misses_into_candidates() {
        let left = Aabb {
            min: v(0, 0),
            max: v(1, 1),
        };
        let right = Aabb {
            min: v(2, 0),
            max: v(3, 1),
        };
        assert!(!left.overlaps(right));
        assert!(
            left.expanded(Fixed::from_int(1)).overlaps(right),
            "a pair inside the tolerance must reach narrowphase"
        );
    }

    #[test]
    fn a_point_box_contains_only_itself_and_its_boundary() {
        let dot = Aabb::point(v(3, 4));
        assert!(dot.contains(v(3, 4)));
        assert!(!dot.contains(v(3, 5)));
        assert!(dot.overlaps(dot));
        let around = Aabb {
            min: v(0, 0),
            max: v(9, 9),
        };
        assert!(around.contains(v(3, 4)));
        assert!(around.contains(v(0, 0)), "the boundary is inside");
        assert!(!around.contains(v(10, 4)));
    }
}
