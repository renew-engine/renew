//! The behaviours a voxel game is judged on.

use renew_fixed::{Fixed, Vec3};
use renew_sample_cube_world::{AIR, Cell, Cube, Face, Grid, Intent, STONE, Tuning, pick};

fn v(x: i32, y: i32, z: i32) -> Vec3 {
    Vec3::new(Fixed::from_int(x), Fixed::from_int(y), Fixed::from_int(z))
}

/// A walled arena: a stone floor with a wall around its edge.
///
/// **Walled rather than open, and that is not decoration.** Outside the grid
/// is not solid, so a player who walks off the floor falls — correctly, and
/// forever. A test that then asserts something about their position is really
/// asserting that they did not wander, which is not what it says it checks.
/// The first version of these tests was open, and two of them failed for
/// exactly that reason.
fn flat_world() -> Grid {
    let mut grid = Grid::new(Cell::new(-20, -2, -20), (41, 14, 41));
    grid.fill(Cell::new(-20, 0, -20), Cell::new(20, 0, 20), STONE);
    // Four walls, three blocks high.
    grid.fill(Cell::new(-20, 1, -20), Cell::new(-20, 3, 20), STONE);
    grid.fill(Cell::new(20, 1, -20), Cell::new(20, 3, 20), STONE);
    grid.fill(Cell::new(-20, 1, -20), Cell::new(20, 3, -20), STONE);
    grid.fill(Cell::new(-20, 1, 20), Cell::new(20, 3, 20), STONE);
    grid
}

fn staged() -> Cube {
    Cube::new(Tuning::default(), flat_world(), v(0, 4, 0))
}

fn run(world: &mut Cube, ticks: u32, intent: Intent) {
    for _ in 0..ticks {
        world.step(intent);
    }
}

#[test]
fn a_cell_and_a_position_agree_about_where_things_are() {
    assert_eq!(Cell::containing(Vec3::ZERO), Cell::new(0, 0, 0));
    assert_eq!(Cell::containing(v(3, -2, 5)), Cell::new(3, -2, 5));
    // A cell spans half a unit either side of its centre.
    let just_inside = Vec3::new(Fixed::from_ratio(49, 100), Fixed::ZERO, Fixed::ZERO);
    assert_eq!(Cell::containing(just_inside), Cell::new(0, 0, 0));
    let just_over = Vec3::new(Fixed::from_ratio(51, 100), Fixed::ZERO, Fixed::ZERO);
    assert_eq!(Cell::containing(just_over), Cell::new(1, 0, 0));

    // **The boundary rule is the same on both sides of zero**, which is what
    // rounding toward zero would get wrong.
    let below = Vec3::new(-Fixed::from_ratio(51, 100), Fixed::ZERO, Fixed::ZERO);
    assert_eq!(Cell::containing(below), Cell::new(-1, 0, 0));
    assert_eq!(Cell::new(3, 4, 5).centre(), v(3, 4, 5));
}

#[test]
fn a_grid_answers_inside_and_declines_outside() {
    let mut grid = Grid::new(Cell::new(0, 0, 0), (4, 4, 4));
    assert_eq!(grid.get(Cell::new(0, 0, 0)), Some(AIR));
    assert_eq!(grid.get(Cell::new(3, 3, 3)), Some(AIR));
    assert_eq!(grid.get(Cell::new(4, 0, 0)), None, "past the far edge");
    assert_eq!(grid.get(Cell::new(-1, 0, 0)), None, "before the near edge");

    assert!(grid.set(Cell::new(1, 2, 3), STONE));
    assert_eq!(grid.get(Cell::new(1, 2, 3)), Some(STONE));
    assert!(grid.is_solid(Cell::new(1, 2, 3)));
    assert!(!grid.set(Cell::new(9, 9, 9), STONE), "outside is refused");

    // **Outside is not solid**, so a player can leave the world rather than
    // being trapped at its edge with no explanation.
    assert!(!grid.is_solid(Cell::new(-1, 0, 0)));
    assert_eq!(grid.solid_count(), 1);
}

