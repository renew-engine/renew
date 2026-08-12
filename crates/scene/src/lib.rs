//! Parenting: local placements composed into world placements.
//!
//! A scene node is an entity carrying a [`Local`] — where it sits relative to
//! whatever it is attached to. Attachment is a second component, [`Parent`],
//! holding a whole [`Entity`] handle. [`propagate`] reads those two stores and
//! fills a third with [`Global`], the world placement, resolving every parent
//! before its children.
//!
//! # Contract
//!
//! * **Deterministic.** For a hierarchy without loops, the output depends on
//!   the *shape* of that hierarchy and on nothing else — not on slot numbers,
//!   not on insertion history, not on how many times [`propagate`] has run.
//!   Two worlds built with different slot assignments but the same shape
//!   produce the same placements, bit for bit; the relabelling property test
//!   holds this.
//!
//!   **A loop is the one exception, and it is exact rather than hedged.** Every
//!   node still gets a placement and the count still reports the loop. The cut
//!   falls on the loop member the climb reaches **last** — the one whose own
//!   parent is the member the climb entered the loop by — and *which* member
//!   that is follows from which node the pass seeded from, which is entity
//!   order. Relabel the world and a different member may be cut, so the
//!   placements inside a loop can differ between two worlds of the same shape.
//!   No ordering fixes this — a loop has no member a hierarchy can call first —
//!   so a caller who needs reproducible placements must not build one, and
//!   [`Propagated::cyclic`] is how they find out they did.
//! * **Total.** Every *live* entity with a [`Local`] gets a [`Global`],
//!   including nodes whose parent died, nodes whose parent is not a scene node,
//!   and nodes inside a cycle. There is no live input for which a node is
//!   silently skipped; [`Propagated`] counts each category so a caller can
//!   notice. A **despawned** entity is not walked at all — see *Stale globals*.
//! * **Exact rotation.** Angles compose by wrapping addition on a binary-angle
//!   integer, so a chain of rotations neither drifts nor accumulates error, at
//!   any depth. Translation composes in fixed point and saturates rather than
//!   wrapping; `renew_fixed::saturations()` counts it when it happens.
//! * **Globals are derived, never authored.** [`Global`] has no public fields,
//!   and no public way to build one *from a placement* — no constructor taking
//!   a translation and a rotation, and no [`Default`]. The only values that
//!   exist are the ones [`propagate`] derived and [`Global::IDENTITY`], the
//!   world origin. That is what makes "a world placement is a function of the
//!   hierarchy" a property of the type rather than a habit a caller can forget.
//!
//! # Two mechanics that look like details and are not
//!
//! **A parent is a whole handle, not a slot.** [`Parent`] stores [`Entity`],
//! generation included, and [`propagate`] checks it against [`Entities`]. Slots
//! are recycled; a bare index would silently re-attach an orphan to whatever
//! moved in next, which is a bug that reproduces perfectly and looks like a
//! physics glitch.
//!
//! The check is against the [`Entities`] handed to [`propagate`], and generations
//! are only unique within one allocator. A handle minted by a *different*
//! `Entities` whose slot and generation both happen to match will be obeyed.
//! Nothing in the engine hands out entities from two allocators into one world,
//! and this crate does not defend against it.
//!
//! **Resolution order is not slot order.** `Entities::spawn` pops its free list
//! newest-first, so a child can hold a *lower* slot than its parent. A single
//! ascending pass would then compose that child against its parent's
//! *previous-tick* placement — one frame of lag, on some entities, depending on
//! spawn history. It is invisible to a determinism test, because it is
//! perfectly reproducible. [`propagate`] therefore walks each node's ancestry
//! upward first and composes on the way back down. `slot_order_does_not_decide`
//! is the regression test.
//!
//! # Stale globals
//!
//! Despawning an entity does not remove its components — that is true of every
//! store in this engine and scene is not special. A [`Global`] left behind by a
//! despawned node is never *read* as anybody's parent, because the handle check
//! rejects it first; it is only occupying a slot until the caller removes it.

