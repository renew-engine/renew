//! The frame vocabulary: a frame is a list of passes, a pass is
//! attachments and items, and the whole list records into one submit.
//!
//! Callers compose these on their own stack — the borrows end at the
//! `render` call, so nothing here is stored across frames by anyone.

#![deny(unsafe_code)]

use std::fmt;

use crate::config::Color;
use crate::vk::binding::{Binding, MAX_SAMPLED_BINDINGS};
use crate::vk::buffer::BufferInner;
use crate::vk::mesh::Mesh;
use crate::vk::pipeline::{FrameData, RenderPipeline};
use crate::vk::render_image::{RenderImage, RenderImageKind};
use crate::vk::transition::ImageUse;

use std::rc::Rc;

use crate::vk::render_image::KeptContents;

/// How many distinct render images one frame may touch — as targets,
/// as sampled sources, or both. A fixed ceiling so the contract's walk
/// table and the record paths' barrier arrays are stack-sized and the
/// frame path allocates nothing; the fifth distinct image is refused
/// by name before any GPU call.
pub const MAX_FRAME_RENDER_IMAGES: usize = 4;

/// Everything one frame needs, for either target: the passes, in order.
///
/// `#[non_exhaustive]` with a constructor, per the descriptor pattern
/// this crate uses everywhere. Frame-level fields arrive as builders on
/// this, later.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct RenderDesc<'a> {
    /// The frame: passes recorded in order, one submit.
    pub passes: &'a [Pass<'a>],
}

impl<'a> RenderDesc<'a> {
    /// A frame of `passes`, recorded in order.
    ///
    /// Positional because a frame without passes is not a
    /// partially-configured frame — an empty list is refused at
    /// `render` as a contract violation.
    #[must_use]
    pub fn new(passes: &'a [Pass<'a>]) -> Self {
        Self { passes }
    }
}

impl fmt::Debug for RenderDesc<'_> {
    /// Reports the frame's shape — how many passes, and each pass's
    /// item count — not the handles inside it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let items: Vec<usize> = self.passes.iter().map(|pass| pass.items.len()).collect();
        f.debug_struct("RenderDesc")
            .field("passes", &self.passes.len())
            .field("items_per_pass", &items)
            .finish_non_exhaustive()
    }
}

/// What a pass renders into: the target's own surface, or a render
/// image whose kind decides the pass's shape.
///
/// An input enum, so `#[non_exhaustive]`: a later target class must
/// not break downstream matchers.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub enum PassTarget<'a> {
    /// The target's own surface — what every pass named implicitly
    /// before targets had identity, and the default [`Pass::new`]
    /// still writes.
    Surface,
    /// A render image, with the ops for its one attachment.
    ///
    /// The ops ride the variant rather than the pass's slices because
    /// an image pass has exactly one attachment and its kind is the
    /// image's — a color list or a depth option would be two more ways
    /// to state a shape the image already states.
    Image(&'a RenderImage, Attachment),
}

/// One pass: what it renders into, and the draws it records.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Pass<'a> {
    /// Surface passes: exactly one, naming the target's own surface.
    /// Image passes: empty — the image's ops ride [`Pass::target`].
    /// Both shapes are refused by name when violated.
    pub color: &'a [Attachment],
    /// The target's own depth image, when Some. Surface passes only:
    /// an image pass carries its one attachment in its target, and a
    /// depth-kinded image *is* the depth attachment.
    pub depth: Option<Attachment>,
    /// What this pass renders into.
    pub target: PassTarget<'a>,
    /// Draws, executed in slice order.
    pub items: &'a [Item<'a>],
}

impl<'a> Pass<'a> {
    /// A surface pass over `color`, drawing `items` in order, with no
    /// depth.
    #[must_use]
    pub fn new(color: &'a [Attachment], items: &'a [Item<'a>]) -> Self {
        Self {
            color,
            depth: None,
            target: PassTarget::Surface,
            items,
        }
    }

    /// A pass rendering into `image` with `attachment`'s ops, drawing
    /// `items` in order.
    ///
    /// The image's kind decides the pass shape — a color image is the
    /// pass's one color attachment, a depth image its one depth
    /// attachment with no color at all — so a kind/shape mismatch is
    /// unrepresentable rather than refused. Render area, viewport and
    /// scissor come from the image's extent.
    #[must_use]
    pub fn render_to(
        image: &'a RenderImage,
        attachment: Attachment,
        items: &'a [Item<'a>],
    ) -> Self {
        Self {
            color: &[],
            depth: None,
            target: PassTarget::Image(image, attachment),
            items,
        }
    }

    /// Attach the target's depth image to this pass with `depth`'s
    /// ops. Surface passes only — an image pass carries its one
    /// attachment in its target, and the contract refuses the mix.
    #[must_use]
    pub fn depth(mut self, depth: Attachment) -> Self {
        self.depth = Some(depth);
        self
    }
}

/// How one attachment is loaded and stored by a pass.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Attachment {
    /// What the pass finds in the attachment when it begins.
    pub load: LoadOp,
    /// What happens to the attachment's contents when the pass ends.
    pub store: StoreOp,
}

impl Attachment {
    /// An attachment loaded by `load` and stored by `store`.
    #[must_use]
    pub fn new(load: LoadOp, store: StoreOp) -> Self {
        Self { load, store }
    }
}

/// What a pass finds in an attachment when it begins.
///
/// A clear value without a clearing load is unrepresentable: the value
/// rides the variant that uses it.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum LoadOp {
    /// The attachment starts as `ClearValue`.
    Clear(ClearValue),
    /// The attachment keeps the previous pass's contents. Refused on
    /// each identity's first use in the frame — the surface, the
    /// target's depth image, and every render image all start a frame
    /// from undefined contents — and on a render image whose last
    /// targeting pass discarded.
    Load,
}

/// What happens to an attachment's contents when a pass ends.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum StoreOp {
    /// The contents survive the pass.
    Store,
    /// The contents may be discarded — for attachments nothing reads
    /// afterwards.
    Discard,
}

/// The value a clearing load writes; the variant must match the
/// attachment's kind.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum ClearValue {
    /// A color attachment's clear.
    Color(Color),
    /// A depth attachment's clear — finite and in `[0, 1]`, asserted
    /// at `render`.
    Depth(f32),
}

/// One draw: a pipeline, optionally the geometry it walks, and
/// optionally this frame's bytes.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Item<'a> {
    /// The pipeline the draw binds.
    pub pipeline: &'a RenderPipeline,
    /// The geometry this draw walks, making it an indexed draw whose
    /// count comes from the mesh.
    ///
    /// Present exactly when the pipeline declares per-vertex input —
    /// asserted before any GPU call, because a mesh pipeline drawn
    /// without geometry reads an unbound binding and geometry handed to
    /// a generative pipeline is silently ignored.
    ///
    /// **A mesh may be named by any number of items** — there is no
    /// copy to race at all, unlike per-frame bytes, whose repeats must
    /// be pointer-identical under the one-buffer-one-`FrameData` rule
    /// below.
    pub mesh: Option<&'a Mesh>,
    /// `FrameData` contained, not forked; room to grow (a
    /// first-instance or vertex-offset field) without touching existing
    /// callers.
    pub frame_data: Option<FrameData<'a>>,
    /// Bytes recorded as the pipeline's push-constant block before this
    /// draw — the per-draw constant channel.
    ///
    /// Present exactly when the pipeline declares a range, and exactly
    /// its declared length; both are refused by the frame contract
    /// before any GPU call, the same way geometry and depth state are
    /// matched. The bytes are copied into the command stream at record
    /// time, so nothing here is retained past the `render` call.
    pub push_data: Option<&'a [u8]>,
    /// The bindings filling the pipeline's descriptor slots, in slot
    /// order — slot `i` is descriptor set `i`, every sampled slot first
    /// and the uniform block last.
    ///
    /// Present exactly when the pipeline declares any slot at all, and
    /// exactly its declared count **and classes**: a block bound where a
    /// sampler is declared, or the reverse, binds a set the layout does
    /// not describe. All of it is refused by the frame contract before any
    /// GPU call, the same way push data matches its range. Named per draw
    /// rather than welded to the pipeline — that is what lets N textures
    /// share one pipeline.
    ///
    /// **Like a mesh, a binding may be named by any number of items**, and
    /// each distinct one costs one retention slot however many mentions.
    ///
    /// For a *sampled* binding nothing copies into it, so the buffer rule
    /// does not reach it at all. For a **uniform block it does**: the
    /// block's bytes are copied into the buffer its binding reads, so two
    /// items naming one block binding must carry pointer-identical bytes,
    /// exactly as two items naming one per-frame buffer must. See
    /// [`Item::uniform_data`].
    pub bindings: Option<Bindings<'a>>,
    /// Bytes written into the pipeline's uniform block for this draw.
    ///
    /// **Per draw as written, per buffer as stored, and the distinction
    /// matters.** Each item states its own block, but the bytes land in
    /// the buffer its binding reads — and the one-buffer-one-write rule
    /// refuses two items carrying *different* bytes for one buffer. So
    /// several draws sharing one block (a camera, a light list) is the
    /// shape this serves, and it costs one buffer and one binding; several
    /// draws wanting *different* blocks in one frame needs a buffer and a
    /// binding each. A block ring suballocated per draw is the answer the
    /// day a consumer needs that, and no consumer does yet.
    ///
    /// Present exactly when the pipeline declares a block, and exactly
    /// its declared length; both are refused by the frame contract
    /// before any GPU call, the same way push data matches its range.
    ///
    /// **Unlike push data, these bytes land in memory**, so they follow
    /// the buffer rule rather than the push rule: the block's buffer is
    /// the one its binding reads, the copy happens where every per-frame
    /// copy happens, and two items writing *different* bytes to one
    /// block would have the second silently win before either draws —
    /// refused, exactly as for [`FrameData`].
    pub uniform_data: Option<&'a [u8]>,
}

