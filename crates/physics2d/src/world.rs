//! The collider set: what is in the world, and every operation that changes it.

use crate::filter::Filter;
use crate::shape::{Shape, Transform};
use renew_ecs::Entity;

/// How a body moves, and what it collides with.
///
/// `Dynamic` is named and refused. v0 has no solver, so accepting a dynamic
/// body would mean accepting it and doing something surprising; naming it
/// keeps the word stable for the implementation that grows one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BodyKind {
    /// Never moves.
    Static,
    /// Moved by the caller, unaffected by contacts.
    Kinematic,
    /// Moved by the simulation. **Refused in v0.**
    Dynamic,
}

impl BodyKind {
    /// Whether two kinds may produce a contact at all.
    ///
    /// Static-versus-static is the only pair that cannot: neither ever moves,
    /// so a contact between them can never change, and reporting it every step
    /// is noise a caller has to filter out forever.
    #[must_use]
    pub const fn collides_with(self, other: Self) -> bool {
        !matches!((self, other), (Self::Static, Self::Static))
    }
}

/// A shape's position in its body's list.
///
/// **Stable for the life of the body.** Removing a shape leaves a hole rather
/// than renumbering, because this index is half of every collider identity,
/// part of the broadphase sort key, and the order overlaps resolve in — and
/// shift-down, swap-remove and tombstone each give a different contact order
/// for the same removal history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShapeIndex(u32);

impl ShapeIndex {
    /// The raw position.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// What identifies a collider: a body and one of its shapes.
///
/// Ordered lexicographically, handle first, and total because both components
/// are — which is what every emitted ordering in this crate rests on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Collider {
    /// The body.
    pub handle: Entity,
    /// Which of its shapes.
    pub index: ShapeIndex,
}

/// How many times a collider has been rebuilt at the same identity.
///
/// A contact identifier derived from a collider pair must not survive either
/// collider's destruction. Purity alone does not give that: a shape removed
/// and re-added takes the same index back by the lowest-free-hole rule, and a
/// body destroyed and recreated against a still-live entity takes the same
/// handle back. Both would make a rebuilt collider bit-identical to the one it
/// replaced, and a caller's one-shot impact sound would not fire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Incarnation(u32);

impl Incarnation {
    /// The raw count.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// One shape on a body.
#[derive(Clone, Copy, Debug)]
struct Slot {
    shape: Shape,
    local: Transform,
    filter: Filter,
}

/// One position in a body's shape list, occupied or not.
///
/// The incarnation lives on the *cell* rather than on the shape, so it
/// survives a removal — which is the point of it.
#[derive(Clone, Copy, Debug, Default)]
struct Cell {
    occupied: Option<Slot>,
    incarnation: u32,
}

/// One body and its shapes.
#[derive(Clone, Debug)]
struct Body {
    /// **The entity as issued, not its parts.** `Entity`'s constructor is
    /// crate-private to the ECS, so physics cannot mint or rebuild a handle:
    /// keeping the whole thing is the only way to hand one back. This is what
    /// makes body identity saved state rather than something derivable.
    entity: Entity,
    live: bool,
    kind: BodyKind,
    transform: Transform,
    /// Holes are unoccupied cells. Never compacted.
    shapes: Vec<Cell>,
    incarnation: u32,
}

/// What this world can say about a handle.
///
/// Named rather than folded into an `Option` because the cases are not equally
/// answerable, and pretending they are is how a specification comes to demand
/// an assertion on something undetectable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleState {
    /// A body exists here at exactly this generation.
    Live,
    /// This world knows the index at a different generation — the entity was
    /// despawned and its slot reused by the allocator.
    Stale,
    /// This world has no body for the handle, either because it never had one
    /// or because the body was destroyed.
    Unknown,
}

/// The bodies, their shapes, and nothing else.
///
/// Holds no global state, so a caller may hold several worlds and they do not
/// interact. Owns the transforms: a caller that also stores one for rendering
/// copies it out.
#[derive(Debug, Default)]
pub struct World {
    /// Indexed by entity index. The vector only grows, so iteration is in
    /// ascending index order — a function of the collider set rather than of
    /// insertion order.
    bodies: Vec<Option<Body>>,
    live: usize,
}

impl World {
    /// An empty world.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many bodies are live.
    #[must_use]
    pub const fn body_count(&self) -> usize {
        self.live
    }

