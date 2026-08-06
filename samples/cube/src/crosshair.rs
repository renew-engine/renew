//! Where the player is pointing, drawn over the world.
//!
//! **A first-person game has to say where you are pointing.** The lit
//! block answers it when something is in reach and says nothing at all
//! when nothing is — which is most of the time in an open room, and
//! exactly when a player is lining a shot up. Two bars at the centre of
//! the screen are what the aim ray actually is.
//!
//! **Drawn through the plain mesh pipeline rather than the camera one**,
//! because these positions *are* clip space: an overlay is the one thing
//! a first-person driver draws that must not move when the view does.
//! That pipeline culls nothing, so the winding here decides nothing.
//!
//! **Clip space is square in its coordinates and not on the screen**: x
//! spans the window's width and y its height, so bars of equal length in
//! clip units are drawn unequal in pixels on any window that is not
//! square. The horizontal extents are divided by the aspect to undo
//! that, which is why everything here takes one.
//!
//! Its own module because two callers need it from behind different
//! features: the window draws it every frame, and a still from the
//! player's eyes draws it for the same reason that view lights the aimed
//! block — it claims to be what the player sees.

use renew_render3d::Scene;

/// The crosshair as a scene, ready to upload.
#[must_use]
pub fn scene(aspect: f32) -> Scene {
    let mut scene = Scene::new();
    for bar in bars(aspect) {
        scene.quad(bar, INK);
    }
    scene
}

/// Bright and slightly cool, so it reads against stone and against the
/// horizon alike.
const INK: [f32; 4] = [0.93, 0.95, 0.97, 1.0];

/// The two bars, as clip-space corners.
///
/// Separate from the scene because this is where every decision is — the
/// sizes, the aspect correction, the near-plane depth — and a scene keeps
/// its bytes to itself. A test beside this can read corners; it cannot
/// read a vertex buffer belonging to another crate.
fn bars(aspect: f32) -> [[[f32; 3]; 4]; 2] {
    /// Half the length of an arm, as a fraction of half the window height.
    const ARM: f32 = 0.024;
    /// Half the thickness of an arm, the same way.
    const THICK: f32 = 0.0035;
    /// Nearest, so nothing in the world can be in front of it.
    const DEPTH: f32 = 0.0;

    // A degenerate aspect would put an infinity in the geometry, and a
    // corner that is not a number takes the whole quad off the screen —
    // so the crosshair would vanish rather than merely look wrong. It
    // cannot arrive from `aspect_of`, which answers 1.0 for a zero
    // height; this is the function that would produce the NaN, so this is
    // the function that refuses to.
    let across = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };

    let bar = |half_width: f32, half_height: f32| {
        [
            [-half_width, -half_height, DEPTH],
            [half_width, -half_height, DEPTH],
            [half_width, half_height, DEPTH],
            [-half_width, half_height, DEPTH],
        ]
    };

    [bar(ARM / across, THICK), bar(THICK / across, ARM)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The arms are equal on the screen, not in the coordinates.** Clip
    /// space spans the window's width in x and its height in y, so bars
    /// of equal length in clip units come out unequal in pixels on any
    /// window that is not square.
    #[test]
    fn the_crosshair_is_square_on_a_wide_window() {
        let (across, down) = extents_of(2.0);
        // The horizontal arm is half as long in clip units on a window
        // twice as wide as it is tall, which is what makes it the same
        // number of pixels.
        assert!(
            (across * 2.0 - down).abs() < 1e-6,
            "arms {across} across and {down} down are not square on a 2:1 window"
        );
    }

    /// On a square window the arms are equal in clip units too, which is
    /// the case that would hide an aspect division applied backwards.
    #[test]
    fn the_crosshair_is_square_on_a_square_window() {
        let (across, down) = extents_of(1.0);
        assert!((across - down).abs() < 1e-6, "{across} across, {down} down");
    }

    /// It sits at the centre, which is where the aim ray goes.
    #[test]
    fn the_crosshair_is_centred() {
        let (across, down) = extents_of(1.6);
        // Extents are measured as maxima; symmetry means the minima
        // mirror them, so a centred cross has equal magnitudes.
        for (name, value) in [("across", across), ("down", down)] {
            assert!(value > 0.0, "the {name} arm has no length");
        }
        let depths: Vec<f32> = corners_of(1.6)
            .into_iter()
            .map(|corner| corner[2])
            .collect();
        assert!(
            depths.iter().all(|depth| depth.abs() < f32::EPSILON),
            "the overlay must sit at the near plane: {depths:?}"
        );
    }

    /// A window that reports nothing usable must not put a NaN in the
    /// geometry — a NaN corner takes the whole quad off the screen, so
    /// the crosshair would vanish rather than look wrong.
    #[test]
    fn a_degenerate_aspect_still_makes_a_crosshair() {
        for aspect in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(
                corners_of(aspect)
                    .iter()
                    .all(|corner| corner.iter().all(|value| value.is_finite())),
                "aspect {aspect} produced a corner that is not a number"
            );
        }
    }

    /// **The crosshair is actually drawn, at the centre.** The tests
    /// above pin its coordinates; this one draws it and looks. Geometry
    /// that is right in the arithmetic and absent on the screen is the
    /// failure this whole file has hit twice.
    ///
    /// Skipped where there is no Vulkan driver, and refused under
    /// `RENEW_GOLDEN=1`, like the other picture oracles.
    #[test]
    fn the_crosshair_lands_in_the_middle_of_the_picture() {
        // `if let` rather than a `let ... else { return }`: the early
        // return would be a line that only a lane with no driver ever
        // runs, and so a line no lane that draws can cover.
        if let Some(pixels) = crate::render::tests::pixels_or_skip(
            crate::render::draw_clip_space(&scene(1.0)),
            crate::render::tests::golden_strict(),
        ) {
            assert_a_cross_at_the_centre(&pixels);
        }
    }

    /// What the picture must show: ink in the middle, backdrop between
    /// the arms.
    fn assert_a_cross_at_the_centre(pixels: &[u8]) {
        let size = crate::render::SIZE;
        let at = |x: u32, y: u32| {
            let index = ((y * size + x) * 4) as usize;
            [pixels[index], pixels[index + 1], pixels[index + 2]]
        };
        let centre = at(size / 2, size / 2);
        let corner = at(1, 1);
        assert_ne!(
            centre, corner,
            "the centre of the picture is the backdrop, so nothing was drawn there"
        );
        // The ink is far brighter than the backdrop it is drawn on.
        assert!(
            centre.iter().all(|channel| *channel > 200),
            "the centre is {centre:?}, which is not the crosshair's ink"
        );
        // An arm's length out along the diagonal is backdrop: the cross
        // has arms rather than being a filled square.
        let out = size / 2 + size / 8;
        assert_eq!(
            at(out, out),
            corner,
            "the diagonal between two arms should be backdrop"
        );
    }

    /// Every corner of both bars.
    fn corners_of(aspect: f32) -> Vec<[f32; 3]> {
        bars(aspect).into_iter().flatten().collect()
    }

    /// The half-length of the horizontal arm and of the vertical one.
    fn extents_of(aspect: f32) -> (f32, f32) {
        let corners = corners_of(aspect);
        let across = corners.iter().fold(0.0f32, |most, c| most.max(c[0]));
        let down = corners.iter().fold(0.0f32, |most, c| most.max(c[1]));
        (across, down)
    }
}
