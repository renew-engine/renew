//! Vectors over [`Fixed`].
//!
//! Two dimensions and three, as separate concrete types rather than one
//! generic over dimension. The physics contract makes the same choice for the
//! same reason: dimension-generic vocabularies produce signatures nobody can
//! read, and the cost of writing `dot` twice is smaller than the cost of every
//! caller reading a bound.

use core::ops::{Add, Mul, Neg, Sub};

use crate::Fixed;

/// A two-dimensional vector.
///
/// # Contract
///
/// - **Every operation is deterministic**, because every operation is
///   [`Fixed`] arithmetic and nothing else.
/// - **Saturating throughout**, inheriting the scalar's behaviour: a component
///   that overflows clamps and is counted rather than wrapping.
/// - **`Eq` and `Hash`**, so a vector can be a map key or enter a state hash
///   directly — which is the thing a float vector cannot offer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Vec2 {
    pub x: Fixed,
    pub y: Fixed,
}

/// A three-dimensional vector. See [`Vec2`] for the contract; it is the same.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Vec3 {
    pub x: Fixed,
    pub y: Fixed,
    pub z: Fixed,
}

impl Vec2 {
    /// The origin.
    pub const ZERO: Self = Self {
        x: Fixed::ZERO,
        y: Fixed::ZERO,
    };
    /// One unit along x.
    pub const X: Self = Self {
        x: Fixed::ONE,
        y: Fixed::ZERO,
    };
    /// One unit along y.
    pub const Y: Self = Self {
        x: Fixed::ZERO,
        y: Fixed::ONE,
    };

    /// From components.
    #[must_use]
    pub const fn new(x: Fixed, y: Fixed) -> Self {
        Self { x, y }
    }

    /// From whole numbers, which is how most call sites write a constant.
    #[must_use]
    pub const fn from_ints(x: i32, y: i32) -> Self {
        Self {
            x: Fixed::from_int(x),
            y: Fixed::from_int(y),
        }
    }

    /// The dot product.
    #[must_use]
    pub fn dot(self, other: Self) -> Fixed {
        self.x.saturating_mul(other.x) + self.y.saturating_mul(other.y)
    }

    /// The 2D cross product: a scalar, the z of the 3D cross of these vectors
    /// lifted into the plane. Positive when `other` is counter-clockwise of
    /// `self`, which is what a winding test reads.
    #[must_use]
    pub fn cross(self, other: Self) -> Fixed {
        self.x.saturating_mul(other.y) - self.y.saturating_mul(other.x)
    }

    /// The squared length.
    ///
    /// Preferred over [`Vec2::length`] wherever a comparison will do, and not
    /// only for speed: this is exact where the length is rounded, so two
    /// vectors that compare equal by squared length may compare unequal by
    /// length.
    #[must_use]
    pub fn length_squared(self) -> Fixed {
        self.dot(self)
    }

    /// The length, floored to the representable value below the exact one.
    #[must_use]
    pub fn length(self) -> Fixed {
        self.length_squared().sqrt()
    }

    /// The distance to another point.
    #[must_use]
    pub fn distance(self, other: Self) -> Fixed {
        (self - other).length()
    }

    /// A unit vector in the same direction, or `None` for the zero vector.
    ///
    /// Fallible rather than asserting, because the zero vector is a value a
    /// simulation legitimately produces — a body at rest, a contact between
    /// coincident points — and refusing it would put an assertion on a path
    /// that runs every frame.
    ///
    /// **The result is unit-length only to the type's resolution.** Each
    /// component is rounded, so the length of the result may differ from one
    /// by up to about 2⁻¹⁵. Callers that need an exact guarantee should
    /// compare squared lengths against a tolerance rather than expecting
    /// exactly [`Fixed::ONE`].
    #[must_use]
    pub fn normalize(self) -> Option<Self> {
        let length = self.length();
        if length == Fixed::ZERO {
            return None;
        }
        Some(Self::new(
            self.x.saturating_div(length),
            self.y.saturating_div(length),
        ))
    }

    /// Linear interpolation, `t` clamped to `[0, 1]`.
    ///
    /// Written as `a + (b - a) * t` rather than `a*(1-t) + b*t`: the second is
    /// the numerically better form in floating point and the worse one here,
    /// because it rounds twice as often and neither form gains anything from
    /// exactness at the endpoints — this one is exact at both by construction.
    #[must_use]
    pub fn lerp(self, other: Self, t: Fixed) -> Self {
        let t = t.clamp(Fixed::ZERO, Fixed::ONE);
        self + (other - self) * t
    }