    /// What this world can say about a handle.
    ///
    /// **There is no `Foreign` answer, and that is not an omission.** Two
    /// entity allocators hand out identical handles by construction — both
    /// start at index 0, generation 0 — so a handle from another world that
    /// collides with a live local body is indistinguishable from a correct
    /// one. It is a caller warrant, and a contract cannot require an
    /// implementation to detect what it cannot see.
    #[must_use]
    pub fn handle_state(&self, handle: Entity) -> HandleState {
        let Some(body) = self
            .bodies
            .get(handle.index() as usize)
            .and_then(Option::as_ref)
        else {
            return HandleState::Unknown;
        };
        if body.entity.generation() == handle.generation() {
            // A destroyed body leaves a tombstone at the same generation. The
            // world has no body for the handle either way, so it answers
            // Unknown rather than inventing a fourth case.
            if body.live {
                HandleState::Live
            } else {
                HandleState::Unknown
            }
        } else {
            HandleState::Stale
        }
    }

    fn body(&self, handle: Entity) -> Option<&Body> {
        self.bodies
            .get(handle.index() as usize)?
            .as_ref()
            .filter(|body| body.live && body.entity == handle)
    }

    fn body_mut(&mut self, handle: Entity) -> Option<&mut Body> {
        self.bodies
            .get_mut(handle.index() as usize)?
            .as_mut()
            .filter(|body| body.live && body.entity == handle)
    }

    // ---- structural mutation: between steps only ----

    /// Create a body against a caller-supplied entity.
    ///
    /// `None` if the entity already owns a live body here, or if the kind is
    /// `Dynamic`. **At most one body per entity** is what keeps collider order
    /// total: two bodies sharing a handle would compare equal on (handle,
    /// shape index), and every emitted ordering rests on that being impossible.
    pub fn create_body(
        &mut self,
        entity: Entity,
        kind: BodyKind,
        transform: Transform,
    ) -> Option<Entity> {
        if kind == BodyKind::Dynamic {
            return None;
        }
        let slot = entity.index() as usize;
        if self.bodies.len() <= slot {
            self.bodies.resize(slot + 1, None);
        }
        let entry = self.bodies.get_mut(slot)?;
        let carried = match entry.as_ref() {
            Some(body) if body.live && body.entity == entity => return None,
            Some(body) => body.incarnation,
            None => 0,
        };
        *entry = Some(Body {
            entity,
            live: true,
            kind,
            transform,
            shapes: Vec::new(),
            incarnation: carried.wrapping_add(1),
        });
        self.live = self.live.saturating_add(1);
        Some(entity)
    }

    /// Remove a body and all its shapes. `false` if the handle named none.
    ///
    /// The record is kept as a tombstone rather than dropped, because the
    /// incarnation has to outlive the body: a later body created against the
    /// same entity must not inherit its identity.
    pub fn destroy_body(&mut self, handle: Entity) -> bool {
        let Some(body) = self.body_mut(handle) else {
            return false;
        };
        body.live = false;
        body.shapes.clear();
        self.live = self.live.saturating_sub(1);
        true
    }

    /// Add a shape, filling the **lowest free hole**. `None` if the handle
    /// named no body, or the operands are not a shape.
    pub fn add_shape(
        &mut self,
        handle: Entity,
        shape: Shape,
        local: Transform,
        filter: Filter,
    ) -> Option<ShapeIndex> {
        if !shape.is_valid() {
            return None;
        }
        let body = self.body_mut(handle)?;
        // The lowest free hole, or a new cell at the end. Never a swap, never
        // a shift: the index is identity.
        let index = if let Some(hole) = body.shapes.iter().position(|cell| cell.occupied.is_none())
        {
            hole
        } else {
            body.shapes.push(Cell::default());
            body.shapes.len() - 1
        };
        let cell = body.shapes.get_mut(index)?;
        cell.occupied = Some(Slot {
            shape,
            local,
            filter,
        });
        cell.incarnation = cell.incarnation.wrapping_add(1);
        u32::try_from(index).ok().map(ShapeIndex)
    }

    /// Remove a shape, leaving a hole that keeps its index. `false` if there
    /// was nothing there.
    pub fn remove_shape(&mut self, handle: Entity, index: ShapeIndex) -> bool {
        let Some(body) = self.body_mut(handle) else {
            return false;
        };
        let Some(cell) = body.shapes.get_mut(index.get() as usize) else {
            return false;
        };
        if cell.occupied.is_none() {
            return false;
        }
        cell.occupied = None;
        true
    }

