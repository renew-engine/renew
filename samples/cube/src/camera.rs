//! Where the viewer stands, and the matrix that follows from it.
//!
//! Pure, like [`crate::mesh`] and [`crate::projection`]: the arithmetic
//! that decides what is on screen is testable without a device.
//!
//! # The conventions, and what each costs if it is wrong
//!
//! Read out of this repository's own rendering code rather than recalled,
//! because every one of them produces a *plausible* wrong picture.
//!
//! **Clip `y` points down.** The viewport is built with a positive height
//! and nothing flips it, so world up is screen `-y`. The projection
//! carries an explicit minus; without it the world renders upside down,
//! which looks like an ordinary picture taken from an odd angle.
//!
//! **Clip `z` runs `[0, 1]` REVERSED: near is one, far is zero.** Depth
//! clears to zero and the compare is `GREATER_OR_EQUAL`, so the larger
//! value survives — the engine's single depth convention, chosen because
//! float precision concentrates toward zero and a conventional mapping
//! spends nearly all of its distinct values within a few blocks of the
//! near plane. This is not OpenGL's `[-1, 1]`, and not the conventional
//! Vulkan mapping either; a projection built for either draws the far
//! wall over everything in front of it.
//!
//! **The matrix goes to the GPU, and the divide happens there.** That is
//! the whole reason the shader takes a matrix: a triangle crossing
//! `w = 0` cannot be divided, so a caller that transformed its own
//! vertices would have to clip polygons against the near plane. Inside a
//! room, with walls behind the viewer, that is not a corner case.

use renew_sample_cube_world::Cube;

/// A viewpoint: where the eye is, what it looks at, and how wide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Where the eye is, in world space.
    pub eye: [f32; 3],
    /// The point it looks at.
    pub target: [f32; 3],
    /// Vertical field of view, in radians.
    pub fov: f32,
    /// Width over height of the picture.
    pub aspect: f32,
    /// Nearest visible distance. Everything closer is clipped.
    pub near: f32,
    /// Furthest visible distance.
    pub far: f32,
}

/// World up. The camera is never asked to roll, so this is fixed rather
/// than carried: a roll control with no consumer would be a knob that
/// exists to be wrong.
const UP: [f32; 3] = [0.0, 1.0, 0.0];

impl Camera {
    /// The view-projection matrix, as four columns.
    ///
    /// Column-major, matching what the push-constant block and the
    /// shader both expect.
    #[must_use]
    pub fn view_projection(&self) -> [[f32; 4]; 4] {
        let (right, up, forward) = self.basis();

        // View: the world moved into the eye's frame. The rotation is
        // the basis transposed, and the translation is minus the eye
        // measured along each axis.
        let view = [
            [right[0], up[0], forward[0], 0.0],
            [right[1], up[1], forward[1], 0.0],
            [right[2], up[2], forward[2], 0.0],
            [
                -dot(right, self.eye),
                -dot(up, self.eye),
                -dot(forward, self.eye),
                1.0,
            ],
        ];

        // Perspective, for the engine's clip space: y negated because
        // screen y grows downward, and z mapped REVERSED into [0, 1] —
        // near lands on one, far on zero, so the depth values with the
        // most float precision describe the distances that need it.
        // Derivation: ndc(z) = (A z + B) / z with ndc(near) = 1 and
        // ndc(far) = 0 gives A = -near / (far - near) and
        // B = far * near / (far - near).
        let focal = 1.0 / (self.fov * 0.5).tan();
        let range = self.far - self.near;
        let projection = [
            [focal / self.aspect, 0.0, 0.0, 0.0],
            [0.0, -focal, 0.0, 0.0],
            [0.0, 0.0, -self.near / range, 1.0],
            [0.0, 0.0, (self.far * self.near) / range, 0.0],
        ];

        multiply(projection, view)
    }

