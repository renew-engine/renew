//! Which pairs are worth testing, in an order that is a function of the world.

use crate::bounds::Aabb;
use crate::filter::Filter;
use crate::world::{BodyKind, Collider, World};
use renew_fixed::Fixed;

/// One collider's contribution to the sweep.
#[derive(Clone, Copy, Debug)]
struct Record {
    collider: Collider,
    bounds: Aabb,
    kind: BodyKind,
    filter: Filter,
}

/// One end of one interval on the swept axis.
///
/// The `begin` flag is not decoration: a zero-extent shape — a zero-radius
/// sphere, which the query vocabulary requires to be answerable — has its two
/// endpoints at the same coordinate, and nothing else in the key separates
/// them. Without it their relative order is whatever the sort happens to do.
#[derive(Clone, Copy, Debug)]
struct Endpoint {
    coordinate: Fixed,
    begin: bool,
    record: usize,
}

/// Candidate pairs, and the storage that produces them.
///
/// Owned by the caller and reused: the vectors are cleared rather than
/// dropped, so a warm broadphase allocates nothing. The first rebuild after a
/// world grows is the only one that can.
#[derive(Debug, Default)]
pub struct Broadphase {
    records: Vec<Record>,
    endpoints: Vec<Endpoint>,
    active: Vec<usize>,
    pairs: Vec<(Collider, Collider)>,
}

impl Broadphase {
    /// Empty, with no storage yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild from the world as it currently stands.
    ///
    /// **One axis is swept and all three are tested.** Sweeping *x* prunes the
    /// pairs that cannot touch along it; the overlap test that follows still
    /// checks *y* and *z*, because separation along any single axis is
    /// separation. A three-dimensional broadphase that forgot the third would
    /// propose every pair in a column — correct, and useless.
    ///
    /// **Rebuilt rather than maintained**, and that is the determinism
    /// argument rather than a simplification. An incrementally-maintained
    /// structure carries the order of the mutations that built it, and two
    /// worlds that reached the same state by different routes would sweep
    /// differently. Rebuilding makes the result a function of the collider set
    /// and nothing else.
    pub fn rebuild(&mut self, world: &World, tolerance: Fixed) {
        self.records.clear();
        self.endpoints.clear();
        self.active.clear();
        self.pairs.clear();

        for collider in world.colliders() {
            // `colliders()` yields only live colliders, so each lookup below
            // succeeds by construction. Written as a `let ... else` that
            // panics rather than one that skips, because a skip here would
            // silently drop a collider out of the broadphase and the world
            // would quietly stop colliding — a defensive `continue` cannot be
            // reached and would hide the bug that reached it.
            let (shape, _, filter) = world
                .shape(collider)
                .unwrap_or_else(|| unreachable!("a live collider has a shape"));
            let transform = world
                .world_transform(collider)
                .unwrap_or_else(|| unreachable!("a live collider has a transform"));
            let kind = world
                .kind(collider.handle)
                .unwrap_or_else(|| unreachable!("a live collider has a body"));
            // Inflated by the contact tolerance, so a pair separated by less
            // than it still reaches narrowphase — a contact at depth zero is
            // one the vocabulary requires, and it cannot be generated from a
            // pair the broadphase never proposed.
            let bounds = shape.world_bounds(transform).expanded(tolerance);
            let record = self.records.len();
            self.records.push(Record {
                collider,
                bounds,
                kind,
                filter,
            });
            self.endpoints.push(Endpoint {
                coordinate: bounds.min.x,
                begin: true,
                record,
            });
            self.endpoints.push(Endpoint {
                coordinate: bounds.max.x,
                begin: false,
                record,
            });
        }

        // The key is total, which is the whole point: equal coordinates are
        // the common case in a tile-aligned world, not a corner one. Begins
        // sort before ends so that colliders which merely touch are proposed.
        self.endpoints.sort_unstable_by(|a, b| {
            let left = self.records.get(a.record).map(|r| r.collider);
            let right = self.records.get(b.record).map(|r| r.collider);
            a.coordinate
                .cmp(&b.coordinate)
                .then_with(|| b.begin.cmp(&a.begin))
                .then_with(|| left.cmp(&right))
        });

        // Four disjoint field borrows, which is why this is a free function
        // rather than a method: it lets the sweep read the records and write
        // the pairs without an index dance whose bounds checks could never
        // fail and could never be tested either.
        sweep(
            &self.records,
            &self.endpoints,
            &mut self.active,
            &mut self.pairs,
        );

        // **Emitted order is over the pair, not over the sweep.** Stating the
        // obligation this way is what lets a different structure — a grid, a
        // tree, three swept axes — produce the same output, and what stops the
        // swept axis leaking into a contact array.
        self.pairs.sort_unstable();
    }

    /// The candidate pairs, lower collider first, in ascending pair order.
    #[must_use]
    pub fn pairs(&self) -> &[(Collider, Collider)] {
        &self.pairs
    }

    /// How many colliders the last rebuild saw.
    #[must_use]
    pub fn collider_count(&self) -> usize {
        self.records.len()
    }
}

/// Walk the sorted endpoints, proposing a pair for every overlapping resident
/// as each interval opens.
fn sweep(
    records: &[Record],
    endpoints: &[Endpoint],
    active: &mut Vec<usize>,
    pairs: &mut Vec<(Collider, Collider)>,
) {
    for endpoint in endpoints {
        let entering = records[endpoint.record];
        if endpoint.begin {
            for &other in active.iter() {
                let resident = records[other];
                if eligible(entering, resident) && entering.bounds.overlaps(resident.bounds) {
                    // Canonical order, lower collider first, so a pair reads
                    // the same however the sweep happened to reach it.
                    pairs.push(if entering.collider < resident.collider {
                        (entering.collider, resident.collider)
                    } else {
                        (resident.collider, entering.collider)
                    });
                }
            }
            active.push(endpoint.record);
        } else if let Some(position) = active
            .iter()
            .position(|&candidate| candidate == endpoint.record)
        {
            // The active set is unordered — the pairs it produces are a set,
            // and they are sorted afterwards — so removing cheaply here costs
            // nothing observable.
            active.swap_remove(position);
        }
    }
}

/// Whether a pair is worth a narrowphase test at all.
///
/// Rejections here mean the pair **never existed** rather than existed and was
/// dropped — which is why the rule lives in one place: two implementations
/// disagreeing about it would report different totals for the same world.
fn eligible(a: Record, b: Record) -> bool {
    // A body does not collide with itself. Its shapes are parts of one object,
    // and an articulated thing whose parts must collide is several bodies.
    if a.collider.handle == b.collider.handle {
        return false;
    }
    a.kind.collides_with(b.kind) && a.filter.eligible(b.filter)
}