    /// Replace a shape in place, keeping its index and advancing its
    /// incarnation. `false` if there was nothing there.
    pub fn replace_shape(
        &mut self,
        handle: Entity,
        index: ShapeIndex,
        shape: Shape,
        local: Transform,
    ) -> bool {
        if !shape.is_valid() {
            return false;
        }
        let Some(body) = self.body_mut(handle) else {
            return false;
        };
        let Some(cell) = body.shapes.get_mut(index.get() as usize) else {
            return false;
        };
        let Some(slot) = cell.occupied.as_mut() else {
            return false;
        };
        slot.shape = shape;
        slot.local = local;
        cell.incarnation = cell.incarnation.wrapping_add(1);
        true
    }

    /// Change a shape's filter. `false` if there was nothing there.
    ///
    /// Does **not** advance the incarnation: the collider is the same one it
    /// was, and a contact that persists across a filter change is a persisting
    /// contact.
    pub fn set_filter(&mut self, handle: Entity, index: ShapeIndex, filter: Filter) -> bool {
        let Some(body) = self.body_mut(handle) else {
            return false;
        };
        let Some(cell) = body.shapes.get_mut(index.get() as usize) else {
            return false;
        };
        match cell.occupied.as_mut() {
            Some(slot) => {
                slot.filter = filter;
                true
            }
            None => false,
        }
    }

    // ---- movement: legal during the caller's phase, effective immediately ----

    /// Place a body. **Teleports; does not sweep.**
    ///
    /// A body moved this way passes through anything between where it was and
    /// where it lands. That is the caller's choice, and the swept operation is
    /// the other one — a placement that quietly swept would make the cheap
    /// operation expensive and leave the expensive one unavailable.
    pub fn set_transform(&mut self, handle: Entity, transform: Transform) -> bool {
        match self.body_mut(handle) {
            Some(body) => {
                body.transform = transform;
                true
            }
            None => false,
        }
    }

    // ---- reading ----

    /// A body's transform.
    #[must_use]
    pub fn transform(&self, handle: Entity) -> Option<Transform> {
        self.body(handle).map(|body| body.transform)
    }

    /// A body's kind.
    #[must_use]
    pub fn kind(&self, handle: Entity) -> Option<BodyKind> {
        self.body(handle).map(|body| body.kind)
    }

    /// One shape, in body-local terms.
    #[must_use]
    pub fn shape(&self, collider: Collider) -> Option<(Shape, Transform, Filter)> {
        let slot = self.cell(collider)?.occupied.as_ref()?;
        Some((slot.shape, slot.local, slot.filter))
    }

    /// A shape's world transform — its local placement composed onto its
    /// body's.
    #[must_use]
    pub fn world_transform(&self, collider: Collider) -> Option<Transform> {
        let body = self.body(collider.handle)?;
        let slot = body
            .shapes
            .get(collider.index.get() as usize)?
            .occupied
            .as_ref()?;
        Some(slot.local.compose(body.transform))
    }

    /// How many times this collider has been rebuilt at the same identity.
    #[must_use]
    pub fn incarnation(&self, collider: Collider) -> Option<Incarnation> {
        let body = self.body(collider.handle)?;
        let cell = body.shapes.get(collider.index.get() as usize)?;
        cell.occupied.as_ref()?;
        Some(Incarnation(
            body.incarnation
                .wrapping_mul(31)
                .wrapping_add(cell.incarnation),
        ))
    }

    fn cell(&self, collider: Collider) -> Option<&Cell> {
        self.body(collider.handle)?
            .shapes
            .get(collider.index.get() as usize)
    }

    /// The highest occupied index plus one — **not** the number of live
    /// shapes, because holes keep their place.
    #[must_use]
    pub fn shape_extent(&self, handle: Entity) -> Option<u32> {
        let shapes = &self.body(handle)?.shapes;
        let highest = shapes.iter().rposition(|cell| cell.occupied.is_some());
        Some(highest.map_or(0, |index| u32::try_from(index).unwrap_or(u32::MAX) + 1))
    }

    /// Every live collider, **in ascending (handle, shape index) order**.
    ///
    /// The order is part of the contract rather than an accident of the
    /// representation: it is a function of the collider set, so two worlds
    /// built by different insertion sequences iterate identically.
    pub fn colliders(&self) -> impl Iterator<Item = Collider> + '_ {
        self.bodies
            .iter()
            .filter_map(Option::as_ref)
            .filter(|body| body.live)
            .flat_map(|body| {
                let handle = body.entity;
                body.shapes
                    .iter()
                    .enumerate()
                    .filter_map(move |(position, cell)| {
                        cell.occupied.as_ref()?;
                        Some(Collider {
                            handle,
                            index: ShapeIndex(u32::try_from(position).unwrap_or(u32::MAX)),
                        })
                    })
            })
    }
}
