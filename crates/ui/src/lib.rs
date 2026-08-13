//! The retained widget tree: an arena of generationally addressed
//! nodes, capacities fixed at construction.
//!
//! This crate is the simulation-side half of the UI: the tree, the
//! fixed-point solver (the [`layout`] module vocabulary) that turns
//! its styles into pixel rectangles, and the integer-only interaction
//! surface ([`Ui::handle`], [`Ui::absorb`]) that turns events into
//! decisions and decisions into digests. Under all three sits the
//! same ground — a tree whose nodes are stable to address, cheap to
//! add and remove, and bounded by an explicit limit rather than by
//! whatever the heap allows.
//!
//! **Shape.** Nodes live in one arena, linked intrusively: parent,
//! first and last child, previous and next sibling. Children keep
//! insertion order — document order — which is the order every later
//! consumer (layout, drawing, hit-testing) walks them in. No node owns
//! a collection, so the steady state allocates nothing: insertion pops
//! a free slot, removal pushes one back, and the arena never grows.
//!
//! **Addressing.** A [`NodeId`] is a slot index plus the generation the
//! slot carried when the node was created. Removing a node bumps the
//! slot's generation, so every id that named it goes stale at once and
//! stays stale forever — the generation is 64 bits, so recycling one
//! slot every nanosecond exhausts it after five centuries; no physical
//! run reaches a repeat. A stale id is data, not a fault, and every
//! operation given one misses — answers `None`, `false`, an empty
//! iterator, or [`UiRefused::MissingParent`] — rather than panicking or
//! touching the wrong node.
//!
//! One honesty the ids cannot offer: a `NodeId` carries no memory of
//! which tree issued it. Two trees issue the same sequence of ids, so
//! an id from one tree used on another is **not detected** — it may
//! miss, or it may name that tree's unrelated node. Holding ids across
//! trees is a logic error; the tag that would catch it needs either
//! global state or an address-derived value, and both are barred here
//! for reasons older than this crate.
//!
//! **Bounds.** [`UiLimits`] fixes the arena's size at construction. A
//! full tree refuses insertion with [`UiRefused::Full`]; nothing here
//! grows, and nothing here panics on data.

// The simulation's crates deny float arithmetic wholesale (the closure
// rule checks every crate in a simulation's shipping graph); the
// solver's numbers are Fixed — integers under the hood — and a float
// anywhere in this crate would be a value a digest could see.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::float_arithmetic)]

pub mod document;
mod field;
mod input;
mod layout;
pub mod state;
pub mod text;

pub use document::{Document, DocumentError};
pub use field::{EditOp, MAX_FIELD_BYTES, MAX_FIELDS, POOL_BYTES};
use input::Interaction;
pub use input::{UiEvent, UiOutput};
pub use layout::{Align, Direction, Edges, Rect, Size, Style};
use layout::{LayoutSlot, Scratch, Solve};
/// Re-exported because the API speaks it: rectangles, styles, and the
/// solver's whole vocabulary are in this arithmetic.
pub use renew_fixed::Fixed;
pub use state::{
    NO_PATCH, STATE_COMBINATIONS, STATE_DISABLED, STATE_FOCUS, STATE_HOVER, STATE_PRESSED,
    StatePatch,
};

/// Nothing here: the marker every slot uses for "no neighbour". One
/// value, not an `Option<u32>`, so a slot stays eight words no matter
/// how many links are empty.
pub(crate) const NIL: u32 = u32::MAX;

/// Capacities for one [`Ui`], fixed at construction.
///
/// One field today; later steps add their own ceilings (text bytes,
/// style tables) beside it. The struct is plain on purpose — the crate
/// is `bootstrap`, and growing a field is cheaper than a builder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiLimits {
    /// The most nodes the tree will hold, root included. Zero is
    /// clamped to one: a tree exists to hold at least its root.
    pub nodes: u32,
}

/// Why the tree refused an operation.
///
/// Refusals are data, not faults: a full tree and a vanished parent
/// are both things a running game can cause and must be able to hear
/// about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiRefused {
    /// The tree already holds [`UiLimits::nodes`] nodes.
    Full,
    /// The named parent is not live: removed since the id was taken.
    MissingParent,
}

impl core::fmt::Display for UiRefused {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Full => write!(f, "the tree is at its node limit"),
            Self::MissingParent => write!(f, "the parent node is not live"),
        }
    }
}

impl core::error::Error for UiRefused {}