impl<'a> Item<'a> {
    /// A draw with `pipeline` and nothing else.
    #[must_use]
    pub fn new(pipeline: &'a RenderPipeline) -> Self {
        Self {
            pipeline,
            mesh: None,
            frame_data: None,
            uniform_data: None,
            push_data: None,
            bindings: None,
        }
    }

    /// Walk `mesh`, making this an indexed draw of its whole index list.
    #[must_use]
    pub fn mesh(mut self, mesh: &'a Mesh) -> Self {
        self.mesh = Some(mesh);
        self
    }

    /// Carry per-frame bytes and an instanced draw in this item.
    #[must_use]
    pub fn frame_data(mut self, data: FrameData<'a>) -> Self {
        self.frame_data = Some(data);
        self
    }

    /// Push `bytes` as the pipeline's push-constant block for this
    /// draw. Must be exactly the length the pipeline declared.
    #[must_use]
    pub fn push_data(mut self, bytes: &'a [u8]) -> Self {
        self.push_data = Some(bytes);
        self
    }

    /// Fill the pipeline's descriptor slots with `bindings`, in slot
    /// order: every sampled slot first, then the uniform block if the
    /// pipeline declares one. Must be exactly the count the pipeline
    /// declared, and of the classes it declared, in that order.
    ///
    /// The references are copied into the item — the slice itself may
    /// be a temporary.
    #[must_use]
    pub fn bindings(mut self, bindings: &[&'a Binding]) -> Self {
        self.bindings = Some(Bindings::new(bindings));
        self
    }

    /// Write `bytes` into the pipeline's uniform block for this draw.
    /// Must be exactly the length the pipeline declared.
    ///
    /// The bytes are copied into the block's buffer, at the region for
    /// the frame being recorded, before anything is submitted — so the
    /// slice may be a temporary of the caller's frame, exactly as
    /// [`FrameData`]'s is.
    #[must_use]
    pub fn uniform_data(mut self, bytes: &'a [u8]) -> Self {
        self.uniform_data = Some(bytes);
        self
    }
}

/// A pass's draw list, built on the stack.
///
/// **The shape every consumer with an optional draw arrives at.** A
/// frame whose middle item is conditional cannot be one array literal,
/// so callers reach for one of three things: two whole branches that
/// each build an array and each call `render` (the duplication is in
/// the render call, which is the part worth writing once), a `Vec`
/// (a heap allocation per frame, in a path whose whole discipline is
/// not to), or an array of `Option`s that nothing downstream accepts.
/// This is the fourth: fixed capacity, pushed conditionally, handed
/// over as a slice.
///
/// Capacity is a const parameter rather than a ceiling this crate
/// picks, because a draw list is the caller's shape — [`Pass`] itself
/// takes any slice, and nothing here bounds how many items a frame may
/// carry.
///
/// Sketched rather than compiled — every item needs a live pipeline,
/// which needs a device; the device suite drives the real thing.
///
/// ```ignore
/// let mut items = ItemList::<3>::new(world.item(&mesh, &matrices));
/// if live > 0 {
///     items.push(dust.item(&instances, live, &push));
/// }
/// items.push(overlay.item(&crosshair));
/// let passes = [Pass::new(&color, items.as_slice())];
/// ```
/// **Deliberately not `Copy`.** This is an accumulator driven by
/// `&mut self`: a copy taken mid-build would swallow every later push
/// with no diagnostic, which is the failure mode `Range` refuses `Copy`
/// to avoid. Nothing in the tree wants one.
pub struct ItemList<'a, const N: usize> {
    /// Seeded with the first item and overwritten by pushes: `Item` is
    /// `Copy` and has no meaningful empty value, so a filled array with
    /// a live count is the shape that needs no `unsafe` and no
    /// `Option` the caller would have to strip.
    items: [Item<'a>; N],
    count: usize,
}

impl<'a, const N: usize> ItemList<'a, N> {
    /// A list holding `first`.
    ///
    /// Seeded rather than empty because a pass drawing nothing is
    /// written `&[]` — clearer than a list that happens to have had
    /// nothing pushed into it, and it keeps this type's slice
    /// non-empty by construction.
    ///
    /// # Panics
    ///
    /// A zero capacity cannot hold the seed, which is a caller mistake
    /// no value can express — asserted rather than returned.
    #[must_use]
    pub fn new(first: Item<'a>) -> Self {
        assert!(
            N > 0,
            "an item list holds at least the item it is seeded with"
        );
        Self {
            items: [first; N],
            count: 1,
        }
    }

    /// Append `item`.
    ///
    /// # Panics
    ///
    /// Past the declared capacity. The capacity is the caller's own
    /// number and the count is known where the list is built, so an
    /// overflow is a mistake in one place rather than a condition to
    /// handle — and silently dropping a draw would be a frame that
    /// renders differently than written.
    pub fn push(&mut self, item: Item<'a>) {
        assert!(
            self.count < N,
            "an item list of capacity {N} is full; item {} was pushed",
            self.count + 1
        );
        self.items[self.count] = item;
        self.count += 1;
    }

    /// Append `item` when there is one — the optional-draw shape, so a
    /// caller writes no `if let` around a push.
    ///
    /// # Panics
    ///
    /// As [`Self::push`], when the item is present.
    pub fn push_some(&mut self, item: Option<Item<'a>>) {
        if let Some(item) = item {
            self.push(item);
        }
    }

    /// The items pushed so far, in order — what [`Pass`] takes.
    #[must_use]
    pub fn as_slice(&self) -> &[Item<'a>] {
        &self.items[..self.count]
    }
}

impl<const N: usize> fmt::Debug for ItemList<'_, N> {
    /// The count and the capacity, not the draws — [`RenderDesc`]'s own
    /// posture.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ItemList")
            .field("items", &self.count)
            .field("capacity", &N)
            .finish_non_exhaustive()
    }
}

/// The colour attachment a frame renders into: cleared to `clear`,
/// stored.
///
/// **One definition and one name, because three crates had written it
/// identically.** The 2D renderer, the 3D renderer and the triangle
/// sample each carried the same two-line function, so a caller
/// composing a frame from more than one of them imported whichever it
/// happened to name first. Forwarding wrappers would have left that
/// ambiguity in place, so the wrappers are gone: this is the name
/// every consumer uses. The depth attachment deliberately does NOT
/// join it — that one encodes the reversed-Z convention and refuses to
/// take the clear value as a parameter, which is a renderer's policy
/// rather than the frame vocabulary's.
#[must_use]
pub fn color_attachment(clear: Color) -> Attachment {
    Attachment::new(LoadOp::Clear(ClearValue::Color(clear)), StoreOp::Store)
}

/// An item's binding list: up to [`MAX_SAMPLED_BINDINGS`] references,
/// stored inline so [`Item`] stays `Copy` and borrows no caller-owned
/// slice storage.
///
/// The fields are private because they carry an invariant the
/// constructor proves: the first `count` slots are `Some`, the rest
/// `None`.
#[derive(Clone, Copy)]
pub struct Bindings<'a> {
    slots: [Option<&'a Binding>; MAX_SAMPLED_BINDINGS],
    count: u8,
}

