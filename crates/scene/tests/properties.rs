//! The two claims that examples cannot hold: the pass agrees with an
//! independent composition of the same hierarchy, and it does not care what
//! slots that hierarchy happens to occupy.

use proptest::prelude::*;
use renew_ecs::{Entities, Entity, Store};
use renew_fixed::{Angle, Fixed, Vec2};
use renew_scene::{Global, Local, Parent, Propagated, Scratch, propagate};

/// A hierarchy as a plain table: node `n`'s parent is `parents[n]`, always an
/// earlier index or none.
///
/// The ordering constraint is what makes the shape a forest by construction —
/// the strategy cannot emit a loop — and it is also what lets the reference
/// below compose in one forward pass. `propagate` gets no such help: the table
/// is laid into entity slots in an order chosen to break the correspondence.
#[derive(Clone, Debug)]
struct Shape {
    parents: Vec<Option<usize>>,
    locals: Vec<Local>,
}

fn shapes(max_nodes: usize) -> impl Strategy<Value = Shape> {
    prop::collection::vec(
        (
            // 0 means "no parent"; otherwise an offset back up the table, so
            // deep chains and wide fans both occur.
            0usize..8,
            -1000i32..1000,
            -1000i32..1000,
            any::<u32>(),
        ),
        1..max_nodes,
    )
    .prop_map(|rows| {
        let mut parents = Vec::with_capacity(rows.len());
        let mut locals = Vec::with_capacity(rows.len());
        for (index, (back, x, y, turn)) in rows.into_iter().enumerate() {
            parents.push(if back == 0 || back > index {
                None
            } else {
                Some(index - back)
            });
            locals.push(Local::new(
                Vec2::new(Fixed::from_int(x), Fixed::from_int(y)),
                Angle::from_bits(turn),
            ));
        }
        Shape { parents, locals }
    })
}

/// Composition done the one way `propagate` is forbidden to do it: straight
/// down the table, relying on every parent preceding its child.
///
/// This shares no code with the implementation — not the traversal, not the
/// marking, not the buffers. Only the arithmetic is common, and it has to be:
/// two different formulas would test that they disagree.
fn reference(shape: &Shape) -> Vec<(Vec2, Angle)> {
    let mut placed: Vec<(Vec2, Angle)> = Vec::with_capacity(shape.parents.len());
    for (index, parent) in shape.parents.iter().enumerate() {
        let (base_translation, base_rotation) = parent
            .and_then(|parent| placed.get(parent).copied())
            .unwrap_or((Vec2::ZERO, Angle::ZERO));
        let local = shape.locals[index];
        placed.push((
            base_translation + local.translation.rotate(base_rotation),
            base_rotation + local.rotation,
        ));
    }
    placed
}

/// A world holding `shape`, with table index `n` living in whatever slot the
/// entity allocator gave it.
struct Laid {
    entities: Vec<Entity>,
    globals: Store<Global>,
    counts: Propagated,
}

/// Build the world, spawning in `order` so the caller decides which slots the
/// nodes land in.
///
/// `order` is a permutation of the table indices: it is the order the entities
/// are *created* in, which is what fixes the slot each node gets.
fn lay_out(shape: &Shape, order: &[usize]) -> Laid {
    let mut entities = Entities::new();
    // Spawn in creation order, then transpose: `created[position]` is the
    // entity handed to table index `order[position]`. Built this way there is
    // no absent case, so there is none to unwrap.
    let created: Vec<Entity> = order.iter().map(|_| entities.spawn()).collect();
    let mut handles = created.clone();
    for (position, &index) in order.iter().enumerate() {
        handles[index] = created[position];
    }

    let mut parents: Store<Parent> = Store::default();
    let mut locals: Store<Local> = Store::default();
    let mut globals: Store<Global> = Store::default();
    for (index, handle) in handles.iter().enumerate() {
        locals.insert(handle.index(), shape.locals[index]);
        if let Some(parent) = shape.parents[index] {
            parents.insert(handle.index(), Parent(handles[parent]));
        }
    }

    let mut scratch = Scratch::new();
    let counts = propagate(&mut scratch, &entities, &parents, &locals, &mut globals);
    assert_eq!(
        counts.nodes as usize,
        shape.parents.len(),
        "totality: every node in the table must be placed"
    );
    assert_eq!(counts.cyclic, 0, "the strategy cannot emit a loop");
    assert_eq!(counts.orphaned, 0, "nothing was despawned");

    Laid {
        entities: handles,
        globals,
        counts,
    }
}

/// Totality again: an unplaced node is the defect, not a case to smooth over.
#[allow(clippy::expect_used, reason = "a missing placement is the defect")]
fn placement(laid: &Laid, index: usize) -> Global {
    *laid
        .globals
        .get(laid.entities[index].index())
        .expect("placed")
}