/// A grid with a dimension of zero is empty rather than an error: a world with
/// no blocks is a legitimate thing to simulate.
#[test]
fn a_degenerate_grid_is_empty_rather_than_refused() {
    let grid = Grid::new(Cell::new(0, 0, 0), (0, 5, 5));
    assert_eq!(grid.solid_count(), 0);
    assert_eq!(grid.get(Cell::new(0, 0, 0)), None);
    assert_eq!(grid.size(), (0, 5, 5));
    assert_eq!(grid.min(), Cell::new(0, 0, 0));

    let negative = Grid::new(Cell::new(0, 0, 0), (-3, 5, 5));
    assert_eq!(negative.size(), (0, 5, 5), "a negative size clamps to none");
}

#[test]
fn a_player_falls_and_lands_on_the_ground() {
    let mut world = staged();
    assert!(!world.grounded(), "it starts in the air");
    run(&mut world, 120, Intent::IDLE);

    assert!(world.grounded(), "it should have landed");
    // The floor's top is y = 0.5 and the player's half-height is 0.9, so the
    // centre rests near 1.4.
    let resting = world.position().y;
    assert!(
        (resting - Fixed::from_ratio(14, 10)).to_bits().abs() <= 8192,
        "rested at y = {}, expected about 1.4",
        resting.to_bits()
    );
    assert!(
        world.velocity().y.to_bits().abs() <= 4096,
        "landing must kill the downward speed"
    );
}

#[test]
fn a_grounded_player_jumps_and_comes_back_down() {
    let mut world = staged();
    run(&mut world, 120, Intent::IDLE);
    let ground = world.position().y;

    world.step(Intent {
        jump: true,
        ..Intent::IDLE
    });
    assert!(world.velocity().y > Fixed::ZERO, "the jump pushed it up");
    run(&mut world, 8, Intent::IDLE);
    assert!(world.position().y > ground, "it went up");

    run(&mut world, 120, Intent::IDLE);
    assert!(world.grounded(), "and came back down");
}

#[test]
fn a_player_walks_on_the_ground() {
    let mut world = staged();
    run(&mut world, 120, Intent::IDLE);
    let start = world.position();

    run(&mut world, 20, Intent::walking(1, 0));
    assert!(world.position().x > start.x, "moved east");
    assert!(world.grounded(), "and stayed on the ground");

    run(&mut world, 20, Intent::walking(0, 1));
    assert!(world.position().z > start.z, "and north");
}

/// **A wall stops the player and lets them slide along it**, which is the
/// three-dimensional case: the two components that are not into the wall both
/// survive.
#[test]
fn a_player_walking_into_a_wall_slides_along_it() {
    let mut grid = flat_world();
    // A wall three blocks high at x = 3, spanning the arena.
    grid.fill(Cell::new(3, 1, -20), Cell::new(3, 3, 20), STONE);
    let mut world = Cube::new(Tuning::default(), grid, v(0, 4, 0));
    run(&mut world, 120, Intent::IDLE);

    run(&mut world, 200, Intent::walking(1, 1));
    // The wall's near face is x = 2.5 and the player is 0.3 wide, so the
    // centre cannot pass 2.2.
    assert!(
        world.position().x < Fixed::from_ratio(23, 10),
        "walked into the wall, reaching x = {}",
        world.position().x.to_bits()
    );
    // And kept going north the whole time.
    assert!(
        world.position().z > Fixed::from_int(3),
        "should have slid along the wall, reached z = {}",
        world.position().z.to_bits()
    );
}

