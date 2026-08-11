//! The fixed-point flex solver: a trimmed flexbox subset over Q47.16.
//!
//! **The v0 surface, and why it is this small.** Row and column
//! containers; sizes in pixels or from content; start, centre, and end
//! placement on both axes; margin, padding, and gap; integer `grow`
//! that shares leftover space exactly. Percent sizes, min/max bounds,
//! shrink, wrapping, and space-between land when a real document needs
//! them — an interface with no consumer is the smell the defaults name.
//!
//! **Two passes, both iterative.** Measurement walks children before
//! parents to answer every `Auto` from content; arrangement walks
//! parents before children to place each child inside its parent's
//! content box. Neither recurses: the tree is data, and data must not
//! choose the stack depth. Both walks and the grow bookkeeping run in
//! scratch buffers sized once at construction, so re-solving allocates
//! exactly nothing — the same gate the tree itself sits behind.
//!
//! **Exact arithmetic, exactly shared.** All positions and sizes are
//! [`Fixed`]: every operation is integer arithmetic under the hood and
//! bit-identical on every target. Leftover space among growers is
//! shared by largest remainder over raw fixed-point units — shares sum
//! to the leftover *exactly*, never one unit over or under — and ties
//! break toward the earlier sibling, so the same tree always solves to
//! the same bits. At the far edge of the range, [`Fixed`]'s saturation
//! applies: sizes summing past ±2^47 pixels clamp rather than wrap,
//! deterministically, and children past the clamp stack at the edge.

use renew_fixed::Fixed;

/// Which way a container lays its children.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    /// Children advance along x; the cross axis is y.
    #[default]
    Row,
    /// Children advance along y; the cross axis is x.
    Column,
}

/// One dimension of a node's size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    /// Whatever the content needs: children plus padding, or nothing.
    #[default]
    Auto,
    /// Exactly this many pixels.
    Px(Fixed),
}

/// Where children sit along an axis when space is left over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// Packed toward the axis origin.
    #[default]
    Start,
    /// Centred; an odd leftover's extra raw unit lands before the
    /// run — the halving rounds to nearest, ties away from zero.
    Center,
    /// Packed toward the axis end.
    End,
}

/// Space on each side of a box, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    pub left: Fixed,
    pub right: Fixed,
    pub top: Fixed,
    pub bottom: Fixed,
}

impl Edges {
    /// The same amount on every side.
    #[must_use]
    pub fn all(amount: Fixed) -> Self {
        Self {
            left: amount,
            right: amount,
            top: amount,
            bottom: amount,
        }
    }
}

/// Everything layout knows about one node.
///
/// Plain fields on purpose: the crate is `bootstrap`, and the document
/// compiler that will author these offline wants a struct it can fill,
/// not a builder it must thread. Negative pixels are expressible and
/// not validated here — the arithmetic simply follows them; the
/// compiler is where authoring validation belongs, with the untrusted
/// blob parser it arrives with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    /// How this node lays out its own children.
    pub direction: Direction,
    /// This node's width, before margins.
    pub width: Size,
    /// This node's height, before margins.
    pub height: Size,
    /// Space outside this node's box, claimed from the parent.
    pub margin: Edges,
    /// Space inside this node's box, before its children.
    pub padding: Edges,
    /// Space between consecutive children, along the main axis.
    pub gap: Fixed,
    /// This node's share of the parent's leftover main-axis space.
    /// Zero — the default — takes none.
    pub grow: u32,
    /// Where the run of children sits along the main axis.
    pub justify: Align,
    /// Where each child sits along the cross axis.
    pub align_cross: Align,
    /// The fill behind this node, as premultiplied RGBA bytes — an
    /// integer, because this crate's numbers must be. All zeros — the
    /// default — draws nothing at all; presentation converts everything
    /// else to its own vocabulary. Richer visuals (borders, images,
    /// text) arrive with the compiled style tables, not as more fields
    /// here.
    pub background: [u8; 4],
}

/// A solved box: absolute position and size, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: Fixed,
    pub y: Fixed,
    pub width: Fixed,
    pub height: Fixed,
}

/// The per-slot layout state the [`crate::Ui`] arena carries.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LayoutSlot {
    pub style: Style,
    pub rect: Rect,
    /// Content-driven size from the measure pass: the box this node
    /// wants (padding included, margin excluded) before the parent has
    /// its say.
    pub wanted_width: Fixed,
    pub wanted_height: Fixed,
}

