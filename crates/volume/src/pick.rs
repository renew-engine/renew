//! Which cell a ray meets first.

use renew_fixed::{Fixed, Vec3};

use crate::{Cell, Face, Volume, Voxel};

/// How many cells a pick may step through before giving up.
///
/// Reach bounds the ray in world units and this bounds it in work: a ray
/// very nearly parallel to an axis crosses many cells per unit travelled.
/// Fixed rather than tuned, because a pick that visited a machine-dependent
/// number of cells would pick machine-dependent cells.
pub const MAX_PICK_STEPS: u32 = 256;

/// A cell a ray met, and how it arrived.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pick {
    /// The cell that stopped the ray.
    pub cell: Cell,
    /// What that cell holds.
    pub voxel: Voxel,
    /// The side the ray entered through.
    pub face: Face,
}

impl Pick {
    /// The cell on the far side of the entry face — the one the ray came
    /// from, and where something placed against this surface would go.
    ///
    /// **Empty whenever the pick came from a traversal**, because the walk
    /// only reaches a cell by passing through that neighbour and finding it
    /// empty. It is *not* guaranteed empty when the ray began inside a
    /// solid: there is no cell it came from, and the one behind the origin
    /// may hold anything. A caller placing voxel should check.
    #[must_use]
    pub const fn neighbour(self) -> Cell {
        let (x, y, z) = self.face.step();
        self.cell.offset(x, y, z)
    }
}

impl Volume {
    /// The first non-empty cell along a ray, within `reach`.
    ///
    /// # How
    ///
    /// Grid traversal rather than a shape query: step from cell to cell,
    /// always crossing whichever axis boundary comes first. That visits
    /// every cell the ray actually passes through, in order, and stops at
    /// the first one holding something — where testing everything in reach
    /// would be thousands of tests to answer a question about a handful.
    ///
    /// The alternative worth naming is sampling the ray at fixed intervals,
    /// which is simpler and wrong: a step longer than a cell skips cells,
    /// and a step short enough not to visits the same cell repeatedly.
    ///
    /// A zero direction picks nothing — there is no ray to walk, and
    /// answering with the cell under the origin would make "look at
    /// nothing" indistinguishable from "look at your feet".
    ///
    /// # Contract
    ///
    /// `reach` is in **world units** and `direction` need not be
    /// normalised — the walk divides by its length, so doubling the
    /// direction vector does not double the reach.
    ///
    /// The walk gives up after [`MAX_PICK_STEPS`] cells and reports
    /// nothing, which is indistinguishable from finding nothing. That
    /// bounds the work a ray nearly parallel to an axis can cost. **A
    /// caller wanting the cap never to be the reason must keep `reach`
    /// below `MAX_PICK_STEPS` units**, since a ray crosses at least one
    /// cell boundary per unit travelled along its dominant axis.
    #[must_use]
    pub fn pick(&self, origin: Vec3, direction: Vec3, reach: Fixed) -> Option<Pick> {
        if direction.x == Fixed::ZERO && direction.y == Fixed::ZERO && direction.z == Fixed::ZERO {
            return None;
        }
        // The walk measures distance as a multiple of `direction`, so the
        // limit it compares against has to be reach in those same units.
        // Without this, handing over a camera's un-normalised forward
        // vector silently scales how far a player can reach.
        let limit = reach.checked_div(direction.length()).unwrap_or(Fixed::MAX);

        let mut cell = Cell::containing(origin);
        let components = [direction.x, direction.y, direction.z];
        let positions = [origin.x, origin.y, origin.z];
        let starts = [cell.x, cell.y, cell.z];

        // Per axis: which way we step, how far along the ray one whole cell
        // is, and how far along the ray the next boundary lies.
        let mut step = [0i32; 3];
        let mut delta = [Fixed::MAX; 3];
        let mut next = [Fixed::MAX; 3];

        for axis in 0..3 {
            let towards = components[axis];
            if towards == Fixed::ZERO {
                // Parallel to this axis, so it never crosses one of these
                // boundaries. A boundary infinitely far away is the honest
                // encoding and keeps the comparison below uniform.
                continue;
            }
            let per_cell = Fixed::ONE.checked_div(towards.abs()).unwrap_or(Fixed::MAX);
            delta[axis] = per_cell;
            let centre = Fixed::from_int(starts[axis]);
            let half = Fixed::from_ratio(1, 2);
            let boundary = if towards > Fixed::ZERO {
                step[axis] = 1;
                centre + half
            } else {
                step[axis] = -1;
                centre - half
            };
            let gap = (boundary - positions[axis]).abs();
            next[axis] = gap.saturating_mul(per_cell);
        }

        // The origin may already be inside something, which a body standing
        // in a wall should be told about rather than having its ray start
        // beyond the wall.
        if let Some(voxel) = self.get(cell).filter(|m| !m.is_empty()) {
            // Entered from nowhere, so name the face the ray would have
            // come through had it started outside: the one on its dominant
            // axis, facing back the way it came. Naming a fixed axis here
            // would report `East` for a ray pointing straight down.
            let axis = dominant_axis(direction);
            let towards = [direction.x, direction.y, direction.z]
                .get(axis)
                .copied()
                .unwrap_or(Fixed::ZERO);
            return Some(Pick {
                cell,
                voxel,
                face: face_of(axis, towards > Fixed::ZERO),
            });
        }

        for _ in 0..MAX_PICK_STEPS {
            // Whichever boundary comes first. Ties go to the lower axis, so
            // a ray through a corner enters by a stated face rather than one
            // that depends on comparison order.
            let mut axis = 0;
            if next[1] < next[axis] {
                axis = 1;
            }
            if next[2] < next[axis] {
                axis = 2;
            }
            if next[axis] > limit {
                return None;
            }
            cell = match axis {
                0 => cell.offset(step[0], 0, 0),
                1 => cell.offset(0, step[1], 0),
                _ => cell.offset(0, 0, step[2]),
            };
            if let Some(voxel) = self.get(cell).filter(|m| !m.is_empty()) {
                return Some(Pick {
                    cell,
                    voxel,
                    face: face_of(axis, step[axis] > 0),
                });
            }
            // Saturating rather than wrapping: an axis whose boundary has
            // been pushed past the representable range is one the ray will
            // never cross again, and `Fixed::MAX` is how that is spelled
            // everywhere else in this walk.
            next[axis] = next[axis].checked_add(delta[axis]).unwrap_or(Fixed::MAX);
        }
        None
    }
}

