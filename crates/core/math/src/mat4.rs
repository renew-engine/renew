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

    /// The inverse of an **affine** matrix, or `None` when there is not
    /// one.
    ///
    /// # Affine, and named that way on purpose
    ///
    /// A general 4×4 inverse would be a larger and less accurate
    /// computation whose only subjects are projective matrices, and
    /// nothing in this tree inverts one. Calling this `inverse` would
    /// promise that; calling it `affine_inverse` says what it does, and
    /// the bottom-row check below turns the assumption into a refusal
    /// rather than a wrong answer.
    ///
    /// # What `None` means
    ///
    /// Either the bottom row is not `0, 0, 0, 1` — the matrix is not
    /// affine and this is the wrong function — or the basis is
    /// degenerate: a scale of zero on some axis flattens space onto a
    /// plane, and nothing maps that back. **A determinant that is merely
    /// small is not refused**, because "small" has no threshold that is
    /// right for every scene: a model in millimetres and one in metres
    /// differ by a factor of a thousand in each axis and a billion in
    /// the determinant, and a bound that rejected the first would be a
    /// bound about units rather than about invertibility.
    #[must_use]
    pub fn affine_inverse(self) -> Option<Self> {
        let [c0, c1, c2, c3] = self.cols;
        // **Exact, and a tolerance here would be worse than the lint it
        // silences.** Every constructor of this type writes the bottom
        // row as literal zeros and a one, and a product of two such
        // matrices computes it as sums of exact zeros and one exact
        // product of ones -- so an affine matrix has exactly this row,
        // arrived at by construction or by arithmetic. A margin would
        // admit a matrix that is slightly projective and then return an
        // inverse that is wrong rather than absent, which is the failure
        // this check exists to prevent. (`-0.0 != 0.0` is false in IEEE,
        // so a negative zero passes, as it should.)
        #[expect(
            clippy::float_cmp,
            reason = "an affine bottom row is exact by construction; see the comment above"
        )]
        let projective = c0.w != 0.0 || c1.w != 0.0 || c2.w != 0.0 || c3.w != 1.0;
        if projective {
            return None;
        }

        // The upper-left basis, by columns.
        let a = Vec3::new(c0.x, c0.y, c0.z);
        let b = Vec3::new(c1.x, c1.y, c1.z);
        let c = Vec3::new(c2.x, c2.y, c2.z);

        // Cofactors of the basis, which are its adjugate's rows and also
        // the cross products of its column pairs.
        let r0 = b.cross(c);
        let r1 = c.cross(a);
        let r2 = a.cross(b);

        let determinant = a.dot(r0);
        if determinant == 0.0 || !determinant.is_finite() {
            return None;
        }
        let scale = 1.0 / determinant;
        let r0 = r0 * scale;
        let r1 = r1 * scale;
        let r2 = r2 * scale;

        // The adjugate's rows become the inverse's columns, which is the
        // transpose the inverse of a column-major basis needs.
        let translation = Vec3::new(c3.x, c3.y, c3.z);
        let back = Vec3::new(
            -r0.dot(translation),
            -r1.dot(translation),
            -r2.dot(translation),
        );

        Some(Self::from_cols(
            Vec4::new(r0.x, r1.x, r2.x, 0.0),
            Vec4::new(r0.y, r1.y, r2.y, 0.0),
            Vec4::new(r0.z, r1.z, r2.z, 0.0),
            Vec4::new(back.x, back.y, back.z, 1.0),
        ))
    }

    /// The matrix that transforms a **normal** the way this one
    /// transforms a point.
    ///
    /// **A normal is not a direction and does not transform like one.**
    /// Under a non-uniform scale a surface tilts one way and its normal
    /// tilts the other: flatten a sphere into a disc and its equator's
    /// normals should splay outward, while `transform_vector` would
    /// squash them flat with the surface. The inverse transpose is what
    /// separates the two, and it is the identity exactly when the basis
    /// is a rotation — which is why a reader that skipped it looks
    /// correct on every model whose transforms are rigid.
    ///
    /// Returns `None` for the same reasons [`Mat4::affine_inverse`]
    /// does. The result is **not** renormalised: a scale changes a
    /// normal's length, and whoever needs a unit vector knows better
    /// than this function whether a zero-length result is a refusal or a
    /// degenerate face.
    #[must_use]
    pub fn normal_matrix(self) -> Option<Self> {
        Some(self.affine_inverse()?.transpose())
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

    /// An inverse undoes what its matrix did, for every affine shape.
    #[test]
    fn an_affine_inverse_undoes_its_matrix() {
        let cases = [
            Mat4::IDENTITY,
            Mat4::from_translation(Vec3::new(10.0, -20.0, 30.0)),
            Mat4::from_scale(Vec3::new(2.0, 3.0, 4.0)),
            Mat4::from_quat(Quat::from_axis_angle(Vec3::new(0.6, 0.0, 0.8), 1.1)),
            // The composition a node transform actually is: scale, then
            // rotate, then translate.
            Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0))
                * Mat4::from_quat(Quat::from_axis_angle(Vec3::Y, 0.7))
                * Mat4::from_scale(Vec3::new(2.0, 0.5, 4.0)),
        ];
        let point = Vec3::new(1.5, -2.5, 3.5);
        for m in cases {
            let inverse = m.affine_inverse().expect("these are all invertible");
            assert!(
                close(inverse.transform_point(m.transform_point(point)), point),
                "an inverse must return the point its matrix moved"
            );
            assert!(
                close(m.transform_point(inverse.transform_point(point)), point),
                "and it must work in the other order too"
            );
        }
    }

    /// A rotation's inverse is its transpose, which is the case an
    /// implementation gets right by accident and the others by design.
    #[test]
    fn a_rotations_inverse_is_its_transpose() {
        let m = Mat4::from_quat(Quat::from_axis_angle(Vec3::new(0.6, 0.0, 0.8), 1.1));
        let inverse = m.affine_inverse().expect("a rotation is invertible");
        let transpose = m.transpose();
        for column in 0..4 {
            let a = inverse.cols[column];
            let b = transpose.cols[column];
            assert!(
                close(Vec3::new(a.x, a.y, a.z), Vec3::new(b.x, b.y, b.z)),
                "column {column} of a rotation's inverse is its transpose"
            );
        }
    }

    /// **A basis that flattens space has no inverse, and neither does a
    /// matrix that is not affine.**
    #[test]
    fn what_cannot_be_inverted_says_so() {
        // A scale of zero on one axis maps everything onto a plane, and
        // nothing maps a plane back into space.
        assert_eq!(
            Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0)).affine_inverse(),
            None
        );
        assert_eq!(Mat4::from_scale(Vec3::ZERO).affine_inverse(), None);

        // Two columns the same is a basis of two dimensions wearing
        // three, which the determinant catches without a special case.
        let flat = Mat4::from_cols(
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        );
        assert_eq!(flat.affine_inverse(), None);

        // Not affine: this function is the wrong one for it, and says so
        // rather than answering with a matrix that is wrong in a way
        // nothing downstream would notice.
        let projective = Mat4::from_cols(
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, -1.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
        );
        assert_eq!(projective.affine_inverse(), None);
    }

    /// **A normal is not a direction, and a non-uniform scale is where
    /// the difference shows.**
    ///
    /// Flatten space along one axis and a surface tilts one way while
    /// its normal tilts the other. This pins it concretely: the
    /// transformed normal stays perpendicular to the transformed
    /// tangent, and transforming the normal as a direction does not.
    #[test]
    fn a_normal_matrix_keeps_normals_perpendicular_to_their_surface() {
        let squash = Mat4::from_scale(Vec3::new(2.0, 1.0, 1.0));
        let tangent = Vec3::new(-1.0, 1.0, 0.0);
        let normal = Vec3::new(1.0, 1.0, 0.0);
        assert!(
            tangent.dot(normal).abs() < 1e-6,
            "the fixture starts perpendicular"
        );

        let moved_tangent = squash.transform_vector(tangent);
        let correct = squash
            .normal_matrix()
            .expect("a scale is invertible")
            .transform_vector(normal);
        // **Computed into a name rather than called inside the message.**
        // A method call in a format argument runs only when the assert
        // fails, which makes it a line nothing executes on a green run;
        // the rest of this file captures variables inline for the same
        // reason.
        let kept = moved_tangent.dot(correct);
        assert!(
            kept.abs() < 1e-5,
            "the normal matrix keeps them perpendicular: {kept}"
        );

        // And the thing a reader does when it forgets: transforming the
        // normal as though it were a direction.
        let wrong = squash.transform_vector(normal);
        let tilted = moved_tangent.dot(wrong);
        assert!(
            tilted.abs() > 1.0,
            "transforming a normal as a direction tilts it the wrong way: {tilted}"
        );
    }

    /// Under a rotation the two agree, which is why skipping the normal
    /// matrix looks correct on every rigidly-placed model.
    #[test]
    fn a_rotation_needs_no_normal_matrix_and_that_is_the_trap() {
        let m = Mat4::from_quat(Quat::from_axis_angle(Vec3::new(0.0, 1.0, 0.0), 0.9));
        let normal = Vec3::new(0.6, 0.8, 0.0);
        let via_normal_matrix = m
            .normal_matrix()
            .expect("a rotation is invertible")
            .transform_vector(normal);
        assert!(
            close(via_normal_matrix, m.transform_vector(normal)),
            "identical under a rotation, which is what hides the bug"
        );
    }

    /// What cannot be inverted has no normal matrix either.
    #[test]
    fn a_degenerate_basis_has_no_normal_matrix() {
        assert_eq!(
            Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0)).normal_matrix(),
            None
        );
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
