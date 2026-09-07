//! A cursor into a parsed document, and the typed questions to ask it.
//!
//! A [`Value`] is a slice of the node table: this node first, then its
//! descendants in the order they were read. That is the whole
//! representation. It makes the cursor `Copy`, which matters more than
//! it sounds — a reader walking a scene description holds a cursor on a
//! node, on the mesh it names and on the accessor that names, all at
//! once, and none of them fights the borrow checker because none of them
//! owns anything.
//!
//! **Two shapes of answer, and the difference is deliberate.** Asking
//! what type something is gets a `Result`, because a document that says
//! a string where the caller needed a number is a document that is
//! wrong, and the refusal carries the offset to prove where. Asking
//! *whether* something is there gets an `Option`, because a member that
//! is absent and a member that is absent from something that is not even
//! an object are the same answer to the same question, and neither is an
//! error until the caller says so.

use crate::error::{JsonError, JsonErrorKind};
use crate::text::Str;
use crate::{Kind, Node, number};
use core::fmt;

/// One value in a parsed document.
#[derive(Clone, Copy)]
pub struct Value<'a> {
    source: &'a str,
    /// This node's subtree, this node first.
    subtree: &'a [Node],
}

impl<'a> Value<'a> {
    pub(crate) const fn new(source: &'a str, subtree: &'a [Node]) -> Self {
        Self { source, subtree }
    }

    /// This node.
    ///
    /// A parsed document never hands out an empty subtree, so the
    /// fallback is unreachable. It is a null value rather than a panic
    /// because this crate does not panic, and a null is the value that
    /// makes the fewest claims.
    fn node(self) -> Node {
        self.subtree.first().copied().unwrap_or(Node::VOID)
    }

    /// What this value is.
    #[must_use]
    pub fn kind(self) -> Kind {
        self.node().kind
    }

    /// The byte offset this value starts at, for a caller building a
    /// refusal of its own about a value this crate was happy with.
    #[must_use]
    pub fn at(self) -> usize {
        self.node().start
    }