impl<'a> Bindings<'a> {
    /// Copy `list`'s references inline, in order.
    ///
    /// # Panics
    ///
    /// Over [`MAX_SAMPLED_BINDINGS`] entries — the same ceiling a
    /// pipeline's slot declaration is held to, asserted rather than
    /// returned because the list was never valid anywhere.
    #[must_use]
    pub fn new(list: &[&'a Binding]) -> Self {
        // The message value is bound first and captured inline: a call
        // left inside the argument list is a region that runs only on
        // failure, which is a hole in the coverage gate.
        let named = list.len();
        assert!(
            named <= MAX_SAMPLED_BINDINGS,
            "an item names at most {MAX_SAMPLED_BINDINGS} bindings, got {named}"
        );
        let mut slots = [None; MAX_SAMPLED_BINDINGS];
        for (slot, binding) in slots.iter_mut().zip(list) {
            *slot = Some(*binding);
        }
        Self {
            slots,
            // The assert above bounds the length far inside u8.
            #[allow(clippy::cast_possible_truncation)]
            count: named as u8,
        }
    }

    /// How many slots are filled.
    #[must_use]
    pub fn len(&self) -> usize {
        self.count as usize
    }

    /// Whether no slots are filled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The named bindings, in slot order.
    ///
    /// Flattening the option array is exact, not defensive: the
    /// constructor's invariant puts every `Some` in the leading
    /// `count` slots, so this yields exactly them, in order, with no
    /// panic path for a hole that cannot exist.
    pub fn iter(&self) -> impl Iterator<Item = &'a Binding> + '_ {
        self.slots.iter().flatten().copied()
    }
}

impl fmt::Debug for Bindings<'_> {
    /// Reports the count, not the handles — the shape is the useful
    /// part, matching [`RenderDesc`]'s own Debug.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bindings")
            .field("count", &self.count)
            .finish_non_exhaustive()
    }
}

/// One resource a recorded frame references and must outlive.
///
/// **The arms exist so retention has one table and one clearing
/// rule.** Release sites only ever write `None`, so none of them cares
/// which arm they hold — which is what let the binding class join
/// without touching a single one of the four proofs that decide when
/// memory may die, and lets the next class do the same.
pub(crate) enum Retained {
    /// A per-frame buffer whose slot region the frame copied into.
    ///
    /// **Never read through — held for its `Drop` alone**: the
    /// recorded command stream holds the Vulkan handles the GPU uses,
    /// and this holds the right to keep those handles valid until the
    /// work has provably ended.
    #[allow(
        dead_code,
        reason = "held to keep the allocation alive across a submit, never read through"
    )]
    Frame(std::rc::Rc<crate::vk::buffer::BufferInner>),
    /// Geometry the frame's draws walk. Read only to recognise a mesh
    /// already retained this frame, so several items may name one mesh
    /// without spending a slot each.
    Mesh(std::rc::Rc<crate::vk::mesh::MeshInner>),
    /// A descriptor set the frame's draws sample through. Read only to
    /// recognise a binding already retained this frame, exactly as a
    /// mesh is — nothing copies into a binding, so items may share one
    /// freely.
    Binding(std::rc::Rc<crate::vk::binding::BindingInner>),
    /// A render image some pass targets. Retained by the pass walk
    /// rather than by any item — the recorded attachment references it
    /// whether or not anything samples it. Read only to recognise an
    /// image already retained this frame; a sampled-only image rides
    /// its binding's hold instead.
    Image(std::rc::Rc<crate::vk::render_image::RenderImageInner>),
}

/// Whether `resource` is already held in `held` — true for every
/// class, so several mentions of one resource spend one slot.
///
/// **One definition, consumed by both targets' fill loops**, so the
/// recognition rule cannot drift between them. Buffers joined the
/// recognised classes when the retention rule relaxed to
/// one-buffer-one-`FrameData`: identical frame data may now repeat
/// across items, so a repeated buffer can reach retention.
pub(crate) fn already_retained(resource: &Retained, held: &[Option<Retained>]) -> bool {
    match resource {
        Retained::Frame(buffer) => held.iter().any(|slot| {
            matches!(slot, Some(Retained::Frame(seen)) if std::rc::Rc::ptr_eq(seen, buffer))
        }),
        Retained::Mesh(mesh) => held.iter().any(|slot| {
            matches!(slot, Some(Retained::Mesh(seen)) if std::rc::Rc::ptr_eq(seen, mesh))
        }),
        Retained::Binding(binding) => held.iter().any(|slot| {
            matches!(slot, Some(Retained::Binding(seen)) if std::rc::Rc::ptr_eq(seen, binding))
        }),
        Retained::Image(image) => held.iter().any(|slot| {
            matches!(slot, Some(Retained::Image(seen)) if std::rc::Rc::ptr_eq(seen, image))
        }),
    }
}

/// Everything one item's recorded work references, in retention order.
///
/// **A total match over the item's shape, in one place, and that is the
/// point.** Both targets' fill loops used to key retention on
/// `frame_data` being `Some`, so any new resource-bearing field would
/// have been skipped silently — memory freed under a live submit, on the
/// asynchronous path only, where freed-but-untouched memory usually still
/// reads fine. Adding a resource-bearing field to [`Item`] now fails to
/// compile here rather than passing every test — the binding list
/// entered through exactly this door.
pub(crate) fn retained_of(item: &Item<'_>) -> [Option<Retained>; MAX_ITEM_RESOURCES] {
    // **Destructured with no `..` rest pattern, and that is the whole
    // mechanism.** Matching a locally-built tuple would compile happily
    // when a new resource-bearing field appeared on `Item` — the
    // guarantee this function advertises would be fiction. Naming every
    // field makes the addition a compile error *here*, which is the one
    // place that has to learn about it. `#[non_exhaustive]` does not
    // apply inside the defining crate, so the pattern really is total.
    let Item {
        pipeline: _,
        mesh,
        frame_data,
        // Copied into the command stream by the record path's push
        // call, so no allocation outlives `render` — nothing to retain.
        push_data: _,
        // Copied into a buffer rather than into the command stream, so
        // memory *is* read past `render` — but the buffer is not named
        // here. It belongs to the uniform binding in the list below,
        // which holds it; retaining the binding retains the buffer, so
        // adding an arm for it would count one allocation twice and
        // spend a retention slot on nothing.
        uniform_data: _,
        bindings,
    } = item;
    let mut out = [const { None }; MAX_ITEM_RESOURCES];
    let mut count = 0usize;
    if let Some(mesh) = mesh {
        out[count] = Some(Retained::Mesh(std::rc::Rc::clone(&mesh.inner)));
        count += 1;
    }
    if let Some(data) = frame_data {
        out[count] = Some(Retained::Frame(std::rc::Rc::clone(&data.buffer.inner)));
        count += 1;
    }
    if let Some(bindings) = bindings {
        for binding in bindings.iter() {
            // In bounds by construction: the list is capped at
            // MAX_SAMPLED_BINDINGS and the array leaves room for it
            // beside the two singleton classes.
            out[count] = Some(Retained::Binding(std::rc::Rc::clone(&binding.inner)));
            count += 1;
        }
    }
    out
}

/// Every uniform block this item writes this frame, as the buffer that
/// holds it and the bytes going into it.
///
/// **A filter rather than a loop with a skip arm.** The obvious shape —
/// walk the bindings, `continue` past the sampled ones — has a branch
/// that only a pipeline mixing sampled slots with a block ever takes, so
/// it sits unexercised in two record paths. Expressed as a filter there
/// is no arm to leave untaken.
///
/// At most one pair today, because a pipeline declares at most one block.
/// An iterator rather than an `Option` because the two record paths then
/// read the same shape whatever the count becomes — but note that a second
/// block is *not* a change confined to the declaration: `Item::uniform_data`
/// is one slice, and two blocks would need an ordered list paired index to
/// index. The iterator saves the targets from changing, not the item.
pub(crate) fn uniform_writes<'a>(
    item: &'a Item<'a>,
) -> impl Iterator<Item = (&'a std::rc::Rc<BufferInner>, &'a [u8])> {
    item.uniform_data.into_iter().flat_map(move |bytes| {
        item.bindings
            .iter()
            .flat_map(Bindings::iter)
            .filter_map(move |binding| binding.inner.uniform_buffer().map(|buffer| (buffer, bytes)))
    })
}