#[test]
fn a_ray_picks_the_first_solid_block_and_names_the_face() {
    let grid = flat_world();
    // Looking straight down from above the floor.
    let picked = pick(
        &grid,
        v(0, 4, 0),
        Vec3::new(Fixed::ZERO, Fixed::from_int(-1), Fixed::ZERO),
        Fixed::from_int(10),
    )
    .expect("the floor is below");
    assert_eq!(picked.cell, Cell::new(0, 0, 0));
    assert_eq!(picked.face, Face::Top, "entered through the top");
    assert_eq!(
        picked.neighbour(),
        Cell::new(0, 1, 0),
        "a block placed here goes above the one looked at"
    );
}

#[test]
fn a_ray_names_the_face_it_came_in_through_on_every_side() {
    let mut grid = Grid::new(Cell::new(-4, -4, -4), (9, 9, 9));
    grid.set(Cell::new(0, 0, 0), STONE);
    let reach = Fixed::from_int(10);
    let cases = [
        (v(-4, 0, 0), v(1, 0, 0), Face::West),
        (v(4, 0, 0), v(-1, 0, 0), Face::East),
        (v(0, -4, 0), v(0, 1, 0), Face::Bottom),
        (v(0, 4, 0), v(0, -1, 0), Face::Top),
        (v(0, 0, -4), v(0, 0, 1), Face::South),
        (v(0, 0, 4), v(0, 0, -1), Face::North),
    ];
    for (origin, direction, expected) in cases {
        let picked = pick(&grid, origin, direction, reach).expect("the block is on the line");
        assert_eq!(picked.cell, Cell::new(0, 0, 0));
        assert_eq!(picked.face, expected, "from {origin:?} along {direction:?}");
        // The neighbour is always back toward where the ray came from.
        let (dx, dy, dz) = expected.step();
        assert_eq!(picked.neighbour(), Cell::new(dx, dy, dz));
    }
}

#[test]
fn a_ray_past_its_reach_finds_nothing() {
    let grid = flat_world();
    let down = Vec3::new(Fixed::ZERO, Fixed::from_int(-1), Fixed::ZERO);
    assert!(
        pick(&grid, v(0, 40, 0), down, Fixed::from_int(5)).is_none(),
        "the floor is forty below and the reach is five"
    );
    assert!(
        pick(&grid, v(0, 40, 0), down, Fixed::from_int(50)).is_some(),
        "and fifty reaches it"
    );
}

#[test]
fn a_ray_into_empty_space_finds_nothing() {
    let grid = Grid::new(Cell::new(-4, -4, -4), (9, 9, 9));
    assert!(
        pick(
            &grid,
            v(0, 0, 0),
            Vec3::new(Fixed::ONE, Fixed::ZERO, Fixed::ZERO),
            Fixed::from_int(100)
        )
        .is_none()
    );
}

/// Breaking removes the block being looked at; placing puts one against its
/// face rather than inside it.
#[test]
fn digging_and_placing_do_what_a_player_expects() {
    let mut world = staged();
    run(&mut world, 120, Intent::IDLE);
    world.look_at(Vec3::new(Fixed::ZERO, Fixed::from_int(-1), Fixed::ZERO));

    let target = world.looking_at().expect("standing on the floor").cell;
    assert!(world.grid().is_solid(target));

    world.step(Intent {
        dig: true,
        ..Intent::IDLE
    });
    assert!(!world.grid().is_solid(target), "the block is gone");
    assert_eq!(world.edits(), (1, 0));

    // Holding the button does not keep digging, which is why the latch exists.
    let before = world.grid().solid_count();
    run(
        &mut world,
        5,
        Intent {
            dig: true,
            ..Intent::IDLE
        },
    );
    assert_eq!(
        world.grid().solid_count(),
        before,
        "a held button digs once"
    );
}

