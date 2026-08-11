//! The document compiler: the compact text grammar, compiled to the
//! runtime's canonical blob.
//!
//! The compiler is deliberately thin. It parses the grammar into a
//! flat preorder list, builds a real [`Ui`] through the same API every
//! test uses, and hands the tree to the runtime crate's own canonical
//! writer — so a compiled document is correct for exactly the reasons
//! a builder-built tree is, and the blob's strict reader accepts it
//! because capture only mints what read accepts.
//!
//! **The grammar, whole.** One root element; `row` and `column` lay
//! their children along an axis, `node` is a leaf spelling of `row`.
//! Children sit in braces, and an element without braces has none.
//! Attributes are `name=value` pairs before the braces: `w`, `h`,
//! `gap`, `grow` (non-negative integers, pixels), `margin` and `pad`
//! (one integer for all sides or `(left right top bottom)`), `justify`
//! and `align` (`start`, `center`, `end`), and `bg` (`#rrggbb` or
//! `#rrggbbaa`). `//` runs to end of line. Integers only: fractional
//! pixels arrive when a consumer needs them, with an exact rule
//! written at that moment.
//!
//! **Every refusal names its place and its expectation** — line,
//! column, and what would have been legal — because a document author
//! reads diagnostics, not source code.

use renew_ui::document::{MAX_NODES, capture};
use renew_ui::{Align, Direction, Edges, Fixed, Size, Style, Ui, UiLimits};

/// Where and why a source text is not a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// 1-based line of the refusal.
    pub line: u32,
    /// 1-based column of the refusal, in characters.
    pub column: u32,
    /// What was found and what was expected, one sentence.
    pub message: String,
}

impl core::fmt::Display for Diagnostic {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(out, "{}:{}: {}", self.line, self.column, self.message)
    }
}

/// A compiled document: the canonical bytes and what they hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compiled {
    /// The blob, exactly as the runtime reader accepts it.
    pub bytes: Vec<u8>,
    /// How many nodes the document holds, root included.
    pub nodes: u32,
}

/// Deeper nesting than this is refused: a hostile document must not
/// choose the compiler's stack depth.
const MAX_DEPTH: usize = 64;

/// The largest pixel or weight integer the grammar accepts. A bound
/// chosen to be absurdly generous for real documents while keeping
/// every arithmetic path trivially in range.
const MAX_VALUE: i64 = 1_000_000;

/// One parsed element, in preorder — the order the canonical blob
/// wants, produced by the walk itself.
struct Parsed {
    parent: Option<u32>,
    style: Style,
}

/// Compile source text into the canonical document blob.
///
/// # Errors
///
/// The first [`Diagnostic`], with its line, column, and expectation.
///
/// # Panics
///
/// When the built tree does not hold every parsed element — a
/// contract violation the parse's parents-before-children order makes
/// unreachable, asserted rather than assumed.
pub fn compile(source: &str) -> Result<Compiled, Diagnostic> {
    let mut scanner = Scanner::new(source);
    let mut nodes: Vec<Parsed> = Vec::new();
    parse_element(&mut scanner, None, 0, &mut nodes)?;
    scanner.skip_trivia();
    if !scanner.at_end() {
        return Err(scanner.refuse("the document ends after its one root element"));
    }
    // Bounded by MAX_DEPTH times per-element growth, but stated
    // directly: the reader's ceiling is the compiler's ceiling, said
    // here as a diagnostic rather than downstream as a panic.
    let count = u32::try_from(nodes.len()).unwrap_or(u32::MAX);
    if count > MAX_NODES {
        return Err(Diagnostic {
            line: 1,
            column: 1,
            message: format!("{count} nodes exceed the document ceiling of {MAX_NODES}"),
        });
    }

    let mut ui = Ui::new(UiLimits { nodes: count });
    let mut ids = Vec::with_capacity(nodes.len());
    for parsed in &nodes {
        let node = match parsed.parent {
            None => ui.root(),
            Some(at) => {
                let parent = ids.get(at as usize).copied().unwrap_or_else(|| ui.root());
                // The parse built parents before children; the arena
                // is sized to the count, so the insert cannot refuse.
                ui.insert(parent).unwrap_or(parent)
            }
        };
        ui.set_style(node, parsed.style);
        ids.push(node);
    }
    assert_eq!(
        ui.live(),
        count,
        "a parsed document must build every element"
    );
    Ok(Compiled {
        bytes: capture(&ui),
        nodes: count,
    })
}