/// The face a ray crossing `axis` enters by.
///
/// `ascending` says the ray is travelling toward increasing coordinates, in
/// which case it arrives at the low side of the cell ahead.
const fn face_of(axis: usize, ascending: bool) -> Face {
    match (axis, ascending) {
        (0, true) => Face::West,
        (0, false) => Face::East,
        (1, true) => Face::Bottom,
        (1, false) => Face::Top,
        (2, true) => Face::South,
        _ => Face::North,
    }
}

/// The axis a direction points along most strongly.
///
/// Ties go to the lower axis, for the same reason the walk's boundary ties
/// do: a diagonal has to name one face, and naming it by comparison order
/// would make the answer depend on how this function is written.
fn dominant_axis(direction: Vec3) -> usize {
    let magnitudes = [direction.x.abs(), direction.y.abs(), direction.z.abs()];
    let mut axis = 0;
    if magnitudes[1] > magnitudes[axis] {
        axis = 1;
    }
    if magnitudes[2] > magnitudes[axis] {
        axis = 2;
    }
    axis
}

#[cfg(test)]
mod tests {
    use super::MAX_PICK_STEPS;
    use crate::{Cell, Face, Volume, Voxel};
    use renew_fixed::{Fixed, Vec3};

    const STONE: Voxel = Voxel(1);

    fn volume() -> Volume {
        Volume::new(Cell::new(0, 0, 0), (32, 16, 16)).expect("a small volume is addressable")
    }

    fn reach() -> Fixed {
        Fixed::from_int(64)
    }

    /// One unit east. `Vec3` carries no axis constants, unlike `Vec2`.
    fn east() -> Vec3 {
        Vec3::new(Fixed::ONE, Fixed::ZERO, Fixed::ZERO)
    }

    #[test]
    fn a_ray_down_an_axis_meets_the_first_solid_cell_and_names_the_face() {
        let mut v = volume();
        v.set(Cell::new(5, 0, 0), STONE);
        let hit = v
            .pick(Cell::new(0, 0, 0).centre(), east(), reach())
            .expect("the ray must reach it");
        assert_eq!(hit.cell, Cell::new(5, 0, 0));
        assert_eq!(hit.voxel, STONE);
        assert_eq!(hit.face, Face::West, "entered through the low side");
        assert_eq!(hit.neighbour(), Cell::new(4, 0, 0));
    }