// The determinism rule a simulation crate lives under: no floating-point
// arithmetic whose result can reach digested state. A placement composed with
// a float would be reproducible on one machine and not across a fleet, which
// is the failure the whole fixed-point stack exists to refuse. Denied here
// rather than left to review — the lint covers operators only, so it is
// necessary and not sufficient, but what it covers it covers with teeth.
#![deny(
    clippy::float_arithmetic,
    clippy::print_stdout,
    clippy::print_stderr,
    missing_docs
)]

use renew_ecs::{Entities, Entity, Store};
use renew_fixed::{Angle, Vec2};

/// Where a node sits relative to its parent, or relative to the world when it
/// has none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Local {
    /// Offset from the parent's origin, in the parent's rotated frame.
    pub translation: Vec2,
    /// Rotation relative to the parent's.
    pub rotation: Angle,
}

impl Local {
    /// A placement built from its parts.
    #[must_use]
    pub const fn new(translation: Vec2, rotation: Angle) -> Self {
        Self {
            translation,
            rotation,
        }
    }
}

/// What a node is attached to.
///
/// The whole handle, generation included — see the crate docs for why a slot
/// would be a defect rather than an optimisation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parent(pub Entity);

/// Where a node sits in the world.
///
/// Derived, never authored: no public fields, and no way to build one from a
/// translation and a rotation. [`Global::IDENTITY`] is the single exception and
/// names one value, the world origin — you can say "nowhere in particular", but
/// you cannot say where something is. Everything else exists because
/// [`propagate`] wrote it.
///
/// Deliberately not [`Default`]: a derived `Default` is a public constructor,
/// and one silently reachable through every `unwrap_or_default` in every
/// consumer, which is exactly how "derived, never authored" would stop being
/// true without anybody deciding it should.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Global {
    translation: Vec2,
    rotation: Angle,
}

impl Global {
    /// The world origin, facing along the x axis.
    ///
    /// What a node with no usable parent composes against.
    pub const IDENTITY: Self = Self {
        translation: Vec2::ZERO,
        rotation: Angle::ZERO,
    };

    /// Position in world space.
    #[must_use]
    pub const fn translation(self) -> Vec2 {
        self.translation
    }

    /// Orientation in world space.
    #[must_use]
    pub const fn rotation(self) -> Angle {
        self.rotation
    }

    /// A child's world placement, given its parent's.
    ///
    /// Rotation adds, wrapping and exact. Translation rotates into the
    /// parent's frame before it adds, which is what makes a child orbit when
    /// its parent turns instead of sliding.
    fn compose(self, local: Local) -> Self {
        Self {
            translation: self.translation + local.translation.rotate(self.rotation),
            rotation: self.rotation + local.rotation,
        }
    }
}

/// What a single [`propagate`] call did.
///
/// The three failure counts are diagnostics, not errors: every node in them
/// still received a [`Global`]. They exist so a caller can assert `orphaned +
/// cyclic == 0` in a test and find out at the point of the mistake rather than
/// three frames later when something is drawn in the wrong place.
///
/// They count nodes composed **against the world origin** rather than against a
/// parent — which is not the same as landing there, since such a node still
/// sits wherever its own [`Local`] puts it. So they are not a partition of
/// `nodes` and their sum is not it: an ordinary child composes against a parent
/// that resolved, and belongs to none of the three. `roots + orphaned + cyclic`
/// is the number of independent trees the pass found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Propagated {
    /// Nodes given a world placement — every entity with a [`Local`].
    pub nodes: u32,
    /// Nodes with no [`Parent`] component, composed against the world.
    pub roots: u32,
    /// Nodes whose [`Parent`] names a despawned entity, or one that is not a
    /// scene node. Composed as if they had no parent.
    pub orphaned: u32,
    /// Nodes whose parent chain closes a loop. One member of each loop is
    /// composed as if it had no parent, so the rest can compose against it and
    /// the pass terminates. See the crate docs: *which* member that is
    /// depends on entity order, and it is the single case where two worlds of
    /// the same shape can disagree.
    pub cyclic: u32,
}