/// Print a live tree in the grammar, one element per line — the
/// compiler's inverse, used by the round-trip property and by porting
/// work that starts from a builder-built tree. Only what differs from
/// the default is printed, so emitted text stays as compact as the
/// hand-written form.
#[must_use]
pub fn emit(ui: &Ui) -> String {
    let mut out = String::new();
    emit_node(ui, ui.root(), 0, &mut out);
    out
}

fn emit_node(ui: &Ui, node: renew_ui::NodeId, depth: usize, out: &mut String) {
    use core::fmt::Write as _;
    let style = ui.style(node).unwrap_or_default();
    let pad = "    ".repeat(depth);
    let children: Vec<_> = ui.children(node).collect();
    let element = match style.direction {
        Direction::Row if children.is_empty() => "node",
        Direction::Row => "row",
        Direction::Column => "column",
    };
    let _ = write!(out, "{pad}{element}");
    let px = |value: Fixed| value.trunc_int();
    if let Size::Px(width) = style.width {
        let _ = write!(out, " w={}", px(width));
    }
    if let Size::Px(height) = style.height {
        let _ = write!(out, " h={}", px(height));
    }
    for (name, edges) in [("margin", style.margin), ("pad", style.padding)] {
        if edges != Edges::default() {
            let (l, r, t, b) = (
                px(edges.left),
                px(edges.right),
                px(edges.top),
                px(edges.bottom),
            );
            if l == r && r == t && t == b {
                let _ = write!(out, " {name}={l}");
            } else {
                let _ = write!(out, " {name}=({l} {r} {t} {b})");
            }
        }
    }
    if style.gap != Fixed::ZERO {
        let _ = write!(out, " gap={}", px(style.gap));
    }
    if style.grow != 0 {
        let _ = write!(out, " grow={}", style.grow);
    }
    if style.justify != Align::Start {
        let _ = write!(out, " justify={}", align_word(style.justify));
    }
    if style.align_cross != Align::Start {
        let _ = write!(out, " align={}", align_word(style.align_cross));
    }
    if style.background != [0, 0, 0, 0] {
        let [r, g, b, a] = style.background;
        if a == 0xFF {
            let _ = write!(out, " bg=#{r:02x}{g:02x}{b:02x}");
        } else {
            let _ = write!(out, " bg=#{r:02x}{g:02x}{b:02x}{a:02x}");
        }
    }
    if children.is_empty() {
        out.push('\n');
    } else {
        out.push_str(" {\n");
        for child in children {
            emit_node(ui, child, depth + 1, out);
        }
        let _ = writeln!(out, "{pad}}}");
    }
}

/// An alignment's word in the grammar — total, though emit only asks
/// for the two non-default arms, because a half-function invites the
/// half that is missing.
fn align_word(align: Align) -> &'static str {
    match align {
        Align::Start => "start",
        Align::Center => "center",
        Align::End => "end",
    }
}

/// The character walker: position, line, column, and the refusal that
/// points at them.
struct Scanner<'a> {
    rest: core::str::Chars<'a>,
    line: u32,
    column: u32,
}

