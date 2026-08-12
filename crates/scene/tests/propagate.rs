//! What the pass promises, one case per promise.
//!
//! Every test here builds its world through the real entity allocator rather
//! than handing `propagate` a table, because two of the promises — the handle
//! check and the resolution order — are *about* what that allocator does with
//! slots. A table would test the arithmetic and quietly skip the mechanics.

use renew_ecs::{Entities, Store};
use renew_fixed::{Angle, Fixed, Vec2};
use renew_scene::{Global, Local, Parent, Propagated, Scratch, propagate};

/// A world under construction, so each test spends its lines on its own case
/// rather than on four parallel stores.
#[derive(Default)]
struct World {
    entities: Entities,
    parents: Store<Parent>,
    locals: Store<Local>,
    globals: Store<Global>,
    scratch: Scratch,
}

impl World {
    fn new() -> Self {
        Self::default()
    }

    fn node(&mut self, local: Local) -> renew_ecs::Entity {
        let entity = self.entities.spawn();
        self.locals.insert(entity.index(), local);
        entity
    }

    fn attach(&mut self, child: renew_ecs::Entity, parent: renew_ecs::Entity) {
        self.parents.insert(child.index(), Parent(parent));
    }

    fn run(&mut self) -> Propagated {
        propagate(
            &mut self.scratch,
            &self.entities,
            &self.parents,
            &self.locals,
            &mut self.globals,
        )
    }

    /// Every scene node is placed — that is the totality promise, and a
    /// helper that softened it into a default would let the suite pass on the
    /// one failure it exists to catch.
    #[allow(clippy::expect_used, reason = "a missing placement is the defect")]
    fn placement(&self, entity: renew_ecs::Entity) -> Global {
        *self
            .globals
            .get(entity.index())
            .expect("every scene node is placed")
    }
}

fn at(x: i32, y: i32) -> Vec2 {
    Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
}

#[test]
fn a_node_with_no_parent_lands_exactly_where_its_local_says() {
    let mut world = World::new();
    let lone = world.node(Local::new(at(3, -4), Angle::QUARTER));

    let counts = world.run();

    assert_eq!(world.placement(lone).translation(), at(3, -4));
    assert_eq!(world.placement(lone).rotation(), Angle::QUARTER);
    assert_eq!(
        counts,
        Propagated {
            nodes: 1,
            roots: 1,
            orphaned: 0,
            cyclic: 0
        }
    );
}

/// The case that separates "compose" from "add": a child one unit along its
/// parent's x axis, with the parent turned a quarter turn, must swing round to
/// the parent's y axis rather than staying put.
#[test]
fn a_child_orbits_when_its_parent_turns() {
    let mut world = World::new();
    let parent = world.node(Local::new(at(10, 0), Angle::QUARTER));
    let child = world.node(Local::new(at(1, 0), Angle::ZERO));
    world.attach(child, parent);

    world.run();

    let placed = world.placement(child);
    assert_eq!(
        placed.translation(),
        at(10, 1),
        "the child swung with x -> y"
    );
    assert_eq!(
        placed.rotation(),
        Angle::QUARTER,
        "and inherited the turn itself"
    );
}

/// The formula, pinned by hand at a depth and with angles where the plausible
/// alternatives disagree.
///
/// Every other test here gives the child a zero rotation, and with that the
/// correct `local.translation.rotate(parent.rotation)` and the wrong
/// `local.translation.rotate(parent.rotation + local.rotation)` produce the
/// same answer. The numbers below are chosen so they do not, and each is
/// worked out on paper rather than read off a run:
///
/// * hub: no parent, so it sits at its own local — (10, 0), turned a quarter.
/// * arm: (2, 0) rotated by the hub's quarter turn is (0, 2), so (10, 2);
///   turned a quarter more, so a half turn in total. The wrong convention
///   would rotate by a half turn and give (8, 0).
/// * tip: (1, 0) rotated by the arm's half turn is (-1, 0), so (9, 2).
#[test]
fn the_composition_formula_is_pinned_by_hand() {
    let mut world = World::new();
    let hub = world.node(Local::new(at(10, 0), Angle::from_degrees(90)));
    let arm = world.node(Local::new(at(2, 0), Angle::from_degrees(90)));
    world.attach(arm, hub);
    let tip = world.node(Local::new(at(1, 0), Angle::ZERO));
    world.attach(tip, arm);

    world.run();

    assert_eq!(world.placement(hub).translation(), at(10, 0));
    assert_eq!(world.placement(hub).rotation(), Angle::from_degrees(90));
    assert_eq!(world.placement(arm).translation(), at(10, 2));
    assert_eq!(world.placement(arm).rotation(), Angle::from_degrees(180));
    assert_eq!(world.placement(tip).translation(), at(9, 2));
    assert_eq!(world.placement(tip).rotation(), Angle::from_degrees(180));
}