/// A node's address: slot index plus the generation the slot carried
/// when this node was created.
///
/// Copyable and order-free on purpose: an id can be stored anywhere,
/// for any length of time, and — **on the tree that issued it** — the
/// worst it can do later is miss. It does not remember which tree that
/// was (see the crate doc): on any other tree it is meaningless and
/// undetected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId {
    index: u32,
    generation: u64,
}

impl NodeId {
    /// The raw slot index. Public for presentation, which keys its
    /// snapshot pairs by (slot, generation); not meaningful across
    /// trees, and not a stable serialization — an id is an address,
    /// never data to store outside the process.
    #[must_use]
    pub fn index(self) -> u32 {
        self.index
    }

    /// The generation half of the address, likewise.
    #[must_use]
    pub fn generation(self) -> u64 {
        self.generation
    }
}

/// One arena slot. Free slots keep their links meaningless except
/// `next_sibling`, which threads the free list.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Slot {
    /// Bumped on every removal, so old ids miss. Sixty-four bits so
    /// the bump can never cycle in a physical run — "stays stale
    /// forever" is a claim about this counter, and a u32 makes it
    /// false after 2^32 recycles of one slot. Tracked beside an
    /// explicit `live` flag rather than encoded in parity — parity is
    /// the kind of cleverness that is wrong once and then wrong
    /// forever.
    generation: u64,
    live: bool,
    parent: u32,
    pub(crate) first_child: u32,
    last_child: u32,
    prev_sibling: u32,
    pub(crate) next_sibling: u32,
}

impl Slot {
    const EMPTY: Self = Self {
        generation: 0,
        live: false,
        parent: NIL,
        first_child: NIL,
        last_child: NIL,
        prev_sibling: NIL,
        next_sibling: NIL,
    };
}

/// The retained widget tree.
///
/// Construction allocates everything the tree will ever hold; no later
/// operation allocates, which is what lets the steady-state gate assert
/// exactly zero.
#[derive(Debug)]
pub struct Ui {
    slots: Vec<Slot>,
    /// Per-slot layout state: style in, solved rectangle out.
    layout: Vec<LayoutSlot>,
    /// The solver's reusable workspace, sized with the arena.
    scratch: Scratch,
    /// Whether anything changed since the last solve — structure,
    /// style, or viewport. One flag for the whole tree in v0: exact
    /// damage arrives with the compiled style tables.
    dirty: bool,
    /// The viewport the current rectangles were solved for.
    solved_for: (Fixed, Fixed),
    /// Pointer, hover, press, focus, and the decision counters.
    interaction: Interaction,
    /// Decisions waiting for the host, capacity fixed with the arena.
    outputs: Vec<UiOutput>,
    /// Per-slot state styling: combination tables, bases, and what is
    /// currently worn.
    pub(crate) states: Vec<state::StateSlot>,
    /// The shared patch pool every table indexes into. Loaded whole,
    /// before tables; empty until a document or host provides one.
    pub(crate) patches: Vec<state::StatePatch>,
    /// Slots wearing nonzero state bits right now — at most the hover
    /// target, the pressed node, and the focus holder. Sized at
    /// construction so the per-event refresh allocates nothing.
    stated: Vec<u32>,
    /// How many times [`Self::solve`] actually walked, as opposed to
    /// returning on the clean early-out. The exact-damage claim is an
    /// assertion against this counter, not prose.
    layout_passes: u64,
    /// Head of the free list, threaded through `next_sibling`.
    free: u32,
    live: u32,
    limits: UiLimits,
    /// The text fields: a fixed pool, present in every tree.
    ///
    /// [`POOL_BYTES`] whether the tree has a field or not, which is the price
    /// of asking no caller to declare a capacity. See [`field`] for the
    /// two designs this beat.
    fields: [field::Field; MAX_FIELDS],
}

impl Ui {
    /// A tree holding only its root, with room for `limits.nodes` nodes
    /// in total.
    #[must_use]
    pub fn new(limits: UiLimits) -> Self {
        let capacity = limits.nodes.max(1);
        let mut slots = vec![Slot::EMPTY; capacity as usize];
        // The root occupies slot zero from birth. Generation starts at
        // one so the all-zeroes NodeId a caller might conjure from
        // nowhere never names anything.
        slots[0].generation = 1;
        slots[0].live = true;
        // The remaining slots thread the free list in index order.
        for (index, slot) in slots.iter_mut().enumerate().skip(1) {
            slot.generation = 1;
            let next = index + 1;
            slot.next_sibling = if next < capacity as usize {
                // Bounded by capacity, which is a u32.
                u32::try_from(next).unwrap_or(NIL)
            } else {
                NIL
            };
        }
        let free = if capacity > 1 { 1 } else { NIL };
        Self {
            slots,
            layout: vec![LayoutSlot::default(); capacity as usize],
            scratch: Scratch::with_capacity(capacity),
            dirty: true,
            solved_for: (Fixed::ZERO, Fixed::ZERO),
            interaction: Interaction::default(),
            fields: [field::Field::EMPTY; MAX_FIELDS],
            outputs: Vec::with_capacity(capacity as usize),
            states: vec![state::StateSlot::default(); capacity as usize],
            patches: Vec::new(),
            // Hover, pressed, focus: three bit-holders at most, with
            // one spare so a push can never be the first allocation.
            stated: Vec::with_capacity(4),
            layout_passes: 0,
            free,
            live: 1,
            limits: UiLimits { nodes: capacity },
        }
    }

