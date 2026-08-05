//! Drawing the level, and the ways a drawing can lie about the simulation.
//!
//! **None of these tests recomputes the overlap arithmetic the drawing uses.**
//! A test that re-derives the implementation's own reasoning agrees with it
//! wherever it is wrong, which is the failure mode this repository has already
//! paid for once. What is asserted here instead is what a *reader* of the
//! picture would conclude — where the character is, what it is touching, how
//! big it is — held against what the simulation independently reports.

use renew_sample_leap::{Options, Script, level, run, world_text};
use renew_sample_leap_world::Platform;

/// The picture as a grid of rows, with the coordinate gutter stripped.
fn rows(text: &str) -> Vec<Vec<char>> {
    text.lines()
        .filter(|line| line.contains('.') || line.contains('#') || line.contains('@'))
        .map(|line| line.chars().skip(5).collect())
        .collect()
}

/// Which rows and columns hold a given character.
fn cells(grid: &[Vec<char>], wanted: char) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    for (r, row) in grid.iter().enumerate() {
        for (c, drawn) in row.iter().enumerate() {
            if *drawn == wanted {
                found.push((r, c));
            }
        }
    }
    found
}

fn drawn(script: Script, ticks: u32) -> Vec<Vec<char>> {
    let report = run(&Options {
        script,
        ticks,
        ..Options::default()
    });
    rows(&world_text(&level(), report.position))
}

/// The view is a fixed rectangle, so two pictures line up column for column.
#[test]
fn the_view_is_a_fixed_rectangle() {
    let grid = drawn(Script::Stand, 200);
    assert_eq!(grid.len(), 17, "seventeen rows");
    for row in &grid {
        assert_eq!(row.len(), 61, "sixty-one columns");
    }
    assert!(
        grid.len() % 2 == 1 && grid[0].len() % 2 == 1,
        "odd on both axes, so the character has an exact middle to sit in"
    );
}

/// **The character is drawn as the size it is**, not as a single cell.
///
/// One wide and two tall are its half-extents doubled; a cell more in either
/// direction is the box straddling a cell boundary, which is ordinary. Three
/// wide or four tall would mean the drawing disagrees with the shape the
/// simulation collides with.
#[test]
fn the_character_is_drawn_at_its_own_size() {
    for (script, ticks) in [
        (Script::Stand, 200),
        (Script::Dash, 120),
        (Script::Hop, 300),
        (Script::Hop, 47),
    ] {
        let grid = drawn(script, ticks);
        let marks = cells(&grid, '@');
        assert!(!marks.is_empty(), "the character is always in view");

        let columns: Vec<usize> = marks.iter().map(|(_, c)| *c).collect();
        let rows_used: Vec<usize> = marks.iter().map(|(r, _)| *r).collect();
        let width = columns.iter().max().unwrap_or(&0) - columns.iter().min().unwrap_or(&0) + 1;
        let height =
            rows_used.iter().max().unwrap_or(&0) - rows_used.iter().min().unwrap_or(&0) + 1;

        assert!(
            (1..=2).contains(&width),
            "{script:?}/{ticks}: drawn {width} cells wide, but it is one unit wide"
        );
        assert!(
            (2..=3).contains(&height),
            "{script:?}/{ticks}: drawn {height} cells tall, but it is two units tall"
        );
        assert_eq!(
            marks.len(),
            width * height,
            "{script:?}/{ticks}: the character is a rectangle with no holes"
        );
    }
}

/// **A character the simulation calls grounded is drawn touching something.**
///
/// This is the assertion that catches an off-by-one in the drawing: if the
/// rows were shifted by one, a resting character would float over a gap of
/// air, and the picture would contradict the `grounded=true` printed beneath
/// it.
#[test]
fn a_grounded_character_is_drawn_standing_on_something() {
    for (script, ticks) in [(Script::Stand, 200), (Script::Dash, 120)] {
        let report = run(&Options {
            script,
            ticks,
            ..Options::default()
        });
        assert!(report.grounded, "{script:?} should end on the floor");

        let grid = rows(&world_text(&level(), report.position));
        let marks = cells(&grid, '@');
        let lowest = marks.iter().map(|(r, _)| *r).max().expect("in view");
        let columns: Vec<usize> = marks
            .iter()
            .filter(|(r, _)| *r == lowest)
            .map(|(_, c)| *c)
            .collect();

        // Rows run downward, so the row below the feet is the next index.
        let below = &grid[lowest + 1];
        assert!(
            columns.iter().any(|c| below[*c] == '#'),
            "{script:?}: nothing solid is drawn under a grounded character"
        );
    }
}