#[test]
fn a_placed_block_goes_against_the_face_and_never_inside_the_player() {
    let mut world = staged();
    run(&mut world, 120, Intent::IDLE);

    // Look at a floor block a little away, so the space above it is free.
    world.look_at(Vec3::new(
        Fixed::from_int(2),
        Fixed::from_int(-2),
        Fixed::ZERO,
    ));
    let picked = world.looking_at().expect("looking at the floor");
    let target = picked.neighbour();
    assert!(!world.grid().is_solid(target), "the space is free first");

    world.step(Intent {
        place: true,
        ..Intent::IDLE
    });
    assert!(world.grid().is_solid(target), "a block appeared beside it");
    assert_eq!(world.edits(), (0, 1));

    // And straight down, where the block would land inside the player's feet,
    // nothing is placed — a block there would trap them with no way out.
    world.look_at(Vec3::new(Fixed::ZERO, Fixed::from_int(-1), Fixed::ZERO));
    let before = world.grid().solid_count();
    world.step(Intent {
        place: true,
        ..Intent::IDLE
    });
    assert_eq!(
        world.grid().solid_count(),
        before,
        "a block must not be placed inside the player"
    );
}

/// **The property that makes a replay an assertion.** The same inputs give the
/// same digest, and a digest that ignored an input would not be an oracle.
#[test]
fn the_same_inputs_give_the_same_digest() {
    let script: Vec<Intent> = (0..200)
        .map(|tick: u32| match tick % 13 {
            0..=3 => Intent::walking(1, 0),
            4 => Intent {
                jump: true,
                ..Intent::IDLE
            },
            5..=7 => Intent::walking(0, 1),
            8 => Intent {
                dig: true,
                ..Intent::IDLE
            },
            9 => Intent {
                place: true,
                ..Intent::IDLE
            },
            _ => Intent::IDLE,
        })
        .collect();

    let digest_of = |script: &[Intent]| {
        let mut world = staged();
        world.look_at(Vec3::new(
            Fixed::ONE,
            Fixed::from_int(-1),
            Fixed::from_ratio(1, 2),
        ));
        for &intent in script {
            world.step(intent);
        }
        world.digest()
    };

    let first = digest_of(&script);
    assert_eq!(first, digest_of(&script), "the same run digests the same");

    // A walk, not a dig: the dig at that tick might find nothing in reach or
    // a block already broken, and a negative control that can silently do
    // nothing proves nothing. A step in a different direction always moves.
    let mut altered = script.clone();
    altered[100] = Intent::walking(-1, -1);
    assert_ne!(
        digest_of(&altered),
        first,
        "a digest that ignores an input is not an oracle"
    );
}

/// The terrain is part of the state, so a world where a block was broken
/// hashes differently from one where it was not — even if the player is in
/// exactly the same place.
#[test]
fn the_digest_covers_the_terrain() {
    let mut untouched = staged();
    let mut edited = staged();
    run(&mut untouched, 120, Intent::IDLE);
    run(&mut edited, 120, Intent::IDLE);
    assert_eq!(untouched.digest(), edited.digest(), "identical so far");

    edited.look_at(Vec3::new(Fixed::ZERO, Fixed::from_int(-1), Fixed::ZERO));
    untouched.look_at(Vec3::new(Fixed::ZERO, Fixed::from_int(-1), Fixed::ZERO));
    edited.step(Intent {
        dig: true,
        ..Intent::IDLE
    });
    untouched.step(Intent::IDLE);
    assert_ne!(
        untouched.digest(),
        edited.digest(),
        "one of them has a hole in the floor"
    );
}

/// Looking nowhere is ignored rather than silently repointing the player, and
/// the tick counter counts.
#[test]
fn a_zero_look_direction_is_ignored_and_ticks_are_counted() {
    let mut world = staged();
    run(&mut world, 120, Intent::IDLE);
    world.look_at(Vec3::new(Fixed::ZERO, Fixed::from_int(-1), Fixed::ZERO));
    let before = world.looking_at();

    world.look_at(Vec3::ZERO);
    assert_eq!(
        world.looking_at(),
        before,
        "a zero direction changes nothing"
    );

    let ticks = world.tick();
    world.step(Intent::IDLE);
    assert_eq!(world.tick(), ticks + 1);
}