/// The most resources one item can reference: its mesh, its per-frame
/// buffer, and a full binding list. Sizes [`retained_of`]'s answer, so
/// the fill loops stay allocation-free.
pub(crate) const MAX_ITEM_RESOURCES: usize = 2 + MAX_SAMPLED_BINDINGS;

/// The frame's per-identity image walk: which attachment identities
/// this frame has used, and how -- ONE definition read by the contract
/// and by both record paths.
///
/// **The rules and the barrier pairs live in the same state machine,
/// and that is the point.** The contract drives a walk over the whole
/// frame before any GPU call, so every refusal below fires there, by
/// name; each record path then drives a fresh walk over the same
/// frame and reads the [`ImageUse`] pairs the contract already proved
/// legal, feeding them to `transition::pass_boundary`. Two targets
/// selecting masks independently is how barrier drift happens; two
/// consumers of one walk cannot drift.
/// Why a first-frame-use `LoadOp::Load` on a render image is refused,
/// if it is — `None` exactly when kept contents are there to load. A
/// pure function so the refusals read as one table.
fn load_refusal(kept: bool, arrived: KeptContents) -> Option<&'static str> {
    match (kept, arrived) {
        (true, KeptContents::Stored | KeptContents::Sampled) => None,
        (true, KeptContents::Undefined) => Some(
            "LoadOp::Load on a kept render image no frame has ever rendered loads undefined \
             contents — render it once before loading it",
        ),
        (true, KeptContents::Discarded) => Some(
            "LoadOp::Load on a kept render image whose last keeping frame discarded its \
             contents loads undefined pixels — store what a later frame loads",
        ),
        (false, _) => Some(
            "LoadOp::Load on a render image's first use this frame loads undefined contents \
             — render-image contents are frame-scoped and start undefined every frame",
        ),
    }
}

/// Why sampling a render image no pass this frame has rendered is
/// refused, if it is — `None` exactly when kept contents are there to
/// read.
fn sample_refusal(kept: bool, arrived: KeptContents) -> Option<&'static str> {
    match (kept, arrived) {
        (true, KeptContents::Stored | KeptContents::Sampled) => None,
        (true, KeptContents::Undefined) => Some(
            "an item samples a kept render image no frame has ever rendered — render it once \
             before reading it",
        ),
        (true, KeptContents::Discarded) => Some(
            "an item samples a kept render image whose last keeping frame discarded its \
             contents — store what a later frame reads",
        ),
        (false, _) => Some(
            "an item samples a render image no pass in this frame has rendered — render-image \
             contents are frame-scoped, so a frame that reads one must write it first",
        ),
    }
}

pub(crate) struct FrameWalk {
    surface_color_used: bool,
    target_depth_used: bool,
    images: [Option<ImageEntry>; MAX_FRAME_RENDER_IMAGES],
}

struct ImageEntry {
    key: *const u8,
    kind: RenderImageKind,
    targeted: bool,
    sampled: bool,
    /// Whether the image's LAST targeting pass discarded its contents
    /// — what decides both whether a later `Load` reads anything and
    /// whether a later sample does.
    discarded: bool,
    /// What an earlier frame left in it, snapshotted at the entry's
    /// mint so both walks of one frame read one answer.
    arrived: KeptContents,
    /// The image to write the after-state into at [`FrameWalk::settle`]
    /// — held exactly for kept images (`Some` IS the kept flag), and
    /// only the record walks settle.
    write_back: Option<Rc<crate::vk::render_image::RenderImageInner>>,
}

/// What one pass does to its target identities, as [`ImageUse`] pairs
/// ready for `transition::pass_boundary`. `color` is `None` exactly
/// for depth-image passes, which have no color attachment at all.
pub(crate) struct TargetUses {
    pub(crate) color: Option<(ImageUse, ImageUse)>,
    pub(crate) depth: Option<(ImageUse, ImageUse)>,
}

/// The most barriers one pass boundary can need: its color and depth
/// attachments plus every image crossing to sampled at this boundary.
/// Sizes both record paths' barrier arrays, so the frame path
/// allocates nothing.
pub(crate) const MAX_PASS_BARRIERS: usize = 2 + MAX_FRAME_RENDER_IMAGES;

/// One sampling transition a pass forces: the image, its barrier
/// subresource, and the use pair. Emitted once per image, at the first
/// sampling pass's boundary.
pub(crate) struct SampleUse {
    pub(crate) image: ash::vk::Image,
    pub(crate) range: ash::vk::ImageSubresourceRange,
    pub(crate) uses: (ImageUse, ImageUse),
}

impl FrameWalk {
    pub(crate) fn new() -> Self {
        Self {
            surface_color_used: false,
            target_depth_used: false,
            images: [const { None }; MAX_FRAME_RENDER_IMAGES],
        }
    }

    /// The occupied slot for `key`, minting one on first mention.
    ///
    /// # Panics
    ///
    /// A fifth distinct image is refused by name.
    fn entry_index(&mut self, inner: &Rc<crate::vk::render_image::RenderImageInner>) -> usize {
        let key = Rc::as_ptr(inner).cast::<u8>();
        let known = self
            .images
            .iter()
            .position(|slot| matches!(slot, Some(entry) if entry.key == key));
        if let Some(index) = known {
            return index;
        }
        // Slots are minted densely, so the occupied count IS the first
        // free index — and the ceiling refusal in one comparison.
        let occupied = self.images.iter().flatten().count();
        assert!(
            occupied < MAX_FRAME_RENDER_IMAGES,
            "a frame touches at most {MAX_FRAME_RENDER_IMAGES} distinct render images (as \
             targets and sampled sources together)"
        );
        self.images[occupied] = Some(ImageEntry {
            key,
            kind: inner.kind,
            targeted: false,
            sampled: false,
            discarded: false,
            arrived: inner.contents.get(),
            write_back: inner.kept.then(|| Rc::clone(inner)),
        });
        occupied
    }