/// The solver's reusable workspace, sized once for the whole arena.
#[derive(Debug)]
pub(crate) struct Scratch {
    /// Traversal order (parents before children), rebuilt per solve.
    order: Vec<u32>,
    /// Largest-remainder bookkeeping, one entry per child of the node
    /// being arranged, in child order: (remainder, share so far).
    shares: Vec<(i64, i64)>,
}

impl Scratch {
    pub fn with_capacity(nodes: u32) -> Self {
        Self {
            order: Vec::with_capacity(nodes as usize),
            shares: Vec::with_capacity(nodes as usize),
        }
    }
}

/// The solver itself, generic over the arena through a small view so
/// the tree module keeps its links private.
pub(crate) struct Solve<'a> {
    /// Read-only on purpose: the solver walks links and never touches
    /// them, and the borrow proves it.
    pub slots: &'a [crate::Slot],
    pub layout: &'a mut [LayoutSlot],
    pub scratch: &'a mut Scratch,
}

impl Solve<'_> {
    /// Solve the whole tree into absolute rectangles, with the root
    /// filling `viewport` at the origin.
    pub fn run(&mut self, viewport_width: Fixed, viewport_height: Fixed) {
        self.collect_order();

        // Measure: children before parents, so every Auto has its
        // content's answer by the time the parent asks.
        for position in (0..self.scratch.order.len()).rev() {
            let index = self.scratch.order[position];
            self.measure(index);
        }

        // The root's box is the viewport, whatever its style says: a
        // document fills the screen it is given.
        self.layout[0].rect = Rect {
            x: Fixed::ZERO,
            y: Fixed::ZERO,
            width: viewport_width,
            height: viewport_height,
        };

        // Arrange: parents before children, each placing its children
        // inside its own already-known rect.
        for position in 0..self.scratch.order.len() {
            let index = self.scratch.order[position];
            self.arrange(index);
        }
    }

    /// Fill `scratch.order` with every live slot, parents first, in
    /// document order — one preallocated list serves both passes, read
    /// forward to arrange and backward to measure.
    fn collect_order(&mut self) {
        self.scratch.order.clear();
        self.scratch.order.push(0);
        let mut at = 0;
        while at < self.scratch.order.len() {
            let index = self.scratch.order[at];
            let mut child = self.slots[index as usize].first_child;
            while child != crate::NIL {
                self.scratch.order.push(child);
                child = self.slots[child as usize].next_sibling;
            }
            at += 1;
        }
    }

    /// What `index` wants to be, from its style and its children.
    fn measure(&mut self, index: u32) {
        let style = self.layout[index as usize].style;
        let (main_sum, cross_max) = self.content_extent(index, style);

        let content_width = match style.direction {
            Direction::Row => main_sum,
            Direction::Column => cross_max,
        };
        let content_height = match style.direction {
            Direction::Row => cross_max,
            Direction::Column => main_sum,
        };

        let slot = &mut self.layout[index as usize];
        slot.wanted_width = match style.width {
            Size::Px(width) => width,
            Size::Auto => content_width + style.padding.left + style.padding.right,
        };
        slot.wanted_height = match style.height {
            Size::Px(height) => height,
            Size::Auto => content_height + style.padding.top + style.padding.bottom,
        };
    }

    /// The children's combined extent: outer sizes plus gaps along the
    /// main axis, and the tallest outer size across it.
    fn content_extent(&self, index: u32, style: Style) -> (Fixed, Fixed) {
        let mut main_sum = Fixed::ZERO;
        let mut cross_max = Fixed::ZERO;
        let mut count = 0u32;
        let mut child = self.slots[index as usize].first_child;
        while child != crate::NIL {
            let outer = self.outer_wanted(child, style.direction);
            main_sum = main_sum + outer.0;
            cross_max = cross_max.max(outer.1);
            count += 1;
            child = self.slots[child as usize].next_sibling;
        }
        if count > 1 {
            // One gap between each consecutive pair. The count fits an
            // i32 long before an arena of this many slots would fit in
            // memory; the saturation is the type conversion's honesty,
            // not a reachable path.
            let gaps = i32::try_from(count - 1).unwrap_or(i32::MAX);
            main_sum = main_sum + style.gap * Fixed::from_int(gaps);
        }
        (main_sum, cross_max)
    }

    /// A child's wanted size plus its margins, as (main, cross) for
    /// the parent's direction.
    fn outer_wanted(&self, child: u32, direction: Direction) -> (Fixed, Fixed) {
        let slot = &self.layout[child as usize];
        let margin = slot.style.margin;
        let outer_width = slot.wanted_width + margin.left + margin.right;
        let outer_height = slot.wanted_height + margin.top + margin.bottom;
        match direction {
            Direction::Row => (outer_width, outer_height),
            Direction::Column => (outer_height, outer_width),
        }
    }

    /// Place `index`'s children inside its solved rect.
    fn arrange(&mut self, index: u32) {
        let style = self.layout[index as usize].style;
        let rect = self.layout[index as usize].rect;

        // The content box: the node's rect minus its padding.
        let content_x = rect.x + style.padding.left;
        let content_y = rect.y + style.padding.top;
        let content_width = rect.width - style.padding.left - style.padding.right;
        let content_height = rect.height - style.padding.top - style.padding.bottom;
        let (content_main, content_cross) = match style.direction {
            Direction::Row => (content_width, content_height),
            Direction::Column => (content_height, content_width),
        };

        let (occupied, _) = self.content_extent(index, style);
        let leftover = content_main - occupied;
        self.plan_growth(index, leftover);

        // Growers consume the leftover; whatever they leave is what
        // justification places. With any grower the run fills the box
        // and justification has nothing to move.
        let grown: i64 = self.scratch.shares.iter().map(|entry| entry.1).sum();
        let free = leftover - Fixed::from_bits(grown);
        let mut cursor = content_main_offset(style.justify, free);

        let mut child = self.slots[index as usize].first_child;
        let mut nth = 0usize;
        while child != crate::NIL {
            let (outer_main, outer_cross) = self.outer_wanted(child, style.direction);
            let growth = self
                .scratch
                .shares
                .get(nth)
                .map_or(Fixed::ZERO, |entry| Fixed::from_bits(entry.1));
            let child_style = self.layout[child as usize].style;
            let cross_free = content_cross - outer_cross;
            let cross_offset = content_main_offset(style.align_cross, cross_free);

            let (width, height, x, y) = match style.direction {
                Direction::Row => (
                    self.layout[child as usize].wanted_width + growth,
                    self.layout[child as usize].wanted_height,
                    content_x + cursor + child_style.margin.left,
                    content_y + cross_offset + child_style.margin.top,
                ),
                Direction::Column => (
                    self.layout[child as usize].wanted_width,
                    self.layout[child as usize].wanted_height + growth,
                    content_x + cross_offset + child_style.margin.left,
                    content_y + cursor + child_style.margin.top,
                ),
            };
            self.layout[child as usize].rect = Rect {
                x,
                y,
                width,
                height,
            };
            cursor = cursor + outer_main + growth + style.gap;
            nth += 1;
            child = self.slots[child as usize].next_sibling;
        }
    }

    /// Share `leftover` among `index`'s growing children by largest
    /// remainder over raw fixed-point units, ties to the earlier
    /// sibling. Fills `scratch.shares` with one entry per child, in
    /// child order; non-growers hold zero. Shares sum to the leftover
    /// exactly — that exactness is property-tested, not assumed.
    fn plan_growth(&mut self, index: u32, leftover: Fixed) {
        self.scratch.shares.clear();
        // The sum cannot overflow an i64 for any arena that fits in
        // memory: each grow is at most 2^32 and each child costs tens
        // of bytes of slot, so 2^31 children — the count that could
        // push the sum past 2^63 — needs more memory than a machine
        // has. The same argument the gap count makes, made here
        // because this code is the one place raw sums leave `Fixed`'s
        // saturation behind.
        let mut total_grow: i64 = 0;
        let mut child = self.slots[index as usize].first_child;
        while child != crate::NIL {
            let grow = i64::from(self.layout[child as usize].style.grow);
            total_grow += grow;
            self.scratch.shares.push((grow, 0));
            child = self.slots[child as usize].next_sibling;
        }
        let leftover_raw = leftover.to_bits();
        if total_grow == 0 || leftover_raw <= 0 {
            return;
        }

        // Integer base shares first; the division truncates toward
        // zero, and what it truncated away is the remainder that ranks
        // who gets the leftover units. The products run in i128 — a
        // large viewport times a large grow overflows an i64 for
        // inputs a caller can actually write, and wrapped arithmetic
        // here would hand the top-up loop a nonsense shortfall. The
        // narrowing back cannot fail: a base share is at most the
        // leftover and a remainder is less than the total, both i64;
        // the fallbacks are the conversion's honesty, not a path.
        let mut distributed: i64 = 0;
        for entry in &mut self.scratch.shares {
            let grow = i128::from(entry.0);
            let product = i128::from(leftover_raw) * grow;
            let base = i64::try_from(product / i128::from(total_grow)).unwrap_or(i64::MAX);
            let remainder = i64::try_from(product % i128::from(total_grow)).unwrap_or(i64::MAX);
            entry.0 = remainder;
            entry.1 = base;
            distributed += base;
        }
        let mut short = leftover_raw - distributed;

        // One extra raw unit each to the largest remainders, earlier
        // siblings first on ties. The list is already in child order,
        // so a stable pass that picks the maximum `short` times keeps
        // the tiebreak by construction without sorting.
        while short > 0 {
            let mut best = 0;
            for (position, entry) in self.scratch.shares.iter().enumerate() {
                if entry.0 > self.scratch.shares[best].0 {
                    best = position;
                }
            }
            self.scratch.shares[best].0 = -1;
            self.scratch.shares[best].1 += 1;
            short -= 1;
        }
    }
}

