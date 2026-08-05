//! The world is a closed box, and the boundary is what closes it.
//!
//! Outside the grid is neither solid nor air — [`Grid::get`] returns nothing
//! and the caller decides — so a player who gets out has nothing to land on and
//! nothing to tell them they have left. The shell exists so that no sequence of
//! edits can put them there.

use renew_sample_cube_world::{AIR, Cell, Grid, STONE};

fn box_of(min: Cell, size: (i32, i32, i32)) -> Grid {
    let mut grid = Grid::new(min, size);
    let high = Cell::new(min.x + size.0 - 1, min.y + size.1 - 1, min.z + size.2 - 1);
    grid.fill(min, high, STONE);
    grid
}

/// **Every cell on the boundary refuses to be cleared, and every cell inside
/// allows it.** Checked over a whole small grid rather than at a few corners,
/// because an off-by-one in any one of the six faces is the failure this rule
/// exists to prevent.
#[test]
fn the_shell_refuses_to_be_cleared_and_the_inside_does_not() {
    let min = Cell::new(-2, -1, 3);
    let size = (5, 4, 6);
    let mut grid = box_of(min, size);

    for x in min.x..min.x + size.0 {
        for y in min.y..min.y + size.1 {
            for z in min.z..min.z + size.2 {
                let cell = Cell::new(x, y, z);
                let on_edge = x == min.x
                    || y == min.y
                    || z == min.z
                    || x == min.x + size.0 - 1
                    || y == min.y + size.1 - 1
                    || z == min.z + size.2 - 1;

                assert_eq!(grid.on_shell(cell), on_edge, "on_shell wrong at {cell:?}");
                assert_eq!(
                    grid.set(cell, AIR),
                    !on_edge,
                    "clearing {cell:?} answered wrongly"
                );
                assert_eq!(
                    grid.is_solid(cell),
                    on_edge,
                    "{cell:?} ended in the wrong state"
                );
            }
        }
    }
}

/// **Filling the shell is allowed; only clearing it is refused.** A rule that
/// rejected every write to the boundary would stop a world being built in the
/// first place, since `fill` is how the shell gets there.
#[test]
fn the_shell_can_be_filled_even_though_it_cannot_be_cleared() {
    let min = Cell::new(0, 0, 0);
    let mut grid = Grid::new(min, (3, 3, 3));
    let corner = Cell::new(0, 0, 0);

    assert!(!grid.is_solid(corner), "it starts empty");
    assert!(grid.set(corner, STONE), "filling the shell is allowed");
    assert!(grid.is_solid(corner));
    assert!(!grid.set(corner, AIR), "clearing it is not");
    assert!(grid.is_solid(corner), "and it did not happen anyway");
}

/// A cell outside the grid is not on its shell — it is not in it at all, and
/// `set` refuses it for that reason rather than this one.
#[test]
fn a_cell_outside_the_grid_is_not_on_its_shell() {
    let grid = box_of(Cell::new(0, 0, 0), (3, 3, 3));
    for cell in [
        Cell::new(-1, 0, 0),
        Cell::new(3, 0, 0),
        Cell::new(0, -1, 0),
        Cell::new(0, 3, 0),
        Cell::new(0, 0, -1),
        Cell::new(0, 0, 3),
        Cell::new(100, 100, 100),
    ] {
        assert!(
            !grid.on_shell(cell),
            "{cell:?} is outside, not on the shell"
        );
        assert_eq!(grid.get(cell), None, "{cell:?} is outside");
    }
}

/// A grid one cell thick in some axis is all shell, which is degenerate but
/// must not be a panic or an off-by-one.
#[test]
fn a_grid_with_no_inside_is_all_shell() {
    let grid = box_of(Cell::new(0, 0, 0), (1, 5, 5));
    for y in 0..5 {
        for z in 0..5 {
            let cell = Cell::new(0, y, z);
            assert!(grid.on_shell(cell), "{cell:?} has no inside to be in");
        }
    }
}

/// The shell rule leaves the solid count alone, because a refused clear is not
/// a clear.
#[test]
fn a_refused_clear_changes_nothing() {
    let mut grid = box_of(Cell::new(0, 0, 0), (4, 4, 4));
    let before = grid.solid_count();
    for x in 0..4 {
        for z in 0..4 {
            grid.set(Cell::new(x, 0, z), AIR);
        }
    }
    assert_eq!(grid.solid_count(), before, "the floor was removed");
}