/// Reusable buffers for [`propagate`].
///
/// Capacity, not state: two calls with the same world produce the same result
/// whatever this held beforehand. Owned by the caller so the pass can promise
/// no steady-state allocation — hand it the same one every tick and it stops
/// growing once the world stops.
#[derive(Clone, Debug, Default)]
pub struct Scratch {
    marks: Vec<Mark>,
    ancestry: Vec<u32>,
}

impl Scratch {
    /// Empty. The first [`propagate`] sizes it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sized up front, for a caller that would rather not allocate on the
    /// first tick.
    #[must_use]
    pub fn with_capacity(entities: usize) -> Self {
        Self {
            marks: Vec::with_capacity(entities),
            ancestry: Vec::with_capacity(entities),
        }
    }
}

/// How far a slot has got in the current call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mark {
    /// Not reached yet.
    #[default]
    Untouched,
    /// On the path currently being climbed. Reaching one of these again is
    /// what a cycle looks like from the inside.
    Climbing,
    /// Has its [`Global`].
    Placed,
}

/// Compose every [`Local`] into a [`Global`], parents first.
///
/// Runs in time proportional to the number of scene nodes plus the highest
/// occupied slot: each node's ancestry is climbed once across the whole call,
/// not once per node. Allocates only while `scratch` or `globals` is still
/// growing to fit the world.
///
/// The return value is the only report of a malformed hierarchy — a discarded
/// [`Propagated`] turns a parent typo into a thing quietly drawn in the wrong
/// place — so it is `#[must_use]`.
///
/// # Example
///
/// A hub turned a quarter turn, with a child one unit along its x axis. The
/// child swings round to the y axis rather than staying put, which is the one
/// thing a caller could not have got by adding two vectors — and the reason
/// this crate exists rather than the composition living at each call site.
///
/// ```
/// use renew_ecs::{Entities, Store};
/// use renew_fixed::{Angle, Fixed, Vec2};
/// use renew_scene::{Global, Local, Parent, Scratch, propagate};
///
/// let mut entities = Entities::new();
/// let (mut parents, mut locals, mut globals) =
///     (Store::default(), Store::default(), Store::default());
///
/// let hub = entities.spawn();
/// locals.insert(hub.index(), Local::new(Vec2::ZERO, Angle::QUARTER));
///
/// let arm = entities.spawn();
/// locals.insert(
///     arm.index(),
///     Local::new(Vec2::new(Fixed::ONE, Fixed::ZERO), Angle::ZERO),
/// );
/// parents.insert(arm.index(), Parent(hub));
///
/// let mut scratch = Scratch::new();
/// let counts = propagate(&mut scratch, &entities, &parents, &locals, &mut globals);
/// assert_eq!(counts.nodes, 2);
///
/// let placed: Global = *globals.get(arm.index()).expect("placed");
/// assert_eq!(placed.translation(), Vec2::new(Fixed::ZERO, Fixed::ONE));
/// assert_eq!(placed.rotation(), Angle::QUARTER);
/// ```
#[must_use]
pub fn propagate(
    scratch: &mut Scratch,
    entities: &Entities,
    parents: &Store<Parent>,
    locals: &Store<Local>,
    globals: &mut Store<Global>,
) -> Propagated {
    let Scratch { marks, ancestry } = scratch;
    marks.clear();
    marks.resize(entities.capacity(), Mark::Untouched);
    ancestry.clear();

    let mut counts = Propagated::default();

    for entity in entities.iter() {
        let slot = entity.index();
        if !locals.contains(slot) || mark_of(marks, slot) != Mark::Untouched {
            continue;
        }

        // Climb to the top of this node's ancestry, marking the path, and stop
        // at the first ancestor that is already placed, already on the path
        // (a cycle), or has no usable parent.
        let mut cursor = slot;
        loop {
            set_mark(marks, cursor, Mark::Climbing);
            ancestry.push(cursor);
            match parent_slot(entities, parents, locals, cursor) {
                Some(next) if mark_of(marks, next) == Mark::Untouched => cursor = next,
                _ => break,
            }
        }

        // Back down. The climb pushed deepest-first, so this walks the buffer
        // in reverse — shallowest ancestor first, every parent placed before
        // the child that composes against it. Reading it *forwards* is the
        // previous-tick lag the crate docs describe, so the direction here is
        // the whole correctness argument and not a style choice.
        for &node in ancestry.iter().rev() {
            // Both fallbacks below are unreachable and asserted rather than
            // quietly taken: every slot in `ancestry` was admitted by a
            // `locals.contains` check, and every slot marked `Placed` had a
            // global written in this same loop. Substituting the identity for
            // either would put a node at the world origin and call it an
            // answer.
            debug_assert!(locals.contains(node), "a climbed node always has a local");
            let local = locals.get(node).copied().unwrap_or_default();
            let base = match parent_slot(entities, parents, locals, node) {
                Some(above) if mark_of(marks, above) == Mark::Placed => {
                    debug_assert!(globals.contains(above), "a placed node always has a global");
                    globals.get(above).copied().unwrap_or(Global::IDENTITY)
                }
                Some(_) => {
                    counts.cyclic = counts.cyclic.saturating_add(1);
                    Global::IDENTITY
                }
                None => {
                    if parents.contains(node) {
                        counts.orphaned = counts.orphaned.saturating_add(1);
                    } else {
                        counts.roots = counts.roots.saturating_add(1);
                    }
                    Global::IDENTITY
                }
            };
            globals.insert(node, base.compose(local));
            set_mark(marks, node, Mark::Placed);
            counts.nodes = counts.nodes.saturating_add(1);
        }
        ancestry.clear();
    }

    counts
}

