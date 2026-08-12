//! Moving a box through the volume without passing through it.

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
        Volume::new(Cell::new(0, 0, 0), (1, 1, 1)).expect("a small volume is addressable")
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
        let mut v = Volume::new(Cell::new(0, 0, 0), (2, 1, 1)).expect("volume");
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