    /// The component of `self` along `direction`, which must be unit-length.
    ///
    /// The building block of move-and-slide: removing this from a
    /// displacement is what makes a body slide along a wall rather than stop
    /// at it.
    #[must_use]
    pub fn project_onto_unit(self, direction: Self) -> Self {
        direction * self.dot(direction)
    }

    /// `self` with its component along `normal` removed.
    ///
    /// `normal` must be unit-length. This is the slide operation itself, named
    /// so the physics implementation does not spell it out at each call site
    /// and get the sign wrong at one of them.
    #[must_use]
    pub fn slide_along(self, normal: Self) -> Self {
        self - self.project_onto_unit(normal)
    }

    /// Perpendicular, rotated a quarter turn counter-clockwise.
    ///
    /// Exact — a quarter turn is a swap and a negation, needing no
    /// trigonometry, which is why this is available when general rotation is
    /// not.
    #[must_use]
    pub fn perpendicular(self) -> Self {
        Self::new(-self.y, self.x)
    }
}

impl Vec3 {
    /// The origin.
    pub const ZERO: Self = Self {
        x: Fixed::ZERO,
        y: Fixed::ZERO,
        z: Fixed::ZERO,
    };

    /// From components.
    #[must_use]
    pub const fn new(x: Fixed, y: Fixed, z: Fixed) -> Self {
        Self { x, y, z }
    }

    /// From whole numbers.
    #[must_use]
    pub const fn from_ints(x: i32, y: i32, z: i32) -> Self {
        Self {
            x: Fixed::from_int(x),
            y: Fixed::from_int(y),
            z: Fixed::from_int(z),
        }
    }

    /// The dot product.
    #[must_use]
    pub fn dot(self, other: Self) -> Fixed {
        self.x.saturating_mul(other.x)
            + self.y.saturating_mul(other.y)
            + self.z.saturating_mul(other.z)
    }

    /// The cross product: a vector perpendicular to both.
    #[must_use]
    pub fn cross(self, other: Self) -> Self {
        Self::new(
            self.y.saturating_mul(other.z) - self.z.saturating_mul(other.y),
            self.z.saturating_mul(other.x) - self.x.saturating_mul(other.z),
            self.x.saturating_mul(other.y) - self.y.saturating_mul(other.x),
        )
    }

    /// The squared length. See [`Vec2::length_squared`] on why to prefer it.
    #[must_use]
    pub fn length_squared(self) -> Fixed {
        self.dot(self)
    }

    /// The length, floored.
    #[must_use]
    pub fn length(self) -> Fixed {
        self.length_squared().sqrt()
    }

    /// The distance to another point.
    #[must_use]
    pub fn distance(self, other: Self) -> Fixed {
        (self - other).length()
    }

    /// A unit vector in the same direction, or `None` for the zero vector.
    /// See [`Vec2::normalize`] on how close to unit the result is.
    #[must_use]
    pub fn normalize(self) -> Option<Self> {
        let length = self.length();
        if length == Fixed::ZERO {
            return None;
        }
        Some(Self::new(
            self.x.saturating_div(length),
            self.y.saturating_div(length),
            self.z.saturating_div(length),
        ))
    }

    /// Linear interpolation, `t` clamped to `[0, 1]`.
    #[must_use]
    pub fn lerp(self, other: Self, t: Fixed) -> Self {
        let t = t.clamp(Fixed::ZERO, Fixed::ONE);
        self + (other - self) * t
    }

    /// `self` with its component along a unit `normal` removed.
    #[must_use]
    pub fn slide_along(self, normal: Self) -> Self {
        self - normal * self.dot(normal)
    }
}

// The operators, rather than inherent `add`/`sub`/`neg`/`scale`. For a vector
// these read the way the maths does, and inherent methods by those names
// shadow the traits confusingly enough that the linter says so.

impl Add for Vec2 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }
}

impl Sub for Vec2 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }
}

impl Neg for Vec2 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

/// Scaled by a scalar. Saturating componentwise, like everything else here.
impl Mul<Fixed> for Vec2 {
    type Output = Self;
    fn mul(self, factor: Fixed) -> Self {
        Self::new(self.x.saturating_mul(factor), self.y.saturating_mul(factor))
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

impl Mul<Fixed> for Vec3 {
    type Output = Self;
    fn mul(self, factor: Fixed) -> Self {
        Self::new(
            self.x.saturating_mul(factor),
            self.y.saturating_mul(factor),
            self.z.saturating_mul(factor),
        )
    }
}