    #[test]
    fn the_nearer_of_two_solids_wins() {
        let mut v = volume();
        v.set(Cell::new(3, 0, 0), STONE);
        v.set(Cell::new(7, 0, 0), Voxel(2));
        let hit = v
            .pick(Cell::new(0, 0, 0).centre(), east(), reach())
            .expect("hit");
        assert_eq!(hit.cell, Cell::new(3, 0, 0));
    }

    #[test]
    fn nothing_within_reach_is_nothing() {
        let mut v = volume();
        v.set(Cell::new(20, 0, 0), STONE);
        let short = Fixed::from_int(4);
        assert!(v.pick(Cell::new(0, 0, 0).centre(), east(), short).is_none());
    }

    #[test]
    fn a_ray_that_leaves_the_volume_finds_nothing() {
        let v = volume();
        assert!(
            v.pick(Cell::new(0, 0, 0).centre(), east(), reach())
                .is_none()
        );
    }

    #[test]
    fn an_origin_inside_a_solid_reports_that_cell() {
        let mut v = volume();
        let inside = Cell::new(2, 0, 0);
        v.set(inside, STONE);
        let hit = v.pick(inside.centre(), east(), reach()).expect("hit");
        assert_eq!(hit.cell, inside, "a body in a wall must be told so");
    }

    #[test]
    fn an_origin_inside_a_solid_names_the_face_the_ray_came_through() {
        // The face has to follow the ray's own axis. Reading it off a fixed
        // axis reports the same side for every direction, so a player buried
        // in sand looking at their feet is told they are looking east.
        let mut v = Volume::new(Cell::new(0, 0, 0), (16, 16, 16)).expect("volume");
        let inside = Cell::new(3, 3, 4);
        v.set(inside, STONE);
        let cases = [
            (Vec3::new(Fixed::ZERO, -Fixed::ONE, Fixed::ZERO), Face::Top),
            (
                Vec3::new(Fixed::ZERO, Fixed::ONE, Fixed::ZERO),
                Face::Bottom,
            ),
            (east(), Face::West),
            (Vec3::new(-Fixed::ONE, Fixed::ZERO, Fixed::ZERO), Face::East),
            (Vec3::new(Fixed::ZERO, Fixed::ZERO, Fixed::ONE), Face::South),
            (
                Vec3::new(Fixed::ZERO, Fixed::ZERO, -Fixed::ONE),
                Face::North,
            ),
        ];
        for (direction, expected) in cases {
            let hit = v.pick(inside.centre(), direction, reach()).expect("hit");
            assert_eq!(hit.face, expected, "for direction {direction:?}");
        }
    }

    #[test]
    fn reach_is_world_units_whatever_the_directions_length() {
        // A camera handing over an un-normalised forward vector must not
        // silently change how far the player can reach.
        let mut v = volume();
        v.set(Cell::new(7, 0, 0), STONE);
        let origin = Cell::new(0, 0, 0).centre();
        let short = Fixed::from_int(4);
        let doubled = Vec3::new(Fixed::from_int(2), Fixed::ZERO, Fixed::ZERO);
        assert!(
            v.pick(origin, east(), short).is_none(),
            "6.5 units is past 4"
        );
        assert!(
            v.pick(origin, doubled, short).is_none(),
            "and doubling the direction must not double the reach"
        );
        let long = Fixed::from_int(8);
        assert!(v.pick(origin, east(), long).is_some());
        assert!(v.pick(origin, doubled, long).is_some());
    }

    #[test]
    fn a_zero_direction_picks_nothing() {
        let mut v = volume();
        v.set(Cell::new(1, 0, 0), STONE);
        assert!(
            v.pick(Cell::new(0, 0, 0).centre(), Vec3::ZERO, reach())
                .is_none(),
            "no ray means no answer, not the cell underfoot"
        );
    }