    /// Advance the walk over `pass`'s target, returning the use pairs
    /// its attachments transition through.
    ///
    /// # Panics
    ///
    /// The identity rules, refused by name: a contents-preserving load
    /// on any identity's first use of the frame; re-targeting a render
    /// image after a pass sampled it; a fifth distinct image.
    pub(crate) fn advance_target(&mut self, index: usize, pass: &Pass<'_>) -> TargetUses {
        match &pass.target {
            PassTarget::Surface => {
                let color = &pass.color[0];
                assert!(
                    self.surface_color_used || !matches!(color.load, LoadOp::Load),
                    "pass {index}: LoadOp::Load on the frame's first surface use loads \
                     undefined contents -- every frame's first use of each attachment starts \
                     undefined"
                );
                let color_pair = if self.surface_color_used {
                    (ImageUse::ColorAttachment, ImageUse::ColorAttachment)
                } else {
                    (ImageUse::ColorAttachmentFirstUse, ImageUse::ColorAttachment)
                };
                self.surface_color_used = true;
                let depth_pair = pass.depth.as_ref().map(|depth| {
                    assert!(
                        self.target_depth_used || !matches!(depth.load, LoadOp::Load),
                        "pass {index}: LoadOp::Load on the frame's first depth use loads \
                         undefined contents -- the depth image transitions from UNDEFINED at \
                         its first carrying pass"
                    );
                    let pair = if self.target_depth_used {
                        (ImageUse::DepthAttachment, ImageUse::DepthAttachment)
                    } else {
                        (ImageUse::DepthAttachmentFirstUse, ImageUse::DepthAttachment)
                    };
                    self.target_depth_used = true;
                    pair
                });
                TargetUses {
                    color: Some(color_pair),
                    depth: depth_pair,
                }
            }
            PassTarget::Image(image, attachment) => {
                let kind = image.inner.kind;
                let slot = self.entry_index(&image.inner);
                let Some(entry) = self.images[slot].as_mut() else {
                    unreachable!("entry_index returns an occupied slot")
                };
                assert!(
                    !entry.sampled,
                    "pass {index}: this frame already sampled this render image — the \
                     per-image walk is one-way (target, then sample), so every pass that \
                     writes an image must precede the first pass that reads it"
                );
                let first_use = !entry.targeted;
                if first_use && matches!(attachment.load, LoadOp::Load) {
                    // A kept image with live contents may open with a
                    // load; everything else meets the frame-scoped
                    // refusals, each naming its own reason.
                    let refusal = load_refusal(entry.write_back.is_some(), entry.arrived);
                    assert!(
                        refusal.is_none(),
                        "pass {index}: {}",
                        refusal.unwrap_or_default()
                    );
                }
                // Loading what the last targeting pass threw away is the
                // same undefined read one pass later.
                assert!(
                    first_use || !matches!(attachment.load, LoadOp::Load) || !entry.discarded,
                    "pass {index}: LoadOp::Load on a render image whose last targeting \
                     pass discarded its contents loads undefined pixels — store what a \
                     later pass loads"
                );
                // The first frame-use pair: a frame-scoped image (or a
                // kept one nothing ever rendered) starts undefined; a
                // kept image re-enters from whichever layout its last
                // frame left — the attachment layout preserves through
                // the plain between-pass arms, the sampled layout walks
                // back through the Kept*FromSampled arms.
                let kept = entry.write_back.is_some();
                let kept_alive = kept && entry.arrived != KeptContents::Undefined;
                let from_sampled = kept && entry.arrived == KeptContents::Sampled;
                entry.targeted = true;
                entry.discarded = matches!(attachment.store, StoreOp::Discard);
                match kind {
                    RenderImageKind::Color => TargetUses {
                        color: Some(if !first_use || kept_alive && !from_sampled {
                            (ImageUse::ColorAttachment, ImageUse::ColorAttachment)
                        } else if from_sampled {
                            (ImageUse::KeptColorFromSampled, ImageUse::ColorAttachment)
                        } else {
                            (ImageUse::RenderColorFirstUse, ImageUse::ColorAttachment)
                        }),
                        depth: None,
                    },
                    RenderImageKind::Depth => TargetUses {
                        color: None,
                        depth: Some(if !first_use || kept_alive && !from_sampled {
                            (ImageUse::DepthAttachment, ImageUse::DepthAttachment)
                        } else if from_sampled {
                            (ImageUse::KeptDepthFromSampled, ImageUse::DepthAttachment)
                        } else {
                            (ImageUse::RenderDepthFirstUse, ImageUse::DepthAttachment)
                        }),
                    },
                    // No rest arm: `#[non_exhaustive]` does not bind
                    // inside the defining crate, so a new kind fails to
                    // compile HERE -- the one place that must learn its
                    // barriers -- rather than panicking at runtime.
                }
            }
        }
    }

    /// Advance the walk over `pass`'s sampled render images, returning
    /// the transitions the first sampling of each forces, in first-use
    /// order.
    ///
    /// # Panics
    ///
    /// Sampling an image no pass in this frame has rendered, and
    /// sampling the image the pass itself renders into -- both refused
    /// by name.
    pub(crate) fn advance_sampling(
        &mut self,
        index: usize,
        pass: &Pass<'_>,
    ) -> ([Option<SampleUse>; MAX_FRAME_RENDER_IMAGES], usize) {
        let own_target = match &pass.target {
            PassTarget::Image(image, _) => Some(Rc::as_ptr(&image.inner).cast::<u8>()),
            PassTarget::Surface => None,
        };
        let mut out = [const { None }; MAX_FRAME_RENDER_IMAGES];
        let mut count = 0usize;
        for item in pass.items {
            let Some(bindings) = &item.bindings else {
                continue;
            };
            for binding in bindings.iter() {
                let Some(inner) = binding.inner.sampled_render_image() else {
                    continue;
                };
                let key = Rc::as_ptr(inner).cast::<u8>();
                assert!(
                    own_target != Some(key),
                    "pass {index}: an item samples the render image this pass renders \
                     into -- feedback within one pass is undefined; split it into a \
                     writing pass and a reading pass"
                );
                // A kept image with live contents may be sampled without
                // any pass this frame rendering it; everything else must
                // have been written this frame, refused by name.
                let slot = self
                    .images
                    .iter()
                    .position(|slot| matches!(slot, Some(entry) if entry.key == key));
                let written_this_frame =
                    slot.is_some_and(|slot| self.images[slot].iter().any(|entry| entry.targeted));
                if !written_this_frame {
                    let refusal = sample_refusal(inner.kept, inner.contents.get());
                    assert!(
                        refusal.is_none(),
                        "pass {index}: {}",
                        refusal.unwrap_or_default()
                    );
                }
                let slot = match slot {
                    Some(slot) => slot,
                    None => self.entry_index(inner),
                };
                let Some(entry) = self.images[slot].as_mut() else {
                    unreachable!("entry_index returns an occupied slot")
                };
                assert!(
                    !(entry.targeted && entry.discarded),
                    "pass {index}: an item samples a render image whose last targeting \
                     pass discarded its contents — a targeting pass whose image is read \
                     later must Store"
                );
                if entry.sampled {
                    continue;
                }
                entry.sampled = true;
                // The crossing barrier. Written this frame: from the
                // attachment life this frame built. Kept and untouched:
                // from the attachment life an earlier frame stored — the
                // same pair, because the layouts and hazards match. Kept
                // and already sampled when it arrived: it never left the
                // sampled layout, and read-after-read needs no barrier
                // at all.
                if !entry.targeted && entry.arrived == KeptContents::Sampled {
                    continue;
                }
                out[count] = Some(SampleUse {
                    image: inner.image,
                    range: match entry.kind {
                        RenderImageKind::Depth => crate::vk::depth::barrier_range(inner.format),
                        _ => ash::vk::ImageSubresourceRange::default()
                            .aspect_mask(ash::vk::ImageAspectFlags::COLOR)
                            .base_mip_level(0)
                            .level_count(1)
                            .base_array_layer(0)
                            .layer_count(1),
                    },
                    uses: match entry.kind {
                        RenderImageKind::Depth => {
                            (ImageUse::DepthAttachment, ImageUse::SampledInPass)
                        }
                        _ => (ImageUse::ColorAttachment, ImageUse::SampledInPass),
                    },
                });
                count += 1;
            }
        }
        (out, count)
    }

    /// Whether any pass so far targeted the surface.
    pub(crate) fn surface_used(&self) -> bool {
        self.surface_color_used
    }

    /// Write each kept image's after-state back onto the image, so the
    /// next frame's walk arrives where this one left.
    ///
    /// **Record walks only.** The contract's dry walk reads the same
    /// cross-frame state and must not move it — the dry walk and the
    /// record walk of one frame have to read one answer, and a frame
    /// that is refused must leave no trace.
    pub(crate) fn settle(&self) {
        for entry in self.images.iter().flatten() {
            let Some(inner) = &entry.write_back else {
                continue;
            };
            let after = if entry.sampled {
                KeptContents::Sampled
            } else if entry.targeted {
                if entry.discarded {
                    KeptContents::Discarded
                } else {
                    KeptContents::Stored
                }
            } else {
                continue;
            };
            inner.contents.set(after);
        }
    }
}

