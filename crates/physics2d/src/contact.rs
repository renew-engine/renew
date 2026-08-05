//! What a touch reports.

use crate::world::Collider;
use renew_fixed::{Fixed, Vec2};

/// The most points one contact can carry in two dimensions.
///
/// A face-face meeting between two convex shapes is the overlap of two
/// segments, which is a segment: two endpoints. Curved shapes touch at one
/// point. So the bound is a property of the dimension rather than a budget,
/// and a manifold either fits it or the geometry was not convex.
pub const MAX_MANIFOLD_POINTS: usize = 2;

/// One point of a manifold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContactPoint {
    /// Where, in world space.
    pub position: Vec2,
    /// How far the shapes interpenetrate along the manifold's normal.
    ///
    /// Never negative. **Zero means touching, or within the contact
    /// tolerance** — not exactly coincident, because exact coincidence is not
    /// something fixed-point arithmetic produces and a contract that demanded
    /// it would describe a case that never arises.
    pub depth: Fixed,
}

/// A touch between two colliders.
///
/// **A report is a manifold, not a point.** Reporting one representative point
/// for a face contact is how a box resting on a floor starts to rock: the
/// caller sees a single support where there are two, and any response it
/// computes is asymmetric. The two colliders are ordered lexicographically,
/// lower first, so two reports of the same pair compare without normalising.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contact {
    /// The lower collider, by the ordering in [`Collider`].
    pub first: Collider,
    /// The higher collider.
    pub second: Collider,
    /// Unit to within four parts in 65536, pointing **from `first` toward
    /// `second`** — the direction `first` would have to move to separate.
    pub normal: Vec2,
    /// The manifold's points, `count` of them valid.
    pub points: [ContactPoint; MAX_MANIFOLD_POINTS],
    /// How many of `points` are valid.
    pub count: u8,
}

impl Contact {
    /// The valid points.
    #[must_use]
    pub fn points(&self) -> &[ContactPoint] {
        let count = usize::from(self.count).min(MAX_MANIFOLD_POINTS);
        self.points.split_at(count).0
    }

    /// The deepest penetration in the manifold.
    #[must_use]
    pub fn deepest(&self) -> Fixed {
        self.points()
            .iter()
            .map(|point| point.depth)
            .max()
            .unwrap_or(Fixed::ZERO)
    }
}

/// A touch between two shapes, before it is attributed to colliders.
///
/// Narrowphase computes geometry; naming the colliders is bookkeeping the
/// caller does once. Keeping them apart means the geometry can be tested
/// without a world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Manifold {
    /// From the first shape toward the second.
    pub normal: Vec2,
    /// The points.
    pub points: [ContactPoint; MAX_MANIFOLD_POINTS],
    /// How many are valid.
    pub count: u8,
}

impl Manifold {
    /// A one-point manifold.
    #[must_use]
    pub fn single(normal: Vec2, position: Vec2, depth: Fixed) -> Self {
        Self {
            normal,
            points: [ContactPoint { position, depth }; MAX_MANIFOLD_POINTS],
            count: 1,
        }
    }

    /// The valid points.
    #[must_use]
    pub fn points(&self) -> &[ContactPoint] {
        let count = usize::from(self.count).min(MAX_MANIFOLD_POINTS);
        self.points.split_at(count).0
    }

    /// The deepest penetration in the manifold.
    #[must_use]
    pub fn deepest(&self) -> Fixed {
        self.points()
            .iter()
            .map(|point| point.depth)
            .max()
            .unwrap_or(Fixed::ZERO)
    }

    /// Attribute this manifold to a pair of colliders.
    ///
    /// **Flips the normal if the pair is given in the other order**, because
    /// the report's normal is defined relative to the *lower* collider and the
    /// geometry knows only which shape it was handed first.
    #[must_use]
    pub fn attribute(self, first: Collider, second: Collider) -> Contact {
        if first <= second {
            Contact {
                first,
                second,
                normal: self.normal,
                points: self.points,
                count: self.count,
            }
        } else {
            Contact {
                first: second,
                second: first,
                normal: -self.normal,
                points: self.points,
                count: self.count,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Contact, ContactPoint, MAX_MANIFOLD_POINTS, Manifold};
    use crate::world::{Collider, ShapeIndex};
    use renew_fixed::{Fixed, Vec2};

    fn v(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    #[test]
    fn a_single_point_manifold_reports_one_point() {
        let manifold = Manifold::single(v(0, 1), v(3, 4), Fixed::from_int(2));
        assert_eq!(manifold.count, 1);
        assert_eq!(manifold.points().len(), 1);
        assert_eq!(manifold.points()[0].position, v(3, 4));
        assert_eq!(manifold.points()[0].depth, Fixed::from_int(2));
    }

    #[test]
    fn a_count_past_the_maximum_cannot_read_past_the_array() {
        let contact = Contact {
            first: Collider {
                handle: renew_ecs::Entities::new().spawn(),
                index: ShapeIndex::from_raw(0),
            },
            second: Collider {
                handle: renew_ecs::Entities::new().spawn(),
                index: ShapeIndex::from_raw(0),
            },
            normal: v(1, 0),
            points: [ContactPoint {
                position: Vec2::ZERO,
                depth: Fixed::ZERO,
            }; MAX_MANIFOLD_POINTS],
            count: 200,
        };
        assert_eq!(contact.points().len(), MAX_MANIFOLD_POINTS);
        assert_eq!(contact.deepest(), Fixed::ZERO);
    }

    /// The normal is defined relative to the lower collider, and the geometry
    /// only knows which shape it was handed first — so attributing a manifold
    /// to a reversed pair has to flip it, or every report from that pair
    /// points the wrong way.
    #[test]
    fn attributing_a_reversed_pair_flips_the_normal() {
        let mut entities = renew_ecs::Entities::new();
        let low = Collider {
            handle: entities.spawn(),
            index: ShapeIndex::from_raw(0),
        };
        let high = Collider {
            handle: entities.spawn(),
            index: ShapeIndex::from_raw(0),
        };
        assert!(low < high, "the fixture depends on this");

        let manifold = Manifold::single(v(1, 0), Vec2::ZERO, Fixed::ONE);
        let forward = manifold.attribute(low, high);
        assert_eq!(forward.first, low);
        assert_eq!(forward.normal, v(1, 0));

        let reversed = manifold.attribute(high, low);
        assert_eq!(reversed.first, low, "the pair is reordered");
        assert_eq!(reversed.normal, v(-1, 0), "and the normal follows it");
        assert_eq!(reversed.deepest(), Fixed::ONE);
    }
}
