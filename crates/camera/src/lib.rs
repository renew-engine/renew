//! The presentation-side camera: where the eye stands, what it looks
//! at, and the matrix that follows from it.
//!
//! Pure functions over [`renew_math`]'s value types — no device, no
//! window, no clock — so every claim here is testable on any machine.
//! Floats are the medium on purpose: a camera lives on the far side of
//! the fixed-point boundary, and the structure checker's float fence is
//! what keeps this crate out of every simulation crate's closure. A
//! game's authoritative eye position and look direction stay wherever
//! that game keeps them; what crosses into this crate is floats, once
//! per draw, and nothing flows back.
//!
//! # Contract
//!
//! The engine's clip conventions, stated once and pinned by the tests
//! beside them — every one of these produces a *plausible* wrong
//! picture when violated, which is why they are the contract rather
//! than trivia:
//!
//! - **Clip `y` points down.** Viewports are built with positive
//!   height and nothing flips it, so world up is screen `-y`; the
//!   projection carries an explicit minus.
//! - **Clip `z` runs `[0, 1]` REVERSED: near is one, far is zero.**
//!   Depth clears to zero and the compare keeps the larger value —
//!   the engine's single depth convention. A perspective mapping is
//!   hyperbolic in distance either way; reversed, its dense end and
//!   the float format's dense end coincide, so the far field keeps
//!   distinct depth values instead of z-fighting.
//! - **The matrix goes to the GPU, and the divide happens there.**
//!   `gl_Position` carries a real `w`, so the hardware divides and the
//!   clipper removes geometry behind the eye. A caller transforming
//!   its own vertices would have to clip polygons against the near
//!   plane itself — inside a room, with walls behind the viewer, that
//!   is the ordinary case rather than a corner.
//! - **No roll.** World up is fixed; a roll control with no consumer
//!   would be a knob that exists to be wrong. It arrives as a field
//!   with a consumer, not before.

// Diagnostics go through sinks; the standard output macros are banned in
// this crate by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use renew_math::{Alpha, Mat4, Vec3, Vec4};

/// World up. Fixed rather than carried — see the crate contract's
/// no-roll clause.
const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);

/// Where the eye is and the point it looks at.
///
/// Two points rather than a point and accumulated angles: two points
/// always produce the same matrix, which is what keeps a picture a
/// pure function of the values that made it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Where the eye is, in world space.
    pub eye: Vec3,
    /// The point it looks at.
    pub target: Vec3,
}

impl View {
    /// A view from `eye` toward `target`.
    #[must_use]
    pub fn look_at(eye: Vec3, target: Vec3) -> Self {
        Self { eye, target }
    }

    /// The view matrix: the world moved into the eye's frame.
    ///
    /// The rotation is the eye's basis transposed, and the translation
    /// is minus the eye measured along each axis. A degenerate view —
    /// the eye at its own target, or looking straight along world up —
    /// answers a finite fallback basis rather than NaN: a picture from
    /// a wrong viewpoint is diagnosable, a screenful of discarded
    /// fragments is not.
    #[must_use]
    pub fn matrix(&self) -> Mat4 {
        let (right, up, forward) = self.axes();
        Mat4::from_cols(
            Vec4::new(right.x, up.x, forward.x, 0.0),
            Vec4::new(right.y, up.y, forward.y, 0.0),
            Vec4::new(right.z, up.z, forward.z, 0.0),
            Vec4::new(
                -right.dot(self.eye),
                -up.dot(self.eye),
                -forward.dot(self.eye),
                1.0,
            ),
        )
    }

    /// Presentation smoothing between two ticks' views: a lerp of eye
    /// and target by `alpha`.
    ///
    /// **At `Alpha::ZERO` the answer is bit-exactly `prev`**, special-
    /// cased rather than trusted to arithmetic: every lerp form turns a
    /// negative zero into a positive one, and this repository compares
    /// bits. Elsewhere the blend converges toward `next` and never
    /// reaches it, because `Alpha` is clamped below one. Never an input
    /// to simulation — the blended view exists only in the frame being
    /// drawn.
    #[must_use]
    pub fn blend(prev: Self, next: Self, alpha: Alpha) -> Self {
        let t = alpha.get();
        if t == 0.0 {
            return prev;
        }
        Self {
            eye: prev.eye.lerp(next.eye, t),
            target: prev.target.lerp(next.target, t),
        }
    }