/// The frame-shape contract, asserted identically by both targets
/// before any GPU call: the refusals that make a malformed frame a
/// named panic instead of a validation stream or a quiet wrong image.
///
/// Retained in release builds: every rule below guards either a
/// memory-ordering argument (first-pass loads of undefined contents,
/// two copies into one region) or a draw that renders differently than
/// written. The per-item device and format checks stay beside each
/// target's own device state, where they always were.
pub(crate) fn check_frame_contract(desc: &RenderDesc<'_>) {
    assert!(
        !desc.passes.is_empty(),
        "a frame needs at least one pass: an empty frame records nothing, and on the window          path would present contents nothing ever defined"
    );
    let mut walk = FrameWalk::new();
    for (index, pass) in desc.passes.iter().enumerate() {
        check_pass_shape(index, pass);
        // The identity rules: first-use loads, the one-way per-image
        // walk, the distinct-image ceiling. One state machine, shared
        // with the record paths.
        let _ = walk.advance_target(index, pass);
        for item in pass.items {
            // A depth-only pipeline draws only into depth images —
            // anywhere else its zero-attachment shape disagrees with
            // the pass's rendering instance, which is invalid usage the
            // driver may answer with anything. Retained, unlike the
            // surface format match: the consequence is not a channel
            // swap but an undefined draw.
            let depth_image_pass = matches!(
                &pass.target,
                PassTarget::Image(image, _) if image.kind() == RenderImageKind::Depth
            );
            assert!(
                depth_image_pass || item.pipeline.format != crate::TargetFormat::DepthOnly,
                "pass {index}: a depth-only pipeline draws only into depth-kinded render                  images — it has no fragment stage and no color attachment for any other                  pass shape to bind"
            );
            // Image passes carry their format in their kind, so the
            // match is contract-checked here; a surface pass's format
            // is the target's own, asserted where the target knows it.
            if let PassTarget::Image(image, _) = &pass.target {
                let expected = match image.kind() {
                    RenderImageKind::Depth => crate::TargetFormat::DepthOnly,
                    _ => crate::TargetFormat::Rgba8Unorm,
                };
                assert!(
                    item.pipeline.format == expected,
                    "pass {index}: an item's pipeline format must match the image it \
                     renders into — a color image draws {:?} pipelines, a depth image \
                     {:?} ones; got {:?}",
                    crate::TargetFormat::Rgba8Unorm,
                    crate::TargetFormat::DepthOnly,
                    item.pipeline.format
                );
            }
            assert!(
                item.pipeline.depth == pass_has_depth(pass),
                "pass {index}: an item's pipeline depth state must match the pass — a \
                 depth-testing pipeline in a depthless pass (or the reverse) draws \
                 differently than written"
            );
            // The same shape as the depth rule above, for the same
            // reason: a mesh pipeline drawn without geometry reads an
            // unbound vertex binding, which is undefined rather than
            // merely wrong, and geometry handed to a generative pipeline
            // is silently ignored — a draw that renders differently than
            // written.
            assert!(
                item.pipeline.vertex_input == item.mesh.is_some(),
                "pass {index}: an item names geometry exactly when its pipeline declares \
                 per-vertex input — a mesh pipeline with no mesh reads an unbound binding, \
                 and a mesh on a pipeline that generates its own vertices is ignored"
            );
            // Retained rather than debug-only: this bounds every vertex
            // fetch the draw makes. Creation proved each index is inside
            // the mesh's own vertex count; that count means bytes only at
            // the stride the pipeline fetches with, so a disagreement
            // reads past the end of the allocation.
            if let Some(mesh) = item.mesh {
                assert!(
                    mesh.vertex_stride() == item.pipeline.vertex_stride,
                    "pass {index}: the mesh's vertex stride ({}) must equal the stride the \
                     pipeline's per-vertex layout packs to ({}) — a mismatch fetches past the \
                     end of the mesh",
                    mesh.vertex_stride(),
                    item.pipeline.vertex_stride
                );
            }
            // The same presence rule as geometry and depth, plus an
            // exact length. A declared range never pushed reads
            // undefined values; a push on a rangeless pipeline is
            // invalid usage the driver may answer with anything; and a
            // partial push leaves the block's tail undefined — every
            // one of them a quiet wrong draw, so every one is refused
            // by name.
            let declared = item.pipeline.push_constant_size as usize;
            assert!(
                item.push_data.is_some() == (declared > 0),
                "pass {index}: an item carries push data exactly when its pipeline declares a \
                 push-constant range — a declared range never pushed reads undefined values, \
                 and a push on a rangeless pipeline is invalid usage"
            );
            if let Some(bytes) = item.push_data {
                assert!(
                    bytes.len() == declared,
                    "pass {index}: push data must be exactly the declared push-constant range \
                     ({declared} bytes), got {} — a partial push leaves the block's tail \
                     undefined, and a surplus one is invalid usage",
                    bytes.len()
                );
            }
            check_binding_contract(index, item);
        }
        let _ = walk.advance_sampling(index, pass);
    }
    assert!(
        walk.surface_used(),
        "a frame needs at least one surface pass: image passes render intermediate \
         contents, and a frame that never touches the surface defines nothing to present \
         or read back"
    );
    check_retention_bound(desc);
}

/// The pass's shape follows its target: a surface pass names the
/// target's own surface as its one color attachment; an image pass
/// carries its one attachment in the target itself, so its slices stay
/// empty. Clear values must match the attachment's kind either way. A
/// sibling of [`check_retention_bound`] for the same reason: one rule
/// family, its own function.
fn check_pass_shape(index: usize, pass: &Pass<'_>) {
    match &pass.target {
        PassTarget::Surface => {
            assert!(
                pass.color.len() == 1,
                "pass {index}: a surface pass carries exactly one color attachment \
                 (the target's own surface), got {}",
                pass.color.len()
            );
            let color = &pass.color[0];
            if let LoadOp::Clear(value) = color.load {
                assert!(
                    matches!(value, ClearValue::Color(_)),
                    "pass {index}: a color attachment clears to ClearValue::Color, not \
                     ClearValue::Depth"
                );
            }
            if let Some(depth) = &pass.depth
                && let LoadOp::Clear(value) = depth.load
            {
                // Kind and range in one refusal, so no arm is
                // unreachable: a depth clear is valid exactly when it
                // is a Depth value that is finite and in the documented
                // range — anything else is invalid usage the driver may
                // answer with anything.
                let valid = match value {
                    ClearValue::Depth(depth_value) => {
                        depth_value.is_finite() && (0.0..=1.0).contains(&depth_value)
                    }
                    _ => false,
                };
                assert!(
                    valid,
                    "pass {index}: a depth attachment clears to ClearValue::Depth, \
                     finite and in [0, 1] — got {value:?}"
                );
            }
        }
        PassTarget::Image(image, attachment) => {
            assert!(
                pass.color.is_empty() && pass.depth.is_none(),
                "pass {index}: an image pass carries its one attachment in its \
                 target — surface color slices and the target-depth option name \
                 images this pass does not render into"
            );
            if let LoadOp::Clear(value) = attachment.load {
                let valid = match (image.kind(), value) {
                    (RenderImageKind::Color, ClearValue::Color(_)) => true,
                    (RenderImageKind::Depth, ClearValue::Depth(depth_value)) => {
                        depth_value.is_finite() && (0.0..=1.0).contains(&depth_value)
                    }
                    _ => false,
                };
                assert!(
                    valid,
                    "pass {index}: an image attachment clears to its kind's value — \
                     Color to ClearValue::Color, Depth to a finite ClearValue::Depth \
                     in [0, 1] — got {value:?}"
                );
            }
        }
    }
}

/// Whether a pass carries a depth attachment — the target's own depth
/// image on a surface pass, or the image itself when it is
/// depth-kinded.
pub(crate) fn pass_has_depth(pass: &Pass<'_>) -> bool {
    match &pass.target {
        PassTarget::Surface => pass.depth.is_some(),
        PassTarget::Image(image, _) => image.kind() == RenderImageKind::Depth,
    }
}

/// The push-data rules again, for sampled slots: a declared slot never
/// filled samples an unbound set, and bindings on a slotless pipeline
/// bind sets its layout does not declare — invalid usage either way,
/// refused by name. A sibling of [`check_retention_bound`] for the same
/// reason: one rule family, its own function.
fn check_binding_contract(index: usize, item: &Item<'_>) {
    let sampled = item.pipeline.sampled_bindings as usize;
    let block = item.pipeline.uniform_block as usize;
    // A block spends one set, after every sampled slot — the order
    // `create_pipeline` built its layout list in.
    let declared_slots = sampled + usize::from(block > 0);
    assert!(
        item.bindings.is_some() == (declared_slots > 0),
        "pass {index}: an item names bindings exactly when its pipeline declares \
         descriptor slots — a declared slot never filled samples an unbound set, and \
         bindings on a slotless pipeline are invalid usage"
    );
    if let Some(bindings) = &item.bindings {
        let named = bindings.len();
        assert!(
            named == declared_slots,
            "pass {index}: an item fills every declared slot, in order \
             ({declared_slots} declared, {named} named) — a partial fill leaves \
             unbound sets, and a surplus binds past the layout"
        );
        // **Class as well as count.** A set's layout fixes its
        // descriptor type, so a block bound where a sampler is declared
        // is a set the layout does not describe — invalid usage that
        // draws a wrong picture on a permissive driver rather than
        // failing.
        for (slot, binding) in bindings.iter().enumerate() {
            // **Class before size, because class is the more basic
            // mistake and its message is the one that helps.** A binding
            // of the wrong class also has the wrong size — a block bound
            // into a sampled slot of a pipeline that declares no block is
            // a 192-byte buffer against a zero-byte declaration — so a
            // size check running first answers a question the caller did
            // not get wrong yet. Found by making these refusals match
            // their clause by name: the class case was being refused, but
            // by the size rule.
            let wants_uniform = slot >= sampled;
            assert!(
                binding.is_uniform() == wants_uniform,
                "pass {index}: slot {slot} of this pipeline is {}, and the binding named                  there is {} — sampled slots come first and the uniform block last",
                if wants_uniform {
                    "a uniform block"
                } else {
                    "sampled"
                },
                if binding.is_uniform() {
                    "a uniform block"
                } else {
                    "sampled"
                }
            );
            // The block and the buffer that holds it are two statements of
            // one size, and the descriptor's range is the whole per-frame
            // capacity — so a buffer of any other size leaves the shader
            // reading bytes no frame writes.
            if let Some(capacity) = binding.inner.block_capacity() {
                assert!(
                    capacity == block,
                    "pass {index}: this pipeline declares a {block}-byte uniform block and                      the buffer bound for it holds {capacity} per frame — the block and its                      buffer are one size, so a shader cannot read past what was written"
                );
            }
        }
    }
    // The block's bytes follow the push range's rule exactly: present
    // when declared, absent when not, and never a different length.
    assert!(
        item.uniform_data.is_some() == (block > 0),
        "pass {index}: an item carries uniform data exactly when its pipeline declares \
         a block — a declared block never written reads the previous frame's bytes, and \
         data for an undeclared one has nowhere to go"
    );
    if let Some(bytes) = item.uniform_data {
        let carried = bytes.len();
        assert!(
            carried == block,
            "pass {index}: an item carries exactly its pipeline's declared uniform block \
             ({block} bytes declared, {carried} carried) — a short write leaves the tail \
             holding the previous frame's bytes"
        );
    }
}