/// The player never ends a tick inside the terrain, over a long run of walking
/// and jumping around a world with obstacles.
#[test]
fn the_player_never_ends_a_tick_inside_a_block() {
    let mut grid = flat_world();
    grid.fill(Cell::new(3, 1, -20), Cell::new(3, 2, 20), STONE);
    grid.fill(Cell::new(-20, 1, 3), Cell::new(20, 2, 3), STONE);
    let mut world = Cube::new(Tuning::default(), grid, v(0, 4, 0));

    for tick in 0..500u32 {
        let intent = match tick % 19 {
            0..=5 => Intent::walking(1, 0),
            6 => Intent {
                jump: true,
                ..Intent::IDLE
            },
            7..=12 => Intent::walking(0, 1),
            13..=16 => Intent::walking(-1, -1),
            _ => Intent::IDLE,
        };
        world.step(intent);

        let at = world.position();
        let cell = Cell::containing(at);
        assert!(
            !world.grid().is_solid(cell),
            "tick {tick}: the player's centre is inside block {cell:?}"
        );
        // And never below the floor, whose top is y = 0.5.
        assert!(
            at.y > Fixed::ZERO,
            "tick {tick}: sank to y = {}, through the floor",
            at.y.to_bits()
        );
    }
}

/// A ray beginning inside a block reports that block rather than starting past
/// it — which is what a player standing in one needs to be told.
#[test]
fn a_ray_starting_inside_a_block_reports_it() {
    let mut grid = Grid::new(Cell::new(-4, -4, -4), (9, 9, 9));
    grid.set(Cell::new(0, 0, 0), STONE);
    let picked = pick(
        &grid,
        Vec3::ZERO,
        Vec3::new(Fixed::ONE, Fixed::ZERO, Fixed::ZERO),
        Fixed::from_int(10),
    )
    .expect("the origin is inside a block");
    assert_eq!(picked.cell, Cell::new(0, 0, 0));
    // Heading east, so it is treated as having come in through the west face —
    // which puts a placed block on the side the ray came from.
    assert_eq!(picked.face, Face::West);
}

/// **A ray that never leaves the grid still stops.** The reach bounds it in
/// world units and the step budget bounds it in work; without the second, a
/// ray very nearly parallel to an axis walks one tiny step at a time.
#[test]
fn a_ray_with_a_huge_reach_stops_at_its_step_budget() {
    // A big empty grid and a reach far beyond what the budget can cover.
    let grid = Grid::new(Cell::new(-500, -500, -500), (1001, 1001, 1001));
    assert!(
        pick(
            &grid,
            Vec3::ZERO,
            Vec3::new(Fixed::ONE, Fixed::ZERO, Fixed::ZERO),
            Fixed::from_int(10_000)
        )
        .is_none(),
        "nothing to find, and it must give up rather than walk forever"
    );
}

/// Placing at the edge of the world is refused rather than silently dropped
/// somewhere else, and the counter does not move.
#[test]
fn placing_outside_the_grid_is_refused() {
    // A one-block grid holding one block: every neighbour is outside.
    let mut grid = Grid::new(Cell::new(0, 0, 0), (1, 1, 1));
    grid.set(Cell::new(0, 0, 0), STONE);
    let mut world = Cube::new(Tuning::default(), grid, v(0, 4, 0));
    world.look_at(Vec3::new(Fixed::ZERO, Fixed::from_int(-1), Fixed::ZERO));

    let picked = world.looking_at().expect("the block is below");
    assert_eq!(picked.cell, Cell::new(0, 0, 0));
    assert_eq!(picked.neighbour(), Cell::new(0, 1, 0), "outside the grid");

    world.step(Intent {
        place: true,
        ..Intent::IDLE
    });
    assert_eq!(
        world.edits(),
        (0, 0),
        "a refused placement must not be counted"
    );
    assert_eq!(world.grid().solid_count(), 1, "and must not appear");
}
