//! Where the viewer stands, built over the engine's camera crate.
//!
//! The maths lives in `renew-camera` now — the look-at basis, the
//! reversed-depth perspective, the clip conventions and the tests that
//! pin them all moved to the engine when the camera stopped being this
//! sample's private invention. What stays here is what is genuinely
//! this game's: the field of view and the planes chosen for its arena,
//! the viewpoint constructors that read its world, and the one place
//! the two number systems meet.

// Re-exported so the sibling modules that consume a viewpoint keep one
// import path; the type is the engine's now.
pub use renew_camera::Camera;

use renew_camera::{Projection, View};
use renew_math::Vec3;
use renew_sample_cube_world::Cube;

/// The player's own viewpoint.
///
/// **The default, because a picture of what the simulation believes is
/// evidence about the simulation.** A free camera shows the world; this
/// shows what the player would see, so a wrong picture here is a wrong
/// world rather than a wrong viewpoint.
#[must_use]
pub fn player_view(world: &Cube, aspect: f32) -> Camera {
    Camera {
        view: player_eye_view(world),
        projection: projection(aspect),
    }
}

/// The view half of the player's viewpoint, aspect-free.
///
/// Split out because display-rate smoothing blends *views* between two
/// ticks — the projection carries the aspect, which is the window's
/// fact rather than either tick's, so a snapshot of where the player
/// stood must not capture it.
#[must_use]
pub fn player_eye_view(world: &Cube) -> View {
    let eye = to_world(world.eye());
    let look = to_world(world.look());
    View::look_at(eye, eye + look)
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
        view: View::look_at(
            Vec3::new(eye[0], eye[1], eye[2]),
            Vec3::new(target[0], target[1], target[2]),
        ),
        projection: projection(aspect),
    }
}

/// This game's projection: the one field of view and the planes chosen
/// for its arena, shared by both viewpoints so stills and play agree.
fn projection(aspect: f32) -> Projection {
    Projection::perspective(FOV, aspect, NEAR, FAR)
}

/// Vertical field of view: 70 degrees, the usual first-person figure —
/// wide enough to see a corridor's walls, narrow enough that a cube still
/// looks like a cube near the edge of the frame.
const FOV: f32 = 70.0 * core::f32::consts::PI / 180.0;

/// The near plane, in blocks. Well inside the player's own half-extent,
/// so a wall the player is pressed against still draws rather than
/// vanishing. Under reversed depth this plane maps to one.
const NEAR: f32 = 0.05;

/// The far plane, mapping to zero. The arena's diagonal is under 60
/// blocks, so this sees all of it from anywhere inside.
const FAR: f32 = 200.0;

/// A fixed-point vector as world-space floats.
///
/// **The one place the two number systems meet.** The world is fixed
/// point because a simulation must be bit-identical everywhere; a camera
/// is float because that is what a GPU takes. Converting here, at the
/// boundary, keeps the conversion out of both.
fn to_world(v: renew_fixed::Vec3) -> Vec3 {
    Vec3::new(scalar(v.x), scalar(v.y), scalar(v.z))
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
    /// where the player looks — the integration this sample owns, now
    /// that the matrix arithmetic is the engine crate's to prove.
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
        let bits = |v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
        assert_eq!(
            bits(camera.view.eye),
            bits(eye),
            "the camera stands somewhere else"
        );
        // And the target is one unit along the look direction, so the
        // two are never the same point and the basis never degenerates.
        let separation = camera.view.target - camera.view.eye;
        assert!(
            (separation.dot(separation) - 1.0).abs() < 1e-4,
            "the look direction should be a unit step: {separation:?}"
        );
        assert!(
            (camera.projection.aspect - 16.0 / 9.0).abs() < f32::EPSILON,
            "the aspect passed in is the aspect used"
        );
    }

    /// A free view is exactly the two points it was given, and its
    /// matrix is usable — the point of naming two points rather than
    /// accumulating a direction.
    #[test]
    fn a_free_camera_is_the_points_it_was_given() {
        let camera = free_view([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], 2.0);
        let bits = |v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
        assert_eq!(bits(camera.view.eye), bits(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(bits(camera.view.target), bits(Vec3::new(4.0, 5.0, 6.0)));
        for column in camera.columns() {
            for value in column {
                assert!(value.is_finite(), "a free camera produced {value}");
            }
        }
    }

    /// This game's planes land where the engine convention puts them —
    /// near on one, far on zero. The engine crate proves the mapping in
    /// general; this pins the sample's own constants to it, because a
    /// sample that drifted to another convention would still compile.
    #[test]
    fn the_games_planes_map_to_the_reversed_depth_range() {
        let camera = free_view([0.0, 0.0, 0.0], [0.0, 0.0, 10.0], 1.0);
        let matrix = camera.view_projection();
        let project = |z: f32| {
            let out = matrix.transform(renew_math::Vec4::new(0.0, 0.0, z, 1.0));
            out.z / out.w
        };
        assert!(
            (project(NEAR) - 1.0).abs() < 1e-4,
            "the near plane should be 1 under reversed depth"
        );
        assert!(
            project(FAR).abs() < 1e-4,
            "the far plane should be 0 under reversed depth"
        );
    }
}
