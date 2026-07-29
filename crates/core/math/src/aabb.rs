//! Axis-aligned bounding box. Layout: `min` then `max` ([`Vec3`] each),
//! 24 bytes, `#[repr(C)]`.

use crate::Vec3;

/// An axis-aligned box, stored as its component-wise minimum and maximum
/// corners. Invariant: `min` does not exceed `max` on any axis (checked
/// by a debug assertion at construction).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb3 {
    min: Vec3,
    max: Vec3,
}

impl Aabb3 {
    /// Caller contract: `min` does not exceed `max` on any axis (debug
    /// assertion).
    #[must_use]
    pub fn new(min: Vec3, max: Vec3) -> Self {
        debug_assert!(
            min.x <= max.x && min.y <= max.y && min.z <= max.z,
            "Aabb3 requires min <= max on every axis"
        );
        Self { min, max }
    }

    /// The tightest box around a set of points; `None` for no points
    /// (an empty set has no bounds — a normal, cause-free absence).
    ///
    /// Caller contract: coordinates are finite — NaN corrupts min/max
    /// ordering (debug assertion; in release, garbage in, garbage out).
    #[must_use]
    pub fn from_points(points: &[Vec3]) -> Option<Self> {
        let (&first, rest) = points.split_first()?;
        let (min, max) = rest.iter().fold((first, first), |(low, high), &point| {
            (low.min(point), high.max(point))
        });
        debug_assert!(
            min.x <= max.x && min.y <= max.y && min.z <= max.z,
            "from_points requires finite coordinates"
        );
        Some(Self { min, max })
    }

    #[must_use]
    pub const fn min(self) -> Vec3 {
        self.min
    }

    #[must_use]
    pub const fn max(self) -> Vec3 {
        self.max
    }

    #[must_use]
    pub fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    #[must_use]
    pub fn extents(self) -> Vec3 {
        self.max - self.min
    }

    /// The smallest box containing both operands.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    /// Whether the point lies inside or on the boundary. Branchless: the
    /// six comparisons combine through [`all`], never through
    /// short-circuit evaluation.
    #[must_use]
    pub fn contains(self, point: Vec3) -> bool {
        all([
            self.min.x <= point.x,
            point.x <= self.max.x,
            self.min.y <= point.y,
            point.y <= self.max.y,
            self.min.z <= point.z,
            point.z <= self.max.z,
        ])
    }

    /// Whether two boxes overlap (touching counts). The intersection of
    /// two boxes is `[max of mins, min of maxes]`; they overlap exactly
    /// when that interval is non-empty on every axis. Branchless via
    /// [`all`].
    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        let low = self.min.max(other.min);
        let high = self.max.min(other.max);
        all([low.x <= high.x, low.y <= high.y, low.z <= high.z])
    }
}

/// Branchless conjunction: folds through integer bits so no short-circuit
/// branch is emitted. (Short-circuit `&&` — including the one inside a
/// derived `PartialEq` — compiles to data-dependent jumps; this fold
/// compiles to `setcc`/`and` chains, verified against the disassembly.)
fn all<const N: usize>(conditions: [bool; N]) -> bool {
    conditions
        .into_iter()
        .map(u8::from)
        .fold(1u8, |accumulator, bit| accumulator & bit)
        == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_box() -> Aabb3 {
        Aabb3::new(Vec3::ZERO, Vec3::splat(1.0))
    }

    #[test]
    fn contains_includes_the_boundary() {
        let b = unit_box();
        assert!(b.contains(Vec3::splat(0.5)));
        assert!(b.contains(Vec3::ZERO));
        assert!(b.contains(Vec3::splat(1.0)));
        assert!(!b.contains(Vec3::new(1.1, 0.5, 0.5)));
        assert!(!b.contains(Vec3::new(0.5, -0.1, 0.5)));
    }

    #[test]
    fn intersects_counts_touching_boxes() {
        let b = unit_box();
        let touching = Aabb3::new(Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 1.0));
        let separate = Aabb3::new(Vec3::splat(2.0), Vec3::splat(3.0));
        assert!(b.intersects(touching));
        assert!(touching.intersects(b));
        assert!(!b.intersects(separate));
    }

    #[test]
    fn union_contains_both_operands() {
        let a = unit_box();
        let b = Aabb3::new(Vec3::splat(-2.0), Vec3::splat(-1.0));
        let u = a.union(b);
        assert!(u.contains(a.min()) && u.contains(a.max()));
        assert!(u.contains(b.min()) && u.contains(b.max()));
    }

    #[test]
    fn from_points_is_tight_and_empty_is_none() {
        assert_eq!(Aabb3::from_points(&[]), None);
        let points = [
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(-2.0, 3.0, 5.0),
            Vec3::new(0.0, 0.0, -4.0),
        ];
        let b = Aabb3::from_points(&points).expect("non-empty");
        assert_eq!(b.min(), Vec3::new(-2.0, -1.0, -4.0));
        assert_eq!(b.max(), Vec3::new(1.0, 3.0, 5.0));
    }

    #[test]
    fn center_and_extents_agree_with_corners() {
        let b = Aabb3::new(Vec3::new(-1.0, 0.0, 2.0), Vec3::new(3.0, 4.0, 6.0));
        assert_eq!(b.center(), Vec3::new(1.0, 2.0, 4.0));
        assert_eq!(b.extents(), Vec3::new(4.0, 4.0, 4.0));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "min <= max")]
    fn an_inverted_box_is_a_contract_violation() {
        let _ = Aabb3::new(Vec3::splat(1.0), Vec3::ZERO);
    }
}
