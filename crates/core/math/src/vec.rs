//! Vector value types. Layout: tightly packed `f32` fields in declaration
//! order (`#[repr(C)]`); `Vec4` is additionally 16-byte aligned.

use core::ops::{Add, Div, Mul, Neg, Sub};

/// A 2-component `f32` vector. Layout: `[x, y]`, 8 bytes, align 4.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// A 3-component `f32` vector. Layout: `[x, y, z]`, 12 bytes, align 4.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// A 4-component `f32` vector. Layout: `[x, y, z, w]`, 16 bytes,
/// align 16 (SIMD-ready).
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

/// Implements the shared surface for a vector type; per-component
/// operations are arithmetic on every path (no branches).
macro_rules! vector_common {
    ($name:ident, $($component:ident),+) => {
        impl $name {
            pub const ZERO: Self = Self { $($component: 0.0),+ };

            #[must_use]
            pub const fn new($($component: f32),+) -> Self {
                Self { $($component),+ }
            }

            #[must_use]
            pub const fn splat(value: f32) -> Self {
                Self { $($component: value),+ }
            }

            /// Component-wise minimum (branchless).
            #[must_use]
            pub fn min(self, other: Self) -> Self {
                Self { $($component: self.$component.min(other.$component)),+ }
            }

            /// Component-wise maximum (branchless).
            #[must_use]
            pub fn max(self, other: Self) -> Self {
                Self { $($component: self.$component.max(other.$component)),+ }
            }

            /// Component-wise clamp (branchless). Caller contract: each
            /// `low` component does not exceed its `high` counterpart.
            #[must_use]
            pub fn clamp(self, low: Self, high: Self) -> Self {
                self.max(low).min(high)
            }

            /// Linear interpolation. For finite inputs whose difference
            /// does not overflow, `t = 0` yields `self` exactly (signed
            /// zero excepted: `-0.0` components come back as `+0.0`).
            #[must_use]
            pub fn lerp(self, other: Self, t: f32) -> Self {
                self + (other - self) * t
            }

            #[must_use]
            pub fn dot(self, other: Self) -> f32 {
                0.0 $(+ self.$component * other.$component)+
            }

            #[must_use]
            pub fn length_squared(self) -> f32 {
                self.dot(self)
            }

            #[must_use]
            pub fn length(self) -> f32 {
                self.length_squared().sqrt()
            }

            /// Unit vector in this vector's direction.
            ///
            /// Caller contract: the squared length is positive — zero
            /// vectors and vectors tiny enough that their squared length
            /// rounds to zero both violate it (debug assertion). In
            /// release such inputs yield non-finite components.
            #[must_use]
            pub fn normalize(self) -> Self {
                debug_assert!(
                    self.length_squared() > 0.0,
                    "normalize requires a positive squared length"
                );
                self / self.length()
            }

            /// Unit vector, or `None` for vectors without a usable
            /// direction (zero, or containing non-finite components).
            /// Robust across the full finite range: components are
            /// pre-scaled by the largest magnitude, so subnormal and
            /// near-overflow vectors normalize accurately instead of
            /// degrading through squared-length rounding.
            #[must_use]
            pub fn try_normalize(self) -> Option<Self> {
                // `f32::max` ignores NaN, so the magnitude alone cannot
                // prove the input clean — the scaled length is guarded
                // too, and NaN poisons it by construction.
                let magnitude = 0.0_f32 $(.max(self.$component.abs()))+;
                if !(magnitude > 0.0 && magnitude.is_finite()) {
                    return None;
                }
                let scaled = self / magnitude;
                let length = scaled.length();
                if length > 0.0 && length.is_finite() {
                    Some(scaled / length)
                } else {
                    None
                }
            }
        }

        impl Add for $name {
            type Output = Self;
            fn add(self, other: Self) -> Self {
                Self { $($component: self.$component + other.$component),+ }
            }
        }

        impl Sub for $name {
            type Output = Self;
            fn sub(self, other: Self) -> Self {
                Self { $($component: self.$component - other.$component),+ }
            }
        }

        impl Neg for $name {
            type Output = Self;
            fn neg(self) -> Self {
                Self { $($component: -self.$component),+ }
            }
        }

        impl Mul<f32> for $name {
            type Output = Self;
            fn mul(self, scalar: f32) -> Self {
                Self { $($component: self.$component * scalar),+ }
            }
        }

        impl Div<f32> for $name {
            type Output = Self;
            fn div(self, scalar: f32) -> Self {
                Self { $($component: self.$component / scalar),+ }
            }
        }
    };
}

