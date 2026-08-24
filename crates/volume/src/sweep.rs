//! Moving a box through the volume without passing through it.

use core::num::NonZeroU32;

use renew_fixed::{Fixed, Vec3};
use renew_physics3d::{Shape, Transform, sweep};

use crate::{Cell, Volume, cell_half_extent};

/// Where a swept box first met the volume.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hit {
    /// Fraction of the displacement travelled before contact, in `[0, 1]`.
    pub time: Fixed,
    /// The surface direction at contact, pointing back at the mover.
    pub normal: Vec3,
    /// Which cell stopped it.
    ///
    /// A sub-cell coordinate when the hit came from
    /// [`Volume::sweep_box_fine`], on that call's own lattice.
    pub cell: Cell,
}

impl Volume {
    /// The earliest cell a box meets while moving along `displacement`.
    ///
    /// `skin` is how far short of contact the sweep stops, which keeps a
    /// body from ending exactly on a surface where the next test cannot
    /// tell touching from overlapping.
    ///
    /// # How candidates are chosen
    ///
    /// From the volume rather than from a broadphase: the cells overlapping
    /// the swept box's bounding box, which for a uniform lattice is a
    /// triple loop rather than a search — no tree to build and nothing to
    /// keep in sync with the writes.
    ///
    /// The set is **conservative, and over-inclusive on purpose**. A
    /// diagonal displacement's bounding box covers many cells the box never
    /// enters, and every one of them is tested and rejected by the exact
    /// sweep; that is the cost of not needing a structure. What matters is
    /// the other direction — that no cell which *could* be hit is left out.
    /// Two things widen the box to guarantee that: `skin` (the engine's
    /// sweep reports contact while a gap is still that wide, so a cell just
    /// outside the raw box can still be a genuine hit) and one further cell
    /// of margin (the rounding to cells is nearest-centre, which loses the
    /// low side at exact half-integers — precisely where grid-aligned
    /// bodies sit).
    ///
    /// The result is then clamped to the volume, so a displacement far
    /// larger than the world costs the world rather than the displacement.
    ///
    /// **Ties go to the lower cell**, by the lexicographic order on
    /// [`Cell`]. A box meeting the seam between two cells at the same
    /// instant has to resolve somehow, and resolving by iteration order
    /// would make the result depend on how this loop happens to be written.
    #[must_use]
    pub fn sweep_box(
        &self,
        half_extents: Vec3,
        from: Vec3,
        displacement: Vec3,
        skin: Fixed,
    ) -> Option<Hit> {
        let end = from + displacement;
        let margin = Vec3::new(
            half_extents.x + skin.abs(),
            half_extents.y + skin.abs(),
            half_extents.z + skin.abs(),
        );
        let low = self.clamp(
            Cell::containing(Vec3::new(
                from.x.min(end.x) - margin.x,
                from.y.min(end.y) - margin.y,
                from.z.min(end.z) - margin.z,
            ))
            .offset(-1, -1, -1),
        );
        let high = self.clamp(
            Cell::containing(Vec3::new(
                from.x.max(end.x) + margin.x,
                from.y.max(end.y) + margin.y,
                from.z.max(end.z) + margin.z,
            ))
            .offset(1, 1, 1),
        );

        let mover = Shape::Box { half_extents };
        let target = Shape::Box {
            half_extents: cell_half_extent(),
        };
        let mut best: Option<Hit> = None;

        for z in low.z..=high.z {
            for y in low.y..=high.y {
                for x in low.x..=high.x {
                    let cell = Cell::new(x, y, z);
                    if !self.is_solid(cell) {
                        continue;
                    }
                    let Some(hit) = sweep(
                        mover,
                        Transform::at(from),
                        displacement,
                        target,
                        Transform::at(cell.centre()),
                        skin,
                    ) else {
                        continue;
                    };
                    let earlier = best.as_ref().is_none_or(|found| {
                        hit.time < found.time || (hit.time == found.time && cell < found.cell)
                    });
                    if earlier {
                        best = Some(Hit {
                            time: hit.time,
                            normal: hit.normal,
                            cell,
                        });
                    }
                }
            }
        }
        best
    }
}

