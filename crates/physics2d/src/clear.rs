//! Moving a body out of what it is inside, or too close to.
//!
//! **The operation the vocabulary called depenetration**, and the one piece of
//! machinery two separate obligations turned out to share.
//!
//! A body may begin an operation overlapping something — spawned there, or a
//! kinematic platform moved into it — and nothing in the sweep can help: a
//! sweep starts from where the body is and asks what it will hit, which is the
//! wrong question when the answer is already touching it. And a slide, measured
//! against real geometry, ends a little closer than the skin distance it was
//! asked to keep, by an amount that grows with the distance travelled. Both are
//! the same problem stated at different times: *the body is nearer than the
//! skin, and something must push it out.*
//!
//! **It moves along the separating direction, one shape at a time, worst
//! first.** Pushing the deepest overlap out can push the body into a shallower
//! one, which is why this iterates rather than doing a single pass, and why it
//! reports how many iterations it took and whether it finished. A corner
//! between two faces needs two.

use renew_fixed::{Fixed, Vec2};

use crate::narrow::separation;
use crate::query::Exclude;
use crate::shape::Transform;
use crate::world::{Collider, ShapeIndex, World};

/// How a clearing ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearEnd {
    /// Nothing was closer than the skin distance, so nothing moved.
    ///
    /// **Distinct from `Cleared` on purpose.** A caller that wants to know
    /// whether the body had been left somewhere it should not be — a spawn
    /// check, a platform that may have crushed something — needs to tell "it
    /// was fine" from "it is fine now", and a displacement of zero does not
    /// say that: a body one raw unit inside can be pushed out by a distance
    /// that rounds to nothing.
    AlreadyClear,
    /// The body was moved until nothing was closer than the skin distance.
    Cleared,
    /// The iteration limit ran out with something still too close.
    ///
    /// **Not an error and not a success.** It happens where geometry genuinely
    /// has no room — a body wider than the gap it is in — and the honest answer
    /// is the best position found plus the fact that it is not enough.
    IterationsExhausted,
}

/// What a clearing did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClearReport {
    /// Where the body ended up.
    pub destination: Vec2,
    /// How far it was moved, which is zero when it was already clear.
    pub moved: Vec2,
    /// How many pushes it took.
    pub iterations: u32,
    /// Whether it finished, and how.
    pub end: ClearEnd,
}

impl World {
    /// The worst violation of the skin distance at a given placement: how far
    /// short it falls, and the direction from this body toward the offender.
    ///
    /// **Ties break on the collider**, the same way the sweep's do, because two
    /// equal deficits must resolve the same way on every machine or the
    /// operation is not reproducible.
    fn deepest_deficit(
        &self,
        handle: renew_ecs::Entity,
        at: Transform,
        mask: u32,
        skin: Fixed,
    ) -> Option<(Fixed, Vec2)> {
        let extent = self.shape_extent(handle)?;
        let excluded = [handle];
        let mut worst: Option<(Fixed, Vec2, Collider)> = None;

        for index in 0..extent {
            let mine = Collider {
                handle,
                index: ShapeIndex::from_raw(index),
            };
            let Some((shape, local, _)) = self.shape(mine) else {
                continue;
            };
            let placed = local.compose(at);

            for collider in self.colliders() {
                let Some((other, other_at)) =
                    self.query_visible(collider, mask, Exclude::bodies(&excluded))
                else {
                    continue;
                };
                let Some((gap, direction)) = separation(shape, placed, other, other_at) else {
                    continue;
                };
                if gap >= skin {
                    continue;
                }
                let deficit = skin - gap;
                let beats_it = worst.as_ref().is_none_or(|(current, _, current_collider)| {
                    deficit > *current || (deficit == *current && collider < *current_collider)
                });
                if beats_it {
                    worst = Some((deficit, direction, collider));
                }
            }
        }
        worst.map(|(deficit, direction, _)| (deficit, direction))
    }

    /// Push a body out until nothing is closer to it than the skin distance.
    ///
    /// Returns nothing if the handle names no live body. The body is excluded
    /// from its own test: its shapes are parts of one object, and an object is
    /// not inside itself.
    ///
    /// **The body moves; nothing else does.** Whatever it was inside stays
    /// where it is, including another body that could have been pushed instead.
    /// That is v0 being explicit rather than clever: choosing which of two
    /// bodies yields is a question about mass and kind that this crate has no
    /// answer for yet, and splitting the movement between them would make the
    /// result depend on the order they were created in.
    pub fn clear_of_geometry(
        &mut self,
        handle: renew_ecs::Entity,
        mask: u32,
        skin: Fixed,
        iteration_limit: u32,
    ) -> Option<ClearReport> {
        let start = self.transform(handle)?;
        let mut position = start.translation;
        let mut iterations = 0;
        let end = loop {
            let here = Transform::new(position, start.rotation);
            let Some((deficit, direction)) = self.deepest_deficit(handle, here, mask, skin) else {
                break if iterations == 0 {
                    ClearEnd::AlreadyClear
                } else {
                    ClearEnd::Cleared
                };
            };
            if iterations >= iteration_limit {
                break ClearEnd::IterationsExhausted;
            }
            iterations += 1;
            // The direction runs from this body toward what it is too close
            // to, so getting away from it means going the other way.
            position = position - direction * deficit;
        };

        self.set_transform(handle, Transform::new(position, start.rotation));
        Some(ClearReport {
            destination: position,
            moved: position - start.translation,
            iterations,
            end,
        })
    }
}
