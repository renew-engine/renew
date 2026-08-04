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
/// circle, which the query vocabulary requires to be answerable — has its two
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
            let (Some((shape, _, filter)), Some(transform), Some(kind)) = (
                world.shape(collider),
                world.world_transform(collider),
                world.kind(collider.handle),
            ) else {
                continue;
            };
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

        for slot in 0..self.endpoints.len() {
            let Some(&endpoint) = self.endpoints.get(slot) else {
                continue;
            };
            if endpoint.begin {
                self.propose(endpoint.record);
                self.active.push(endpoint.record);
            } else if let Some(position) = self
                .active
                .iter()
                .position(|&candidate| candidate == endpoint.record)
            {
                // The active set is unordered — the pairs it produces are a
                // set, and they are sorted below — so removing cheaply here
                // costs nothing that is observable.
                self.active.swap_remove(position);
            }
        }

        // **Emitted order is over the pair, not over the sweep.** Stating the
        // obligation this way is what lets a different structure — a grid, a
        // tree, three swept axes — produce the same output, and what stops the
        // swept axis leaking into a contact array.
        self.pairs.sort_unstable();
    }

    fn propose(&mut self, incoming: usize) {
        let Some(&entering) = self.records.get(incoming) else {
            return;
        };
        for slot in 0..self.active.len() {
            let Some(&other) = self.active.get(slot) else {
                continue;
            };
            let Some(&resident) = self.records.get(other) else {
                continue;
            };
            if !eligible(entering, resident) {
                continue;
            }
            if !entering.bounds.overlaps(resident.bounds) {
                continue;
            }
            let pair = if entering.collider < resident.collider {
                (entering.collider, resident.collider)
            } else {
                (resident.collider, entering.collider)
            };
            self.pairs.push(pair);
        }
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