/// The lattice index a coordinate falls in, on a lattice `steps` finer
/// than the cell lattice.
///
/// **Floor, not truncation.** A lattice index is a floor; rounding toward
/// zero folds the two sub-cells either side of the origin into one and
/// puts a seam through the middle of the world. A cell is centred on its
/// integer coordinate and spans half a unit either side, so the sub-cell
/// containing `value` is `floor((value + 1/2) * steps)`.
fn fine_index(value: Fixed, steps: i32) -> i32 {
    let half = Fixed::from_ratio(1, 2);
    let scaled = (value + half).saturating_mul(Fixed::from_int(steps));
    i32::try_from(scaled.floor_int()).unwrap_or(0)
}

/// The centre of a sub-cell, in world units.
fn fine_centre(sub: Cell, steps: i32) -> Vec3 {
    let half = Fixed::from_ratio(1, 2);
    // `sub / steps - 1/2` is the low corner, and half a sub-cell above it
    // is the centre. Written as one ratio so the division rounds once.
    let axis = |value: i32| Fixed::from_ratio(2 * value + 1, 2 * steps) - half;
    Vec3::new(axis(sub.x), axis(sub.y), axis(sub.z))
}

impl Volume {
    /// The earliest **sub-cell** a box meets while moving along
    /// `displacement`.
    ///
    /// The sibling of [`Volume::sweep_box`] for a world whose cells are
    /// subdivided. `subdivision` is how many sub-cells span a cell along
    /// one axis, and `solid` is asked about sub-cell coordinates — the
    /// same lattice, `subdivision` times finer, with the sub-cell at
    /// `s` spanning `s / n - 1/2 .. (s + 1) / n - 1/2` so that sub-cell
    /// `n * c` starts exactly at cell `c`'s low edge.
    ///
    /// # Why a predicate rather than a second volume
    ///
    /// **What lives below a cell is the consumer's business.** Destructible
    /// terrain keeps a bitmask, a heightfield-shaped surface derives one
    /// from a function of position, a level-of-detail scheme has several at
    /// once, and none of those wants to store a second lattice the size of
    /// its world just to be swept against. The predicate is the whole of
    /// what a sweep needs to know, and it costs one call per candidate that
    /// is not already rejected by the bounding box.
    ///
    /// A `subdivision` of one is the cell lattice itself, and passing the
    /// volume's own solidity gives exactly [`Volume::sweep_box`] — which is
    /// asserted rather than claimed.
    ///
    /// # How candidates are chosen
    ///
    /// From the swept box's bounding box, expressed on the fine lattice
    /// directly rather than by expanding whole cells. That matters for
    /// cost: a body a quarter of a cell across moving a fraction of a cell
    /// overlaps roughly three cells on an axis, and expanding those to
    /// sub-cells would test `3n` of them where the box itself only reaches
    /// about `2n / 3`.
    ///
    /// The same two widenings as the coarse sweep keep the set
    /// conservative — `skin`, because contact is reported while a gap is
    /// still that wide, and one further sub-cell of margin against the
    /// rounding at exact boundaries, which is precisely where lattice-
    /// aligned bodies sit.
    ///
    /// **Ties go to the lower sub-cell**, by the lexicographic order on
    /// [`Cell`], for the reason the coarse sweep gives: resolving by
    /// iteration order would make the result depend on how the loop
    /// happens to be written.
    ///
    /// Integer throughout, like everything it calls, so two machines
    /// sweeping one body cannot disagree about where it stopped.
    #[must_use]
    pub fn sweep_box_fine(
        &self,
        half_extents: Vec3,
        from: Vec3,
        displacement: Vec3,
        skin: Fixed,
        subdivision: NonZeroU32,
        solid: impl Fn(Cell) -> bool,
    ) -> Option<Hit> {
        let steps = i32::try_from(subdivision.get()).unwrap_or(1).max(1);
        let end = from + displacement;
        let margin = Vec3::new(
            half_extents.x + skin.abs(),
            half_extents.y + skin.abs(),
            half_extents.z + skin.abs(),
        );

        // The volume's own extent, on the fine lattice, so a displacement
        // far larger than the world costs the world rather than the
        // displacement.
        let (width, height, depth) = self.size();
        let origin = self.origin();
        let bound = |low: i32, span: i32, at: i32| at.clamp(low * steps, (low + span) * steps - 1);

        let reach = |pick: fn(Fixed, Fixed) -> Fixed, sign: i32| {
            let corner = Vec3::new(
                pick(from.x, end.x) + Fixed::from_int(sign).saturating_mul(margin.x),
                pick(from.y, end.y) + Fixed::from_int(sign).saturating_mul(margin.y),
                pick(from.z, end.z) + Fixed::from_int(sign).saturating_mul(margin.z),
            );
            Cell::new(
                fine_index(corner.x, steps) + sign,
                fine_index(corner.y, steps) + sign,
                fine_index(corner.z, steps) + sign,
            )
        };
        let low = reach(Fixed::min, -1);
        let high = reach(Fixed::max, 1);
        let low = Cell::new(
            bound(origin.x, width, low.x),
            bound(origin.y, height, low.y),
            bound(origin.z, depth, low.z),
        );
        let high = Cell::new(
            bound(origin.x, width, high.x),
            bound(origin.y, height, high.y),
            bound(origin.z, depth, high.z),
        );

        let mover = Shape::Box { half_extents };
        let reach = Fixed::from_ratio(1, 2 * steps);
        let target = Shape::Box {
            half_extents: Vec3::new(reach, reach, reach),
        };
        let mut best: Option<Hit> = None;

        for z in low.z..=high.z {
            for y in low.y..=high.y {
                for x in low.x..=high.x {
                    let sub = Cell::new(x, y, z);
                    if !solid(sub) {
                        continue;
                    }
                    let Some(hit) = sweep(
                        mover,
                        Transform::at(from),
                        displacement,
                        target,
                        Transform::at(fine_centre(sub, steps)),
                        skin,
                    ) else {
                        continue;
                    };
                    let earlier = best.as_ref().is_none_or(|found| {
                        hit.time < found.time || (hit.time == found.time && sub < found.cell)
                    });
                    if earlier {
                        best = Some(Hit {
                            time: hit.time,
                            normal: hit.normal,
                            cell: sub,
                        });
                    }
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use crate::{Cell, Volume, Voxel};
    use renew_fixed::{Fixed, Vec3};

    const STONE: Voxel = Voxel(1);

    fn half() -> Vec3 {
        let quarter = Fixed::from_ratio(1, 4);
        Vec3::new(quarter, quarter, quarter)
    }

    fn skin() -> Fixed {
        Fixed::from_ratio(1, 128)
    }

    fn volume() -> Volume {
        Volume::new(Cell::new(0, 0, 0), (16, 16, 16)).expect("a small volume is addressable")
    }

    fn one() -> core::num::NonZeroU32 {
        core::num::NonZeroU32::new(1).expect("one is not zero")
    }

    fn eight() -> core::num::NonZeroU32 {
        core::num::NonZeroU32::new(8).expect("eight is not zero")
    }

    /// **At one sub-cell per cell the fine sweep is the coarse one.**
    ///
    /// The strongest thing that can be said about a second implementation
    /// of an existing routine, and the reason the fine sweep is not a
    /// rewrite of the coarse one's geometry: with the volume's own
    /// solidity and a subdivision of one, the two must agree on every
    /// field, for every case — including the ones where nothing is hit and
    /// the ones where the tie-break decides.
    ///
    /// Probed by shifting the fine lattice's centres half a sub-cell: the
    /// times stop matching and the assertion names the displacement.
    #[test]
    fn at_one_sub_cell_per_cell_it_is_the_coarse_sweep() {
        let mut v = volume();
        for at in [
            Cell::new(4, 1, 1),
            Cell::new(4, 2, 1),
            Cell::new(8, 1, 1),
            Cell::new(1, 0, 1),
            Cell::new(6, 1, 2),
        ] {
            v.set(at, STONE);
        }
        let from = Cell::new(1, 1, 1).centre();
        let steps = [
            Vec3::new(Fixed::from_int(9), Fixed::ZERO, Fixed::ZERO),
            Vec3::new(Fixed::ZERO, Fixed::from_int(-3), Fixed::ZERO),
            Vec3::new(Fixed::from_int(6), Fixed::ZERO, Fixed::from_int(2)),
            Vec3::new(Fixed::from_int(-9), Fixed::ZERO, Fixed::ZERO),
            Vec3::new(Fixed::from_ratio(1, 8), Fixed::ZERO, Fixed::ZERO),
        ];
        let mut struck = 0;
        for displacement in steps {
            let coarse = v.sweep_box(half(), from, displacement, skin());
            let fine = v.sweep_box_fine(half(), from, displacement, skin(), one(), |cell| {
                v.is_solid(cell)
            });
            struck += usize::from(coarse.is_some());
            assert_eq!(
                coarse, fine,
                "the two sweeps disagree along {displacement:?}"
            );
        }
        assert!(
            struck >= 3,
            "only {struck} of these displacements hit anything"
        );
    }

    /// **A body stands on what is actually under it, not on the cell.**
    ///
    /// The reason this exists. A cell filled only in its lower half stops
    /// a falling body half a cell lower than a whole one does, and a
    /// consumer whose ground is shaped below the cell has no way to say so
    /// through the coarse sweep.
    ///
    /// Probed by calling the whole cell solid: the body stops at the
    /// cell's top and the two heights come out equal.
    #[test]
    fn a_half_filled_cell_stops_a_body_half_a_cell_lower() {
        let mut v = volume();
        v.set(Cell::new(1, 1, 1), STONE);
        let from = Cell::new(1, 4, 1).centre();
        let down = Vec3::new(Fixed::ZERO, Fixed::from_int(-4), Fixed::ZERO);

        let whole = v
            .sweep_box(half(), from, down, skin())
            .expect("the ground is in the way");
        // The lower half of that cell only: sub-cells 8..12 of 8..16.
        let filled = v
            .sweep_box_fine(half(), from, down, skin(), eight(), |sub| {
                sub.x / 8 == 1 && sub.z / 8 == 1 && (8..12).contains(&sub.y)
            })
            .expect("the shaped ground is in the way");

        assert!(
            filled.time > whole.time,
            "a half-filled cell stopped the body no later than a whole one: {:?} against {:?}",
            filled.time,
            whole.time
        );
        let dropped = (filled.time - whole.time).saturating_mul(Fixed::from_int(4));
        assert!(
            dropped > Fixed::from_ratio(3, 8) && dropped < Fixed::from_ratio(5, 8),
            "the extra drop was {dropped:?}, and half a cell is what four filled sub-cells buy"
        );
        assert!(
            filled.normal.y > Fixed::ZERO,
            "the ground's normal points up at the mover"
        );
    }

    /// A gap narrower than a cell but wider than the body is a gap the
    /// body goes through — which the coarse sweep cannot express at all.
    ///
    /// The slot is six sub-cells tall against a body four sub-cells tall,
    /// so there is an eighth of a cell of clearance either side: more than
    /// `skin`, which is what the sweep reports contact within. A slot cut
    /// exactly to the body reports a hit, correctly, and the first draft
    /// of this asserted otherwise.
    ///
    /// Probed by the second half, which closes the slot: without it a
    /// fixture that simply missed the wall would pass.
    #[test]
    fn a_sub_cell_gap_lets_a_smaller_body_through() {
        let v = volume();
        let from = Cell::new(1, 1, 1).centre();
        let along = Vec3::new(Fixed::from_int(6), Fixed::ZERO, Fixed::ZERO);
        let open = v.sweep_box_fine(half(), from, along, skin(), eight(), |sub| {
            sub.x == 36 && !(9..15).contains(&sub.y)
        });
        assert!(
            open.is_none(),
            "the body was stopped by a slot it fits through: {open:?}"
        );
        let closed = v.sweep_box_fine(half(), from, along, skin(), eight(), |sub| sub.x == 36);
        assert!(
            closed.is_some(),
            "the closed wall stopped nothing, so the open one proves nothing"
        );
    }

    /// **Ties go to the lower sub-cell**, so a body meeting two of them at
    /// the same instant resolves the same way on every machine rather than
    /// by however the loop happens to be written.
    ///
    /// Two sub-cells in one plane, differing only in `y`, both across the
    /// body's path: they are struck at the same time by construction, and
    /// the lexicographically lesser must be the one named.
    ///
    /// Probed by reversing the tie-break to prefer the greater: the upper
    /// sub-cell is reported and the assertion names it.
    #[test]
    fn a_tie_goes_to_the_lower_sub_cell() {
        let v = volume();
        let from = Cell::new(1, 1, 1).centre();
        let along = Vec3::new(Fixed::from_int(6), Fixed::ZERO, Fixed::ZERO);
        let lower = Cell::new(36, 11, 12);
        let upper = Cell::new(36, 12, 12);
        assert!(
            lower < upper,
            "the fixture's own ordering is the wrong way round"
        );
        let hit = v
            .sweep_box_fine(half(), from, along, skin(), eight(), |sub| {
                sub == lower || sub == upper
            })
            .expect("two sub-cells stand across the path");
        assert_eq!(
            hit.cell, lower,
            "a tie went to {:?} rather than to the lower sub-cell",
            hit.cell
        );
        // Each alone is struck, and at the same instant — which is what
        // makes the pair a tie rather than a race one of them wins.
        let struck_at = |at: Cell| {
            v.sweep_box_fine(half(), from, along, skin(), eight(), |sub| sub == at)
                .expect("one sub-cell stands across the path")
                .time
        };
        assert_eq!(
            struck_at(lower),
            struck_at(upper),
            "the two are not struck at the same time, so this is not a tie"
        );
    }

    #[test]
    fn a_box_moving_through_empty_space_meets_nothing() {
        let v = volume();
        let hit = v.sweep_box(
            half(),
            Cell::new(1, 1, 1).centre(),
            Vec3::new(Fixed::from_int(3), Fixed::ZERO, Fixed::ZERO),
            skin(),
        );
        assert!(hit.is_none());
    }

    #[test]
    fn a_box_is_stopped_by_a_solid_cell_and_names_it() {
        let mut v = volume();
        v.set(Cell::new(5, 1, 1), STONE);
        let hit = v
            .sweep_box(
                half(),
                Cell::new(1, 1, 1).centre(),
                Vec3::new(Fixed::from_int(8), Fixed::ZERO, Fixed::ZERO),
                skin(),
            )
            .expect("the wall is in the way");
        assert_eq!(hit.cell, Cell::new(5, 1, 1));
        assert!(hit.time > Fixed::ZERO, "contact is not at the start");
        assert!(hit.time < Fixed::ONE, "and not at the end");
        assert!(
            hit.normal.x < Fixed::ZERO,
            "the normal points back at the mover"
        );
    }

    #[test]
    fn the_nearer_of_two_walls_stops_it() {
        let mut v = volume();
        v.set(Cell::new(4, 1, 1), STONE);
        v.set(Cell::new(8, 1, 1), STONE);
        let hit = v
            .sweep_box(
                half(),
                Cell::new(1, 1, 1).centre(),
                Vec3::new(Fixed::from_int(10), Fixed::ZERO, Fixed::ZERO),
                skin(),
            )
            .expect("hit");
        assert_eq!(hit.cell, Cell::new(4, 1, 1));
    }

    #[test]
    fn a_tie_between_two_cells_resolves_to_the_lower_one() {
        // The two candidates are placed so that loop order and the stated
        // order DISAGREE: the triple loop reaches (5,1,1) first, but the
        // lexicographic winner is (4,1,2). A tie-break that did nothing
        // would keep whichever arrived first and fail here.
        let mut v = volume();
        v.set(Cell::new(5, 1, 1), STONE);
        v.set(Cell::new(4, 1, 2), STONE);
        let from = Vec3::new(
            Fixed::from_int(1),
            Fixed::from_int(1),
            Fixed::from_ratio(3, 2),
        );
        let hit = v
            .sweep_box(
                half(),
                from,
                Vec3::new(Fixed::from_int(8), Fixed::ZERO, Fixed::ZERO),
                skin(),
            )
            .expect("hit");
        assert_eq!(
            hit.cell,
            Cell::new(4, 1, 2),
            "a tie must resolve by the stated order, not by loop order"
        );
    }

    #[test]
    fn the_sweep_cannot_tunnel_through_a_thin_wall() {
        // The displacement is far longer than the wall is thick, which is
        // exactly the case a naive end-point test misses.
        let mut v = Volume::new(Cell::new(0, 0, 0), (32, 16, 16)).expect("volume");
        v.set(Cell::new(10, 1, 1), STONE);
        let hit = v
            .sweep_box(
                half(),
                Cell::new(1, 1, 1).centre(),
                Vec3::new(Fixed::from_int(25), Fixed::ZERO, Fixed::ZERO),
                skin(),
            )
            .expect("a long step must not pass through a wall");
        assert_eq!(hit.cell, Cell::new(10, 1, 1), "and must stop at the wall");
        assert!(hit.time < Fixed::ONE, "before the end of the displacement");
        assert!(hit.normal.x < Fixed::ZERO, "facing back at the mover");
    }

    #[test]
    fn a_contact_inside_the_skin_band_is_not_missed() {
        // The mover stops a fraction short of the wall — closer than `skin`,
        // so the engine's sweep calls it a contact. A candidate box built
        // from the raw swept extent excludes that cell and reports free
        // passage, leaving the body standing inside the skin band.
        let mut v = volume();
        v.set(Cell::new(6, 1, 1), STONE);
        let displacement = Vec3::new(Fixed::from_ratio(1087, 256), Fixed::ZERO, Fixed::ZERO);
        let hit = v.sweep_box(
            half(),
            Vec3::new(Fixed::from_int(1), Fixed::from_int(1), Fixed::from_int(1)),
            displacement,
            skin(),
        );
        assert_eq!(
            hit.map(|h| h.cell),
            Some(Cell::new(6, 1, 1)),
            "a gap smaller than the skin is a contact, and must be a candidate"
        );
    }

    #[test]
    fn a_displacement_far_larger_than_the_world_costs_the_world() {
        // Unclamped, the candidate box spans the displacement rather than
        // the volume: billions of cells visited to reject every one, which
        // is a hang rather than a wrong answer. Both calls below return
        // promptly only because the box is clamped.
        let mut v = volume();
        v.set(Cell::new(3, 1, 1), STONE);
        let huge = Fixed::from_int(2_000);
        let along = v.sweep_box(
            half(),
            Cell::new(1, 1, 1).centre(),
            Vec3::new(huge, Fixed::ZERO, Fixed::ZERO),
            skin(),
        );
        assert_eq!(
            along.map(|hit| hit.cell),
            Some(Cell::new(3, 1, 1)),
            "clamping the candidates must not lose the hit"
        );

        // A diagonal of the same size leaves the wall's row at once, so the
        // answer is nothing — the point is that it arrives at all.
        let diagonal = v.sweep_box(
            half(),
            Cell::new(1, 1, 1).centre(),
            Vec3::new(huge, huge, huge),
            skin(),
        );
        assert!(diagonal.is_none());
    }

    #[test]
    fn a_zero_displacement_against_open_space_is_no_hit() {
        let v = volume();
        let hit = v.sweep_box(half(), Cell::new(1, 1, 1).centre(), Vec3::ZERO, skin());
        assert!(hit.is_none());
    }
}