    /// The root: created with the tree, never removable.
    #[must_use]
    pub fn root(&self) -> NodeId {
        NodeId {
            index: 0,
            generation: self.slots[0].generation,
        }
    }

    /// The limits the tree was built with (with zero already clamped).
    #[must_use]
    pub fn limits(&self) -> UiLimits {
        self.limits
    }

    /// How many nodes are live, root included.
    #[must_use]
    pub fn live(&self) -> u32 {
        self.live
    }

    /// Whether `node` still names a live node of this tree.
    #[must_use]
    pub fn is_live(&self, node: NodeId) -> bool {
        self.slot_of(node).is_some()
    }

    /// Append a new node as `parent`'s last child.
    ///
    /// # Errors
    ///
    /// [`UiRefused::Full`] when the tree already holds its limit;
    /// [`UiRefused::MissingParent`] when `parent` is stale. Either way
    /// the tree is unchanged.
    pub fn insert(&mut self, parent: NodeId) -> Result<NodeId, UiRefused> {
        let parent_index = self.slot_of(parent).ok_or(UiRefused::MissingParent)?;
        if self.free == NIL {
            return Err(UiRefused::Full);
        }
        let index = self.free;
        let slot = &mut self.slots[index as usize];
        self.free = slot.next_sibling;
        let generation = slot.generation;
        *slot = Slot {
            generation,
            live: true,
            parent: parent_index,
            ..Slot::EMPTY
        };
        self.link_last(parent_index, index);
        self.live += 1;
        self.layout[index as usize] = LayoutSlot::default();
        self.states[index as usize] = state::StateSlot::default();
        self.dirty = true;
        Ok(NodeId { index, generation })
    }

    /// Remove `node` and its whole subtree.
    ///
    /// Answers whether anything was removed: `false` is the miss for a
    /// stale id — and for the root, which is structural and lives as
    /// long as the tree.
    pub fn remove(&mut self, node: NodeId) -> bool {
        let Some(index) = self.slot_of(node) else {
            return false;
        };
        if index == 0 {
            return false;
        }
        if let Some(slot) = self.field_slot(node).and_then(|at| self.fields.get_mut(at)) {
            *slot = field::Field::EMPTY;
        }
        // The field slot goes back to the pool with the node. Without
        // this the pool leaks — a form torn down and rebuilt eight times
        // refuses the ninth field while no live node holds any of them —
        // and, worse, the departed id keeps naming live text, which is
        // the one thing this crate's addressing promises cannot happen.
        self.unlink(index);
        self.free_subtree(index);
        self.dirty = true;
        true
    }

    /// The parent of `node`; `None` for the root and for stale ids.
    #[must_use]
    pub fn parent(&self, node: NodeId) -> Option<NodeId> {
        let index = self.slot_of(node)?;
        let parent = self.slots[index as usize].parent;
        (parent != NIL).then(|| self.id_at(parent))
    }

    /// The children of `node`, in insertion order — document order,
    /// the order layout and drawing will walk. Empty for stale ids.
    pub fn children(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let first = self
            .slot_of(node)
            .map_or(NIL, |index| self.slots[index as usize].first_child);
        Children {
            ui: self,
            at: first,
        }
    }

    /// Give `node` a new style. Answers whether it landed: `false` is
    /// the miss for a stale id. Any change marks the tree for
    /// re-solving; setting the same style again does too, and the
    /// solve it provokes is idempotent, so the cost is a no-op walk
    /// rather than a wrong picture.
    ///
    /// This authors the **base** style. While a state patch is worn
    /// (see [`Self::set_state_table`]) the node keeps wearing it —
    /// patches are complete resolved styles — and the new base shows
    /// when the patch comes off.
    pub fn set_style(&mut self, node: NodeId, style: Style) -> bool {
        let Some(index) = self.slot_of(node) else {
            return false;
        };
        self.states[index as usize].base = style;
        if self.states[index as usize].applied == state::NO_PATCH {
            self.layout[index as usize].style = style;
        }
        self.dirty = true;
        true
    }