    /// The eye's own axes: right, up, and the direction it looks.
    ///
    /// `forward` points away from the eye into the scene, so a larger
    /// distance along it is further away. With a right-handed basis,
    /// `right × up = forward`, so right is up crossed *into* forward —
    /// the other order mirrors the picture, which reads as an ordinary
    /// photograph taken from an odd angle.
    ///
    /// Public because a billboard needs exactly these two spanning
    /// vectors to face the eye, and recomputing them at a call site
    /// would be a second copy of the one basis this crate owns.
    #[must_use]
    pub fn axes(&self) -> (Vec3, Vec3, Vec3) {
        // Plain single-division normalization, guarded — NOT
        // `try_normalize`, whose two-step rescale is more robust and
        // bit-different. This crate replaced arithmetic that consumers'
        // committed pictures pin, and the extraction's whole claim is
        // that no pixel moved; the robustness the two-step form buys
        // guards magnitudes no camera reaches. The DEGENERATE outputs
        // deliberately differ from the code this replaced: straight-up
        // views get a right of (1, 0, 0) where the old fallback gave
        // (0, 0, 1), and NaN inputs take the fallback rather than
        // propagating — different finite pictures for views no consumer
        // produces, chosen for being explainable over being historical.
        let direction = self.target - self.eye;
        let forward = if direction.length_squared() > 0.0 {
            direction.normalize()
        } else {
            Vec3::new(0.0, 0.0, 1.0)
        };
        let horizontal = UP.cross(forward);
        let right = if horizontal.length_squared() > 0.0 {
            horizontal.normalize()
        } else {
            // Looking straight along world up leaves no horizontal
            // perpendicular; any fixed right keeps the basis finite.
            Vec3::new(1.0, 0.0, 0.0)
        };
        let up = forward.cross(right);
        (right, up, forward)
    }
}

/// A perspective projection under the engine conventions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    /// Vertical field of view, in radians.
    pub fov_y_radians: f32,
    /// Width over height of the picture.
    pub aspect: f32,
    /// Nearest visible distance. Everything closer is clipped — and
    /// under reversed depth, this plane maps to one.
    pub near: f32,
    /// Furthest visible distance, mapping to zero.
    pub far: f32,
}

impl Projection {
    /// A perspective of `fov_y_radians` at `aspect`, seeing from
    /// `near` to `far`.
    #[must_use]
    pub fn perspective(fov_y_radians: f32, aspect: f32, near: f32, far: f32) -> Self {
        Self {
            fov_y_radians,
            aspect,
            near,
            far,
        }
    }

    /// The projection matrix: y negated because screen y grows
    /// downward, z mapped REVERSED into `[0, 1]`.
    ///
    /// Derivation: `ndc(z) = (A z + B) / z` with `ndc(near) = 1` and
    /// `ndc(far) = 0` gives `A = -near / (far - near)` and
    /// `B = far * near / (far - near)`.
    #[must_use]
    pub fn matrix(&self) -> Mat4 {
        let focal = 1.0 / (self.fov_y_radians * 0.5).tan();
        let range = self.far - self.near;
        Mat4::from_cols(
            Vec4::new(focal / self.aspect, 0.0, 0.0, 0.0),
            Vec4::new(0.0, -focal, 0.0, 0.0),
            Vec4::new(0.0, 0.0, -self.near / range, 1.0),
            Vec4::new(0.0, 0.0, (self.far * self.near) / range, 0.0),
        )
    }
}

/// An orthographic projection under the same engine conventions — a
/// box of view space mapped to clip space with no perspective divide.
///
/// **A light's projection, first and foremost.** A sun does not
/// foreshorten: every ray arrives parallel, so the box that a shadow
/// map covers is exactly this shape. It follows the crate contract to
/// the letter — y negated, z REVERSED into `[0, 1]` — because a depth
/// image rendered under one convention and compared under another is
/// the plausible-wrong-picture failure the contract exists to prevent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orthographic {
    /// Half the box's width: view-space x in `[-half_width,
    /// half_width]` maps to clip `[-1, 1]`.
    pub half_width: f32,
    /// Half the box's height, mapping like the width (then negated
    /// with the rest of clip y).
    pub half_height: f32,
    /// Nearest visible distance, mapping to one under reversed depth.
    pub near: f32,
    /// Furthest visible distance, mapping to zero.
    pub far: f32,
}

impl Orthographic {
    /// A box `2 * half_width` across and `2 * half_height` tall,
    /// seeing from `near` to `far`.
    #[must_use]
    pub fn new(half_width: f32, half_height: f32, near: f32, far: f32) -> Self {
        Self {
            half_width,
            half_height,
            near,
            far,
        }
    }