impl<'a> Scanner<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            rest: source.chars(),
            line: 1,
            column: 1,
        }
    }

    fn peek(&self) -> Option<char> {
        self.rest.clone().next()
    }

    fn bump(&mut self) -> Option<char> {
        let next = self.rest.next();
        match next {
            Some('\n') => {
                self.line += 1;
                self.column = 1;
            }
            Some(_) => self.column += 1,
            None => {}
        }
        next
    }

    fn at_end(&self) -> bool {
        self.peek().is_none()
    }

    /// Whitespace and `//` comments, however many of each.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(space) if space.is_whitespace() => {
                    self.bump();
                }
                Some('/') if self.rest.clone().nth(1) == Some('/') => {
                    while let Some(inside) = self.peek() {
                        if inside == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                _ => break,
            }
        }
    }

    fn refuse(&self, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            line: self.line,
            column: self.column,
            message: message.into(),
        }
    }

    /// An identifier: ASCII letters. Empty when the cursor sits on
    /// anything else, which callers turn into their own expectation.
    fn ident(&mut self) -> String {
        let mut name = String::new();
        while let Some(letter) = self.peek() {
            if letter.is_ascii_alphabetic() {
                name.push(letter);
                self.bump();
            } else {
                break;
            }
        }
        name
    }

    /// A non-negative integer within [`MAX_VALUE`].
    fn integer(&mut self, what: &str) -> Result<i64, Diagnostic> {
        let mut digits = String::new();
        while let Some(digit) = self.peek() {
            if digit.is_ascii_digit() {
                digits.push(digit);
                self.bump();
            } else {
                break;
            }
        }
        if digits.is_empty() {
            return Err(self.refuse(format!("expected an integer for {what}")));
        }
        let value: i64 = digits
            .parse()
            .map_err(|_| self.refuse(format!("{what} does not fit an integer")))?;
        if value > MAX_VALUE {
            return Err(self.refuse(format!("{what} is larger than the ceiling of {MAX_VALUE}")));
        }
        Ok(value)
    }
}

/// One element and, recursively, its brace-block of children.
fn parse_element(
    scanner: &mut Scanner<'_>,
    parent: Option<u32>,
    depth: usize,
    nodes: &mut Vec<Parsed>,
) -> Result<(), Diagnostic> {
    if depth >= MAX_DEPTH {
        return Err(scanner.refuse(format!("nesting deeper than {MAX_DEPTH} levels")));
    }
    scanner.skip_trivia();
    let name = scanner.ident();
    let direction = match name.as_str() {
        "row" | "node" => Direction::Row,
        "column" => Direction::Column,
        _ => {
            return Err(scanner.refuse(format!(
                "expected an element (row, column, or node), found {name:?}"
            )));
        }
    };
    let style = parse_attributes(scanner, direction)?;
    let own = u32::try_from(nodes.len()).unwrap_or(u32::MAX);
    nodes.push(Parsed { parent, style });

    scanner.skip_trivia();
    if scanner.peek() == Some('{') {
        scanner.bump();
        loop {
            scanner.skip_trivia();
            match scanner.peek() {
                Some('}') => {
                    scanner.bump();
                    break;
                }
                None => {
                    return Err(scanner.refuse("expected a child element or a closing brace"));
                }
                Some(_) => parse_element(scanner, Some(own), depth + 1, nodes)?,
            }
        }
    }
    Ok(())
}