    /// The source text of this value, verbatim.
    ///
    /// For a container that is everything from its opening bracket to
    /// its closing one, so a caller can quote a subtree without walking
    /// it. For a number it is the characters the number was written
    /// with, which is the only lossless form of it.
    #[must_use]
    pub fn text(self) -> &'a str {
        let node = self.node();
        self.source.get(node.start..node.end).unwrap_or("")
    }

    /// Is this `null`?
    #[must_use]
    pub fn is_null(self) -> bool {
        self.kind() == Kind::Null
    }

    /// How many elements an array has, or how many members an object
    /// has. Zero for everything else.
    ///
    /// Counted by walking, because the node table stores what a walk
    /// needs and nothing more. That is the right trade for a document
    /// whose containers are read once: paying a field per node to make
    /// this constant would cost every document to serve the caller who
    /// asks twice.
    #[must_use]
    pub fn len(self) -> usize {
        match self.kind() {
            Kind::Array => self.children().count(),
            // An object's children are name, value, name, value.
            Kind::Object => self.children().count() / 2,
            _ => 0,
        }
    }

    /// Has this container nothing in it? True for every scalar.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// The elements of an array.
    ///
    /// # Errors
    ///
    /// [`NotThisKind`](JsonErrorKind::NotThisKind) if this is not an
    /// array.
    pub fn elements(self) -> Result<Elements<'a>, JsonError> {
        if self.kind() == Kind::Array {
            Ok(self.children())
        } else {
            Err(self.wrong_kind(Kind::Array))
        }
    }

    /// The members of an object, in the order the document wrote them.
    ///
    /// Order is preserved rather than sorted, and duplicates are all
    /// here rather than resolved, so this is the iterator to reach for
    /// when a caller needs to know what a document actually said.
    ///
    /// # Errors
    ///
    /// [`NotThisKind`](JsonErrorKind::NotThisKind) if this is not an
    /// object.
    pub fn entries(self) -> Result<Entries<'a>, JsonError> {
        if self.kind() == Kind::Object {
            Ok(Entries {
                inner: self.children(),
            })
        } else {
            Err(self.wrong_kind(Kind::Object))
        }
    }

    /// The element at `index`, if this is an array and it has one.
    ///
    /// Walks to it, so reading a whole array by index costs the square
    /// of its length. Read it with [`elements`](Self::elements) instead
    /// and the walk happens once — and a layer that wants random access
    /// to a table it will hit thousands of times should build its own
    /// table in one pass, which is a thing it can do and this crate
    /// cannot do for it without knowing which arrays matter.
    #[must_use]
    pub fn index(self, index: usize) -> Option<Value<'a>> {
        self.elements().ok().and_then(|mut walk| walk.nth(index))
    }

    /// The member called `key`, if this is an object and it has one.
    ///
    /// **With a duplicate name, the last one wins**, which is what the
    /// asset formats this reader was sized for ask a reader to do. Use
    /// [`get_all`](Self::get_all) to see every one of them.
    #[must_use]
    pub fn get(self, key: &str) -> Option<Value<'a>> {
        self.get_all(key).last()
    }

    /// Every member called `key`, in document order.
    ///
    /// Normally nought or one. More than one is a document that named a
    /// member twice — legal, and something a stricter layer may want to
    /// refuse. It cannot refuse what it cannot see, which is why this is
    /// here beside [`get`](Self::get) rather than instead of it.
    pub fn get_all(self, key: &str) -> impl Iterator<Item = Value<'a>> {
        self.members()
            .filter(move |(name, _)| name.eq_str(key))
            .map(|(_, value)| value)
    }

    /// This value as a boolean.
    ///
    /// # Errors
    ///
    /// [`NotThisKind`](JsonErrorKind::NotThisKind) if this is not
    /// `true` or `false`.
    pub fn as_bool(self) -> Result<bool, JsonError> {
        if self.kind() == Kind::Bool {
            Ok(self.text() == "true")
        } else {
            Err(self.wrong_kind(Kind::Bool))
        }
    }

    /// This value as a string, escapes still in it.
    ///
    /// # Errors
    ///
    /// [`NotThisKind`](JsonErrorKind::NotThisKind) if this is not a
    /// string.
    pub fn as_str(self) -> Result<Str<'a>, JsonError> {
        if self.kind() == Kind::String {
            Ok(self.as_text())
        } else {
            Err(self.wrong_kind(Kind::String))
        }
    }

    /// This value as an unsigned 32-bit whole number.
    ///
    /// Every spelling of a whole number is accepted: `3`, `3.0` and
    /// `3e2` are all whole, and the formats this reader was sized for
    /// say so explicitly. Only a non-zero fraction is refused.
    ///
    /// # Errors
    ///
    /// [`NotThisKind`](JsonErrorKind::NotThisKind) if this is not a
    /// number, [`FractionalInteger`](JsonErrorKind::FractionalInteger)
    /// if it has a fraction, and
    /// [`IntegerOutOfRange`](JsonErrorKind::IntegerOutOfRange) if it
    /// does not fit.
    pub fn as_u32(self) -> Result<u32, JsonError> {
        let value = self.whole("u32", u128::from(u32::MAX))?;
        Ok(u32::try_from(value).unwrap_or(u32::MAX))
    }

    /// This value as an unsigned 64-bit whole number.
    ///
    /// Exact at every magnitude a `u64` holds, because the conversion is
    /// decimal arithmetic over the characters rather than a trip through
    /// a double.
    ///
    /// # Errors
    ///
    /// The same three as [`as_u32`](Self::as_u32).
    pub fn as_u64(self) -> Result<u64, JsonError> {
        let value = self.whole("u64", u128::from(u64::MAX))?;
        Ok(u64::try_from(value).unwrap_or(u64::MAX))
    }

    /// This value as a signed 64-bit whole number.
    ///
    /// # Errors
    ///
    /// The same three as [`as_u32`](Self::as_u32).
    pub fn as_i64(self) -> Result<i64, JsonError> {
        let node = self.number_node()?;
        number::signed(self.text(), node.start)
    }

    /// This value as a double.
    ///
    /// # Errors
    ///
    /// [`NotThisKind`](JsonErrorKind::NotThisKind) if this is not a
    /// number, and [`NumberNotFinite`](JsonErrorKind::NumberNotFinite)
    /// if it is written finitely and rounds to an infinity.
    pub fn as_f64(self) -> Result<f64, JsonError> {
        let node = self.number_node()?;
        number::double(self.text(), node.start)
    }

    /// This value as a single.
    ///
    /// # Errors
    ///
    /// The same two as [`as_f64`](Self::as_f64) — and a number that fits
    /// a double and not a single, `3e300` among them, is the second of
    /// them here.
    pub fn as_f32(self) -> Result<f32, JsonError> {
        let node = self.number_node()?;
        number::single(self.text(), node.start)
    }

    /// The children of a container, whatever kind it is.
    ///
    /// Empty for a scalar, because a scalar's subtree is one node and
    /// this is everything after the first.
    fn children(self) -> Elements<'a> {
        Elements {
            source: self.source,
            rest: self.subtree.get(1..).unwrap_or(&[]),
        }
    }

    /// The members of this value if it is an object, and nothing if it
    /// is not.
    ///
    /// The quiet half of [`entries`](Self::entries), for the lookups
    /// that answer `Option` and have nothing to do with a refusal.
    fn members(self) -> Entries<'a> {
        Entries {
            inner: if self.kind() == Kind::Object {
                self.children()
            } else {
                Elements {
                    source: self.source,
                    rest: &[],
                }
            },
        }
    }

    /// This string node's text, quotes stripped.
    fn as_text(self) -> Str<'a> {
        let node = self.node();
        Str::new(
            self.source
                .get(node.start.saturating_add(1)..node.end.saturating_sub(1))
                .unwrap_or(""),
        )
    }

    fn number_node(self) -> Result<Node, JsonError> {
        let node = self.node();
        if node.kind == Kind::Number {
            Ok(node)
        } else {
            Err(self.wrong_kind(Kind::Number))
        }
    }

    fn whole(self, target: &'static str, ceiling: u128) -> Result<u128, JsonError> {
        let node = self.number_node()?;
        number::unsigned(self.text(), node.start, target, ceiling)
    }

    fn wrong_kind(self, wanted: Kind) -> JsonError {
        let node = self.node();
        JsonError::new(
            node.start,
            JsonErrorKind::NotThisKind {
                wanted,
                found: node.kind,
            },
        )
    }
}