    /// The projection matrix: y negated because screen y grows
    /// downward, z mapped REVERSED into `[0, 1]`, and `w` stays one —
    /// no divide, because parallel rays have no vanishing point.
    ///
    /// Derivation: `ndc(z) = A z + B` with `ndc(near) = 1` and
    /// `ndc(far) = 0` gives `A = -1 / (far - near)` and
    /// `B = far / (far - near)`.
    #[must_use]
    pub fn matrix(&self) -> Mat4 {
        let range = self.far - self.near;
        Mat4::from_cols(
            Vec4::new(1.0 / self.half_width, 0.0, 0.0, 0.0),
            Vec4::new(0.0, -1.0 / self.half_height, 0.0, 0.0),
            Vec4::new(0.0, 0.0, -1.0 / range, 0.0),
            Vec4::new(0.0, 0.0, self.far / range, 1.0),
        )
    }
}

/// A viewpoint and an orthographic projection: what a directional
/// light needs to render a shadow map, in [`Camera`]'s shape.
///
/// # Contract
///
/// **A light camera's view-projection is affine: its bottom row is
/// exactly `(0, 0, 0, 1)`, for any FINITE eye and target.** The view is
/// rigid and the projection is orthographic, so the product's fourth row
/// is the view's fourth row unchanged, and no arithmetic reaches it.
///
/// The finiteness qualifier is not a formality. `View::axes` guards its
/// basis against a non-finite input and falls back; the translation
/// column does not, and `0.0 * NaN` is NaN — so a light built from a
/// non-finite eye produces a bottom row that is not `(0, 0, 0, 1)`, and
/// a consumer asserting this contract will refuse it rather than draw.
/// That is the better failure of the two available, because the picture
/// such a light draws is meaningless either way, but it is a refusal and
/// callers should know it is there. Guarding the translation as the
/// basis is guarded would remove the asymmetry; it is a change to this
/// crate's behaviour rather than to its documentation, and is recorded
/// as debt rather than made in passing.
///
/// This is a promise, not an accident, because a consumer relies on it:
/// a renderer that must fit a camera matrix, a light matrix and a scene
/// light into the guaranteed 128-byte push range drops this row and
/// writes a literal one in its place. Without the promise stated here,
/// that renderer's refusal would be a trap sprung on a producer that
/// never agreed to anything; `a_light_cameras_view_projection_is_affine`
/// holds this side to it.
///
/// Giving [`Orthographic`] a perspective term, or changing the fourth
/// column of [`View::matrix`], breaks the promise and must be a
/// deliberate change to this paragraph as well as to the code.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightCamera {
    pub view: View,
    pub projection: Orthographic,
}

impl LightCamera {
    /// The view-projection matrix.
    #[must_use]
    pub fn view_projection(&self) -> Mat4 {
        self.projection.matrix() * self.view.matrix()
    }

    /// The four columns, in the column-major order a GPU-facing pack
    /// type takes — [`Camera::columns`]'s boundary shape.
    #[must_use]
    pub fn columns(&self) -> [[f32; 4]; 4] {
        let matrix = self.view_projection();
        matrix
            .cols
            .map(|column| [column.x, column.y, column.z, column.w])
    }
}

/// A viewpoint and a projection: everything a draw needs from a camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub view: View,
    pub projection: Projection,
}

impl Camera {
    /// The view-projection matrix.
    #[must_use]
    pub fn view_projection(&self) -> Mat4 {
        self.projection.matrix() * self.view.matrix()
    }

    /// The four columns, in the column-major order a GPU-facing pack
    /// type takes — the boundary shape, so a consumer never reaches
    /// into [`Mat4`]'s representation.
    #[must_use]
    pub fn columns(&self) -> [[f32; 4]; 4] {
        let matrix = self.view_projection();
        matrix
            .cols
            .map(|column| [column.x, column.y, column.z, column.w])
    }
}

/// Width over height; a zero or otherwise degenerate extent answers
/// 1.0, so no infinity ever reaches a projection.
#[must_use]
pub fn aspect_of(width: u32, height: u32) -> f32 {
    if width == 0 || height == 0 {
        return 1.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "window extents are far below f32's exact-integer range"
    )]
    let ratio = width as f32 / height as f32;
    ratio
}

