//! Moving a body against the world and sliding along what stops it.

use crate::query::{Counts, Exclude};
use crate::shape::Transform;
use crate::sweep::sweep;
use crate::world::{Collider, World};
use renew_fixed::{Fixed, Vec3};

/// Why a slide stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlideEnd {
    /// The whole displacement was used, or what remained pointed into a
    /// surface and had nowhere left to go.
    Displaced,
    /// The iteration limit ran out with displacement still unspent.
    ///
    /// **Reported rather than swallowed.** A body that silently stopped short
    /// looks to a caller exactly like one that arrived, and the difference
    /// shows up as a character that sticks in corners for reasons nothing
    /// explains.
    IterationsExhausted,
}

/// One surface a slide met.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlideHit {
    /// What was hit.
    pub collider: Collider,
    /// The surface direction, facing the mover.
    pub normal: Vec3,
    /// Where the body's origin sat when it met this.
    pub origin: Vec3,
}

/// What a slide did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlideReport {
    /// Where the body ended up.
    pub destination: Vec3,
    /// How many surfaces were met, and how many fitted the caller's buffer.
    pub hits: Counts,
    /// How many sweep-and-slide iterations were spent.
    pub iterations: u32,
    /// Why it stopped.
    pub end: SlideEnd,
}

/// A displacement shorter than this is nothing left to spend.
///
/// Not zero: the slide subtracts a normal component each iteration and the
/// remainder is rounded, so an exactly-zero remainder is a case that does not
/// arise. One raw unit squared is the smallest length the number type can tell
/// from nothing.
const SPENT: i64 = 1;

impl World {
    /// Move a body along a displacement, sliding along whatever stops it.
    ///
    /// **This is where a character's ground state comes from, not from the
    /// contact array.** A body stopped here rests a skin distance from what
    /// stopped it — far enough that a contact test will not report it — so
    /// the hits written here are the only record that it landed, what it
    /// landed on, and which way that surface faced.
    ///
    /// The moving body is excluded from its own sweep. Its shapes are parts of
    /// one object and an object does not block itself; an articulated thing
    /// whose parts must collide is several bodies.
    pub fn move_and_slide(
        &mut self,
        handle: renew_ecs::Entity,
        displacement: Vec3,
        mask: u32,
        skin: Fixed,
        iteration_limit: u32,
        out: &mut [SlideHit],
    ) -> Option<SlideReport> {
        // **Out of anything it is already inside, before asking what it will
        // hit.** A sweep starts from where the body is and answers what is
        // ahead of it, which is the wrong question when the answer is already
        // touching — so a body spawned overlapping would otherwise sweep out
        // from inside the thing it is stuck in and stay stuck.
        self.clear_of_geometry(handle, mask, skin, iteration_limit)?;

        let start = self.transform(handle)?;
        let mut position = start.translation;
        let mut remaining = displacement;
        let mut hits = Counts::default();
        let mut iterations = 0;
        let mut end = SlideEnd::IterationsExhausted;
        let excluded = [handle];

        while iterations < iteration_limit {
            if remaining.length_squared().to_bits() <= SPENT {
                end = SlideEnd::Displaced;
                break;
            }
            iterations += 1;

            let here = Transform::at(position);
            let Some((hit, collider)) = self.sweep_body(
                handle,
                here,
                remaining,
                mask,
                skin,
                Exclude::bodies(&excluded),
            ) else {
                // Nothing in the way: spend what is left and finish.
                position = position + remaining;
                end = SlideEnd::Displaced;
                break;
            };

            if let Some(slot) = out.get_mut(hits.written) {
                *slot = SlideHit {
                    collider,
                    normal: hit.normal,
                    origin: hit.origin,
                };
                hits.written += 1;
            }
            hits.existed += 1;

            position = hit.origin;
            // What is left of the displacement after the part already
            // travelled, with the component into the surface removed. Sliding
            // rather than stopping is what lets a character run along a wall
            // instead of sticking to it.
            let unspent = remaining * (Fixed::ONE - hit.time);
            remaining = unspent.slide_along(hit.normal);
        }

        self.set_transform(handle, Transform::at(position));

        // **And out again at the end, which is what makes the clearance
        // guarantee true rather than approximate.** Measured against real
        // geometry in this crate as well as the two-dimensional one, the slide
        // alone lands inside the skin distance by an amount that grows with
        // the distance travelled: here a 1024-unit slide ended 232 raw units
        // *inside* a wall it should have rested 256 clear of. Re-establishing
        // the clearance removes the dependence rather than bounding it.
        let restored = self.clear_of_geometry(handle, mask, skin, iteration_limit)?;

        Some(SlideReport {
            destination: restored.destination,
            hits,
            iterations,
            end,
        })
    }

    /// Sweep every live shape of a body and take the earliest impact.
    ///
    /// **Ties break by the collider hit**, lowest first, so two surfaces met
    /// at the same instant — which is what a corner is — resolve the same way
    /// on every machine.
    fn sweep_body(
        &self,
        handle: renew_ecs::Entity,
        from: Transform,
        displacement: Vec3,
        mask: u32,
        skin: Fixed,
        exclude: Exclude<'_>,
    ) -> Option<(crate::sweep::SweepHit, Collider)> {
        let extent = self.shape_extent(handle)?;
        let mut best: Option<(crate::sweep::SweepHit, Collider)> = None;

        for index in 0..extent {
            let mine = Collider {
                handle,
                index: crate::world::ShapeIndex::from_raw(index),
            };
            let Some((shape, local, _)) = self.shape(mine) else {
                continue;
            };
            let placed = local.compose(from);

            for collider in self.colliders() {
                let Some((other, other_at)) = self.query_visible(collider, mask, exclude) else {
                    continue;
                };
                let Some(hit) = sweep(shape, placed, displacement, other, other_at, skin) else {
                    continue;
                };
                let earlier = best.as_ref().is_none_or(|(current, current_collider)| {
                    hit.time < current.time
                        || (hit.time == current.time && collider < *current_collider)
                });
                if earlier {
                    // The reported origin is the shape's, and the caller moves
                    // the body — so it is shifted back by the local placement.
                    let body_origin = hit.origin - (placed.translation - from.translation);
                    best = Some((
                        crate::sweep::SweepHit {
                            time: hit.time,
                            origin: body_origin,
                            normal: hit.normal,
                        },
                        collider,
                    ));
                }
            }
        }
        best
    }
}