impl fmt::Debug for Value<'_> {
    /// The value's own source text.
    ///
    /// What a failing test wants to see is the document, not the node
    /// table that describes it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text())
    }
}

/// The elements of an array, in order.
///
/// Deliberately not `Copy`, though every field in it would allow it: a
/// copy of a half-walked iterator silently restarts, and an iterator is
/// the one place that is a bug rather than a convenience.
#[derive(Clone, Debug)]
pub struct Elements<'a> {
    source: &'a str,
    rest: &'a [Node],
}

impl<'a> Iterator for Elements<'a> {
    type Item = Value<'a>;

    /// Take one whole subtree off the front.
    ///
    /// This is what the `subtree` count on a node buys: the next
    /// sibling is exactly that many nodes along, so a walk needs no
    /// pointers and no second pass to build them.
    fn next(&mut self) -> Option<Value<'a>> {
        let first = self.rest.first()?;
        // Clamped so the split cannot be asked for more than there is.
        // A table this crate built always agrees with itself; the clamp
        // is what keeps "always" from being spelled as a panic.
        let take = first.subtree.min(self.rest.len()).max(1);
        let (head, tail) = self.rest.split_at(take);
        self.rest = tail;
        Some(Value::new(self.source, head))
    }
}

/// The members of an object, in the order the document wrote them.
#[derive(Clone, Debug)]
pub struct Entries<'a> {
    inner: Elements<'a>,
}

impl<'a> Iterator for Entries<'a> {
    type Item = (Str<'a>, Value<'a>);

    /// A name and its value, which the parse wrote as two nodes side by
    /// side and in that order.
    fn next(&mut self) -> Option<(Str<'a>, Value<'a>)> {
        let name = self.inner.next()?;
        let value = self.inner.next()?;
        Some((name.as_text(), value))
    }
}

#[cfg(test)]
mod tests {
    use crate::{Json, JsonErrorKind, Kind};