/// The `name=value` pairs before an element's braces. Each attribute
/// may appear once; an unknown name lists the known ones.
fn parse_attributes(scanner: &mut Scanner<'_>, direction: Direction) -> Result<Style, Diagnostic> {
    let mut style = Style {
        direction,
        ..Style::default()
    };
    let mut seen: Vec<&'static str> = Vec::new();
    loop {
        scanner.skip_trivia();
        // An identifier here is an attribute only when `=` follows;
        // otherwise it opens the next sibling and belongs to the
        // caller. Clone-ahead keeps the scanner untouched until known.
        let mut lookahead = Scanner {
            rest: scanner.rest.clone(),
            line: scanner.line,
            column: scanner.column,
        };
        let name = lookahead.ident();
        if name.is_empty() || lookahead.peek() != Some('=') {
            return Ok(style);
        }
        *scanner = lookahead;
        scanner.bump();

        let known = |sighted: &str| -> Result<&'static str, Diagnostic> {
            const KNOWN: [&str; 9] = [
                "w", "h", "margin", "pad", "gap", "grow", "justify", "align", "bg",
            ];
            KNOWN
                .into_iter()
                .find(|&option| option == sighted)
                .ok_or_else(|| {
                    scanner.refuse(format!(
                        "unknown attribute {sighted:?}; the attributes are w, h, margin, \
                         pad, gap, grow, justify, align, and bg"
                    ))
                })
        };
        let attribute = known(&name)?;
        if seen.contains(&attribute) {
            return Err(scanner.refuse(format!("{attribute} appears twice on one element")));
        }
        seen.push(attribute);

        match attribute {
            "w" => style.width = Size::Px(fixed_px(scanner.integer("w")?)),
            "h" => style.height = Size::Px(fixed_px(scanner.integer("h")?)),
            "gap" => style.gap = fixed_px(scanner.integer("gap")?),
            "grow" => {
                style.grow = u32::try_from(scanner.integer("grow")?).unwrap_or(0);
            }
            "margin" => style.margin = parse_edges(scanner, "margin")?,
            "pad" => style.padding = parse_edges(scanner, "pad")?,
            "justify" => style.justify = parse_align(scanner, "justify")?,
            "align" => style.align_cross = parse_align(scanner, "align")?,
            _ => style.background = parse_color(scanner)?,
        }
    }
}

/// Q47.16 pixels from a grammar integer, exact by construction: the
/// value passed the [`MAX_VALUE`] bound, far inside `i32`.
fn fixed_px(value: i64) -> Fixed {
    Fixed::from_int(i32::try_from(value).unwrap_or(0))
}

/// `n` for all four sides, or `(left right top bottom)`.
fn parse_edges(scanner: &mut Scanner<'_>, what: &str) -> Result<Edges, Diagnostic> {
    if scanner.peek() == Some('(') {
        scanner.bump();
        let mut sides = [Fixed::ZERO; 4];
        for (nth, side) in ["left", "right", "top", "bottom"].into_iter().enumerate() {
            scanner.skip_trivia();
            sides[nth] = fixed_px(scanner.integer(&format!("{what} {side}"))?);
        }
        scanner.skip_trivia();
        if scanner.peek() != Some(')') {
            return Err(scanner.refuse(format!(
                "expected a closing parenthesis after four {what} sides"
            )));
        }
        scanner.bump();
        Ok(Edges {
            left: sides[0],
            right: sides[1],
            top: sides[2],
            bottom: sides[3],
        })
    } else {
        let all = fixed_px(scanner.integer(what)?);
        Ok(Edges::all(all))
    }
}

/// `start`, `center`, or `end`.
fn parse_align(scanner: &mut Scanner<'_>, what: &str) -> Result<Align, Diagnostic> {
    let name = scanner.ident();
    match name.as_str() {
        "start" => Ok(Align::Start),
        "center" => Ok(Align::Center),
        "end" => Ok(Align::End),
        _ => Err(scanner.refuse(format!(
            "expected start, center, or end for {what}, found {name:?}"
        ))),
    }
}