/// Rotation composes by wrapping integer addition, so a chain of any depth
/// lands on exactly the angle one multiplication would give — no drift, no
/// epsilon, nothing that grows with depth.
///
/// The step is deliberately one that a turn does *not* divide evenly. Its own
/// representation error is real and is the input's; the point is that the
/// chain adds nothing to it.
#[test]
fn a_chain_of_rotations_adds_exactly_and_drifts_not_at_all() {
    let mut world = World::new();
    let step = Angle::from_degrees(3);
    let mut previous = world.node(Local::new(Vec2::ZERO, step));
    for _ in 0..119 {
        let next = world.node(Local::new(Vec2::ZERO, step));
        world.attach(next, previous);
        previous = next;
    }

    world.run();

    let summed = Angle::from_bits(step.to_bits().wrapping_mul(120));
    assert_eq!(world.placement(previous).rotation(), summed);

    // And a step a turn *does* divide evenly comes back to exactly zero, so
    // the assertion above is not passing on two matching wrong answers.
    let mut world = World::new();
    let quarter = Angle::from_degrees(90);
    let mut previous = world.node(Local::new(Vec2::ZERO, quarter));
    for _ in 0..3 {
        let next = world.node(Local::new(Vec2::ZERO, quarter));
        world.attach(next, previous);
        previous = next;
    }
    world.run();
    assert_eq!(world.placement(previous).rotation(), Angle::ZERO);
}

/// The mechanic the crate docs single out. The entity allocator hands out
/// recycled slots newest-first, so a child can hold a lower slot than its
/// parent; a pass that walked slots in order would compose this child against
/// the placement its parent held on the *previous* tick.
///
/// The premise is asserted, not assumed: if a future allocator change stops
/// producing the inversion, this test fails loudly instead of passing on a
/// world that no longer contains the case it was written for.
#[test]
fn slot_order_does_not_decide() {
    let mut world = World::new();

    // Burn three slots and free them. The free list now hands them back in
    // an order that lets the child land below the parent.
    let scratch: Vec<_> = (0..3).map(|_| world.entities.spawn()).collect();
    for entity in scratch {
        world.entities.despawn(entity);
    }

    let parent = world.node(Local::new(at(100, 0), Angle::ZERO));
    let child = world.node(Local::new(at(5, 0), Angle::ZERO));
    world.attach(child, parent);

    assert!(
        child.index() < parent.index(),
        "premise: the child must hold the lower slot, or this test proves nothing \
         (child {}, parent {})",
        child.index(),
        parent.index()
    );

    world.run();

    assert_eq!(
        world.placement(child).translation(),
        at(105, 0),
        "the child composed against its parent's placement from this tick"
    );

    // And again on a second tick, because the failure mode this guards is
    // *stale* data — a slot-order pass gets the first tick wrong in the same
    // direction every time, so one tick alone could not tell the two apart.
    world.run();
    assert_eq!(world.placement(child).translation(), at(105, 0));
}

/// A despawned parent must not keep placing its children, and — the sharper
/// half — a *new* entity moving into the dead parent's slot must not inherit
/// them.
#[test]
fn a_dead_parent_leaves_an_orphan_and_never_a_stand_in() {
    let mut world = World::new();
    let parent = world.node(Local::new(at(100, 0), Angle::ZERO));
    let child = world.node(Local::new(at(5, 0), Angle::ZERO));
    world.attach(child, parent);
    world.run();
    assert_eq!(world.placement(child).translation(), at(105, 0));

    world.entities.despawn(parent);
    let counts = world.run();
    assert_eq!(
        world.placement(child).translation(),
        at(5, 0),
        "the child fell back to the world"
    );
    assert_eq!(counts.orphaned, 1);
    assert_eq!(
        counts.roots, 0,
        "an orphan is not a root, and is counted apart"
    );

    // Somebody else takes the freed slot. The stale handle names that slot;
    // only the generation stops it being obeyed.
    let stand_in = world.node(Local::new(at(-70, 0), Angle::ZERO));
    assert_eq!(
        stand_in.index(),
        parent.index(),
        "premise: the slot must really be reused, or this proves nothing"
    );
    let counts = world.run();
    assert_eq!(
        world.placement(child).translation(),
        at(5, 0),
        "the child did not adopt the slot's new tenant"
    );
    assert_eq!(counts.orphaned, 1);
}

/// A parent that is alive but is not a scene node has no placement to compose
/// against, which is the same outcome by a different route.
#[test]
fn a_parent_that_is_not_a_scene_node_orphans_its_child() {
    let mut world = World::new();
    let bare = world.entities.spawn();
    let child = world.node(Local::new(at(5, 0), Angle::ZERO));
    world.attach(child, bare);

    let counts = world.run();

    assert_eq!(world.placement(child).translation(), at(5, 0));
    assert_eq!(counts.orphaned, 1);
    assert_eq!(
        counts.nodes, 1,
        "the bare entity is not a node and is not placed"
    );
}