    /// Load the shared patch pool every state table indexes into.
    /// Answers whether it landed: `false` when the pool is too large
    /// for a table entry to address. Loading replaces the whole pool
    /// and clears every table with it — a table indexes one pool, and
    /// letting it survive a swap would let an entry point past or
    /// into the wrong one. Pools load before tables, at document-load
    /// time, never mid-interaction.
    pub fn set_patch_pool(&mut self, pool: Vec<StatePatch>) -> bool {
        if pool.len() >= usize::from(state::NO_PATCH) {
            return false;
        }
        for index in 0..self.states.len() {
            if self.states[index].applied != state::NO_PATCH {
                self.layout[index].style = self.states[index].base;
                self.dirty = true;
            }
            let base = self.states[index].base;
            self.states[index] = state::StateSlot {
                base,
                ..state::StateSlot::default()
            };
        }
        self.stated.clear();
        self.patches = pool;
        true
    }

    /// Give `node` its state table: one patch index per combination of
    /// the four state bits, [`NO_PATCH`] wearing the base. Answers
    /// whether it landed: `false` for a stale id or any entry past the
    /// loaded pool. The node's current state bits apply immediately.
    pub fn set_state_table(&mut self, node: NodeId, table: [u16; STATE_COMBINATIONS]) -> bool {
        let Some(index) = self.slot_of(node) else {
            return false;
        };
        let in_pool =
            |entry: u16| entry == state::NO_PATCH || usize::from(entry) < self.patches.len();
        if !table.iter().copied().all(in_pool) {
            return false;
        }
        self.states[index as usize].table = table;
        let bits = self.states[index as usize].bits;
        self.wear(index, bits);
        true
    }

    /// How many times [`Self::solve`] actually walked the tree, as
    /// opposed to returning on the clean early-out. The exact-damage
    /// promise — a colour-only state flip re-solves nothing — is an
    /// assertion against this counter.
    #[must_use]
    pub fn layout_passes(&self) -> u64 {
        self.layout_passes
    }

    /// The base style [`Self::set_style`] authored for `node` — what
    /// shows when no patch is worn; `None` for stale ids. Writers and
    /// serializers want this, never the worn effective style: a
    /// document is authored state, and dress is derived at runtime.
    #[must_use]
    pub fn base_style(&self, node: NodeId) -> Option<Style> {
        let index = self.slot_of(node)?;
        Some(self.states[index as usize].base)
    }

    /// The state table `node` carries; `None` for stale ids. All
    /// [`NO_PATCH`] when none was set.
    #[must_use]
    pub fn state_table(&self, node: NodeId) -> Option<[u16; STATE_COMBINATIONS]> {
        let index = self.slot_of(node)?;
        Some(self.states[index as usize].table)
    }

    /// The loaded patch pool, whole — empty until a document or host
    /// provides one.
    #[must_use]
    pub fn patch_pool(&self) -> &[StatePatch] {
        &self.patches
    }

    /// Re-derive the state bits of every node that can hold any, and
    /// swap patches where they changed. Called by [`Self::handle`]
    /// after each event; allocation-free — the candidate set is at
    /// most the hover target, the pressed node, and the focus holder,
    /// plus whoever wore bits before.
    pub(crate) fn refresh_states(&mut self) {
        // Whoever wore bits last time starts at zero unless re-earned.
        while let Some(index) = self.stated.pop() {
            let bits = self.state_bits_of(index);
            self.wear(index, bits);
        }
        let (x, y) = self.interaction.pointer;
        let hover = self.hit_test(x, y);
        for candidate in [hover, self.interaction.pressed, self.interaction.focus] {
            if let Some(node) = candidate
                && let Some(index) = self.slot_of(node)
            {
                let bits = self.state_bits_of(index);
                self.wear(index, bits);
                if bits != 0 && !self.stated.contains(&index) {
                    self.stated.push(index);
                }
            }
        }
    }

    /// Reset one slot's state styling as it is freed, so a dead slot
    /// never keeps worn dress: the later `stated` pop becomes a clean
    /// no-op instead of a spurious layout pass.
    pub(crate) fn reset_state_slot(&mut self, index: u32) {
        self.states[index as usize] = state::StateSlot::default();
    }

