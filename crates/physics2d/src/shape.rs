//! What a collider is shaped like, and where it sits on its body.

use renew_fixed::{Angle, Fixed, Vec2};

/// A translation and a rotation.
///
/// Rotation is an `Angle` — a binary angle, so it wraps exactly and has no
/// unrepresentable values — which the number type supports in two dimensions
/// because a rotation there is one scalar. Three dimensions need an
/// orientation representation that does not exist yet, which is why this type
/// is 2D-only rather than shared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Transform {
    /// Position.
    pub translation: Vec2,
    /// Rotation about the origin.
    pub rotation: Angle,
}

impl Transform {
    /// At the origin, unrotated.
    pub const IDENTITY: Self = Self {
        translation: Vec2::ZERO,
        rotation: Angle::ZERO,
    };

    /// A translation with no rotation.
    #[must_use]
    pub const fn at(translation: Vec2) -> Self {
        Self {
            translation,
            rotation: Angle::ZERO,
        }
    }

    /// Both.
    #[must_use]
    pub const fn new(translation: Vec2, rotation: Angle) -> Self {
        Self {
            translation,
            rotation,
        }
    }

    /// This transform applied on top of `outer` — used to put a shape's local
    /// placement into world space, since a shape sits on a body and the body
    /// has a transform of its own.
    ///
    /// Without a per-shape placement a body could not own two shapes in
    /// different places, and the one-way platform that motivates multi-shape
    /// bodies at all would be unbuildable.
    #[must_use]
    pub fn compose(self, outer: Self) -> Self {
        Self {
            translation: outer.translation + self.translation.rotate(outer.rotation),
            rotation: outer.rotation + self.rotation,
        }
    }
}

/// The shape families, with their operands named.
///
/// Each is defined in **body space**, so a box rotates with its body: its
/// axis-alignment is to the body's axes rather than the world's. Stating the
/// frame matters because "axis-aligned box" under a rotated body has two
/// readings and they are different shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    /// Half-extents from the shape's local origin.
    Box { half_extents: Vec2 },
    /// Radius about the shape's local origin.
    Circle { radius: Fixed },
    /// A segment along the local *y* axis with rounded ends.
    Capsule { radius: Fixed, half_height: Fixed },
}

impl Shape {
    /// The furthest any point of this shape lies from its local origin.
    ///
    /// Rotation-invariant, so a caller can bound a rotating shape without
    /// recomputing per angle. Deliberately not the broadphase interval, which
    /// wants the exact support along an axis rather than a circle around
    /// everything — a conservative bound there would change the candidate set.
    #[must_use]
    pub fn bounding_radius(self) -> Fixed {
        match self {
            Self::Box { half_extents } => half_extents.length(),
            Self::Circle { radius } => radius,
            Self::Capsule {
                radius,
                half_height,
            } => radius + half_height,
        }
    }

    /// Whether the operands describe a shape at all.
    ///
    /// Zero extents are legal — the query vocabulary requires a zero-radius
    /// circle to be answerable rather than refused — but negative ones are
    /// not, and they are what a caller produces by subtracting in the wrong
    /// order.
    #[must_use]
    pub fn is_valid(self) -> bool {
        match self {
            Self::Box { half_extents } => {
                half_extents.x >= Fixed::ZERO && half_extents.y >= Fixed::ZERO
            }
            Self::Circle { radius } => radius >= Fixed::ZERO,
            Self::Capsule {
                radius,
                half_height,
            } => radius >= Fixed::ZERO && half_height >= Fixed::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Shape, Transform};
    use renew_fixed::{Angle, Fixed, Vec2};

    fn v(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    #[test]
    fn composing_with_the_identity_changes_nothing() {
        let t = Transform::new(v(3, 4), Angle::from_turn_ratio(1, 8));
        assert_eq!(t.compose(Transform::IDENTITY), t);
        assert_eq!(Transform::IDENTITY.compose(t), t);
    }

    /// The property a per-shape placement exists for: a shape offset from its
    /// body swings around the body when the body turns, rather than sliding.
    #[test]
    fn a_local_offset_rotates_with_its_body() {
        let local = Transform::at(v(1, 0));
        let body = Transform::new(v(0, 0), Angle::from_turn_ratio(1, 4));
        let world = local.compose(body);
        // A quarter turn takes +x to +y.
        assert_eq!(world.translation.x.trunc_int(), 0);
        assert_eq!(world.translation.y.trunc_int(), 1);
    }

    #[test]
    fn rotations_add_when_composed() {
        let eighth = Angle::from_turn_ratio(1, 8);
        let a = Transform::new(Vec2::ZERO, eighth);
        let b = Transform::new(Vec2::ZERO, eighth);
        assert_eq!(a.compose(b).rotation, Angle::from_turn_ratio(1, 4));
    }

    #[test]
    fn zero_sized_shapes_are_valid_and_negative_ones_are_not() {
        // The query vocabulary requires a zero-radius circle to answer rather
        // than refuse, so it has to be constructible.
        assert!(
            Shape::Circle {
                radius: Fixed::ZERO
            }
            .is_valid()
        );
        assert!(
            Shape::Box {
                half_extents: Vec2::ZERO
            }
            .is_valid()
        );
        assert!(
            !Shape::Circle {
                radius: Fixed::from_int(-1)
            }
            .is_valid()
        );
        assert!(
            !Shape::Box {
                half_extents: v(1, -1)
            }
            .is_valid()
        );
    }

    #[test]
    fn a_bounding_radius_covers_the_shape() {
        assert_eq!(
            Shape::Circle {
                radius: Fixed::from_int(3)
            }
            .bounding_radius(),
            Fixed::from_int(3)
        );
        assert_eq!(
            Shape::Capsule {
                radius: Fixed::from_int(1),
                half_height: Fixed::from_int(4)
            }
            .bounding_radius(),
            Fixed::from_int(5)
        );
        // A 3-4 box reaches its corner at 5.
        assert_eq!(
            Shape::Box {
                half_extents: v(3, 4)
            }
            .bounding_radius(),
            Fixed::from_int(5)
        );
    }
}
