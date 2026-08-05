//! World-space extents: how far a shape reaches along each axis.

use crate::shape::{Shape, Transform};
use renew_fixed::{Fixed, Vec3};

/// An axis-aligned box in world space, given by its lowest and highest corner.
///
/// Touching counts as overlapping in every dimension: a body whose maximum
/// equals another's minimum *is* touching, and the broadphase proposes the
/// pair so narrowphase can decide. Under-reporting loses a contact silently;
/// over-reporting costs one narrowphase test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aabb {
    /// Lowest corner.
    pub min: Vec3,
    /// Highest corner.
    pub max: Vec3,
}

impl Aabb {
    /// The box containing exactly this point.
    #[must_use]
    pub const fn point(at: Vec3) -> Self {
        Self { min: at, max: at }
    }

    /// Grown by `margin` in every direction.
    #[must_use]
    pub fn expanded(self, margin: Fixed) -> Self {
        let by = Vec3::new(margin, margin, margin);
        Self {
            min: self.min - by,
            max: self.max + by,
        }
    }

    /// Whether two boxes share any point, touching included.
    ///
    /// **Three axes, and all three must overlap.** A two-dimensional test
    /// lifted by forgetting one is the classic way to make a 3D broadphase
    /// propose everything in a column, which is correct and useless.
    #[must_use]
    pub fn overlaps(self, other: Self) -> bool {
        self.min.x <= other.max.x
            && other.min.x <= self.max.x
            && self.min.y <= other.max.y
            && other.min.y <= self.max.y
            && self.min.z <= other.max.z
            && other.min.z <= self.max.z
    }

    /// Whether a point is inside or on the boundary.
    #[must_use]
    pub fn contains(self, point: Vec3) -> bool {
        self.min.x <= point.x
            && point.x <= self.max.x
            && self.min.y <= point.y
            && point.y <= self.max.y
            && self.min.z <= point.z
            && point.z <= self.max.z
    }
}

impl Shape {
    /// The world-space box this shape occupies under `transform`.
    ///
    /// Exact, and with no rotation to account for it is also trivial — which
    /// is a fair share of why the axis-aligned case is worth having on its own
    /// rather than waiting for an orientation type.
    #[must_use]
    pub fn world_bounds(self, transform: Transform) -> Aabb {
        let reach = match self {
            Self::Sphere { radius } => Vec3::new(radius, radius, radius),
            Self::Box { half_extents } => half_extents,
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
    use renew_fixed::{Fixed, Vec3};

    fn v(x: i32, y: i32, z: i32) -> Vec3 {
        Vec3::new(Fixed::from_int(x), Fixed::from_int(y), Fixed::from_int(z))
    }

    #[test]
    fn a_box_bounds_exactly_its_half_extents() {
        let bounds = Shape::Box {
            half_extents: v(1, 2, 3),
        }
        .world_bounds(Transform::at(v(10, 20, 30)));
        assert_eq!(bounds.min, v(9, 18, 27));
        assert_eq!(bounds.max, v(11, 22, 33));
    }

    #[test]
    fn a_sphere_bounds_its_radius_on_every_axis() {
        let bounds = Shape::Sphere {
            radius: Fixed::from_int(2),
        }
        .world_bounds(Transform::at(v(0, 0, 5)));
        assert_eq!(bounds.min, v(-2, -2, 3));
        assert_eq!(bounds.max, v(2, 2, 7));
    }

    /// **Separation on any one axis is separation.** A test that checks two
    /// and forgets the third proposes every pair in a column — correct, and
    /// useless.
    #[test]
    fn separation_on_any_single_axis_is_enough() {
        let here = Aabb {
            min: v(0, 0, 0),
            max: v(1, 1, 1),
        };
        for offset in [v(3, 0, 0), v(0, 3, 0), v(0, 0, 3)] {
            let there = Aabb {
                min: offset,
                max: offset + v(1, 1, 1),
            };
            assert!(
                !here.overlaps(there),
                "separated along one axis is separated"
            );
            assert!(!there.overlaps(here), "and symmetrically so");
        }
    }

    #[test]
    fn boxes_that_merely_touch_overlap() {
        let here = Aabb {
            min: v(0, 0, 0),
            max: v(1, 1, 1),
        };
        let touching = Aabb {
            min: v(1, 0, 0),
            max: v(2, 1, 1),
        };
        assert!(here.overlaps(touching));
        assert!(touching.overlaps(here));
    }

    #[test]
    fn expanding_makes_near_misses_into_candidates() {
        let here = Aabb {
            min: v(0, 0, 0),
            max: v(1, 1, 1),
        };
        let apart = Aabb {
            min: v(0, 0, 2),
            max: v(1, 1, 3),
        };
        assert!(!here.overlaps(apart));
        assert!(here.expanded(Fixed::ONE).overlaps(apart));
    }

    #[test]
    fn a_point_box_contains_only_itself_and_a_bigger_one_contains_more() {
        let dot = Aabb::point(v(3, 4, 5));
        assert!(dot.contains(v(3, 4, 5)));
        assert!(!dot.contains(v(3, 4, 6)));
        assert!(dot.overlaps(dot));

        let around = Aabb {
            min: v(0, 0, 0),
            max: v(9, 9, 9),
        };
        assert!(around.contains(v(3, 4, 5)));
        assert!(around.contains(v(0, 0, 0)), "the boundary is inside");
        assert!(!around.contains(v(3, 4, 10)), "and past it is not");
    }
}
