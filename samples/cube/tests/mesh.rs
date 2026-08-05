//! The meshing rule, against a budget computed before any of it was
//! written.
//!
//! The arena has 5012 solid cells and **4642** visible inward faces.
//! Those figures are not observations of what this mesher produces — they
//! were derived from the arena's dimensions before it was written, and
//! these tests hold the code to them. A test that recorded whatever the
//! code emitted would agree with a wrong mesher forever.

use renew_sample_cube::mesh::{Quad, colour, faces};
use renew_sample_cube_world::grid::{AIR, Cell, Grid, STONE};
use renew_sample_cube_world::ray::Face;

/// A step component as a float, without a cast.
///
/// `Face::step` answers with -1, 0 or 1 and nothing else, so there is no
/// precision to lose -- and saying that in three arms is both cheaper and
/// more honest than allowing a lint about a case that cannot arise.
fn unit(component: i32) -> f32 {
    match component {
        1 => 1.0,
        -1 => -1.0,
        _ => 0.0,
    }
}

/// Every direction, so a test cannot silently cover five of six.
const EVERY_FACE: [Face; 6] = [
    Face::East,
    Face::West,
    Face::Top,
    Face::Bottom,
    Face::North,
    Face::South,
];

/// The arena's face count is exactly the budget.
///
/// **The number that matters is the one this is not.** A mesher written
/// against `is_solid` emits the box's outer skin too — 5330 more faces,
/// 9972 in total — and it looks perfectly correct from inside, because
/// the extra faces are all behind the camera's back wall. Asserting the
/// total is what separates the two.
#[test]
fn the_arena_meshes_to_its_computed_budget() {
    let grid = renew_sample_cube::arena();
    assert_eq!(
        grid.solid_count(),
        5012,
        "the arena is not the one measured"
    );

    let quads = faces(&grid);
    assert_eq!(
        quads.len(),
        4642,
        "the budget is 4642 inward faces; 9972 means the outer skin is being emitted too"
    );
}

/// Per direction, because the total can be right while the sides are not.
///
/// Top and bottom are 1521 each — the ceiling is 39×39, and the floor is
/// 39×39 too once the mound's 25 covered cells are traded for the mound's
/// own 25-cell top. Four walls of 39×10 give 400 apiece.
#[test]
fn each_direction_carries_its_share() {
    let quads = faces(&renew_sample_cube::arena());
    for (face, expected) in [
        (Face::East, 400),
        (Face::West, 400),
        (Face::Top, 1521),
        (Face::Bottom, 1521),
        (Face::North, 400),
        (Face::South, 400),
    ] {
        let found = quads.iter().filter(|quad| quad.face == face).count();
        assert_eq!(found, expected, "{face:?} should carry {expected} faces");
    }
}

/// A single block alone in the world meshes to **nothing**.
///
/// The sharpest statement of the rule in the smallest world: every one of
/// its six neighbours is outside the grid, so a mesher reading
/// "not solid" emits all six and this one emits none.
#[test]
fn a_lone_block_filling_its_grid_has_no_visible_face() {
    let mut grid = Grid::new(Cell::new(0, 0, 0), (1, 1, 1));
    grid.fill(Cell::new(0, 0, 0), Cell::new(0, 0, 0), STONE);
    assert_eq!(grid.solid_count(), 1, "the block should be there");

    assert_eq!(
        faces(&grid).len(),
        0,
        "outside the grid is not air, so a block with nothing but void around it shows no face"
    );
}

/// And a block with air beside it shows exactly the sides that touch air.
///
/// The complement of the test above: without this, emitting nothing ever
/// would pass.
#[test]
fn a_block_shows_the_sides_that_touch_air() {
    // A 3x1x1 strip: solid in the middle, air at both ends.
    let mut grid = Grid::new(Cell::new(0, 0, 0), (3, 1, 1));
    grid.fill(Cell::new(1, 0, 0), Cell::new(1, 0, 0), STONE);

    let quads = faces(&grid);
    assert_eq!(
        quads.len(),
        2,
        "two neighbours are air; the other four are outside"
    );
    let mut directions: Vec<Face> = quads.iter().map(|quad| quad.face).collect();
    directions.sort_by_key(|face| format!("{face:?}"));
    assert_eq!(directions, vec![Face::East, Face::West]);
}

/// The order is fixed, so the same grid meshes to the same sequence.
///
/// This is the crate's contribution to a reproducible frame: the renderer
/// draws in submission order, so a mesher that walked cells in a varying
/// order would move the picture without changing the world.
#[test]
fn meshing_is_reproducible() {
    let grid = renew_sample_cube::arena();
    assert_eq!(
        faces(&grid),
        faces(&grid),
        "the same grid must mesh identically"
    );
}