    /// The eye's own axes: right, up, and the direction it looks.
    ///
    /// `forward` points away from the eye into the scene, so a larger
    /// distance along it is further away — which is what makes the
    /// depth mapping monotone in the direction the compare expects.
    fn basis(&self) -> ([f32; 3], [f32; 3], [f32; 3]) {
        let forward = normalise([
            self.target[0] - self.eye[0],
            self.target[1] - self.eye[1],
            self.target[2] - self.eye[2],
        ]);
        // right = up x forward, and up = forward x right. The other
        // order mirrors the picture: with a right-handed basis,
        // right x up = forward, so right is up crossed INTO forward.
        let right = normalise(cross(UP, forward));
        let up = cross(forward, right);
        (right, up, forward)
    }
}

/// Column-major product: `left * right`.
fn multiply(left: [[f32; 4]; 4], right: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for (column, source) in out.iter_mut().zip(right) {
        for (row, cell) in column.iter_mut().enumerate() {
            *cell = left[0][row] * source[0]
                + left[1][row] * source[1]
                + left[2][row] * source[2]
                + left[3][row] * source[3];
        }
    }
    out
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalise(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    if length == 0.0 {
        // A zero direction has no normalisation. Answering with forward
        // rather than with NaN keeps a degenerate camera producing a
        // picture instead of an empty screen full of discarded
        // fragments, which is far harder to diagnose.
        return [0.0, 0.0, 1.0];
    }
    [v[0] / length, v[1] / length, v[2] / length]
}

/// The player's own viewpoint.
///
/// **The default, because a picture of what the simulation believes is
/// evidence about the simulation.** A free camera shows the world; this
/// shows what the player would see, so a wrong picture here is a wrong
/// world rather than a wrong viewpoint.
#[must_use]
pub fn player_view(world: &Cube, aspect: f32) -> Camera {
    let eye = to_world(world.eye());
    let look = to_world(world.look());
    Camera {
        eye,
        target: [eye[0] + look[0], eye[1] + look[1], eye[2] + look[2]],
        fov: FOV,
        aspect,
        near: NEAR,
        far: FAR,
    }
}

/// A viewpoint given outright, for looking at the world from outside it.
///
/// **Explicit rather than accumulated**, which the design calls for: a
/// free camera driven by mouse deltas would make a picture a function of
/// how somebody moved their hand, and this repository compares pictures.
/// Two points on a command line always produce the same frame.
#[must_use]
pub fn free_view(eye: [f32; 3], target: [f32; 3], aspect: f32) -> Camera {
    Camera {
        eye,
        target,
        fov: FOV,
        aspect,
        near: NEAR,
        far: FAR,
    }
}

/// Vertical field of view: 70 degrees, the usual first-person figure —
/// wide enough to see a corridor's walls, narrow enough that a cube still
/// looks like a cube near the edge of the frame.
const FOV: f32 = 70.0 * core::f32::consts::PI / 180.0;

/// The near plane, in blocks. Well inside the player's own half-extent,
/// so a wall the player is pressed against still draws rather than
/// vanishing.
const NEAR: f32 = 0.05;

/// The far plane. The arena's diagonal is under 60 blocks, so this sees
/// all of it from anywhere inside.
const FAR: f32 = 200.0;

/// A fixed-point vector as world-space floats.
///
/// **The one place the two number systems meet.** The world is fixed
/// point because a simulation must be bit-identical everywhere; a camera
/// is float because that is what a GPU takes. Converting here, at the
/// boundary, keeps the conversion out of both.
fn to_world(v: renew_fixed::Vec3) -> [f32; 3] {
    [scalar(v.x), scalar(v.y), scalar(v.z)]
}

/// One fixed-point scalar as a float.
#[expect(
    clippy::cast_precision_loss,
    reason = "a coordinate large enough to lose precision is far outside any world this builds"
)]
fn scalar(value: renew_fixed::Fixed) -> f32 {
    value.to_bits() as f32 / renew_fixed::Fixed::ONE.to_bits() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The player's camera stands where the player stands and looks
    /// where the player looks.
    #[test]
    fn the_player_camera_is_the_players_own_viewpoint() {
        let world = crate::run_world(&crate::Options {
            script: crate::Script::Stand,
            ticks: 30,
            ..crate::Options::default()
        });
        let camera = player_view(&world, 16.0 / 9.0);

        // The eye is the world's eye, converted.
        let eye = to_world(world.eye());
        assert_eq!(
            camera.eye.map(f32::to_bits),
            eye.map(f32::to_bits),
            "the camera stands somewhere else"
        );
        // And the target is one unit along the look direction, so the
        // two are never the same point and the basis never degenerates.
        let separation = [
            camera.target[0] - camera.eye[0],
            camera.target[1] - camera.eye[1],
            camera.target[2] - camera.eye[2],
        ];
        assert!(
            (dot(separation, separation) - 1.0).abs() < 1e-4,
            "the look direction should be a unit step: {separation:?}"
        );
        assert!(
            (camera.aspect - 16.0 / 9.0).abs() < f32::EPSILON,
            "the aspect passed in is the aspect used"
        );
    }

    /// A free view is exactly the two points it was given.
    #[test]
    fn a_free_camera_is_the_points_it_was_given() {
        let camera = free_view([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], 2.0);
        assert_eq!(
            camera.eye.map(f32::to_bits),
            [1.0f32, 2.0, 3.0].map(f32::to_bits)
        );
        assert_eq!(
            camera.target.map(f32::to_bits),
            [4.0f32, 5.0, 6.0].map(f32::to_bits)
        );
        // And it produces a usable matrix, which is the point of naming
        // two points rather than accumulating a direction.
        for column in camera.view_projection() {
            for value in column {
                assert!(value.is_finite(), "a free camera produced {value}");
            }
        }
    }

    /// Apply a column-major matrix to a point, dividing by `w` — which
    /// is what the hardware does and what `Mat4::transform_point`
    /// deliberately does **not**.
    fn project(matrix: [[f32; 4]; 4], point: [f32; 3]) -> [f32; 3] {
        let mut out = [0.0f32; 4];
        for (row, cell) in out.iter_mut().enumerate() {
            *cell = matrix[0][row] * point[0]
                + matrix[1][row] * point[1]
                + matrix[2][row] * point[2]
                + matrix[3][row];
        }
        [out[0] / out[3], out[1] / out[3], out[2] / out[3]]
    }

    fn looking_north() -> Camera {
        Camera {
            eye: [0.0, 0.0, 0.0],
            target: [0.0, 0.0, 10.0],
            fov: core::f32::consts::FRAC_PI_2,
            aspect: 1.0,
            near: 0.1,
            far: 100.0,
        }
    }

    /// The point the camera looks at lands in the middle of the picture.
    #[test]
    fn the_target_is_the_centre_of_the_picture() {
        let clip = project(looking_north().view_projection(), [0.0, 0.0, 10.0]);
        assert!(clip[0].abs() < 1e-5, "x: {clip:?}");
        assert!(clip[1].abs() < 1e-5, "y: {clip:?}");
    }

    /// **Up in the world is up on the screen.**
    ///
    /// Its own test with its own message: the failure is a world
    /// rendered upside down, which is a perfectly ordinary-looking
    /// picture.
    #[test]
    fn a_higher_point_is_higher_on_the_screen() {
        let matrix = looking_north().view_projection();
        let low = project(matrix, [0.0, -1.0, 10.0]);
        let high = project(matrix, [0.0, 1.0, 10.0]);
        assert!(
            high[1] < low[1],
            "screen y grows downward, so a higher point needs a smaller y; the world is upside \
             down: high {high:?} against low {low:?}"
        );
    }

    /// Right in the world is right on the screen.
    #[test]
    fn a_point_to_the_right_is_to_the_right() {
        let matrix = looking_north().view_projection();
        let left = project(matrix, [-1.0, 0.0, 10.0]);
        let right = project(matrix, [1.0, 0.0, 10.0]);
        assert!(
            right[0] > left[0],
            "the picture is mirrored: {left:?} {right:?}"
        );
    }

    /// The near and far planes land on one and nought — the reversed
    /// mapping, deliberately, and not OpenGL's range either.
    #[test]
    fn the_planes_map_to_the_reversed_depth_range() {
        let camera = looking_north();
        let matrix = camera.view_projection();
        let near = project(matrix, [0.0, 0.0, camera.near]);
        let far = project(matrix, [0.0, 0.0, camera.far]);
        assert!(
            (near[2] - 1.0).abs() < 1e-4,
            "the near plane should be 1 under reversed depth: {near:?}"
        );
        assert!(
            far[2].abs() < 1e-4,
            "the far plane should be 0 under reversed depth: {far:?}"
        );
    }

    /// A degenerate camera answers with a picture rather than with NaN.
    #[test]
    fn an_eye_at_its_own_target_still_produces_a_matrix() {
        let camera = Camera {
            eye: [3.0, 4.0, 5.0],
            target: [3.0, 4.0, 5.0],
            ..looking_north()
        };
        for column in camera.view_projection() {
            for value in column {
                assert!(value.is_finite(), "a degenerate camera produced {value}");
            }
        }
    }

    /// **Why the depth mapping is reversed, as a measurement.** Two
    /// walls a block apart at the far end of the arena must land on
    /// distinct f32 depths, or the depth test cannot order them and
    /// which one shows becomes a rounding accident. Under the
    /// conventional mapping (near to 0, far to 1) this camera's
    /// near/far ratio of four thousand crushes the far half of the
    /// range into a handful of representable values — measured here so
    /// the motivation is a number, not a memory: at 190 and 191 blocks
    /// the conventional mapping's gap is several times fewer f32 steps
    /// than the reversed one's.
    #[test]
    fn reversed_depth_separates_the_far_field_better_than_conventional() {
        let camera = Camera {
            eye: [0.0, 0.0, 0.0],
            target: [0.0, 0.0, 10.0],
            fov: core::f32::consts::FRAC_PI_2,
            aspect: 1.0,
            near: NEAR,
            far: FAR,
        };
        // Both mappings, computed the way the projection computes them.
        let range = camera.far - camera.near;
        let conventional =
            |z: f32| (z * (camera.far / range) - (camera.far * camera.near) / range) / z;
        let reversed =
            |z: f32| (z * (-camera.near / range) + (camera.far * camera.near) / range) / z;
        // Distinct representable values between the two depths: count
        // f32 bit-steps between the mapped endpoints.
        let steps = |mapping: &dyn Fn(f32) -> f32, a: f32, b: f32| {
            let (lo, hi) = (mapping(a).min(mapping(b)), mapping(a).max(mapping(b)));
            hi.to_bits() - lo.to_bits()
        };
        let far_pair = (190.0f32, 191.0f32);
        let conventional_steps = steps(&conventional, far_pair.0, far_pair.1);
        let reversed_steps = steps(&reversed, far_pair.0, far_pair.1);
        assert!(
            reversed_steps > 4 * conventional_steps,
            "reversed depth must separate the far field far better: {reversed_steps} \
             f32 steps against {conventional_steps} between {} and {} blocks",
            far_pair.0,
            far_pair.1
        );
        assert!(
            reversed_steps > 0 && conventional_steps > 0,
            "both mappings must at least distinguish the pair at all: \
             reversed {reversed_steps}, conventional {conventional_steps}"
        );
    }

    proptest::proptest! {
        /// Anything in front of the eye is nearer than anything behind
        /// it, over arbitrary points — and under reversed depth, nearer
        /// means larger.
        ///
        /// The property the depth test rests on. A projection monotone
        /// at the two points somebody typed can still fold in between.
        #[test]
        fn depth_shrinks_with_distance_from_the_eye(
            x in -20.0f32..20.0,
            y in -20.0f32..20.0,
            near_z in 1.0f32..40.0,
            step in 0.5f32..40.0,
        ) {
            let matrix = looking_north().view_projection();
            let here = project(matrix, [x, y, near_z]);
            let there = project(matrix, [x, y, near_z + step]);
            proptest::prop_assert!(
                there[2] < here[2],
                "further should be smaller under reversed depth: {:?} then {:?}", here, there
            );
        }

        /// Everything inside the frustum lands inside clip space.
        #[test]
        fn points_in_front_land_in_the_unit_depth_range(
            x in -5.0f32..5.0,
            y in -5.0f32..5.0,
            z in 0.2f32..90.0,
        ) {
            let clip = project(looking_north().view_projection(), [x, y, z]);
            proptest::prop_assert!(
                (0.0..=1.0).contains(&clip[2]),
                "depth outside the range the viewport maps: {:?}", clip
            );
        }
    }
}