/// The slot this node composes against, or `None` when it composes against the
/// world.
///
/// `None` covers three different situations that the caller distinguishes by
/// asking whether a [`Parent`] component exists at all: no parent, a parent
/// handle whose entity is gone, and a parent that carries no [`Local`] and so
/// has no placement to compose against.
///
/// A node parented to *itself* is not one of them. It is a loop of length one
/// and is reported as such, because the climb marks a node before asking what
/// is above it. Answering `None` here instead would file the same node under
/// `orphaned`, which points at a different mistake with a different fix.
fn parent_slot(
    entities: &Entities,
    parents: &Store<Parent>,
    locals: &Store<Local>,
    slot: u32,
) -> Option<u32> {
    let Parent(handle) = *parents.get(slot)?;
    if !entities.is_alive(handle) {
        return None;
    }
    let above = handle.index();
    if !locals.contains(above) {
        return None;
    }
    Some(above)
}

fn mark_of(marks: &[Mark], slot: u32) -> Mark {
    usize::try_from(slot)
        .ok()
        .and_then(|slot| marks.get(slot))
        .copied()
        .unwrap_or_default()
}

/// Every slot reaching here indexes within `marks`: it is sized to the entity
/// allocator's capacity before the walk begins, and slots arrive either from a
/// live entity or from a parent handle already checked against `Entities` —
/// both below that capacity.
///
/// Asserted rather than clamped, because a slot that fell outside would make
/// this a silent no-op, and a mark that never gets set makes the climb's cycle
/// detection blind: the loop below would stop terminating rather than answer
/// wrongly, which is the worse of the two failures.
fn set_mark(marks: &mut [Mark], slot: u32, mark: Mark) {
    debug_assert!(
        usize::try_from(slot).is_ok_and(|slot| slot < marks.len()),
        "slot {slot} is outside the range the walk marked"
    );
    if let Some(entry) = usize::try_from(slot)
        .ok()
        .and_then(|slot| marks.get_mut(slot))
    {
        *entry = mark;
    }
}