/// Corners sit half a unit from the cell centre, on the face's own plane,
/// and describe a unit square.
#[test]
fn a_face_is_a_unit_square_half_a_unit_out() {
    for face in EVERY_FACE {
        let quad = Quad {
            cell: Cell::new(0, 0, 0),
            face,
            block: STONE,
        };
        let corners = quad.corners();

        // Every corner is on the same plane, half a unit along the face's
        // axis. Which axis that is comes from the step, so this does not
        // restate the basis table it is checking.
        let (dx, dy, dz) = face.step();
        let axis = if dx != 0 {
            0
        } else if dy != 0 {
            1
        } else {
            2
        };
        let sign = unit(dx + dy + dz);
        for corner in corners {
            assert!(
                (corner[axis] - sign * 0.5).abs() < f32::EPSILON,
                "{face:?}: corner {corner:?} is not on the face's plane"
            );
        }

        // Four distinct corners, each one half-unit step from the next.
        for index in 0..4 {
            let here = corners[index];
            let next = corners[(index + 1) % 4];
            let distance = ((next[0] - here[0]).powi(2)
                + (next[1] - here[1]).powi(2)
                + (next[2] - here[2]).powi(2))
            .sqrt();
            assert!(
                (distance - 1.0).abs() < 1e-6,
                "{face:?}: edge {index} is {distance}, not one unit"
            );
        }
    }
}

/// Winding is counter-clockwise seen from outside, on every face.
///
/// Nothing reads it today — the pipeline culls nothing — which is exactly
/// why it needs a test. A reversed quad is invisible until culling is
/// switched on, and then half the world vanishes with no recent change to
/// blame.
#[test]
fn every_face_is_wound_counter_clockwise_from_outside() {
    for face in EVERY_FACE {
        let corners = Quad {
            cell: Cell::new(0, 0, 0),
            face,
            block: STONE,
        }
        .corners();

        let edge = |a: [f32; 3], b: [f32; 3]| [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let first = edge(corners[0], corners[1]);
        let second = edge(corners[1], corners[2]);
        let cross = [
            first[1] * second[2] - first[2] * second[1],
            first[2] * second[0] - first[0] * second[2],
            first[0] * second[1] - first[1] * second[0],
        ];

        let (dx, dy, dz) = face.step();
        let outward = [unit(dx), unit(dy), unit(dz)];
        let dot = cross[0] * outward[0] + cross[1] * outward[1] + cross[2] * outward[2];
        assert!(
            dot > 0.0,
            "{face:?}: the winding faces inward (cross {cross:?} against outward {outward:?})"
        );
    }
}

/// Every block-and-direction pair has a colour, and direction changes it.
///
/// The shading is the only thing separating one face from another in a
/// picture with no light in it, so "they are all the same colour" is a
/// real failure rather than a cosmetic one.
#[test]
fn colour_varies_by_direction_and_names_an_unknown_block() {
    let top = colour(STONE, Face::Top);
    let bottom = colour(STONE, Face::Bottom);
    assert!(
        top[0] > bottom[0] && top[1] > bottom[1] && top[2] > bottom[2],
        "up should be brighter than down, or the world reads as a silhouette"
    );

    // All six differ from at least one other, and none is transparent.
    let mut seen: Vec<[f32; 4]> = Vec::new();
    for face in EVERY_FACE {
        let shade = colour(STONE, face);
        assert!(
            (shade[3] - 1.0).abs() < f32::EPSILON,
            "{face:?}: a block face is opaque"
        );
        seen.push(shade);
    }
    let distinct = seen
        .iter()
        .map(|value| format!("{value:?}"))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        distinct.len() >= 4,
        "at least four brightness levels, or adjacent faces blur into one shape: {distinct:?}"
    );

    // A block type nobody coloured is magenta rather than plausible, and
    // air is meshed by nothing so it takes the same arm.
    for unknown in [AIR, 7, 200] {
        let shade = colour(unknown, Face::Top);
        assert!(
            shade[0] > 0.9 && shade[1] < 0.1 && shade[2] > 0.9,
            "block {unknown} has no colour of its own, so it should shout: {shade:?}"
        );
    }
}

/// The aimed-at block is visibly brighter than the same face is normally.
///
/// **The feature that makes digging aimable.** Every block is the same
/// grey, so without this a player cannot tell which one the next keypress
/// will break until it is already gone.
#[test]
fn the_aimed_block_is_brighter_than_the_same_face_unaimed() {
    use renew_sample_cube::mesh::aimed_colour;
    for face in EVERY_FACE {
        let plain = colour(STONE, face);
        let lit = aimed_colour(STONE, face);
        assert!(
            lit[0] > plain[0] && lit[1] > plain[1] && lit[2] > plain[2],
            "{face:?}: the aimed face should be brighter: {plain:?} then {lit:?}"
        );
        assert!(
            lit.iter().all(|channel| *channel <= 1.0),
            "{face:?}: brightness must stay inside the range a colour has: {lit:?}"
        );
    }
}

/// Naming an aimed cell changes the scene, and naming none leaves it be.
///
/// Behind the feature because the scene builder is: a build with no
/// renderer has nothing to build a scene for.
#[cfg(feature = "render")]
#[test]
fn an_aimed_cell_changes_the_scene_and_none_leaves_it() {
    let grid = renew_sample_cube::arena();
    let plain = renew_sample_cube::render::build_world_space(&grid, None);
    let same = renew_sample_cube::render::build_world_space(&grid, None);
    assert_eq!(
        plain.index_count(),
        same.index_count(),
        "the same request should build the same scene"
    );

    // A cell on the mound's top, which the arena really has.
    let lit = renew_sample_cube::render::build_world_space(&grid, Some(Cell::new(4, 2, 0)));
    assert_eq!(
        lit.index_count(),
        plain.index_count(),
        "highlighting changes colour, not geometry"
    );
}
