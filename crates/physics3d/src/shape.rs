//! What a collider is shaped like, and where it sits on its body.

use renew_fixed::{Fixed, Vec3};

/// A placement in three dimensions.
///
/// **A translation, and nothing else.** Rotating in three dimensions needs a
/// fixed-point *orientation representation* — a quaternion or a matrix — and
/// that is a decision with its own review that has not been taken. The two
/// dimensional crate rotates because a rotation there is one scalar; here it
/// would be four, with a normalisation rule and a composition order to fix.
///
/// This is a restriction with a named unblocker rather than a shape of the
/// world: the vocabulary defines rotated shapes and this crate cannot yet
/// express one. A voxel world never asks for one, which is why it is the right
/// place to start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Transform {
    /// Position.
    pub translation: Vec3,
}

impl Transform {
    /// At the origin.
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
    };

    /// At a position.
    #[must_use]
    pub const fn at(translation: Vec3) -> Self {
        Self { translation }
    }

    /// This placement applied on top of another.
    ///
    /// Addition, while there is no rotation. It is a named operation rather
    /// than a bare `+` because the moment an orientation type exists this is
    /// the one call site that has to change, and a search for `compose` finds
    /// it where a search for `+` finds everything.
    #[must_use]
    pub fn compose(self, outer: Self) -> Self {
        Self {
            translation: outer.translation + self.translation,
        }
    }
}

/// The shape families.
///
/// Each is defined in body space. With no rotation the box's axes are the
/// world's, which is exactly what a voxel world wants and is the reason this
/// crate is useful before the orientation decision is taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    /// Half-extents from the shape's local origin.
    Box { half_extents: Vec3 },
    /// Radius about the shape's local origin.
    Sphere { radius: Fixed },
}

impl Shape {
    /// The furthest any point of this shape lies from its local origin.
    #[must_use]
    pub fn bounding_radius(self) -> Fixed {
        match self {
            Self::Box { half_extents } => half_extents.length(),
            Self::Sphere { radius } => radius,
        }
    }

    /// Whether the operands describe a shape at all.
    ///
    /// Zero extents are legal — the query vocabulary requires a zero-radius
    /// sphere to be answerable rather than refused — but negative ones are
    /// not, and they are what a caller produces by subtracting in the wrong
    /// order.
    #[must_use]
    pub fn is_valid(self) -> bool {
        match self {
            Self::Box { half_extents } => {
                half_extents.x >= Fixed::ZERO
                    && half_extents.y >= Fixed::ZERO
                    && half_extents.z >= Fixed::ZERO
            }
            Self::Sphere { radius } => radius >= Fixed::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Shape, Transform};
    use renew_fixed::{Fixed, Vec3};

    fn v(x: i32, y: i32, z: i32) -> Vec3 {
        Vec3::new(Fixed::from_int(x), Fixed::from_int(y), Fixed::from_int(z))
    }

    #[test]
    fn composing_with_the_identity_changes_nothing() {
        let placed = Transform::at(v(3, 4, 5));
        assert_eq!(placed.compose(Transform::IDENTITY), placed);
        assert_eq!(Transform::IDENTITY.compose(placed), placed);
    }

    #[test]
    fn composing_adds_the_placements() {
        let local = Transform::at(v(1, 2, 3));
        let body = Transform::at(v(10, 20, 30));
        assert_eq!(local.compose(body).translation, v(11, 22, 33));
    }

    #[test]
    fn a_bounding_radius_covers_the_shape() {
        assert_eq!(
            Shape::Sphere {
                radius: Fixed::from_int(3)
            }
            .bounding_radius(),
            Fixed::from_int(3)
        );
        // A 1-2-2 box reaches its corner at 3.
        assert_eq!(
            Shape::Box {
                half_extents: v(1, 2, 2)
            }
            .bounding_radius(),
            Fixed::from_int(3)
        );
    }

    #[test]
    fn zero_sized_shapes_are_valid_and_negative_ones_are_not() {
        assert!(
            Shape::Sphere {
                radius: Fixed::ZERO
            }
            .is_valid()
        );
        assert!(
            Shape::Box {
                half_extents: Vec3::ZERO
            }
            .is_valid()
        );
        assert!(
            !Shape::Sphere {
                radius: Fixed::from_int(-1)
            }
            .is_valid()
        );
        // Every axis is checked, not just the first — a negative on z is as
        // wrong as one on x and easier to miss.
        assert!(
            !Shape::Box {
                half_extents: v(-1, 1, 1)
            }
            .is_valid()
        );
        assert!(
            !Shape::Box {
                half_extents: v(1, -1, 1)
            }
            .is_valid()
        );
        assert!(
            !Shape::Box {
                half_extents: v(1, 1, -1)
            }
            .is_valid()
        );
    }
}