    /// The four bits as they stand for one slot.
    fn state_bits_of(&self, index: u32) -> u8 {
        let (x, y) = self.interaction.pointer;
        let id = NodeId {
            index,
            generation: self.slots[index as usize].generation,
        };
        let mut bits = 0;
        if self.hit_test(x, y) == Some(id) {
            bits |= STATE_HOVER;
        }
        if self.interaction.pressed == Some(id) {
            bits |= STATE_PRESSED;
        }
        if self.interaction.focus == Some(id) {
            bits |= STATE_FOCUS;
        }
        bits
    }

    /// Wear whatever the table holds for `bits`, if that differs from
    /// what is worn: one lookup, one swap, and layout dirtied only
    /// when either side of the swap moves geometry.
    fn wear(&mut self, index: u32, bits: u8) {
        let slot = &self.states[index as usize];
        let next = slot.table[usize::from(bits) % STATE_COMBINATIONS];
        let worn = slot.applied;
        if next == worn {
            self.states[index as usize].bits = bits;
            return;
        }
        let moves = state::StateSlot::touches_layout(worn, &self.patches)
            || state::StateSlot::touches_layout(next, &self.patches);
        // The base fallback holds even if a table ever outlived its
        // pool: the swap-clears-tables rule above makes that
        // unreachable, and wearing the base is the fail-closed answer
        // if it ever bent — never an index past the pool.
        self.layout[index as usize].style = self
            .patches
            .get(usize::from(next))
            .map_or(self.states[index as usize].base, |patch| patch.style);
        self.states[index as usize].applied = next;
        self.states[index as usize].bits = bits;
        if moves {
            self.dirty = true;
        }
    }

    /// Make `node` a text field, claiming one of the pool's slots.
    ///
    /// Idempotent: a node that is already a field keeps its slot and its
    /// contents, so a caller rebuilding a screen does not wipe what
    /// somebody typed.
    ///
    /// # Errors
    ///
    /// [`UiRefused::Full`] when every slot in the pool is taken — the
    /// same refusal a full arena gives, for the same reason. A ninth
    /// simultaneous field is a different kind of interface.
    pub fn make_field(&mut self, node: NodeId) -> Result<(), UiRefused> {
        if !self.is_live(node) {
            return Err(UiRefused::MissingParent);
        }
        if self.field_slot(node).is_some() {
            return Ok(());
        }
        // Reclaim before refusing. A slot whose owner is no longer live
        // belongs to nobody, and chasing every path that can end a
        // node's life is a losing game: removing a parent ends its
        // children too, and the next kind of removal will end them some
        // third way. Asking the arena who is alive is the answer that
        // cannot go stale — a review probe found exactly this, by
        // removing a panel rather than the field inside it.
        for index in 0..MAX_FIELDS {
            let dead = self
                .fields
                .get(index)
                .and_then(|slot| slot.owner)
                .is_some_and(|owner| !self.is_live(owner));
            if let Some(slot) = self.fields.get_mut(index).filter(|_| dead) {
                *slot = field::Field::EMPTY;
            }
        }
        let free = self
            .fields
            .iter()
            .position(|slot| slot.owner.is_none())
            .ok_or(UiRefused::Full)?;
        if let Some(slot) = self.fields.get_mut(free) {
            // No clear here, and that is checked rather than assumed. A
            // review flagged deleting one as a surviving mutation, which
            // it was — because every path that frees a slot already
            // clears it: `remove` empties it, the reclaim above empties
            // it, and a slot never claimed starts empty. A second clear
            // is a line no test can distinguish, which §15 calls dead.
            // The property it protected is now asserted where it is
            // real, on the reclaim path.
            slot.owner = Some(node);
        }
        Ok(())
    }

    /// What a field holds, as bytes.
    ///
    /// **Always valid UTF-8**, because insertion encodes a whole scalar
    /// and deletion steps whole characters. [`None`] for a node that is
    /// not a field.
    #[must_use]
    pub fn field_text(&self, node: NodeId) -> Option<&[u8]> {
        self.fields
            .get(self.field_slot(node)?)
            .map(field::Field::text)
    }

    /// Where the cursor sits in a field, in bytes from the start.
    #[must_use]
    pub fn field_cursor(&self, node: NodeId) -> Option<u8> {
        self.fields
            .get(self.field_slot(node)?)
            .map(field::Field::cursor)
    }

    /// Which pool slot a node owns, if any.
    ///
    /// **Liveness is checked here rather than trusted.** Releasing on
    /// removal is what keeps the pool honest, and this is the second
    /// half of the same promise: a stale id misses, the way it misses
    /// everywhere else in this crate, even if a slot were ever left
    /// behind.
    fn field_slot(&self, node: NodeId) -> Option<usize> {
        if !self.is_live(node) {
            return None;
        }
        self.fields.iter().position(|slot| slot.owner == Some(node))
    }