/// Where a run starts along an axis with `free` space left over: at
/// the origin, centred, or at the end. Negative free space — overflow —
/// keeps the start placement, so overflowing content spills toward the
/// axis end rather than in both directions.
fn content_main_offset(align: Align, free: Fixed) -> Fixed {
    if free <= Fixed::ZERO {
        return Fixed::ZERO;
    }
    match align {
        Align::Start => Fixed::ZERO,
        Align::Center => free / Fixed::from_int(2),
        Align::End => free,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Ui, UiLimits};

    fn px(value: i32) -> Size {
        Size::Px(Fixed::from_int(value))
    }

    fn f(value: i32) -> Fixed {
        Fixed::from_int(value)
    }

    /// A row with padding and gap places its children where a reader
    /// doing the arithmetic by hand would put them.
    #[test]
    fn a_row_places_children_after_padding_and_gap() {
        let mut ui = Ui::new(UiLimits { nodes: 8 });
        let root = ui.root();
        ui.set_style(
            root,
            Style {
                padding: Edges::all(f(10)),
                gap: f(5),
                ..Style::default()
            },
        );
        let first = ui.insert(root).expect("room");
        ui.set_style(
            first,
            Style {
                width: px(20),
                height: px(30),
                ..Style::default()
            },
        );
        let second = ui.insert(root).expect("room");
        ui.set_style(
            second,
            Style {
                width: px(40),
                height: px(10),
                ..Style::default()
            },
        );
        ui.solve(f(200), f(100));
        assert_eq!(
            ui.rect(first),
            Some(Rect {
                x: f(10),
                y: f(10),
                width: f(20),
                height: f(30)
            })
        );
        assert_eq!(
            ui.rect(second),
            Some(Rect {
                x: f(35),
                y: f(10),
                width: f(40),
                height: f(10)
            })
        );
    }

    /// A column advances along y with the same rules turned sideways.
    #[test]
    fn a_column_advances_downward() {
        let mut ui = Ui::new(UiLimits { nodes: 8 });
        let root = ui.root();
        ui.set_style(
            root,
            Style {
                direction: Direction::Column,
                gap: f(4),
                ..Style::default()
            },
        );
        let first = ui.insert(root).expect("room");
        ui.set_style(
            first,
            Style {
                width: px(10),
                height: px(6),
                ..Style::default()
            },
        );
        let second = ui.insert(root).expect("room");
        ui.set_style(
            second,
            Style {
                width: px(10),
                height: px(6),
                ..Style::default()
            },
        );
        ui.solve(f(100), f(100));
        assert_eq!(ui.rect(first).expect("solved").y, f(0));
        assert_eq!(ui.rect(second).expect("solved").y, f(10));
    }

    /// Centre and end placement put the leftover where they promise.
    #[test]
    fn justification_places_the_leftover() {
        for (justify, expected_x) in [
            (Align::Start, f(0)),
            (Align::Center, f(45)),
            (Align::End, f(90)),
        ] {
            let mut ui = Ui::new(UiLimits { nodes: 4 });
            let root = ui.root();
            ui.set_style(
                root,
                Style {
                    justify,
                    ..Style::default()
                },
            );
            let child = ui.insert(root).expect("room");
            ui.set_style(
                child,
                Style {
                    width: px(10),
                    height: px(10),
                    ..Style::default()
                },
            );
            ui.solve(f(100), f(100));
            assert_eq!(ui.rect(child).expect("solved").x, expected_x, "{justify:?}");
        }
    }

    /// The cross axis obeys the container's alignment the same way.
    #[test]
    fn cross_alignment_places_each_child() {
        for (align_cross, expected_y) in [
            (Align::Start, f(0)),
            (Align::Center, f(45)),
            (Align::End, f(90)),
        ] {
            let mut ui = Ui::new(UiLimits { nodes: 4 });
            let root = ui.root();
            ui.set_style(
                root,
                Style {
                    align_cross,
                    ..Style::default()
                },
            );
            let child = ui.insert(root).expect("room");
            ui.set_style(
                child,
                Style {
                    width: px(10),
                    height: px(10),
                    ..Style::default()
                },
            );
            ui.solve(f(100), f(100));
            assert_eq!(
                ui.rect(child).expect("solved").y,
                expected_y,
                "{align_cross:?}"
            );
        }
    }

    /// Margins claim space from the parent: the box shifts in by its
    /// margin, and the next sibling starts past it.
    #[test]
    fn margins_claim_space() {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        let first = ui.insert(root).expect("room");
        ui.set_style(
            first,
            Style {
                width: px(20),
                height: px(10),
                margin: Edges {
                    left: f(7),
                    right: f(3),
                    top: f(2),
                    bottom: Fixed::ZERO,
                },
                ..Style::default()
            },
        );
        let second = ui.insert(root).expect("room");
        ui.set_style(
            second,
            Style {
                width: px(5),
                height: px(5),
                ..Style::default()
            },
        );
        ui.solve(f(100), f(100));
        let first_rect = ui.rect(first).expect("solved");
        assert_eq!((first_rect.x, first_rect.y), (f(7), f(2)));
        assert_eq!(ui.rect(second).expect("solved").x, f(30));
    }

    /// An Auto box is its content plus its padding, measured from the
    /// bottom of the tree up.
    #[test]
    fn auto_wraps_content_plus_padding() {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        let wrapper = ui.insert(root).expect("room");
        ui.set_style(
            wrapper,
            Style {
                padding: Edges::all(f(4)),
                ..Style::default()
            },
        );
        let leaf = ui.insert(wrapper).expect("room");
        ui.set_style(
            leaf,
            Style {
                width: px(10),
                height: px(10),
                ..Style::default()
            },
        );
        ui.solve(f(100), f(100));
        let rect = ui.rect(wrapper).expect("solved");
        assert_eq!((rect.width, rect.height), (f(18), f(18)));
        let inner = ui.rect(leaf).expect("solved");
        assert_eq!((inner.x, inner.y), (f(4), f(4)));
    }

    /// Growth shares the leftover exactly, one raw unit at a time,
    /// earlier siblings first: seventy pixels among three growers is
    /// twice 23.333 and once 23.333 plus one raw unit, on the first.
    #[test]
    fn growth_is_exact_and_ties_break_early() {
        let mut ui = Ui::new(UiLimits { nodes: 8 });
        let root = ui.root();
        let mut children = Vec::new();
        for _ in 0..3 {
            let child = ui.insert(root).expect("room");
            ui.set_style(
                child,
                Style {
                    width: px(10),
                    height: px(10),
                    grow: 1,
                    ..Style::default()
                },
            );
            children.push(child);
        }
        ui.solve(f(100), f(100));
        let widths: Vec<i64> = children
            .iter()
            .map(|&child| ui.rect(child).expect("solved").width.to_bits())
            .collect();
        let seventy_thirds = f(70).to_bits() / 3;
        assert_eq!(widths[0], f(10).to_bits() + seventy_thirds + 1);
        assert_eq!(widths[1], f(10).to_bits() + seventy_thirds);
        assert_eq!(widths[2], f(10).to_bits() + seventy_thirds);
        assert_eq!(
            widths.iter().sum::<i64>(),
            f(100).to_bits(),
            "the grown run must fill the container to the bit"
        );
    }

    /// The same operations build the same picture: two trees, one
    /// script, bit-identical rectangles.
    #[test]
    fn the_same_operations_solve_to_the_same_bits() {
        let build = || {
            let mut ui = Ui::new(UiLimits { nodes: 16 });
            let root = ui.root();
            ui.set_style(
                root,
                Style {
                    direction: Direction::Column,
                    padding: Edges::all(f(3)),
                    gap: f(2),
                    ..Style::default()
                },
            );
            let mut ids = vec![root];
            for nth in 0..6i32 {
                let child = ui.insert(root).expect("room");
                ui.set_style(
                    child,
                    Style {
                        width: if nth % 2 == 0 { px(20) } else { Size::Auto },
                        height: px(5 + nth),
                        grow: (nth % 3).unsigned_abs(),
                        ..Style::default()
                    },
                );
                ids.push(child);
            }
            ui.solve(f(320), f(200));
            ids.iter().map(|&id| ui.rect(id)).collect::<Vec<_>>()
        };
        assert_eq!(build(), build());
    }

    /// An odd leftover's extra raw unit lands before a centred run:
    /// the halving rounds to nearest, ties away from zero, and this
    /// pins the arm so the doc cannot drift from the arithmetic.
    #[test]
    fn a_centred_odd_leftover_puts_its_extra_unit_before() {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        ui.set_style(
            root,
            Style {
                justify: Align::Center,
                ..Style::default()
            },
        );
        let child = ui.insert(root).expect("room");
        ui.set_style(
            child,
            Style {
                width: Size::Px(Fixed::from_bits(2)),
                height: Size::Px(Fixed::from_bits(2)),
                ..Style::default()
            },
        );
        // Five raw units of free space: three land before, two after.
        ui.solve(Fixed::from_bits(7), Fixed::from_bits(7));
        assert_eq!(ui.rect(child).expect("solved").x.to_bits(), 3);
    }

    /// An overflowing run refuses to grow: with no leftover there is
    /// nothing to share, and the growers keep their asked-for sizes.
    #[test]
    fn an_overflowing_run_grows_nothing() {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        let mut children = Vec::new();
        for _ in 0..2 {
            let child = ui.insert(root).expect("room");
            ui.set_style(
                child,
                Style {
                    width: px(80),
                    height: px(10),
                    grow: 3,
                    ..Style::default()
                },
            );
            children.push(child);
        }
        ui.solve(f(100), f(100));
        for &child in &children {
            assert_eq!(
                ui.rect(child).expect("solved").width,
                f(80),
                "negative leftover must not shrink or grow anyone"
            );
        }
    }

    /// A column grower stretches along y, not x: growth follows the
    /// container's main axis.
    #[test]
    fn a_column_grower_stretches_downward() {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        ui.set_style(
            root,
            Style {
                direction: Direction::Column,
                ..Style::default()
            },
        );
        let child = ui.insert(root).expect("room");
        ui.set_style(
            child,
            Style {
                width: px(10),
                height: px(10),
                grow: 1,
                ..Style::default()
            },
        );
        ui.solve(f(50), f(200));
        let rect = ui.rect(child).expect("solved");
        assert_eq!(rect.width, f(10), "the cross axis is not the grower's");
        assert_eq!(rect.height, f(200), "the main axis is");
    }

    /// Editing the tree after a solve reaches the next picture: both
    /// removal and insertion invalidate the retained rectangles.
    #[test]
    fn structural_edits_invalidate_the_solve() {
        let mut ui = Ui::new(UiLimits { nodes: 8 });
        let root = ui.root();
        let first = ui.insert(root).expect("room");
        ui.set_style(
            first,
            Style {
                width: px(10),
                height: px(10),
                ..Style::default()
            },
        );
        let second = ui.insert(root).expect("room");
        ui.set_style(
            second,
            Style {
                width: px(10),
                height: px(10),
                ..Style::default()
            },
        );
        ui.solve(f(100), f(100));
        assert_eq!(ui.rect(second).expect("solved").x, f(10));

        assert!(ui.remove(first));
        ui.solve(f(100), f(100));
        assert_eq!(
            ui.rect(second).expect("solved").x,
            f(0),
            "a removal must reach the next solve"
        );

        let third = ui.insert(root).expect("room");
        ui.set_style(
            third,
            Style {
                width: px(5),
                height: px(5),
                ..Style::default()
            },
        );
        ui.solve(f(100), f(100));
        assert_eq!(
            ui.rect(third).expect("solved").x,
            f(10),
            "an insertion must reach the next solve"
        );
    }

    /// A clean tree keeps its rectangles: solving is retained, and a
    /// style change is what invalidates.
    #[test]
    fn solving_is_retained_until_something_changes() {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        let child = ui.insert(root).expect("room");
        ui.set_style(
            child,
            Style {
                width: px(10),
                height: px(10),
                ..Style::default()
            },
        );
        ui.solve(f(100), f(100));
        let before = ui.rect(child);
        ui.solve(f(100), f(100));
        assert_eq!(ui.rect(child), before, "a clean solve moves nothing");
        ui.set_style(
            child,
            Style {
                width: px(30),
                height: px(10),
                ..Style::default()
            },
        );
        ui.solve(f(100), f(100));
        assert_eq!(
            ui.rect(child).expect("solved").width,
            f(30),
            "a style change must reach the picture"
        );
    }

    proptest::proptest! {
        /// Children that fit stay inside their parent's content box.
        #[test]
        fn fitting_children_are_contained(
            sizes in proptest::collection::vec((1i32..20, 1i32..20), 1..6),
            pad in 0i32..10,
            gap in 0i32..5,
        ) {
            let mut ui = Ui::new(UiLimits { nodes: 16 });
            let root = ui.root();
            ui.set_style(root, Style {
                padding: Edges::all(f(pad)),
                gap: f(gap),
                ..Style::default()
            });
            let mut children = Vec::new();
            for &(width, height) in &sizes {
                let child = ui.insert(root).expect("room");
                ui.set_style(child, Style {
                    width: px(width),
                    height: px(height),
                    ..Style::default()
                });
                children.push(child);
            }
            ui.solve(f(500), f(500));
            let content_right = f(500) - f(pad);
            let content_bottom = f(500) - f(pad);
            for &child in &children {
                let rect = ui.rect(child).expect("solved");
                proptest::prop_assert!(rect.x >= f(pad));
                proptest::prop_assert!(rect.y >= f(pad));
                proptest::prop_assert!(rect.x + rect.width <= content_right);
                proptest::prop_assert!(rect.y + rect.height <= content_bottom);
            }
        }

        /// However grow factors fall, growers fill the container to
        /// the exact bit — never one raw unit over or under.
        #[test]
        fn growth_sums_exactly(
            grows in proptest::collection::vec(0u32..5, 1..8),
            viewport in 50i32..400,
        ) {
            let mut ui = Ui::new(UiLimits { nodes: 16 });
            let root = ui.root();
            let mut children = Vec::new();
            for &grow in &grows {
                let child = ui.insert(root).expect("room");
                ui.set_style(child, Style {
                    width: px(5),
                    height: px(5),
                    grow,
                    ..Style::default()
                });
                children.push(child);
            }
            ui.solve(f(viewport), f(100));
            let occupied: i64 = children
                .iter()
                .map(|&child| ui.rect(child).expect("solved").width.to_bits())
                .sum();
            let base: i64 = f(5).to_bits() * i64::try_from(children.len()).unwrap_or(0);
            if grows.iter().all(|&grow| grow == 0) {
                proptest::prop_assert_eq!(occupied, base, "no grower, no growth");
            } else {
                proptest::prop_assert_eq!(
                    occupied,
                    f(viewport).to_bits(),
                    "growers must absorb the leftover exactly"
                );
            }
        }

        /// Solving twice is the same picture: the solver is a pure
        /// function of the tree, not of how often it ran.
        #[test]
        fn solving_is_idempotent(
            sizes in proptest::collection::vec((1i32..30, 1i32..30, 0u32..3), 1..8),
        ) {
            let mut ui = Ui::new(UiLimits { nodes: 16 });
            let root = ui.root();
            let mut children = vec![root];
            for &(width, height, grow) in &sizes {
                let child = ui.insert(root).expect("room");
                ui.set_style(child, Style {
                    width: px(width),
                    height: px(height),
                    grow,
                    ..Style::default()
                });
                children.push(child);
            }
            ui.solve(f(300), f(300));
            let first: Vec<_> = children.iter().map(|&id| ui.rect(id)).collect();
            ui.set_style(root, ui.style(root).expect("root is live"));
            ui.solve(f(300), f(300));
            let second: Vec<_> = children.iter().map(|&id| ui.rect(id)).collect();
            proptest::prop_assert_eq!(first, second);
        }
    }
}