proptest! {
    /// The pass composes the same hierarchy the same way an independent
    /// top-down pass does, whatever shape it is.
    #[test]
    fn the_pass_agrees_with_an_independent_composition(shape in shapes(40)) {
        let expected = reference(&shape);
        let order: Vec<usize> = (0..shape.parents.len()).collect();
        let laid = lay_out(&shape, &order);

        for (index, &(translation, rotation)) in expected.iter().enumerate() {
            let placed = placement(&laid, index);
            prop_assert_eq!(placed.translation(), translation, "node {}", index);
            prop_assert_eq!(placed.rotation(), rotation, "node {}", index);
        }
    }

    /// Relabelling. The same shape laid into a different set of slots must
    /// produce identical placements, bit for bit — the property that says the
    /// answer comes from the hierarchy and not from the allocator.
    ///
    /// The permuted layout is also held against the independent reference.
    /// Comparing the two runs only to *each other* would pass an implementation
    /// that was wrong the same way in every layout, and the identity layout —
    /// the one the other property test uses — is the one where the climb is
    /// shallowest.
    ///
    /// Creating the nodes in a permuted order is what makes the slots differ,
    /// and it routinely puts children below their parents, which is exactly
    /// the arrangement a slot-order pass gets wrong.
    #[test]
    fn placements_survive_relabelling(shape in shapes(40), seed in any::<u64>()) {
        let count = shape.parents.len();
        let straight: Vec<usize> = (0..count).collect();

        // A permutation from the seed, so the shrinker can reproduce it.
        let mut shuffled = straight.clone();
        let mut state = seed | 1;
        for index in (1..count).rev() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let span = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
            let pick = usize::try_from(state % span).unwrap_or(0);
            shuffled.swap(index, pick);
        }
        // A shuffle is allowed to come out as the identity, and when it does
        // the two layouts are the same world and the comparison below is
        // vacuous. Nudge it rather than skipping the case: a skip would leave
        // the run reporting successes it never made.
        if shuffled == straight && count > 1 {
            shuffled.swap(0, 1);
        }

        // The arrangement this test exists for is a child in a *lower* slot
        // than its parent. The identity layout can never contain one — the
        // strategy only ever names an earlier table index as a parent — so it
        // has to come from the permutation, and a random permutation need not
        // provide it. Reversing the creation order inverts every edge at once,
        // which is the strongest form of the case rather than a weakening.
        let edges = shape.parents.iter().filter(|p| p.is_some()).count();
        let inverts = |order: &[usize]| {
            let mut slots = vec![0usize; count];
            for (position, &index) in order.iter().enumerate() {
                slots[index] = position;
            }
            (0..count).any(|n| shape.parents[n].is_some_and(|p| slots[n] < slots[p]))
        };
        if edges > 0 && !inverts(&shuffled) {
            shuffled = straight.iter().rev().copied().collect();
        }

        let first = lay_out(&shape, &straight);
        let second = lay_out(&shape, &shuffled);

        // The premise: unless the two layouts really differ, this proves
        // nothing. A one-node shape cannot differ, so it is excused by name.
        let differs = (0..count).any(|n| first.entities[n].index() != second.entities[n].index());
        prop_assert!(
            differs || count == 1,
            "premise: the permutation must actually move something"
        );

        // Checked against the world that was built, not the order it was built
        // from: a child really did end up below its parent.
        if edges > 0 {
            prop_assert!(
                (0..count).any(|n| {
                    shape.parents[n]
                        .is_some_and(|p| second.entities[n].index() < second.entities[p].index())
                }),
                "premise: no child ended up below its parent, so resolution order went untested"
            );
        }

        let expected = reference(&shape);
        for (index, &(translation, rotation)) in expected.iter().enumerate() {
            let a = placement(&first, index);
            let b = placement(&second, index);
            prop_assert_eq!(a.translation(), b.translation(), "node {}", index);
            prop_assert_eq!(a.rotation(), b.rotation(), "node {}", index);
            prop_assert_eq!(b.translation(), translation, "node {} vs reference", index);
            prop_assert_eq!(b.rotation(), rotation, "node {} vs reference", index);
        }
    }

    /// The three counts are documented as the number of independent trees the
    /// pass found. For a forest with no loops and nothing despawned that is
    /// exactly the number of nodes with no parent — asserted here because the
    /// claim lives in the public docs and was held by nothing.
    #[test]
    fn the_counts_report_how_many_independent_trees_there_were(shape in shapes(40)) {
        let order: Vec<usize> = (0..shape.parents.len()).collect();
        let laid = lay_out(&shape, &order);
        let rootless = shape.parents.iter().filter(|p| p.is_none()).count();
        prop_assert_eq!(laid.counts.roots as usize, rootless);
        prop_assert_eq!(laid.counts.orphaned, 0);
        prop_assert_eq!(laid.counts.cyclic, 0);
        prop_assert_eq!(laid.counts.nodes as usize, shape.parents.len());
    }

    /// Running twice changes nothing: the pass is a function of the world, and
    /// the globals it wrote last time are not an input to it.
    #[test]
    fn a_second_pass_over_an_unchanged_world_changes_nothing(shape in shapes(24)) {
        let mut entities = Entities::new();
        let handles: Vec<Entity> = (0..shape.parents.len()).map(|_| entities.spawn()).collect();
        let mut parents: Store<Parent> = Store::default();
        let mut locals: Store<Local> = Store::default();
        let mut globals: Store<Global> = Store::default();
        for (index, handle) in handles.iter().enumerate() {
            locals.insert(handle.index(), shape.locals[index]);
            if let Some(parent) = shape.parents[index] {
                parents.insert(handle.index(), Parent(handles[parent]));
            }
        }

        let mut scratch = Scratch::new();
        let first = propagate(&mut scratch, &entities, &parents, &locals, &mut globals);
        let after_first: Vec<Global> = handles
            .iter()
            .map(|h| *globals.get(h.index()).expect("placed"))
            .collect();
        let second = propagate(&mut scratch, &entities, &parents, &locals, &mut globals);
        let after_second: Vec<Global> = handles
            .iter()
            .map(|h| *globals.get(h.index()).expect("placed"))
            .collect();

        prop_assert_eq!(first, second);
        prop_assert_eq!(after_first, after_second);
    }
}