#[cfg(test)]
mod tests {
    /// The orthographic contract, corner by corner: the box's corners
    /// land on clip corners with y negated, near maps to one and far
    /// to zero (REVERSED), and w stays one so nothing divides.
    #[test]
    fn the_orthographic_box_maps_to_clip_under_the_conventions() {
        let ortho = super::Orthographic::new(4.0, 2.0, 1.0, 9.0);
        let matrix = ortho.matrix();
        let corner = matrix.transform(super::Vec4::new(4.0, 2.0, 1.0, 1.0));
        assert!((corner.x - 1.0).abs() < 1e-6, "{}", corner.x);
        assert!((corner.y + 1.0).abs() < 1e-6, "y negated: {}", corner.y);
        assert!((corner.z - 1.0).abs() < 1e-6, "near is one: {}", corner.z);
        assert!((corner.w - 1.0).abs() < 1e-6, "no divide: {}", corner.w);
        let far_corner = matrix.transform(super::Vec4::new(-4.0, -2.0, 9.0, 1.0));
        assert!((far_corner.x + 1.0).abs() < 1e-6, "{}", far_corner.x);
        assert!((far_corner.y - 1.0).abs() < 1e-6, "{}", far_corner.y);
        assert!(far_corner.z.abs() < 1e-6, "far is zero: {}", far_corner.z);
        // Midway in z is midway in clip: the mapping is linear, unlike
        // the perspective's hyperbola.
        let mid = matrix.transform(super::Vec4::new(0.0, 0.0, 5.0, 1.0));
        assert!((mid.z - 0.5).abs() < 1e-6, "{}", mid.z);
    }

    /// **A light camera's view-projection is affine**, which is the
    /// promise the type's Contract makes and a renderer downstream
    /// depends on: it drops this row to fit three things into 128 bytes
    /// and writes a literal one where it belonged.
    ///
    /// Bit-exact rather than near, because the consumer's refusal is
    /// bit-exact — and because nothing here does arithmetic on the fourth
    /// row, so anything but exactness would mean the composition changed
    /// shape.
    ///
    /// **Both degenerate branches of `View::axes` are included.** A view
    /// whose eye sits on its own target, and one looking straight along
    /// world up, take fallback paths that build their basis differently;
    /// a fallback that returned a non-rigid basis would break affineness
    /// exactly where nobody was looking.
    ///
    /// Probed by giving `Orthographic::matrix` a perspective term in the
    /// fourth row: every case fails.
    #[test]
    fn a_light_cameras_view_projection_is_affine() {
        let projection = super::Orthographic::new(8.0, 8.0, 0.5, 20.0);
        let cases = [
            (
                "an ordinary light looking down",
                super::Vec3::new(0.0, 10.0, 0.0),
                super::Vec3::new(0.0, 0.0, 0.0),
            ),
            (
                "a light off to one side",
                super::Vec3::new(-3.0, 7.0, 4.0),
                super::Vec3::new(1.0, 0.0, -2.0),
            ),
            (
                // Degenerate: no direction to look in at all.
                "an eye at its own target",
                super::Vec3::new(2.0, 2.0, 2.0),
                super::Vec3::new(2.0, 2.0, 2.0),
            ),
            (
                // Degenerate: the look direction is world up, so the
                // usual right-vector cross product vanishes.
                "a light looking straight down world up",
                super::Vec3::new(0.0, 5.0, 0.0),
                super::Vec3::new(0.0, -5.0, 0.0),
            ),
        ];
        for (what, eye, target) in cases {
            let light = super::LightCamera {
                view: super::View::look_at(eye, target),
                projection,
            };
            let columns = light.columns();
            for (index, expected) in [0.0f32, 0.0, 0.0, 1.0].into_iter().enumerate() {
                assert_eq!(
                    columns[index][3].to_bits(),
                    expected.to_bits(),
                    "{what}: bottom row, column {index} — a light camera must be affine"
                );
            }
        }
    }

    /// The light camera composes exactly as the perspective camera
    /// does: projection times view, columns in the same order.
    #[test]
    fn the_light_camera_composes_like_the_camera() {
        let light = super::LightCamera {
            view: super::View::look_at(
                super::Vec3::new(0.0, 10.0, 0.0),
                super::Vec3::new(0.0, 0.0, 0.0),
            ),
            projection: super::Orthographic::new(8.0, 8.0, 0.5, 20.0),
        };
        let composed = light.projection.matrix() * light.view.matrix();
        let columns = light.columns();
        for (index, column) in composed.cols.iter().enumerate() {
            // Bit equality, deliberately: `columns` MOVES the values,
            // it never does arithmetic on them, so exactness is the
            // claim — the clear-value test's own reasoning.
            let moved = [column.x, column.y, column.z, column.w].map(f32::to_bits);
            assert_eq!(columns[index].map(f32::to_bits), moved, "column {index}");
        }
    }