    #[test]
    fn a_ray_along_each_axis_meets_what_is_in_front_of_it() {
        // Every axis, both directions. The walk picks the nearest boundary
        // by comparing three candidates, and a test suite that only ever
        // fires along x leaves the z arm of that comparison — and the z
        // step that follows it — never executed. Two lines nothing had
        // run, in the middle of the hot loop.
        let mut v = Volume::new(Cell::new(0, 0, 0), (32, 32, 32)).expect("volume");
        let origin = Cell::new(8, 8, 8);
        let cases = [
            (
                Vec3::new(Fixed::ONE, Fixed::ZERO, Fixed::ZERO),
                Cell::new(12, 8, 8),
            ),
            (
                Vec3::new(-Fixed::ONE, Fixed::ZERO, Fixed::ZERO),
                Cell::new(4, 8, 8),
            ),
            (
                Vec3::new(Fixed::ZERO, Fixed::ONE, Fixed::ZERO),
                Cell::new(8, 12, 8),
            ),
            (
                Vec3::new(Fixed::ZERO, -Fixed::ONE, Fixed::ZERO),
                Cell::new(8, 4, 8),
            ),
            (
                Vec3::new(Fixed::ZERO, Fixed::ZERO, Fixed::ONE),
                Cell::new(8, 8, 12),
            ),
            (
                Vec3::new(Fixed::ZERO, Fixed::ZERO, -Fixed::ONE),
                Cell::new(8, 8, 4),
            ),
        ];
        for (direction, target) in cases {
            v.set(target, STONE);
            let hit = v.pick(origin.centre(), direction, reach());
            assert_eq!(
                hit.map(|pick| pick.cell),
                Some(target),
                "a ray toward {direction:?} missed {target:?}"
            );
            v.set(target, Voxel::EMPTY);
        }
    }

    #[test]
    fn a_backward_ray_names_the_opposite_face() {
        let mut v = volume();
        v.set(Cell::new(1, 0, 0), STONE);
        let origin = Cell::new(6, 0, 0).centre();
        let hit = v
            .pick(
                origin,
                Vec3::new(-Fixed::ONE, Fixed::ZERO, Fixed::ZERO),
                reach(),
            )
            .expect("hit");
        assert_eq!(hit.cell, Cell::new(1, 0, 0));
        assert_eq!(hit.face, Face::East);
        assert_eq!(hit.neighbour(), Cell::new(2, 0, 0));
    }

    #[test]
    fn the_step_cap_stops_the_walk_short_of_a_reachable_solid() {
        // Non-vacuous by construction: the solid is IN the volume, exactly
        // on the ray, and well inside reach — so `None` can only be the cap.
        // The contract documents this, and the test is what keeps the
        // documented behaviour and the real behaviour the same thing.
        const { assert!(MAX_PICK_STEPS < 1024, "the cap is the bound, not reach") };
        let mut v = Volume::new(Cell::new(0, 0, 0), (1024, 16, 16)).expect("volume");
        let far = i32::try_from(MAX_PICK_STEPS).unwrap_or(i32::MAX) + 44;
        v.set(Cell::new(far, 0, 0), STONE);
        let generous = Fixed::from_int(1000);
        let origin = Cell::new(0, 0, 0).centre();
        assert!(
            v.pick(origin, east(), generous).is_none(),
            "past the cap, the walk gives up rather than walking forever"
        );

        // The same solid within the cap is found, which proves the previous
        // assertion is about the cap and not about the volume or the reach.
        let near = i32::try_from(MAX_PICK_STEPS).unwrap_or(i32::MAX) - 44;
        let mut w = Volume::new(Cell::new(0, 0, 0), (1024, 16, 16)).expect("volume");
        w.set(Cell::new(near, 0, 0), STONE);
        assert_eq!(
            w.pick(origin, east(), generous).map(|hit| hit.cell),
            Some(Cell::new(near, 0, 0))
        );
    }

    #[test]
    fn a_diagonal_ray_finds_a_cell_it_actually_passes_through() {
        let mut v = Volume::new(Cell::new(0, 0, 0), (16, 16, 16)).expect("volume");
        v.set(Cell::new(4, 4, 0), STONE);
        let diagonal = Vec3::new(Fixed::ONE, Fixed::ONE, Fixed::ZERO);
        let hit = v
            .pick(Cell::new(0, 0, 0).centre(), diagonal, reach())
            .expect("hit");
        assert_eq!(hit.cell, Cell::new(4, 4, 0));
    }
}