    /// The style `node` currently holds; `None` for stale ids.
    #[must_use]
    pub fn style(&self, node: NodeId) -> Option<Style> {
        let index = self.slot_of(node)?;
        Some(self.layout[index as usize].style)
    }

    /// Solve the tree into absolute rectangles, the root filling the
    /// viewport at the origin.
    ///
    /// Retained: when nothing changed since the last solve — no edit,
    /// no style, same viewport — this returns without walking. The
    /// walk itself allocates nothing; construction sized everything.
    pub fn solve(&mut self, viewport_width: Fixed, viewport_height: Fixed) {
        if !self.dirty && self.solved_for == (viewport_width, viewport_height) {
            return;
        }
        Solve {
            slots: &mut self.slots,
            layout: &mut self.layout,
            scratch: &mut self.scratch,
        }
        .run(viewport_width, viewport_height);
        self.dirty = false;
        self.layout_passes += 1;
        self.solved_for = (viewport_width, viewport_height);
    }

    /// The rectangle the last [`Self::solve`] gave `node`; `None` for
    /// stale ids. Meaningful only after a solve — before one, every
    /// rectangle is the zero default.
    #[must_use]
    pub fn rect(&self, node: NodeId) -> Option<Rect> {
        let index = self.slot_of(node)?;
        Some(self.layout[index as usize].rect)
    }

    /// The live slot index behind `node`, if the id still names one.
    fn slot_of(&self, node: NodeId) -> Option<u32> {
        let slot = self.slots.get(node.index as usize)?;
        (slot.live && slot.generation == node.generation).then_some(node.index)
    }

    /// The id currently naming `index`. Only called for live slots.
    fn id_at(&self, index: u32) -> NodeId {
        NodeId {
            index,
            generation: self.slots[index as usize].generation,
        }
    }

    /// Append `index` to `parent`'s child list.
    fn link_last(&mut self, parent: u32, index: u32) {
        let last = self.slots[parent as usize].last_child;
        if last == NIL {
            self.slots[parent as usize].first_child = index;
        } else {
            self.slots[last as usize].next_sibling = index;
            self.slots[index as usize].prev_sibling = last;
        }
        self.slots[parent as usize].last_child = index;
    }

    /// Detach `index` from its parent's child list. The subtree below
    /// it stays attached to it.
    fn unlink(&mut self, index: u32) {
        let Slot {
            parent,
            prev_sibling,
            next_sibling,
            ..
        } = self.slots[index as usize];
        if prev_sibling == NIL {
            self.slots[parent as usize].first_child = next_sibling;
        } else {
            self.slots[prev_sibling as usize].next_sibling = next_sibling;
        }
        if next_sibling == NIL {
            self.slots[parent as usize].last_child = prev_sibling;
        } else {
            self.slots[next_sibling as usize].prev_sibling = prev_sibling;
        }
    }

    /// Free `index` and everything below it, bumping generations so
    /// every id into the subtree goes stale at once.
    ///
    /// Iterative with the free list itself as the work list: freed
    /// slots are pushed as they are visited, and children are walked
    /// before their parent's links are overwritten. No recursion — a
    /// document is data, and data must not choose the stack depth.
    fn free_subtree(&mut self, index: u32) {
        // The pending stack borrows `prev_sibling` of already-detached
        // slots, which nothing else reads between here and the free
        // push: a slot enters pending exactly once and leaves freed.
        let mut pending = index;
        self.slots[index as usize].prev_sibling = NIL;
        while pending != NIL {
            let current = pending;
            pending = self.slots[current as usize].prev_sibling;
            // Push every child onto pending.
            let mut child = self.slots[current as usize].first_child;
            while child != NIL {
                let next = self.slots[child as usize].next_sibling;
                self.slots[child as usize].prev_sibling = pending;
                pending = child;
                child = next;
            }
            // A dead slot wears nothing: the reset keeps the later
            // `stated` pop a clean no-op instead of a spurious layout
            // pass shedding a ghost patch.
            self.reset_state_slot(current);
            // Free the slot: bump the generation, thread the free
            // list. Wrapping spelled for the overflow lint's sake only
            // — a 64-bit recycle counter cannot wrap in a physical run.
            let slot = &mut self.slots[current as usize];
            slot.generation = slot.generation.wrapping_add(1);
            slot.live = false;
            slot.parent = NIL;
            slot.first_child = NIL;
            slot.last_child = NIL;
            slot.next_sibling = self.free;
            slot.prev_sibling = NIL;
            self.free = current;
            self.live -= 1;
        }
    }
}