#[test]
fn a_cycle_terminates_and_is_counted_rather_than_hanging() {
    let mut world = World::new();
    let first = world.node(Local::new(at(1, 0), Angle::ZERO));
    let second = world.node(Local::new(at(2, 0), Angle::ZERO));
    let third = world.node(Local::new(at(4, 0), Angle::ZERO));
    world.attach(first, third);
    world.attach(second, first);
    world.attach(third, second);

    let counts = world.run();

    assert_eq!(counts.nodes, 3, "totality holds even inside a loop");
    assert_eq!(counts.cyclic, 1, "the loop is cut in exactly one place");
    assert_eq!(counts.roots, 0);

    // The cut lands on the member the climb reaches last — the one whose own
    // parent is the node the climb started from. Seeding at `first` walks
    // first -> third -> second, so `second` is cut and the rest compose down
    // from it: 2, then 2+4, then 2+4+1.
    assert_eq!(world.placement(second).translation(), at(2, 0));
    assert_eq!(world.placement(third).translation(), at(6, 0));
    assert_eq!(world.placement(first).translation(), at(7, 0));
}

#[test]
fn a_node_parented_to_itself_is_a_loop_of_one_not_an_orphan() {
    let mut world = World::new();
    let ouroboros = world.node(Local::new(at(9, 0), Angle::ZERO));
    world.attach(ouroboros, ouroboros);

    let counts = world.run();

    assert_eq!(counts.cyclic, 1);
    assert_eq!(counts.orphaned, 0);
    assert_eq!(world.placement(ouroboros).translation(), at(9, 0));
}

/// Totality, stated as one assertion over a world holding every category at
/// once — the arrangement a caller actually hands over.
#[test]
fn every_local_gets_a_global_whatever_else_is_wrong() {
    let mut world = World::new();
    let root = world.node(Local::new(at(1, 1), Angle::ZERO));
    let child = world.node(Local::new(at(1, 0), Angle::ZERO));
    world.attach(child, root);
    let doomed = world.node(Local::new(at(2, 2), Angle::ZERO));
    let orphan = world.node(Local::new(at(3, 3), Angle::ZERO));
    world.attach(orphan, doomed);
    world.entities.despawn(doomed);
    let looped = world.node(Local::new(at(4, 4), Angle::ZERO));
    world.attach(looped, looped);

    let counts = world.run();

    for entity in [root, child, orphan, looped] {
        assert!(
            world.globals.get(entity.index()).is_some(),
            "slot {} went unplaced",
            entity.index()
        );
    }
    assert_eq!(counts.nodes, 4);
    // Three of the four composed against the world origin, one for each way of
    // having no usable parent — which is not the same as landing there, since
    // each still sits where its own local puts it. `child` composed against a
    // real parent and so appears in none of the three.
    assert_eq!(counts.roots, 1);
    assert_eq!(counts.orphaned, 1);
    assert_eq!(counts.cyclic, 1);
}

/// The scratch buffer is capacity, not state: a second call on an unchanged
/// world must produce an identical result, and so must a call on a scratch
/// that has just serviced a completely different world.
#[test]
fn the_scratch_carries_capacity_and_never_answers() {
    let mut used = World::new();
    let deep = used.node(Local::new(at(1, 0), Angle::from_degrees(30)));
    let deeper = used.node(Local::new(at(1, 0), Angle::from_degrees(30)));
    used.attach(deeper, deep);
    used.run();

    let mut fresh = World::new();
    let lone = fresh.node(Local::new(at(7, 8), Angle::from_degrees(45)));
    let first = propagate(
        &mut fresh.scratch,
        &fresh.entities,
        &fresh.parents,
        &fresh.locals,
        &mut fresh.globals,
    );
    let expected = fresh.placement(lone);

    // Same world, but through the scratch that just walked a two-deep chain.
    let second = propagate(
        &mut used.scratch,
        &fresh.entities,
        &fresh.parents,
        &fresh.locals,
        &mut fresh.globals,
    );

    assert_eq!(first, second);
    assert_eq!(fresh.placement(lone), expected);
}

/// Composition saturates rather than wrapping, and says so through the
/// counter — silence would be a placement that reads plausible and is not.
#[test]
fn overflow_saturates_and_is_counted() {
    let mut world = World::new();
    let huge = Vec2::new(Fixed::MAX, Fixed::ZERO);
    let base = world.node(Local::new(huge, Angle::ZERO));
    let further = world.node(Local::new(huge, Angle::ZERO));
    world.attach(further, base);

    let before = renew_fixed::saturations();
    world.run();
    let after = renew_fixed::saturations();

    assert!(
        after.0 > before.0,
        "the overflow must be reported, not absorbed"
    );
    assert_eq!(
        world.placement(further).translation().x,
        Fixed::MAX,
        "and clamped rather than wrapped to a negative"
    );
}

/// An empty world is a legal world.
#[test]
fn nothing_in_produces_nothing_out() {
    let mut world = World::new();
    assert_eq!(world.run(), Propagated::default());
}
