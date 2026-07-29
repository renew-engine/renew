//! Rotation quaternion. Layout: `[x, y, z, w]` (`w` is the scalar part),
//! 16 bytes, `#[repr(C)]`.

use crate::Vec3;

/// A rotation quaternion. Only unit quaternions represent rotations;
/// constructors that promise a unit result say so.
///
/// Deliberately no `Default`: the all-zero quaternion is not a rotation.
/// Use [`Quat::IDENTITY`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    /// The no-rotation quaternion.
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// Rotation of `angle` radians around `axis`.
    ///
    /// Caller contract: `axis` is a unit vector (debug assertion). The
    /// result is a unit quaternion.
    #[must_use]
    pub fn from_axis_angle(axis: Vec3, angle: f32) -> Self {
        debug_assert!(
            (axis.length_squared() - 1.0).abs() < 1e-4,
            "from_axis_angle requires a unit axis"
        );
        let half = angle * 0.5;
        let (sin, cos) = half.sin_cos();
        Self {
            x: axis.x * sin,
            y: axis.y * sin,
            z: axis.z * sin,
            w: cos,
        }
    }

    #[must_use]
    pub fn length_squared(self) -> f32 {
        self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w
    }

    #[must_use]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Unit quaternion in this quaternion's direction.
    ///
    /// Caller contract: the squared length is positive (debug assertion);
    /// in release, violating inputs yield non-finite components.
    #[must_use]
    pub fn normalize(self) -> Self {
        debug_assert!(
            self.length_squared() > 0.0,
            "normalize requires a positive squared length"
        );
        let inverse_length = 1.0 / self.length();
        Self {
            x: self.x * inverse_length,
            y: self.y * inverse_length,
            z: self.z * inverse_length,
            w: self.w * inverse_length,
        }
    }

    /// The reverse rotation (conjugate; equals the inverse for unit
    /// quaternions).
    #[must_use]
    pub fn conjugate(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: self.w,
        }
    }

    /// The vector (imaginary) part.
    #[must_use]
    const fn vector_part(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }

    /// Rotate a vector by this quaternion (which must be unit for the
    /// result to be a pure rotation).
    #[must_use]
    pub fn rotate(self, v: Vec3) -> Vec3 {
        // v' = v + 2w(u x v) + 2(u x (u x v)) with u the vector part —
        // branchless and cheaper than the double quaternion product.
        let u = self.vector_part();
        let uv = u.cross(v);
        let uuv = u.cross(uv);
        v + (uv * self.w + uuv) * 2.0
    }
}

impl core::ops::Mul for Quat {
    type Output = Self;

    /// Hamilton product: `a * b` applies `b` first, then `a`.
    fn mul(self, other: Self) -> Self {
        Self {
            x: self.w * other.x + self.x * other.w + self.y * other.z - self.z * other.y,
            y: self.w * other.y - self.x * other.z + self.y * other.w + self.z * other.x,
            z: self.w * other.z + self.x * other.y - self.y * other.x + self.z * other.w,
            w: self.w * other.w - self.x * other.x - self.y * other.y - self.z * other.z,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAU: f32 = core::f32::consts::TAU;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn identity_rotates_nothing_bit_exactly() {
        let v = Vec3::new(0.25, -3.5, 8.0);
        let rotated = Quat::IDENTITY.rotate(v);
        assert_eq!(rotated.x.to_bits(), v.x.to_bits());
        assert_eq!(rotated.y.to_bits(), v.y.to_bits());
        assert_eq!(rotated.z.to_bits(), v.z.to_bits());
    }

    #[test]
    fn quarter_turns_land_on_axes() {
        let quarter = Quat::from_axis_angle(Vec3::Z, TAU / 4.0);
        assert!(close(quarter.rotate(Vec3::X), Vec3::Y));
        assert!(close(quarter.rotate(Vec3::Y), -Vec3::X));

        let half = Quat::from_axis_angle(Vec3::Y, TAU / 2.0);
        assert!(close(half.rotate(Vec3::X), -Vec3::X));
    }

    #[test]
    fn composition_matches_sequential_rotation() {
        let first = Quat::from_axis_angle(Vec3::X, 0.7);
        let second = Quat::from_axis_angle(Vec3::Z, -1.3);
        let composed = second * first;
        let v = Vec3::new(1.0, 2.0, 3.0);
        assert!(close(composed.rotate(v), second.rotate(first.rotate(v))));
    }

    #[test]
    fn conjugate_reverses_a_rotation() {
        let q = Quat::from_axis_angle(Vec3::new(0.0, 0.6, 0.8), 0.9);
        let v = Vec3::new(-2.0, 1.0, 0.5);
        assert!(close(q.conjugate().rotate(q.rotate(v)), v));
    }

    #[test]
    fn normalize_yields_unit_length() {
        let q = Quat::new(1.0, 2.0, 3.0, 4.0).normalize();
        assert!((q.length() - 1.0).abs() < 1e-6);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "unit axis")]
    fn a_non_unit_axis_is_a_contract_violation() {
        let _ = Quat::from_axis_angle(Vec3::splat(3.0), 1.0);
    }
}