/// The retention half of the frame contract: how many distinct resources
/// one frame may keep alive, and which of them may repeat.
///
/// Split out of [`check_frame_contract`] because it walks the frame a
/// second time for a different reason — the walk above is per pass and
/// about attachments, this one is per resource and about the retention
/// table's fixed width.
fn check_retention_bound(desc: &RenderDesc<'_>) {
    // Two rules, deliberately different:
    //
    // - **One buffer, one write, per frame.** Two items carrying
    //   DIFFERENT data for one per-frame buffer would have the second
    //   copy silently win before either draws — refused. Two items
    //   carrying the POINTER-IDENTICAL data (same bytes, same count)
    //   are one copy written twice: drawing the same instanced world
    //   from two passes is an ordinary thing to want, and it costs one
    //   retention slot. **Both write channels share the rule and the
    //   table**, because they share the hazard: a uniform block and an
    //   instance stream are the same memory written the same way, and a
    //   buffer used as both in one frame would race itself.
    // - **A mesh, a binding, or a pass-target image may repeat.**
    //   Nothing copies into them, so there is no race at all; each
    //   distinct one costs one retention slot however many mentions.
    let mut seen: [Option<*const u8>; MAX_RETAINED_RESOURCES] = [None; MAX_RETAINED_RESOURCES];
    let mut count = 0usize;
    // Per-buffer FrameData signatures: (bytes pointer, length, count).
    // Pointer identity, not content equality — the rule is about which
    // copy wins, and two copies of one allocation cannot disagree.
    let mut buffer_data: [Option<BufferRecord>; MAX_RETAINED_RESOURCES] =
        [None; MAX_RETAINED_RESOURCES];
    // The repeatable classes share one recognise-or-count arm; the
    // pointer key spaces cannot collide across classes, because each is
    // the address of a distinct live allocation.
    let count_repeatable = |seen: &mut [Option<*const u8>; MAX_RETAINED_RESOURCES],
                            count: &mut usize,
                            key: *const u8| {
        if !seen[..*count].contains(&Some(key)) {
            assert!(
                *count < MAX_RETAINED_RESOURCES,
                "a frame carries at most {MAX_RETAINED_RESOURCES} distinct resources \
                 (per-frame buffers, meshes, bindings and pass-target images together)"
            );
            seen[*count] = Some(key);
            *count += 1;
        }
    };
    for pass in desc.passes {
        // A pass-target image is retained by the pass walk, exactly
        // once however many passes target it — the mesh rule, one
        // resource class over.
        if let PassTarget::Image(image, _) = &pass.target {
            let key = std::rc::Rc::as_ptr(&image.inner).cast::<u8>();
            count_repeatable(&mut seen, &mut count, key);
        }
        for item in pass.items {
            if let Some(mesh) = item.mesh {
                let key = std::rc::Rc::as_ptr(&mesh.inner).cast::<u8>();
                count_repeatable(&mut seen, &mut count, key);
            }
            if let Some(bindings) = &item.bindings {
                for binding in bindings.iter() {
                    let key = std::rc::Rc::as_ptr(&binding.inner).cast::<u8>();
                    count_repeatable(&mut seen, &mut count, key);
                }
            }
            // The uniform block's bytes, keyed by the buffer its
            // binding reads. The instance field of the signature is
            // zero: this channel has no instance count, and a buffer
            // written through both channels in one frame is exactly the
            // collision the shared table exists to catch.
            for (buffer, bytes) in uniform_writes(item) {
                record_write(
                    &mut seen,
                    &mut count,
                    &mut buffer_data,
                    std::rc::Rc::as_ptr(buffer).cast::<u8>(),
                    (bytes.as_ptr(), bytes.len(), 0),
                    // The binding was counted just above; the buffer it
                    // holds must not be counted again.
                    Charge::Nothing,
                    &count_repeatable,
                );
            }
            let Some(data) = &item.frame_data else {
                continue;
            };
            let key = std::rc::Rc::as_ptr(&data.buffer.inner).cast::<u8>();
            record_write(
                &mut seen,
                &mut count,
                &mut buffer_data,
                key,
                (data.bytes.as_ptr(), data.bytes.len(), data.instances),
                Charge::RetentionSlot,
                &count_repeatable,
            );
        }
    }
}

/// Claim one write to `key`'s buffer, or prove it repeats a claim
/// already made.
///
/// **One definition read by both write channels**, because the rule is
/// about the memory rather than about which channel wrote it: an
/// instance stream and a uniform block copied into the same buffer race
/// each other exactly as two instance streams would.
///
/// # Panics
///
/// When a prior claim on the same buffer carried a different signature —
/// the second copy would silently win before either draw.
fn record_write(
    seen: &mut [Option<*const u8>; MAX_RETAINED_RESOURCES],
    count: &mut usize,
    records: &mut [Option<BufferRecord>; MAX_RETAINED_RESOURCES],
    key: *const u8,
    signature: (*const u8, usize, u32),
    charge: Charge,
    count_repeatable: &impl Fn(&mut [Option<*const u8>; MAX_RETAINED_RESOURCES], &mut usize, *const u8),
) {
    // **Charged before the early return, not after it.** The two write
    // channels can produce an equal signature for one buffer — a uniform
    // block and a zero-instance `FrameData` over pointer-identical bytes —
    // and a charge that only ran on the first claim would then let a
    // buffer be retained twice and counted never, overrunning the table
    // with an index panic instead of the named refusal. `count_repeatable`
    // dedups by key, so charging on every claim is idempotent.
    if charge == Charge::RetentionSlot {
        count_repeatable(seen, count, key);
    }
    let prior = records[..count_matters(records)]
        .iter()
        .flatten()
        .find(|(prior_key, _)| *prior_key == key);
    if let Some((_, prior_signature)) = prior {
        assert!(
            *prior_signature == signature,
            "one buffer, one write, per frame: two items carry different data for the \
             same buffer, and the second copy would silently win before either draws \
             (pointer-identical data may repeat)"
        );
        return;
    }
    let Some(slot) = records.iter_mut().find(|slot| slot.is_none()) else {
        // Unreachable, and the argument is longer than it was: `records`
        // is now filled by claims that may not increment `count`, so the
        // ceiling assert does not bound it directly. It still holds —
        // every distinct uniform buffer implies at least one distinct
        // `Binding`, charged in the same item iteration just above the
        // uniform loop — so `#records <= count <= MAX_RETAINED_RESOURCES`.
        // That depends on the loop order in `check_retention_bound`;
        // written down here because a reorder would break it silently.
        unreachable!("every claimed buffer implies a charged resource, bounded above")
    };
    *slot = Some((key, signature));
}

