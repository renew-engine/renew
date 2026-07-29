//! 4×4 matrix. Layout: **column-major** — four [`Vec4`] columns in order,
//! 64 bytes, 16-byte aligned (`#[repr(C)]` over aligned columns). Matches
//! the convention of the graphics APIs this engine targets.

use crate::{Quat, Vec3, Vec4};

/// A column-major 4×4 `f32` matrix.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    /// Columns, in order: the basis vectors, then translation.
    pub cols: [Vec4; 4],
}

impl Mat4 {
    pub const IDENTITY: Self = Self {
        cols: [
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ],
    };

    #[must_use]
    pub const fn from_cols(x: Vec4, y: Vec4, z: Vec4, w: Vec4) -> Self {
        Self { cols: [x, y, z, w] }
    }

    #[must_use]
    pub fn from_translation(translation: Vec3) -> Self {
        let mut matrix = Self::IDENTITY;
        matrix.cols[3] = translation.extend(1.0);
        matrix
    }

    #[must_use]
    pub fn from_scale(scale: Vec3) -> Self {
        Self::from_cols(
            Vec4::new(scale.x, 0.0, 0.0, 0.0),
            Vec4::new(0.0, scale.y, 0.0, 0.0),
            Vec4::new(0.0, 0.0, scale.z, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    /// Rotation matrix from a quaternion (which must be unit for the
    /// result to be a pure rotation).
    #[must_use]
    pub fn from_quat(rotation: Quat) -> Self {
        let (x, y, z, w) = (rotation.x, rotation.y, rotation.z, rotation.w);
        let (xx, yy, zz) = (x * x, y * y, z * z);
        let (xy, yz, zx) = (x * y, y * z, z * x);
        let (wx, wy, wz) = (w * x, w * y, w * z);
        Self::from_cols(
            Vec4::new(1.0 - 2.0 * (yy + zz), 2.0 * (xy + wz), 2.0 * (zx - wy), 0.0),
            Vec4::new(2.0 * (xy - wz), 1.0 - 2.0 * (xx + zz), 2.0 * (yz + wx), 0.0),
            Vec4::new(2.0 * (zx + wy), 2.0 * (yz - wx), 1.0 - 2.0 * (xx + yy), 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    #[must_use]
    pub fn transpose(self) -> Self {
        let [c0, c1, c2, c3] = self.cols;
        Self::from_cols(
            Vec4::new(c0.x, c1.x, c2.x, c3.x),
            Vec4::new(c0.y, c1.y, c2.y, c3.y),
            Vec4::new(c0.z, c1.z, c2.z, c3.z),
            Vec4::new(c0.w, c1.w, c2.w, c3.w),
        )
    }

    /// Transform a [`Vec4`].
    #[must_use]
    pub fn transform(self, vector: Vec4) -> Vec4 {
        let [c0, c1, c2, c3] = self.cols;
        c0 * vector.x + c1 * vector.y + c2 * vector.z + c3 * vector.w
    }

    /// Transform a point (`w = 1`: translation applies). Assumes an
    /// affine matrix (bottom row `0, 0, 0, 1`): the result's `w` is
    /// dropped without a perspective divide.
    #[must_use]
    pub fn transform_point(self, point: Vec3) -> Vec3 {
        self.transform(point.extend(1.0)).truncate()
    }

    /// Transform a direction (`w = 0`: translation does not apply).
    #[must_use]
    pub fn transform_vector(self, vector: Vec3) -> Vec3 {
        self.transform(vector.extend(0.0)).truncate()
    }
}

// Layout is API (see the type docs); hold it at compile time.
const _: () = {
    assert!(core::mem::size_of::<Mat4>() == 64 && core::mem::align_of::<Mat4>() == 16);
};

impl core::ops::Mul for Mat4 {
    type Output = Self;

    /// `a * b` applies `b` first, then `a`.
    fn mul(self, other: Self) -> Self {
        let [c0, c1, c2, c3] = other.cols;
        Self::from_cols(
            self.transform(c0),
            self.transform(c1),
            self.transform(c2),
            self.transform(c3),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn identity_transforms_are_bit_exact() {
        let v = Vec4::new(1.5, -2.5, 3.5, 1.0);
        let out = Mat4::IDENTITY.transform(v);
        assert_eq!(out.x.to_bits(), v.x.to_bits());
        assert_eq!(out.y.to_bits(), v.y.to_bits());
        assert_eq!(out.z.to_bits(), v.z.to_bits());
        assert_eq!(out.w.to_bits(), v.w.to_bits());
    }

    #[test]
    fn translation_moves_points_but_not_vectors() {
        let m = Mat4::from_translation(Vec3::new(10.0, 20.0, 30.0));
        assert_eq!(m.transform_point(Vec3::ZERO), Vec3::new(10.0, 20.0, 30.0));
        assert_eq!(m.transform_vector(Vec3::X), Vec3::X);
    }

    #[test]
    fn scale_scales() {
        let m = Mat4::from_scale(Vec3::new(2.0, 3.0, 4.0));
        assert_eq!(
            m.transform_point(Vec3::new(1.0, 1.0, 1.0)),
            Vec3::new(2.0, 3.0, 4.0)
        );
    }

    #[test]
    fn matrix_rotation_matches_quaternion_rotation() {
        let q = Quat::from_axis_angle(Vec3::new(0.6, 0.0, 0.8), 1.1);
        let m = Mat4::from_quat(q);
        let v = Vec3::new(1.0, -2.0, 0.5);
        assert!(close(m.transform_vector(v), q.rotate(v)));
    }

    #[test]
    fn multiplication_composes_right_to_left() {
        let scale = Mat4::from_scale(Vec3::splat(2.0));
        let translate = Mat4::from_translation(Vec3::X);
        // translate * scale: scale first, then translate.
        let composed = translate * scale;
        assert_eq!(
            composed.transform_point(Vec3::new(1.0, 0.0, 0.0)),
            Vec3::new(3.0, 0.0, 0.0)
        );
    }

    #[test]
    fn transpose_swaps_rows_and_columns() {
        let m = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let t = m.transpose();
        assert_eq!(t.cols[0].w.to_bits(), 1.0f32.to_bits());
        assert_eq!(t.cols[1].w.to_bits(), 2.0f32.to_bits());
        assert_eq!(t.cols[2].w.to_bits(), 3.0f32.to_bits());
        assert_eq!(t.transpose(), m);
    }
}
