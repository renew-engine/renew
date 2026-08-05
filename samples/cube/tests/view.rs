//! Drawing the voxel world, and the defect the drawing exposed.
//!
//! The first picture of this world showed the player at `y = -25` after a
//! four-hundred-tick run in an arena whose floor is at `y = 0`. Nothing was
//! wrong with the drawing — the player really was there, twenty-five blocks
//! below a world it had left, and every existing test passed because they all
//! asked whether the run reproduced rather than whether it made sense.
//!
//! The containment test below is the one that was missing.

use renew_sample_cube::{Options, Script, elevation_text, plan_text, run, run_cli, run_world};
use renew_sample_cube_world::Cell;

fn args(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

const SCRIPTS: [Script; 3] = [Script::Stand, Script::Patrol, Script::Build];

/// **No script puts the player outside the world, however long it runs.**
///
/// This is the assertion whose absence let a script spend most of its run
/// falling below the floor. It is checked at several lengths rather than one,
/// because the original defect needed a few hundred ticks to appear and would
/// have passed at fifty.
#[test]
fn no_script_ever_leaves_the_world() {
    for script in SCRIPTS {
        for ticks in [1, 50, 200, 400, 1000, 2500] {
            let world = run_world(&Options {
                script,
                ticks,
                ..Options::default()
            });
            let grid = world.grid();
            let (width, height, depth) = grid.size();
            let min = grid.min();
            let at = Cell::containing(world.position());

            assert!(
                at.x >= min.x
                    && at.y >= min.y
                    && at.z >= min.z
                    && at.x < min.x + width
                    && at.y < min.y + height
                    && at.z < min.z + depth,
                "{script:?} left the world after {ticks} ticks, at {at:?}"
            );
        }
    }
}

/// **A script that digs must have something to dig**, or it is testing that
/// nothing happens.
///
/// Closing the box made the floor unbreakable, which silently turned the
/// digging script into a script that reaches for the shell and is refused. The
/// arena grew a mound for it; this is what keeps the mound there.
#[test]
fn the_building_script_actually_breaks_and_places_blocks() {
    let report = run(&Options {
        script: Script::Build,
        ticks: 300,
        ..Options::default()
    });
    assert!(report.edits.0 > 0, "building broke nothing");
    assert!(report.edits.1 > 0, "building placed nothing");
}

/// The two views are the whole grid, so two runs line up cell for cell.
#[test]
fn both_views_span_the_whole_grid() {
    let world = run_world(&Options {
        script: Script::Patrol,
        ticks: 200,
        ..Options::default()
    });
    let (width, height, depth) = world.grid().size();

    let plan = plan_text(&world);
    let plan_rows: Vec<&str> = plan
        .lines()
        .skip(1)
        .take_while(|l| l.contains('.') || l.contains('#') || l.contains('@'))
        .collect();
    assert_eq!(plan_rows.len(), usize::try_from(depth).expect("small"));
    for row in &plan_rows {
        assert_eq!(
            row.chars().skip(5).count(),
            usize::try_from(width).expect("small")
        );
    }

    let elevation = elevation_text(&world);
    let side_rows: Vec<&str> = elevation
        .lines()
        .skip(1)
        .take_while(|l| l.contains('.') || l.contains('#') || l.contains('@'))
        .collect();
    assert_eq!(side_rows.len(), usize::try_from(height).expect("small"));
    for row in &side_rows {
        assert_eq!(
            row.chars().skip(5).count(),
            usize::try_from(width).expect("small")
        );
    }
}

/// **The player appears in both views**, which is only true if both slice
/// through the cell it is actually in.
///
/// A slice taken at the wrong height or the wrong depth would draw a perfectly
/// plausible picture of the world with no player in it.
#[test]
fn the_player_appears_in_both_views() {
    for script in SCRIPTS {
        for ticks in [1, 120, 400] {
            let world = run_world(&Options {
                script,
                ticks,
                ..Options::default()
            });
            assert!(
                plan_text(&world).contains('@'),
                "{script:?}/{ticks}: the player is not in the plan"
            );
            assert!(
                elevation_text(&world).contains('@'),
                "{script:?}/{ticks}: the player is not in the elevation"
            );
        }
    }
}

/// Both views name the slice they took, because a slice that does not say
/// where it cut is not readable against another one.
#[test]
fn both_views_say_where_they_cut() {
    let world = run_world(&Options {
        script: Script::Stand,
        ticks: 100,
        ..Options::default()
    });
    let at = Cell::containing(world.position());
    assert!(plan_text(&world).starts_with(&format!("plan, looking down, at y={}", at.y)));
    assert!(
        elevation_text(&world).starts_with(&format!("elevation, looking along z, at z={}", at.z))
    );
}

/// **The elevation puts height upward.** Drawn the other way it is still a
/// correct slice and an unreadable one, and the floor is what shows it: the
/// arena's solid floor must be at the bottom of the picture.
#[test]
fn the_elevation_puts_the_floor_at_the_bottom() {
    let world = run_world(&Options {
        script: Script::Stand,
        ticks: 100,
        ..Options::default()
    });
    let text = elevation_text(&world);
    let rows: Vec<&str> = text
        .lines()
        .skip(1)
        .take_while(|l| l.contains('.') || l.contains('#') || l.contains('@'))
        .collect();

    let solid_in = |row: &str| row.chars().skip(5).filter(|c| *c == '#').count();
    let first = rows.first().expect("rows");
    let last = rows.last().expect("rows");
    assert_eq!(
        solid_in(last),
        last.chars().skip(5).count(),
        "the bottom row is the floor, solid all the way across"
    );
    assert_eq!(
        solid_in(first),
        first.chars().skip(5).count(),
        "and the top row is the ceiling, since the box is closed"
    );
}

/// A picture is a pure function of the world it is given.
#[test]
fn the_views_are_reproducible() {
    let world = run_world(&Options {
        script: Script::Build,
        ticks: 250,
        ..Options::default()
    });
    assert_eq!(plan_text(&world), plan_text(&world));
    assert_eq!(elevation_text(&world), elevation_text(&world));

    let other = run_world(&Options {
        script: Script::Build,
        ticks: 400,
        ..Options::default()
    });
    assert_ne!(
        plan_text(&world),
        plan_text(&other),
        "two very different runs drew the same plan"
    );
}

/// **`run` and `run_world` answer about the same run.** They are separate
/// entry points now, and a report describing a different run than the picture
/// beside it would be worse than having no picture.
#[test]
fn the_report_and_the_world_agree() {
    let options = Options {
        script: Script::Build,
        ticks: 300,
        ..Options::default()
    };
    let report = run(&options);
    let world = run_world(&options);
    assert_eq!(report.digest, world.digest());
    assert_eq!(report.solids, world.grid().solid_count());
    assert_eq!(report.grounded, world.grounded());
    assert_eq!(report.ticks, world.tick());
}

/// The whole binary, drawing.
#[test]
fn the_command_line_draws() {
    assert_eq!(run_cli(args("--ticks 30 --show")), 0);
    assert_eq!(run_cli(args("--ticks 30 --show --json")), 0);
}