/// `#rrggbb` or `#rrggbbaa`, case-insensitive.
fn parse_color(scanner: &mut Scanner<'_>) -> Result<[u8; 4], Diagnostic> {
    if scanner.peek() != Some('#') {
        return Err(scanner.refuse("expected a color like #rrggbb or #rrggbbaa"));
    }
    scanner.bump();
    let mut digits = String::new();
    while let Some(digit) = scanner.peek() {
        if digit.is_ascii_hexdigit() {
            digits.push(digit);
            scanner.bump();
        } else {
            break;
        }
    }
    let byte = |at: usize| u8::from_str_radix(&digits[2 * at..2 * at + 2], 16).unwrap_or(0);
    match digits.len() {
        6 => Ok([byte(0), byte(1), byte(2), 0xFF]),
        8 => Ok([byte(0), byte(1), byte(2), byte(3)]),
        found => Err(scanner.refuse(format!("a color is 6 or 8 hex digits, found {found}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use renew_ui::Document;

    /// The two arms nothing above can reach, held directly: the
    /// walker answers None at the end without moving, and the default
    /// alignment has a word even though emit never prints it.
    #[test]
    fn the_edges_of_the_toolkit_answer() {
        let mut empty = Scanner::new("");
        assert_eq!(empty.bump(), None);
        assert_eq!((empty.line, empty.column), (1, 1), "the end moves nothing");
        assert_eq!(align_word(Align::Start), "start");
        assert_eq!(align_word(Align::Center), "center");
        assert_eq!(align_word(Align::End), "end");
    }

    /// The fixture: the shape of a real pause menu, written by hand.
    const MENU: &str = "\
// a pause menu, as the grammar spells it
column gap=8 justify=center align=end bg=#0a141eff {
    row margin=2 pad=(12 12 5 5) grow=3 bg=#282c34e6 {
        node w=64 h=16
        node
    }
}
";

    /// The same tree through the builder API — the twin the compiler
    /// must match byte for byte.
    fn builder_twin() -> Ui {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        ui.set_style(
            root,
            Style {
                direction: Direction::Column,
                gap: Fixed::from_int(8),
                justify: Align::Center,
                align_cross: Align::End,
                background: [0x0a, 0x14, 0x1e, 0xff],
                ..Style::default()
            },
        );
        let row = ui.insert(root).unwrap_or(root);
        ui.set_style(
            row,
            Style {
                margin: Edges::all(Fixed::from_int(2)),
                padding: Edges {
                    left: Fixed::from_int(12),
                    right: Fixed::from_int(12),
                    top: Fixed::from_int(5),
                    bottom: Fixed::from_int(5),
                },
                grow: 3,
                background: [0x28, 0x2c, 0x34, 0xe6],
                ..Style::default()
            },
        );
        let wide = ui.insert(row).unwrap_or(root);
        ui.set_style(
            wide,
            Style {
                width: Size::Px(Fixed::from_int(64)),
                height: Size::Px(Fixed::from_int(16)),
                ..Style::default()
            },
        );
        ui.insert(row).unwrap_or(root);
        ui
    }

    /// The whole pipeline in one assertion: compiled text equals the
    /// builder-built twin through the canonical writer, and the bytes
    /// read back as a document.
    #[test]
    fn the_fixture_compiles_to_the_builder_built_twin() {
        let compiled = compile(MENU).expect("the fixture is legal");
        assert_eq!(compiled.nodes, 4);
        assert_eq!(
            compiled.bytes,
            capture(&builder_twin()),
            "the compiler and the builder must spell the same tree identically"
        );
        let document = Document::read(&compiled.bytes).expect("compiled bytes read back");
        assert_eq!(document.len(), 4);
    }

    /// emit is the compiler's inverse: emitted text recompiles to the
    /// original bytes, and emitting the fixture's tree reproduces a
    /// text that compiles to the fixture's bytes.
    #[test]
    fn emitted_text_recompiles_to_the_same_bytes() {
        let original = builder_twin();
        let text = emit(&original);
        let compiled = compile(&text).expect("emitted text is legal");
        assert_eq!(
            compiled.bytes,
            capture(&original),
            "emit then compile must reproduce the canonical bytes:\n{text}"
        );
    }

    /// A bare tree emits the shortest spelling and still round-trips.
    #[test]
    fn a_bare_leaf_emits_and_returns() {
        let ui = Ui::new(UiLimits { nodes: 1 });
        let text = emit(&ui);
        assert_eq!(text, "node\n");
        let compiled = compile(&text).expect("legal");
        assert_eq!(compiled.bytes, capture(&ui));
    }

    proptest::proptest! {
        /// Random shallow trees survive the full circle: emit, then
        /// compile, then the canonical bytes agree with the original.
        /// Styles draw from the grammar's own vocabulary — integer
        /// pixels, the three alignments, hex colors — because emit
        /// prints exactly what the grammar can say.
        #[test]
        fn every_emitted_tree_recompiles(
            widths in proptest::collection::vec(0i32..1000, 1..12),
            grows in proptest::collection::vec(0u32..5, 1..12),
            colored in proptest::collection::vec(proptest::bool::ANY, 1..12),
        ) {
            let count = widths.len().min(grows.len()).min(colored.len());
            let nodes = u32::try_from(count).unwrap_or(1) + 1;
            let mut ui = Ui::new(UiLimits { nodes });
            let root = ui.root();
            ui.set_style(
                root,
                Style {
                    direction: Direction::Column,
                    gap: Fixed::from_int(2),
                    ..Style::default()
                },
            );
            for nth in 0..count {
                let leaf = ui.insert(root).expect("room");
                ui.set_style(
                    leaf,
                    Style {
                        width: Size::Px(Fixed::from_int(widths[nth])),
                        grow: grows[nth],
                        background: if colored[nth] {
                            [10, 20, 30, 200]
                        } else {
                            [0, 0, 0, 0]
                        },
                        ..Style::default()
                    },
                );
            }
            let text = emit(&ui);
            let compiled = compile(&text).expect("emitted text is legal");
            proptest::prop_assert_eq!(compiled.bytes, capture(&ui));
        }
    }

    /// Each refusal points at its place and says what was expected.
    #[test]
    fn refusals_name_their_place_and_expectation() {
        let cases: [(&str, u32, &str); 10] = [
            ("", 1, "expected an element"),
            ("panel", 1, "row, column, or node"),
            ("node q=1", 1, "unknown attribute"),
            ("node w=1 w=2", 1, "appears twice"),
            ("node w=x", 1, "expected an integer for w"),
            ("node w=1000001", 1, "ceiling of 1000000"),
            ("node justify=middle", 1, "start, center, or end"),
            ("node bg=red", 1, "expected a color"),
            ("node bg=#fff", 1, "6 or 8 hex digits, found 3"),
            ("row {", 1, "closing brace"),
        ];
        for (source, line, needle) in cases {
            let refused = compile(source).expect_err(source);
            assert_eq!(refused.line, line, "line for {source:?}");
            assert!(
                refused.message.contains(needle),
                "{:?} must mention {needle:?} for {source:?}",
                refused.message
            );
        }
    }

    /// Line and column track through newlines and comments: an error
    /// on the third line says so.
    #[test]
    fn the_diagnostic_points_at_the_right_line() {
        let source = "column {\n    node w=3\n    panel\n}\n";
        let refused = compile(source).expect_err("panel is not an element");
        assert_eq!((refused.line, refused.column), (3, 10));
        assert_eq!(refused.to_string(), format!("3:10: {}", refused.message));
    }

    /// A second root is trailing garbage, said plainly.
    #[test]
    fn a_second_root_is_refused() {
        let refused = compile("node\nnode\n").expect_err("one root only");
        assert_eq!(refused.line, 2);
        assert!(refused.message.contains("one root element"));
    }

    /// Nesting past the depth ceiling is refused, not recursed into.
    #[test]
    fn nesting_past_the_ceiling_is_refused() {
        let mut source = String::new();
        for _ in 0..70 {
            source.push_str("row {");
        }
        let refused = compile(&source).expect_err("too deep");
        assert!(refused.message.contains("deeper than 64"));
    }

    /// More nodes than the blob may hold is the compiler's diagnostic,
    /// not the writer's panic.
    #[test]
    fn the_node_ceiling_is_a_diagnostic() {
        let mut source = String::from("row {\n");
        for _ in 0..MAX_NODES {
            source.push_str("node\n");
        }
        source.push('}');
        let refused = compile(&source).expect_err("past the ceiling");
        assert!(
            refused.message.contains("exceed the document ceiling"),
            "{:?}",
            refused.message
        );
    }
}