/// Sibling-walk iterator over one node's children.
struct Children<'a> {
    ui: &'a Ui,
    at: u32,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        if self.at == NIL {
            return None;
        }
        let id = self.ui.id_at(self.at);
        self.at = self.ui.slots[self.at as usize].next_sibling;
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> Ui {
        Ui::new(UiLimits { nodes: 8 })
    }

    /// The tree is born holding its root and nothing else, and the
    /// zero limit is clamped rather than obeyed into absurdity.
    #[test]
    fn a_new_tree_is_a_root_alone() {
        let ui = small();
        assert_eq!(ui.live(), 1);
        assert!(ui.is_live(ui.root()));
        assert_eq!(ui.parent(ui.root()), None);
        assert_eq!(ui.children(ui.root()).count(), 0);
        assert_eq!(Ui::new(UiLimits { nodes: 0 }).limits().nodes, 1);
    }

    /// Children come back in insertion order — document order, which
    /// is the order everything downstream will walk.
    #[test]
    fn children_keep_document_order() {
        let mut ui = small();
        let root = ui.root();
        let a = ui.insert(root).expect("room");
        let b = ui.insert(root).expect("room");
        let c = ui.insert(root).expect("room");
        assert_eq!(ui.children(root).collect::<Vec<_>>(), vec![a, b, c]);
        assert_eq!(ui.parent(b), Some(root));
    }

    /// A full tree refuses with `Full` and is left unchanged.
    #[test]
    fn a_full_tree_refuses_and_stands_still() {
        let mut ui = Ui::new(UiLimits { nodes: 2 });
        let root = ui.root();
        let only = ui.insert(root).expect("one slot beyond the root");
        assert_eq!(ui.insert(root), Err(UiRefused::Full));
        assert_eq!(ui.live(), 2);
        assert!(ui.is_live(only));
    }

    /// A stale parent refuses with `MissingParent` — a miss, not a
    /// panic, and not a lie about capacity.
    #[test]
    fn a_stale_parent_is_a_miss() {
        let mut ui = small();
        let root = ui.root();
        let gone = ui.insert(root).expect("room");
        assert!(ui.remove(gone));
        assert_eq!(ui.insert(gone), Err(UiRefused::MissingParent));
        assert!(!ui.is_live(gone));
        assert_eq!(ui.parent(gone), None);
        assert_eq!(ui.children(gone).count(), 0);
        assert!(!ui.remove(gone), "a second removal is a miss too");
        assert!(
            !ui.set_style(gone, Style::default()),
            "styling a stale id is a miss, not a landing"
        );
        assert_eq!(ui.style(gone), None);
        assert_eq!(ui.rect(gone), None);
    }

    /// Removing a node takes its whole subtree, stales every id into
    /// it at once, and returns every slot to use.
    #[test]
    fn removal_is_the_whole_subtree() {
        let mut ui = small();
        let root = ui.root();
        let branch = ui.insert(root).expect("room");
        let leaf = ui.insert(branch).expect("room");
        let twig = ui.insert(leaf).expect("room");
        let sibling = ui.insert(root).expect("room");
        assert!(ui.remove(branch));
        assert_eq!(ui.live(), 2);
        for stale in [branch, leaf, twig] {
            assert!(!ui.is_live(stale));
        }
        assert!(ui.is_live(sibling));
        assert_eq!(ui.children(root).collect::<Vec<_>>(), vec![sibling]);
        // The three freed slots are usable again, to the exact limit.
        for _ in 0..6 {
            ui.insert(root).expect("freed slots must come back");
        }
        assert_eq!(ui.insert(root), Err(UiRefused::Full));
    }

    /// A recycled slot's new tenant is not reachable through the old
    /// tenant's id: generations, not indices, are the address.
    #[test]
    fn a_recycled_slot_does_not_answer_to_its_old_name() {
        let mut ui = small();
        let root = ui.root();
        let old = ui.insert(root).expect("room");
        assert!(ui.remove(old));
        let new = ui.insert(root).expect("room");
        assert!(!ui.is_live(old));
        assert!(ui.is_live(new));
        assert_ne!(old, new);
    }

    /// The root refuses removal: it is structural, not content.
    #[test]
    fn the_root_is_not_removable() {
        let mut ui = small();
        let root = ui.root();
        assert!(!ui.remove(root));
        assert!(ui.is_live(root));
        assert_eq!(ui.live(), 1);
    }

    /// A deep chain frees without recursion: depth equal to the whole
    /// arena neither overflows nor leaks a slot.
    #[test]
    fn a_chain_as_deep_as_the_arena_frees_completely() {
        let mut ui = Ui::new(UiLimits { nodes: 1024 });
        let root = ui.root();
        let mut at = root;
        let mut top = None;
        for _ in 0..1023 {
            at = ui.insert(at).expect("room");
            top.get_or_insert(at);
        }
        assert_eq!(ui.live(), 1024);
        assert!(ui.remove(top.expect("the chain has a first link")));
        assert_eq!(ui.live(), 1);
    }

    /// The baked advance table: every printable glyph advances, the
    /// widths are proportional (an i is narrower than a W), and
    /// measurement is exactly the sum — the same integers on every
    /// target, which is the whole reason the table is integers.
    #[test]
    fn text_measures_as_the_sum_of_integer_advances() {
        assert_eq!(text::ADVANCES.len(), 95, "printable ASCII, all of it");
        assert!(
            text::ADVANCES.iter().skip(1).all(|&advance| advance > 0),
            "every visible glyph advances the pen"
        );
        assert!(
            text::advance_of('i') < text::advance_of('W'),
            "the face is proportional, and the table must show it"
        );
        let expected: i32 = "Play"
            .chars()
            .map(|character| {
                i32::try_from(text::advance_of(character)).expect("advances are small")
            })
            .sum();
        assert_eq!(text::measure("Play"), Fixed::from_int(expected));
        assert_eq!(text::measure(""), Fixed::ZERO);
        assert_eq!(
            text::advance_of('\u{00e9}'),
            text::advance_of('?'),
            "outside the baked range, the stand-in's width is the width"
        );
    }

    /// Every refusal says what happened in words a reader can act on.
    #[test]
    fn refusals_say_what_they_are() {
        assert!(UiRefused::Full.to_string().contains("limit"));
        assert!(UiRefused::MissingParent.to_string().contains("parent"));
    }

    /// Everything the tree promises, checked from the outside: the
    /// public API alone must be able to see a sound tree, because the
    /// public API is all any consumer gets.
    fn assert_sound(ui: &Ui, ever_issued: &[NodeId]) {
        // Every node reachable from the root agrees with its children
        // about the relationship, and the reachable count is exactly
        // what `live()` reports.
        let mut seen = vec![ui.root()];
        let mut at = 0;
        while at < seen.len() {
            let node = seen[at];
            for child in ui.children(node) {
                assert_eq!(ui.parent(child), Some(node), "a child must name its parent");
                assert!(ui.is_live(child), "a listed child must be live");
                seen.push(child);
            }
            at += 1;
        }
        assert_eq!(
            seen.len(),
            ui.live() as usize,
            "every live node hangs off the root, and none hangs twice"
        );
        // Every id ever issued either still names a reachable node or
        // misses everywhere at once.
        for &id in ever_issued {
            if ui.is_live(id) {
                assert!(
                    seen.contains(&id),
                    "a live id must be reachable from the root"
                );
            } else {
                assert_eq!(ui.parent(id), None);
                assert_eq!(ui.children(id).count(), 0);
            }
        }
    }

    proptest::proptest! {
        /// Any sequence of inserts and removals leaves a sound tree:
        /// reachability matches the live count, children and parents
        /// agree, and stale ids miss everywhere. The ops draw their
        /// targets from every id ever issued — including stale ones —
        /// so the miss paths are exercised as heavily as the hits.
        #[test]
        fn any_operation_sequence_keeps_the_tree_sound(
            ops in proptest::collection::vec((0u8..=1, 0usize..1024), 1..256)
        ) {
            let mut ui = Ui::new(UiLimits { nodes: 24 });
            let mut issued = vec![ui.root()];
            for (op, pick) in ops {
                let target = issued[pick % issued.len()];
                if op == 0 {
                    if let Ok(id) = ui.insert(target) {
                        issued.push(id);
                    }
                } else {
                    ui.remove(target);
                }
                assert_sound(&ui, &issued);
            }
        }

        /// Capacity is a wall, not a suggestion: however the tree got
        /// full, one more insert refuses, and a removal opens exactly
        /// the freed room back up.
        #[test]
        fn full_means_full_and_freed_means_free(extra in 1u32..16) {
            let mut ui = Ui::new(UiLimits { nodes: extra + 1 });
            let root = ui.root();
            let mut last = root;
            for _ in 0..extra {
                last = ui.insert(root).expect("under the limit");
            }
            proptest::prop_assert_eq!(ui.insert(root), Err(UiRefused::Full));
            proptest::prop_assert!(ui.remove(last));
            proptest::prop_assert!(ui.insert(root).is_ok());
            proptest::prop_assert_eq!(ui.insert(root), Err(UiRefused::Full));
        }
    }
}
