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

use renew_camera::{LightCamera, Orthographic, Projection, View};
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

/// The arena's sun: a fixed directional light, tilted so every cast
/// shadow leans the same visible way.
///
/// **A sun over a closed box works because the roof does not cast.**
/// The world's shell has a ceiling, and every ray from outside would
/// stop there — one uniform dimming, which is no picture at all. What
/// makes the interior lit is [`crate::render::casting_scene`], which
/// leaves the top layer out of the shadow map: the sun shines in as
/// though the roof were glass, and the walls, the mound and anything a
/// player builds all cast onto the floor.
///
/// **The box is fitted to the arena's bounding sphere, and that is
/// what makes it total.** The eye sits two radii back along the light
/// direction, the half-extents are one radius, and the depth range
/// runs from half a radius to three and a half — so every point within
/// a radius of the centre is inside the box from *any* direction the
/// light might come from. A frustum fitted to the room's corners
/// instead would depend on the tilt, and the first arrangement that
/// clipped a corner would draw the clip line itself as a shadow.
///
/// **Fixed, because a shadow is evidence too.** A light that moved
/// would make every picture a function of when it was taken; this one
/// is a constant of the arena's corners, so the same world always
/// casts the same shadows and the committed renders stay reproducible.
#[must_use]
pub fn sun_light(low: [f32; 3], high: [f32; 3]) -> LightCamera {
    let low = Vec3::new(low[0], low[1], low[2]);
    let high = Vec3::new(high[0], high[1], high[2]);
    let centre = (low + high) * 0.5;
    let extent = high - low;
    // Where the light comes FROM, as fractions of the room's own size:
    // mostly above, leaning over +x and +z so shadows fall away from
    // the corner the pictures are taken from rather than pooling under
    // their casters. Normalised from the room's proportions, so the
    // tilt reads the same in a room of any shape.
    let toward_sun = Vec3::new(extent.x * 0.30, extent.y * 0.80, extent.z * 0.22).normalize();
    // The bounding sphere of the whole shell. Rotation-invariant: no
    // tilt can put a corner outside a box fitted to it.
    let radius = extent.length() * 0.5;
    LightCamera {
        view: View::look_at(centre + toward_sun * (radius * 2.0), centre),
        projection: Orthographic::new(radius, radius, radius * 0.5, radius * 3.5),
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

    /// **Every corner of the arena is inside the sun's box, from any
    /// tilt.** This is the rule a shadow map lives or dies by: a
    /// caster clipped by the light's frustum does not merely lose its
    /// shadow, it makes the clip plane ITSELF read as a hard shadow
    /// edge across the floor — a straight line no geometry explains.
    /// Corners rather than a sampled interior, because the frustum is
    /// convex: if the eight corners are in, everything between them
    /// is.
    #[test]
    fn every_arena_corner_sits_inside_the_suns_box() {
        // Deliberately not the arena's own numbers: a lopsided room
        // with an off-origin minimum exercises the fitting rather than
        // one lucky shape. The square room is the arena's case.
        for (low, high) in [
            ([-9.5f32, -0.5, -9.5], [9.5f32, 8.5, 9.5]),
            ([-3.0, 0.0, -40.0], [37.0, 4.0, -2.0]),
            ([100.0, 100.0, 100.0], [101.0, 101.0, 101.0]),
        ] {
            let matrix = sun_light(low, high).view_projection();
            for corner in [
                [low[0], low[1], low[2]],
                [high[0], low[1], low[2]],
                [low[0], high[1], low[2]],
                [high[0], high[1], low[2]],
                [low[0], low[1], high[2]],
                [high[0], low[1], high[2]],
                [low[0], high[1], high[2]],
                [high[0], high[1], high[2]],
            ] {
                let clip =
                    matrix.transform(renew_math::Vec4::new(corner[0], corner[1], corner[2], 1.0));
                // Orthographic: w is one, so clip space IS normalised
                // device space and no divide is needed.
                assert!(
                    (clip.w - 1.0).abs() < 1e-5,
                    "corner {corner:?} of {low:?}..{high:?} has w {}",
                    clip.w
                );
                assert!(
                    clip.x.abs() <= 1.0 && clip.y.abs() <= 1.0,
                    "corner {corner:?} of {low:?}..{high:?} falls outside the map at                      ({}, {})",
                    clip.x,
                    clip.y
                );
                assert!(
                    (0.0..=1.0).contains(&clip.z),
                    "corner {corner:?} of {low:?}..{high:?} is clipped in depth at {}",
                    clip.z
                );
            }
        }
    }

    /// The sun is a constant of the arena's corners: the same room
    /// always produces the same matrix, so a committed picture is a
    /// function of the world and nothing else.
    #[test]
    fn the_sun_is_a_function_of_the_room_alone() {
        let first = sun_light([-9.5, -0.5, -9.5], [9.5, 8.5, 9.5]);
        let again = sun_light([-9.5, -0.5, -9.5], [9.5, 8.5, 9.5]);
        assert_eq!(first, again);
        let elsewhere = sun_light([-9.5, -0.5, -9.5], [9.5, 12.5, 9.5]);
        assert_ne!(
            first, elsewhere,
            "a differently-shaped room must fit a different box"
        );
    }

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