/// Whether a claimed write also spends one of the frame's retention slots.
///
/// **The two write channels differ here, and only here.** An item's
/// [`FrameData`] names its buffer directly, so the buffer is what the
/// target retains and it costs a slot. A uniform block's buffer is reached
/// through its [`Binding`], which the target retains instead — so charging
/// the buffer as well would spend two of the budget on one held resource,
/// and refuse a legal frame by a message naming a table with room in it.
///
/// The signature is recorded either way: the one-buffer-one-write rule is
/// about which copy wins, which has nothing to do with who holds the
/// memory alive.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Charge {
    /// The caller retains this buffer directly.
    RetentionSlot,
    /// Something else already retains it — record the claim, spend nothing.
    Nothing,
}

/// One buffer's recorded claim: its key, then its `FrameData`
/// signature — bytes pointer, length, instance count.
type BufferRecord = (*const u8, (*const u8, usize, u32));

/// How many leading buffer records are occupied — a helper the borrow
/// checker demands, not a second counter.
fn count_matters(records: &[Option<BufferRecord>; MAX_RETAINED_RESOURCES]) -> usize {
    records.iter().take_while(|slot| slot.is_some()).count()
}

/// How many distinct resources one frame may keep alive, per target slot
/// — the hard bound that keeps retention tables fixed-width and the frame
/// path allocation-free. Per-frame buffers, meshes, bindings and
/// pass-target render images share it, because they share one table. The
/// next distinct resource past the bound is refused by name in
/// [`check_frame_contract`].
///
/// Thirty-two, doubled from sixteen the day a real consumer's fullest
/// frame — a world mesh, a marker, a held item, billboard layers, two
/// cutout meshes, machines, two particle pipelines and an interface
/// overlay, each with its bindings — landed on exactly seventeen. It
/// was doubled from eight before that, when bindings joined the table.
/// The table is `Option`s in fixed arrays: the cost of headroom is
/// thirty-two words of `None` per frame, and the cost of too little is
/// a consumer cutting a feature to fit a constant this crate chose.
pub(crate) const MAX_RETAINED_RESOURCES: usize = 32;

impl LoadOp {
    pub(crate) fn to_vk(self) -> ash::vk::AttachmentLoadOp {
        match self {
            Self::Clear(_) => ash::vk::AttachmentLoadOp::CLEAR,
            Self::Load => ash::vk::AttachmentLoadOp::LOAD,
        }
    }
}

impl StoreOp {
    pub(crate) fn to_vk(self) -> ash::vk::AttachmentStoreOp {
        match self {
            Self::Store => ash::vk::AttachmentStoreOp::STORE,
            Self::Discard => ash::vk::AttachmentStoreOp::DONT_CARE,
        }
    }
}

/// The color clear an attachment's load op carries, or a zeroed value
/// for `Load` (the driver ignores it). The kind mismatch is refused by
/// [`check_frame_contract`] before any conversion runs.
pub(crate) fn vk_clear_color(attachment: &Attachment) -> ash::vk::ClearValue {
    match attachment.load {
        LoadOp::Clear(ClearValue::Color(color)) => ash::vk::ClearValue {
            color: ash::vk::ClearColorValue {
                float32: [color.r, color.g, color.b, color.a],
            },
        },
        _ => ash::vk::ClearValue::default(),
    }
}

/// The depth clear an attachment's load op carries, or a zeroed value
/// for `Load`. Kind mismatches are refused before conversion, as above.
pub(crate) fn vk_clear_depth(attachment: &Attachment) -> ash::vk::ClearValue {
    match attachment.load {
        LoadOp::Clear(ClearValue::Depth(depth)) => ash::vk::ClearValue {
            depth_stencil: ash::vk::ClearDepthStencilValue { depth, stencil: 0 },
        },
        _ => ash::vk::ClearValue::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The colour attachment three crates had written identically.**
    /// Pinned on content rather than smoke-called: the load op carries
    /// the clear value, the store op keeps it, and a caller that got
    /// either backwards would draw a frame that discarded what it just
    /// rendered.
    #[test]
    #[allow(unsafe_code)]
    fn the_colour_attachment_clears_to_its_value_and_stores() {
        let attachment = color_attachment(Color::new(0.25, 0.5, 0.75, 1.0));
        assert!(matches!(attachment.store, StoreOp::Store));
        // Read through the crate's own converter rather than matching
        // the load op: the converter is total, so this proves the same
        // thing with no arm that only a broken helper could reach —
        // and it proves the two agree, which a match would not.
        // SAFETY: reading the union arm the converter just wrote, as
        // the clear-value test beside this one does.
        let raw = unsafe { vk_clear_color(&attachment).color.float32 };
        // Bit equality: the helper moves the value, it never does
        // arithmetic on it.
        assert_eq!(
            raw.map(f32::to_bits),
            [0.25f32, 0.5, 0.75, 1.0].map(f32::to_bits)
        );
    }

    /// The binding list's device-free boundary: an empty list is a
    /// legal value of the type (the frame contract, not the
    /// constructor, is what refuses it on any pipeline), reporting
    /// itself consistently through every accessor. The populated side
    /// lives with the device suites, where a real binding exists to
    /// name.
    #[test]
    fn an_empty_binding_list_is_consistent_across_its_accessors() {
        let bindings = Bindings::new(&[]);
        assert_eq!(bindings.len(), 0);
        assert!(bindings.is_empty());
        assert_eq!(bindings.iter().count(), 0);
        let shown = format!("{bindings:?}");
        assert!(shown.contains("Bindings"), "{shown}");
        assert!(shown.contains("count: 0"), "{shown}");
    }

    /// The Debug form reports shape, not handles — pinned on content so
    /// the claim cannot rot into a mere smoke call.
    #[test]
    fn the_debug_form_reports_the_frames_shape() {
        let color = [Attachment::new(
            LoadOp::Clear(ClearValue::Color(Color::new(0.0, 0.0, 0.0, 1.0))),
            StoreOp::Store,
        )];
        let passes = [Pass::new(&color, &[]), Pass::new(&color, &[])];
        let shown = format!("{:?}", RenderDesc::new(&passes));
        assert!(shown.contains("RenderDesc"), "{shown}");
        assert!(shown.contains("passes: 2"), "{shown}");
        assert!(shown.contains("items_per_pass: [0, 0]"), "{shown}");
        // `finish_non_exhaustive` renders the trailing `..`, the signal
        // that the struct grows.
        assert!(shown.contains(".."), "{shown}");
    }

    /// The op conversions, both arms of each, with no device involved.
    #[test]
    fn every_op_maps_to_its_vulkan_spelling() {
        let clear = LoadOp::Clear(ClearValue::Color(Color::new(0.0, 0.0, 0.0, 1.0)));
        assert_eq!(clear.to_vk(), ash::vk::AttachmentLoadOp::CLEAR);
        assert_eq!(LoadOp::Load.to_vk(), ash::vk::AttachmentLoadOp::LOAD);
        assert_eq!(StoreOp::Store.to_vk(), ash::vk::AttachmentStoreOp::STORE);
        assert_eq!(
            StoreOp::Discard.to_vk(),
            ash::vk::AttachmentStoreOp::DONT_CARE
        );
    }

    /// The clear values ride their variants into the raw union. The
    /// module denies `unsafe`; this test alone allows it to read back
    /// the union arms the converters write.
    #[test]
    #[allow(unsafe_code)]
    fn clear_values_survive_conversion() {
        let color = Attachment::new(
            LoadOp::Clear(ClearValue::Color(Color::new(0.25, 0.5, 0.75, 1.0))),
            StoreOp::Store,
        );
        // SAFETY: reading the union arm the converter just wrote.
        let raw = unsafe { vk_clear_color(&color).color.float32 };
        // Bit equality: the converter moves the values, it never does
        // arithmetic on them.
        assert_eq!(
            raw.map(f32::to_bits),
            [0.25f32, 0.5, 0.75, 1.0].map(f32::to_bits)
        );
        let depth = Attachment::new(LoadOp::Clear(ClearValue::Depth(0.5)), StoreOp::Discard);
        // SAFETY: as above.
        let raw = unsafe { vk_clear_depth(&depth).depth_stencil };
        assert_eq!(raw.depth.to_bits(), 0.5f32.to_bits());
        assert_eq!(raw.stencil, 0);
        // A Load carries no clear: the converters hand the driver a
        // zeroed value it is required to ignore.
        let load = Attachment::new(LoadOp::Load, StoreOp::Store);
        // SAFETY: as above.
        let raw = unsafe { vk_clear_depth(&load).depth_stencil };
        assert_eq!(raw.depth.to_bits(), 0.0f32.to_bits());
    }
}