/// **A character the simulation calls wall-bound is drawn beside something.**
#[test]
fn a_walled_character_is_drawn_against_something() {
    let report = run(&Options {
        script: Script::Dash,
        ticks: 120,
        ..Options::default()
    });
    assert!(report.against_wall, "dashing ends against the wall");

    let grid = rows(&world_text(&level(), report.position));
    let marks = cells(&grid, '@');
    let rightmost = marks.iter().map(|(_, c)| *c).max().expect("in view");
    assert!(
        marks
            .iter()
            .filter(|(_, c)| *c == rightmost)
            .any(|(r, _)| grid[*r][rightmost + 1] == '#'),
        "nothing solid is drawn beside a character pressed against a wall"
    );
}

/// **The ledge on the negative side of the origin is drawn on the correct
/// side of it.**
///
/// This is the test for rounding toward negative infinity rather than toward
/// zero. Truncation folds everything in (-1, 1) onto the cell `0`, which
/// shifts every drawn thing left of the origin by one — invisible in a level
/// built entirely to the right, and this level has a ledge at x = -14.
#[test]
fn the_ledge_left_of_the_origin_is_drawn_where_it_is() {
    // Drawn from a viewpoint that puts the whole ledge in frame.
    let ledge = Platform::new(-14, 4, 3, 1);
    let text = world_text(
        &[ledge],
        renew_fixed::Vec2::new(
            renew_fixed::Fixed::from_int(-14),
            renew_fixed::Fixed::from_int(4),
        ),
    );
    let grid = rows(&text);
    let solid = cells(&grid, '#');
    assert!(!solid.is_empty(), "the ledge is in frame");

    // The view is centred on x = -14, so the ledge's cells sit around the
    // middle column. Its span is [-17, -11], which is six cells.
    let columns: Vec<usize> = solid.iter().map(|(_, c)| *c).collect();
    let left = *columns.iter().min().expect("in frame");
    let right = *columns.iter().max().expect("in frame");
    assert_eq!(right - left + 1, 6, "the ledge is six cells wide");
    assert!(
        left < 30 && right > 30,
        "the ledge straddles the centre column it is centred on, at {left}..={right}"
    );

    // And the same platform mirrored to the positive side must draw the same
    // shape — the whole point of rounding consistently.
    let mirrored = Platform::new(14, 4, 3, 1);
    let there = rows(&world_text(
        &[mirrored],
        renew_fixed::Vec2::new(
            renew_fixed::Fixed::from_int(14),
            renew_fixed::Fixed::from_int(4),
        ),
    ));
    assert_eq!(
        cells(&there, '#'),
        solid,
        "a platform drew differently on the two sides of the origin"
    );
}

/// The view follows the character, so it is never off the picture however far
/// it walks.
#[test]
fn the_character_is_always_in_frame() {
    for ticks in [0, 1, 37, 120, 240, 359, 600] {
        for script in [Script::Stand, Script::Dash, Script::Hop] {
            let grid = drawn(script, ticks);
            assert!(
                !cells(&grid, '@').is_empty(),
                "{script:?} at {ticks} ticks walked off its own picture"
            );
        }
    }
}

/// Drawing is a pure function of the position it is given.
///
/// **The picture is quantised to cells, so a single tick of movement may draw
/// identically and that is correct** — this asserted the opposite at first and
/// the failure was the test's, not the drawing's. What must differ is a
/// position that differs by more than a cell; what must not is the same
/// position drawn twice.
#[test]
fn the_picture_follows_the_position() {
    let at = |ticks| {
        world_text(
            &level(),
            run(&Options {
                script: Script::Hop,
                ticks,
                ..Options::default()
            })
            .position,
        )
    };

    assert_eq!(at(200), at(200), "the same position drew differently");
    assert_ne!(
        at(0),
        at(200),
        "two positions a long way apart drew the same picture"
    );
}

/// An empty level draws as empty air with the character in the middle of it.
#[test]
fn a_level_with_nothing_in_it_draws_the_character_alone() {
    let text = world_text(&[], renew_fixed::Vec2::ZERO);
    let grid = rows(&text);
    assert!(cells(&grid, '#').is_empty(), "nothing solid to draw");
    assert!(
        !cells(&grid, '@').is_empty(),
        "the character is still there"
    );
    assert!(text.contains("x=0 y=0"), "the coordinates are printed");
}

/// The coordinates are printed, because a view that follows the character
/// makes two pictures incomparable without them.
#[test]
fn the_view_says_where_it_is_looking() {
    let report = run(&Options {
        script: Script::Dash,
        ticks: 120,
        ..Options::default()
    });
    let text = world_text(&level(), report.position);
    assert!(
        text.lines().last().expect("a last line").contains("x="),
        "the last line names the coordinates"
    );
    // The gutter labels every row with its world y, so a reader can locate a
    // feature without counting rows.
    assert!(
        text.lines()
            .next()
            .expect("a first line")
            .trim()
            .starts_with(char::is_numeric)
            || text
                .lines()
                .next()
                .expect("a first line")
                .trim()
                .starts_with('-'),
        "each row is labelled with its world coordinate"
    );
}
