//! Moving a shape and finding what it meets first.

use crate::narrow::separation;
use crate::shape::{Shape, Transform};
use renew_fixed::{Fixed, Vec2};

/// How many advancement steps a sweep may take before giving up.
///
/// Fixed rather than tuned, because it is part of the answer: a sweep that
/// took a machine-dependent number of steps would produce a machine-dependent
/// time of impact. Twenty-four is enough for the grazing cases that converge
/// slowly, and cheap for the ordinary ones that finish in two or three.
pub const MAX_ADVANCE_STEPS: u32 = 24;

/// Where a moving shape first met a stationary one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SweepHit {
    /// Fraction of the displacement travelled before contact, in `[0, 1]`.
    pub time: Fixed,
    /// Where the moving shape's origin sits at that moment.
    pub origin: Vec2,
    /// The surface direction at contact, pointing back at the mover.
    pub normal: Vec2,
}

/// Sweep `moving` from `from` along `displacement` against a stationary shape.
///
/// `skin` is how far short of contact the sweep stops, which is what keeps a
/// body from ending exactly on a surface where the next test cannot tell touch
/// from overlap.
///
/// # Why conservative advancement rather than a closed form
///
/// The exact answer is a ray cast against the Minkowski sum of the two shapes.
/// For two circles that sum is a circle and the closed form is a line; for a
/// box and a circle it is a rounded box; for two boxes at different angles it
/// is an octagon whose vertices depend on both rotations. Three shapes give
/// six special cases, and each is a separate opportunity to be subtly wrong.
///
/// Advancing conservatively needs one thing instead: a lower bound on the
/// distance between the shapes, which the separating-axis test already
/// produces. Each step moves by exactly the time it would take to close that
/// bound, so it can approach the true time of impact but never pass it —
/// **the sweep cannot tunnel**, whatever the shapes are.
///
/// The cost is that a grazing approach converges slowly, which is why the step
/// count is capped and the cap is part of the contract rather than a tuning
/// knob.
#[must_use]
pub fn sweep(
    moving: Shape,
    from: Transform,
    displacement: Vec2,
    target: Shape,
    target_at: Transform,
    skin: Fixed,
) -> Option<SweepHit> {
    let mut time = Fixed::ZERO;
    let mut outcome = None;

    for step_index in 0..MAX_ADVANCE_STEPS {
        let here = Transform::new(from.translation + displacement * time, from.rotation);
        let (gap, direction) = separation(moving, here, target, target_at)?;

        // **Touching is not the same as being obstructed.** A body resting
        // against a wall and sliding along it is in contact for the whole
        // move, and reporting that as a blocking hit stops it dead: the slide
        // removes the normal component, the remainder is already parallel, and
        // the next iteration meets the same surface at the same instant. The
        // body burns its whole iteration budget standing still.
        //
        // So contact only blocks when the motion goes *into* it. This is the
        // one check that separates a wall a character is running along from a
        // wall it is running at.
        //
        // **Penetration is the exception**: a body genuinely inside something
        // reports whichever way it is moving, because a caller needs to know
        // it is there before it can decide to push out. Only the band between
        // touching and the skin distance is treated as passable.
        let resting = gap >= Fixed::ZERO && displacement.dot(direction) <= Fixed::ZERO;
        if gap <= skin && resting {
            break;
        }

        // Contact, or the budget spent. **The last step reports where it got
        // to rather than nothing**, because reporting no hit would let a body
        // pass straight through something it was converging on — and since
        // every advance is bounded below the true time of impact, the position
        // reached is always short of the surface rather than past it.
        //
        // The two share an arm deliberately: written as a separate tail after
        // the loop it was twelve lines restating this one, and unreachable for
        // every shape family that exists — a sweep of eight thousand rotated
        // box approaches left at worst twenty-nine raw units unconverged.
        if gap <= skin || step_index + 1 == MAX_ADVANCE_STEPS {
            outcome = Some(SweepHit {
                time,
                origin: here.translation,
                // The direction points from the mover toward the target, and a
                // caller sliding wants the surface facing back at it.
                normal: -direction,
            });
            break;
        }

        // How fast the gap closes along the axis that measured it. Moving away
        // or sliding parallel means this axis will never be crossed, and since
        // it is a separating axis, nothing else will be either.
        let approach = displacement.dot(direction);
        if approach <= Fixed::ZERO {
            break;
        }

        // Time to close the gap if the mover kept going straight. The bound is
        // a lower bound on the true distance, so this is a lower bound on the
        // true time — advancing by it can never overshoot. The guard above
        // established a positive divisor, so the fallback is unreachable and
        // routes any surprise through the no-progress check below rather than
        // inventing a distance.
        let step = (gap - skin).checked_div(approach).unwrap_or(Fixed::ZERO);
        time = time + step;
        if time > Fixed::ONE {
            break;
        }
        // A step that rounds to nothing cannot make progress, and repeating it
        // would burn the whole budget standing still. The shapes are within a
        // raw unit of the skin distance here, which is contact by any measure
        // the number type can express.
        if step <= Fixed::ZERO {
            outcome = Some(SweepHit {
                time,
                origin: from.translation + displacement * time,
                normal: -direction,
            });
            break;
        }
    }

    outcome
}