    use super::*;
    use std::num::NonZeroU64;

    /// Apply a matrix to a point, dividing by `w` — what the hardware
    /// does, and what `Mat4::transform_point` deliberately does not.
    fn project(matrix: Mat4, point: Vec3) -> [f32; 3] {
        let out = matrix.transform(Vec4::new(point.x, point.y, point.z, 1.0));
        [out.x / out.w, out.y / out.w, out.z / out.w]
    }

    fn looking_north() -> Camera {
        Camera {
            view: View::look_at(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 10.0)),
            projection: Projection::perspective(core::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0),
        }
    }

    /// The point the camera looks at lands in the middle of the picture.
    #[test]
    fn the_target_is_the_centre_of_the_picture() {
        let clip = project(looking_north().view_projection(), Vec3::new(0.0, 0.0, 10.0));
        assert!(clip[0].abs() < 1e-5, "x: {clip:?}");
        assert!(clip[1].abs() < 1e-5, "y: {clip:?}");
    }

    /// **Up in the world is up on the screen.** Its own test with its
    /// own message: the failure is a world rendered upside down, which
    /// is a perfectly ordinary-looking picture.
    #[test]
    fn a_higher_point_is_higher_on_the_screen() {
        let matrix = looking_north().view_projection();
        let low = project(matrix, Vec3::new(0.0, -1.0, 10.0));
        let high = project(matrix, Vec3::new(0.0, 1.0, 10.0));
        assert!(
            high[1] < low[1],
            "screen y grows downward, so a higher point needs a smaller y; the world is upside \
             down: high {high:?} against low {low:?}"
        );
    }

    /// Right in the world is right on the screen — the mirror check the
    /// basis comment stakes its cross-product order on.
    #[test]
    fn a_point_to_the_right_is_to_the_right() {
        let matrix = looking_north().view_projection();
        let left = project(matrix, Vec3::new(-1.0, 0.0, 10.0));
        let right = project(matrix, Vec3::new(1.0, 0.0, 10.0));
        assert!(
            right[0] > left[0],
            "the picture is mirrored: {left:?} {right:?}"
        );
    }

    /// The near and far planes land on one and nought — the reversed
    /// mapping, which is the crate contract and not a convention this
    /// test happens to share.
    #[test]
    fn the_planes_map_to_the_reversed_depth_range() {
        let camera = looking_north();
        let matrix = camera.view_projection();
        let near = project(matrix, Vec3::new(0.0, 0.0, camera.projection.near));
        let far = project(matrix, Vec3::new(0.0, 0.0, camera.projection.far));
        assert!(
            (near[2] - 1.0).abs() < 1e-4,
            "the near plane should be 1 under reversed depth: {near:?}"
        );
        assert!(
            far[2].abs() < 1e-4,
            "the far plane should be 0 under reversed depth: {far:?}"
        );
    }

    /// A degenerate camera answers with a picture rather than with NaN,
    /// in both degenerate directions.
    #[test]
    fn degenerate_views_still_produce_finite_matrices() {
        let eye_at_target = View::look_at(Vec3::new(3.0, 4.0, 5.0), Vec3::new(3.0, 4.0, 5.0));
        let straight_up = View::look_at(Vec3::ZERO, Vec3::new(0.0, 7.0, 0.0));
        for (name, view) in [
            ("eye at target", eye_at_target),
            ("straight up", straight_up),
        ] {
            let camera = Camera {
                view,
                projection: looking_north().projection,
            };
            for column in camera.columns() {
                for value in column {
                    assert!(value.is_finite(), "{name} produced {value}");
                }
            }
        }
    }

    /// The columns are the matrix, in order — the boundary a pack type
    /// consumes, pinned so a transposition cannot hide in the accessor.
    #[test]
    fn columns_reproduce_the_matrix_in_column_order() {
        let camera = looking_north();
        let matrix = camera.view_projection();
        let columns = camera.columns();
        for (index, column) in matrix.cols.iter().enumerate() {
            // Bits, not floats: the claim is that these are the same
            // values, and the house style makes exactness spellable.
            assert_eq!(
                columns[index].map(f32::to_bits),
                [column.x, column.y, column.z, column.w].map(f32::to_bits),
                "column {index} moved"
            );
        }
    }

    /// Blending at zero is bit-exactly the previous view — including a
    /// negative zero, which every lerp form would flip positive and
    /// which this repository's bit comparisons would catch downstream.
    #[test]
    fn blend_at_zero_is_bit_exactly_the_previous_view() {
        let prev = View::look_at(Vec3::new(-0.0, 1.0, 2.0), Vec3::new(3.0, -0.0, 5.0));
        let next = View::look_at(Vec3::new(9.0, 9.0, 9.0), Vec3::new(8.0, 8.0, 8.0));
        let blended = View::blend(prev, next, Alpha::ZERO);
        let bits = |v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
        assert_eq!(bits(blended.eye), bits(prev.eye), "eye moved at alpha 0");
        assert_eq!(
            bits(blended.target),
            bits(prev.target),
            "target moved at alpha 0"
        );
    }

    /// Away from zero the blend converges toward the next view and
    /// never reaches it: `Alpha` is clamped below one.
    #[test]
    fn blend_converges_toward_the_next_view() {
        let prev = View::look_at(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0));
        let next = View::look_at(Vec3::new(10.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 1.0));
        let step = NonZeroU64::new(1_000).expect("nonzero");
        let half = View::blend(prev, next, Alpha::new(500, step));
        assert!(
            (half.eye.x - 5.0).abs() < 1e-4,
            "halfway should be halfway: {:?}",
            half.eye
        );
        let nearly = View::blend(prev, next, Alpha::new(999_999, step));
        assert!(
            nearly.eye.x < 10.0,
            "alpha is clamped below one, so the blend never reaches next: {:?}",
            nearly.eye
        );
        assert!(
            nearly.eye.x > 9.9,
            "but it should get close: {:?}",
            nearly.eye
        );
    }

    /// The degenerate-extent fallback, both axes, and the ordinary
    /// case — no infinity may ever reach a projection.
    #[test]
    fn aspect_of_answers_one_for_degenerate_extents() {
        assert!((aspect_of(0, 720) - 1.0).abs() < f32::EPSILON);
        assert!((aspect_of(1280, 0) - 1.0).abs() < f32::EPSILON);
        assert!((aspect_of(0, 0) - 1.0).abs() < f32::EPSILON);
        assert!((aspect_of(1280, 720) - 1280.0 / 720.0).abs() < f32::EPSILON);
    }

    proptest::proptest! {
        /// Anything in front of the eye is nearer than anything behind
        /// it, over arbitrary points — and under reversed depth, nearer
        /// means larger. A projection monotone at two typed points can
        /// still fold in between.
        #[test]
        fn depth_shrinks_with_distance_from_the_eye(
            x in -20.0f32..20.0,
            y in -20.0f32..20.0,
            near_z in 1.0f32..40.0,
            step in 0.5f32..40.0,
        ) {
            let matrix = looking_north().view_projection();
            let here = project(matrix, Vec3::new(x, y, near_z));
            let there = project(matrix, Vec3::new(x, y, near_z + step));
            proptest::prop_assert!(
                there[2] < here[2],
                "further should be smaller under reversed depth: {:?} then {:?}", here, there
            );
        }

        /// Everything inside the frustum lands inside the unit depth
        /// range the viewport maps.
        #[test]
        fn points_in_front_land_in_the_unit_depth_range(
            x in -5.0f32..5.0,
            y in -5.0f32..5.0,
            z in 0.2f32..90.0,
        ) {
            let clip = project(looking_north().view_projection(), Vec3::new(x, y, z));
            proptest::prop_assert!(
                (0.0..=1.0).contains(&clip[2]),
                "depth outside the range the viewport maps: {:?}", clip
            );
        }

        /// The blend is a pure function of its inputs: the same call
        /// twice is the same bits.
        #[test]
        fn blending_is_reproducible(
            ex in -30.0f32..30.0, ey in -30.0f32..30.0, ez in -30.0f32..30.0,
            remainder in 0u64..1_000,
        ) {
            let prev = View::look_at(Vec3::new(ex, ey, ez), Vec3::new(ey, ez, ex));
            let next = View::look_at(Vec3::new(ez, ex, ey), Vec3::new(ex, ez, ey));
            let step = NonZeroU64::new(1_000).expect("nonzero");
            let once = View::blend(prev, next, Alpha::new(remainder, step));
            let twice = View::blend(prev, next, Alpha::new(remainder, step));
            let bits = |v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
            proptest::prop_assert_eq!(bits(once.eye), bits(twice.eye));
            proptest::prop_assert_eq!(bits(once.target), bits(twice.target));
        }
    }
}