vector_common!(Vec2, x, y);
vector_common!(Vec3, x, y, z);
vector_common!(Vec4, x, y, z, w);

// Layout is API (see the type docs); hold it at compile time.
const _: () = {
    assert!(core::mem::size_of::<Vec2>() == 8 && core::mem::align_of::<Vec2>() == 4);
    assert!(core::mem::size_of::<Vec3>() == 12 && core::mem::align_of::<Vec3>() == 4);
    assert!(core::mem::size_of::<Vec4>() == 16 && core::mem::align_of::<Vec4>() == 16);
};

impl Vec3 {
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);

    /// Cross product (right-handed).
    #[must_use]
    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    /// Extend with a `w` component.
    #[must_use]
    pub const fn extend(self, w: f32) -> Vec4 {
        Vec4::new(self.x, self.y, self.z, w)
    }
}

impl Vec4 {
    /// Drop the `w` component.
    #[must_use]
    pub const fn truncate(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_and_dot_products_behave() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(4.0, -5.0, 6.0);
        assert_eq!(a + b, Vec3::new(5.0, -3.0, 9.0));
        assert_eq!(a - b, Vec3::new(-3.0, 7.0, -3.0));
        assert_eq!(-a, Vec3::new(-1.0, -2.0, -3.0));
        assert_eq!(a * 2.0, Vec3::new(2.0, 4.0, 6.0));
        assert_eq!(b / 2.0, Vec3::new(2.0, -2.5, 3.0));
        assert_eq!(a.dot(b).to_bits(), 12.0f32.to_bits());
    }

    #[test]
    fn cross_products_follow_the_right_hand_rule() {
        assert_eq!(Vec3::X.cross(Vec3::Y), Vec3::Z);
        assert_eq!(Vec3::Y.cross(Vec3::Z), Vec3::X);
        assert_eq!(Vec3::Z.cross(Vec3::X), Vec3::Y);
    }

    #[test]
    fn min_max_clamp_and_lerp_are_componentwise() {
        let a = Vec3::new(1.0, 5.0, -2.0);
        let b = Vec3::new(3.0, 2.0, -4.0);
        assert_eq!(a.min(b), Vec3::new(1.0, 2.0, -4.0));
        assert_eq!(a.max(b), Vec3::new(3.0, 5.0, -2.0));
        assert_eq!(
            Vec3::new(10.0, -10.0, 0.5).clamp(Vec3::splat(0.0), Vec3::splat(1.0)),
            Vec3::new(1.0, 0.0, 0.5)
        );
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0), Vec3::new(3.0, 2.0, -4.0));
    }

    #[test]
    fn normalize_produces_unit_vectors() {
        let n = Vec3::new(3.0, 0.0, 4.0).normalize();
        assert!((n.length() - 1.0).abs() < 1e-6);
        assert_eq!(n, Vec3::new(0.6, 0.0, 0.8));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "positive squared length")]
    fn normalizing_zero_is_a_contract_violation() {
        let _ = Vec3::ZERO.normalize();
    }

    #[test]
    fn try_normalize_reports_directionless_vectors() {
        assert_eq!(Vec3::ZERO.try_normalize(), None);
        assert_eq!(Vec2::ZERO.try_normalize(), None);
        assert!(Vec3::new(0.0, 2.0, 0.0).try_normalize().is_some());
        assert_eq!(Vec3::splat(f32::INFINITY).try_normalize(), None);
        assert_eq!(Vec3::new(f32::NAN, 1.0, 0.0).try_normalize(), None);
    }

    #[test]
    fn try_normalize_is_accurate_for_subnormal_and_huge_vectors() {
        // Squared length would round to zero here; pre-scaling keeps the
        // direction usable and the result unit-length.
        let tiny = Vec3::new(3.7e-23, 0.0, 0.0)
            .try_normalize()
            .expect("tiny but directed");
        assert!((tiny.length() - 1.0).abs() < 1e-6, "{tiny:?}");
        let even_tinier = Vec3::new(1.0e-38, -1.0e-38, 0.0)
            .try_normalize()
            .expect("subnormal but directed");
        assert!((even_tinier.length() - 1.0).abs() < 1e-6);
        // Squared length would overflow here.
        let huge = Vec3::splat(3.0e38)
            .try_normalize()
            .expect("huge but directed");
        assert!((huge.length() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn extend_and_truncate_round_trip() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(v.extend(4.0), Vec4::new(1.0, 2.0, 3.0, 4.0));
        assert_eq!(v.extend(4.0).truncate(), v);
    }
}