    /// A document shaped like the metadata this reader was sized for:
    /// sibling arrays, indices between them, nested objects, and a
    /// member whose value is arbitrary application data.
    const SCENE: &[u8] = br#"{
  "asset": {"version": "2.0", "generator": "hand"},
  "meshes": [
    {"name": "hull", "primitives": [{"attributes": {"POSITION": 0}, "indices": 1}]},
    {"name": "fin", "primitives": []}
  ],
  "accessors": [
    {"componentType": 5126, "count": 24, "min": [-1.0, -1.0, -1.0], "normalized": false},
    {"componentType": 5123, "count": 36, "extras": {"note": null}}
  ],
  "scene": 0
}"#;

    fn scene() -> Json<'static> {
        Json::parse(SCENE).expect("the scene document parses")
    }

    /// The refusal a typed question produced, whatever it was asked for.
    fn kind<T>(result: Result<T, crate::JsonError>) -> Option<JsonErrorKind> {
        result.err().map(|error| error.kind().clone())
    }

    /// **A walk reaches everything a document holds**, and each value
    /// answers as what it is.
    ///
    /// The nesting is the point: the position accessor is reached by
    /// walking to a mesh, into its first primitive, into its attributes,
    /// and out to the index that names — which is how every reader of
    /// this kind of document actually moves.
    ///
    /// Probed by making a container's subtree count one node too small:
    /// the walk runs off the end of each container into its parent's
    /// next child, and the root reports five members rather than four.
    #[test]
    fn a_walk_reaches_every_value_a_document_holds() {
        let document = scene();
        let root = document.root();
        assert_eq!(root.kind(), Kind::Object);
        assert_eq!(root.len(), 4);
        assert!(!root.is_empty());

        let version = root
            .get("asset")
            .and_then(|asset| asset.get("version"))
            .expect("the asset version");
        assert!(version.as_str().is_ok_and(|text| text == "2.0"));

        let meshes = root.get("meshes").expect("the meshes");
        assert_eq!(meshes.kind(), Kind::Array);
        assert_eq!(meshes.len(), 2);

        let position = meshes
            .index(0)
            .and_then(|mesh| mesh.get("primitives"))
            .and_then(|primitives| primitives.index(0))
            .and_then(|primitive| primitive.get("attributes"))
            .and_then(|attributes| attributes.get("POSITION"))
            .expect("the position attribute");
        assert_eq!(position.as_u32(), Ok(0));

        let accessor = root
            .get("accessors")
            .and_then(|accessors| accessors.index(1))
            .expect("the second accessor");
        assert_eq!(
            accessor.get("count").map(super::Value::as_u32),
            Some(Ok(36))
        );
        assert!(
            accessor
                .get("extras")
                .and_then(|extras| extras.get("note"))
                .is_some_and(super::Value::is_null)
        );

        let first = root
            .get("accessors")
            .and_then(|accessors| accessors.index(0))
            .expect("the first accessor");
        assert_eq!(
            first.get("normalized").map(super::Value::as_bool),
            Some(Ok(false))
        );
        let bounds = first.get("min").expect("the bounds");
        assert_eq!(bounds.len(), 3);
        assert_eq!(bounds.index(2).map(super::Value::as_f32), Some(Ok(-1.0)));

        // An empty container is empty, and says so.
        let fin = meshes
            .index(1)
            .and_then(|mesh| mesh.get("primitives"))
            .expect("the second mesh's primitives");
        assert_eq!(fin.len(), 0);
        assert!(fin.is_empty());
        assert_eq!(fin.elements().expect("an array").count(), 0);
    }

    /// **The members of an object come back in the order it wrote
    /// them.**
    ///
    /// Order carries no meaning in the format and is preserved anyway,
    /// because it costs nothing in a flat table and it is what lets a
    /// caller re-emit a document as it arrived.
    ///
    /// Probed by swapping which node of a member pair is the name: the
    /// test stops at "the asset object", because a name read from the
    /// value's node names no member at all.
    ///
    /// **No mutant reverses the order without breaking the pairing**,
    /// and that is worth saying rather than leaving to be rediscovered:
    /// order here is structural. The table is written as the document is
    /// read and walked forwards, so there is no step in between for an
    /// order to be chosen at.
    #[test]
    fn members_come_back_in_the_order_the_document_wrote_them() {
        let document = scene();
        let asset = document.root().get("asset").expect("the asset object");
        let names: Vec<String> = asset
            .entries()
            .expect("an object")
            .map(|(name, _)| name.decode())
            .collect();
        assert_eq!(names, ["version", "generator"]);

        let root_names: Vec<String> = document
            .root()
            .entries()
            .expect("an object")
            .map(|(name, _)| name.decode())
            .collect();
        assert_eq!(root_names, ["asset", "meshes", "accessors", "scene"]);
    }

    /// **A member named twice resolves to the last one, and both are
    /// still visible.**
    ///
    /// The house habit is to refuse rather than repair; this is the one
    /// place it is deliberately not followed, because the formats this
    /// reader was sized for say a later value overrides an earlier one.
    /// Refusing would reject files they bless. What keeps that from
    /// being a silent choice is that both values stay reachable, so a
    /// layer that wants to be stricter can be.
    ///
    /// Probed by making `get` take the first match: the lookup answers
    /// `Some(Ok(1))` where `Some(Ok(3))` was expected, which is the
    /// exact disagreement with the format this rule exists to avoid.
    #[test]
    fn a_duplicate_member_resolves_to_the_last_and_keeps_the_rest() {
        let document = Json::parse(br#"{"a": 1, "b": 2, "a": 3}"#).expect("a duplicated name");
        let root = document.root();
        assert_eq!(root.get("a").map(super::Value::as_u32), Some(Ok(3)));
        assert_eq!(root.get("b").map(super::Value::as_u32), Some(Ok(2)));

        let all: Vec<u32> = root
            .get_all("a")
            .map(|value| value.as_u32().unwrap_or(u32::MAX))
            .collect();
        assert_eq!(
            all,
            [1, 3],
            "a stricter layer cannot refuse what it cannot see"
        );
        assert_eq!(root.get_all("missing").count(), 0);
        // Three members, one of them named twice.
        assert_eq!(root.len(), 3);
    }

    /// **Asking the wrong question of a value is a refusal that names
    /// both kinds**, and asking whether something is there is not.
    ///
    /// The split is the crate's own rule: a type mismatch is a document
    /// that is wrong, and an absent member is a question with an answer.
    /// A reader that returned `Err` for both would make every optional
    /// field look like a fault.
    ///
    /// Probed by reporting every mismatch's `found` kind as `Null`: the
    /// first case fails with `NotThisKind { wanted: String, found: Null }`
    /// against `found: Number`, so the half of the message that says
    /// what is actually there is measured rather than assumed.
    #[test]
    fn a_type_mismatch_refuses_and_an_absent_member_does_not() {
        let document =
            Json::parse(br#"{"n": 1, "s": "x", "a": [1], "b": true}"#).expect("one of each kind");
        let root = document.root();
        let number = root.get("n").expect("the number");
        let string = root.get("s").expect("the string");
        let array = root.get("a").expect("the array");

        assert_eq!(
            kind(number.as_str()),
            Some(JsonErrorKind::NotThisKind {
                wanted: Kind::String,
                found: Kind::Number
            })
        );
        assert_eq!(
            kind(string.as_u32()),
            Some(JsonErrorKind::NotThisKind {
                wanted: Kind::Number,
                found: Kind::String
            })
        );
        assert_eq!(
            kind(string.as_i64()),
            Some(JsonErrorKind::NotThisKind {
                wanted: Kind::Number,
                found: Kind::String
            })
        );
        assert_eq!(
            kind(string.as_f64()),
            Some(JsonErrorKind::NotThisKind {
                wanted: Kind::Number,
                found: Kind::String
            })
        );
        assert_eq!(
            kind(string.as_f32()),
            Some(JsonErrorKind::NotThisKind {
                wanted: Kind::Number,
                found: Kind::String
            })
        );
        assert_eq!(
            kind(string.as_u64()),
            Some(JsonErrorKind::NotThisKind {
                wanted: Kind::Number,
                found: Kind::String
            })
        );
        assert_eq!(
            kind(number.as_bool()),
            Some(JsonErrorKind::NotThisKind {
                wanted: Kind::Bool,
                found: Kind::Number
            })
        );
        assert!(array.entries().is_err());
        assert!(root.elements().is_err());
        assert_eq!(root.get("b").map(super::Value::as_bool), Some(Ok(true)));

        // The questions that answer with an absence rather than a fault.
        assert!(root.get("nothing").is_none());
        assert!(number.get("anything").is_none());
        assert_eq!(number.get_all("anything").count(), 0);
        assert!(number.index(0).is_none());
        assert!(array.index(7).is_none());
        assert_eq!(number.len(), 0);
        assert!(number.is_empty());
    }

    /// **A value can hand back its own text and its own offset**, which
    /// is what a layer above needs to write a refusal of its own.
    ///
    /// A schema layer refuses things this crate never could — an index
    /// past the end of a sibling array, a required member that is
    /// missing — and a refusal that cannot say where is half a refusal.
    ///
    /// Probed by returning the container's end rather than its start
    /// from `at`: the root reports 26, the offset one past its closing
    /// brace, rather than 0.
    #[test]
    fn a_value_can_say_where_it_came_from() {
        let source = br#"{"a": [1, 2.50], "b": "x"}"#;
        let document = Json::parse(source).expect("a small document");
        let root = document.root();
        assert_eq!(root.at(), 0);
        assert_eq!(root.text(), r#"{"a": [1, 2.50], "b": "x"}"#);

        let array = root.get("a").expect("the array");
        assert_eq!(array.text(), "[1, 2.50]");
        assert_eq!(&source[array.at()..array.at() + 9], b"[1, 2.50]");

        // A number's text is what it was written with, which is the only
        // form of it that loses nothing.
        let second = array.index(1).expect("the second element");
        assert_eq!(second.text(), "2.50");
        assert_eq!(format!("{second:?}"), "2.50");
        assert_eq!(second.as_f64(), Ok(2.5));
    }
}
