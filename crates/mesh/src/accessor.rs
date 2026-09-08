//! A typed view over a byte range, and the arithmetic that keeps it
//! inside one.
//!
//! An accessor is four numbers and two flags — a component type, an
//! element shape, a count, an offset, an optional stride, and whether
//! integers are normalised — and between them they claim that a
//! particular sequence of typed values lives in a particular range of
//! bytes. **Every one of those numbers arrives from a file, and the
//! claim they make together is the one thing nothing downstream can
//! check.** A reader that trusts the count and computes offsets from
//! the stride reads outside the region without ever failing a bounds
//! check on the region itself, because it never compared the two.
//!
//! # No JSON here, and that is the point of the layer
//!
//! Nothing in this module knows where its numbers came from. They are
//! read out of a document by whatever reads documents; an accessor over
//! a container's binary chunk and an accessor over a file loaded beside
//! it are the same arithmetic. Keeping the split means this layer is
//! fuzzable on its own, with no document to get past first — which
//! matters, because a coverage-guided search that has to produce valid
//! JSON before it can reach a bounds check will spend all its time on
//! the JSON.
//!
//! # The bound is not the obvious one
//!
//! The last element needs its own size, not a whole stride. So the
//! region must hold
//!
//! ```text
//! offset + (count - 1) * stride + size
//! ```
//!
//! and **not** `offset + count * stride`, which is larger whenever the
//! stride exceeds the element size — that is, whenever the data is
//! interleaved, which is the case worth reading well. Checking the
//! larger figure would refuse files that are correct, and it is the
//! easier expression to write by accident.
//!
//! # What is deliberately not here
//!
//! **Matrix shapes.** The format defines `MAT2`, `MAT3` and `MAT4`,
//! whose columns are padded to four-byte boundaries and whose element
//! size is therefore not the product of its parts. They carry inverse
//! bind matrices, which belong to skinning, which this work does not
//! read. They are unrepresentable here rather than accepted and
//! mis-sized, and the refusal for a document that asks for one belongs
//! where the document's type name is mapped.

use core::fmt;

/// The data type of one component, as the format enumerates them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    /// 8-bit signed integer.
    I8,
    /// 8-bit unsigned integer.
    U8,
    /// 16-bit signed integer.
    I16,
    /// 16-bit unsigned integer.
    U16,
    /// 32-bit unsigned integer.
    U32,
    /// 32-bit float.
    F32,
}

impl Component {
    /// The code the format spells this type with.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::I8 => 5120,
            Self::U8 => 5121,
            Self::I16 => 5122,
            Self::U16 => 5123,
            Self::U32 => 5125,
            Self::F32 => 5126,
        }
    }

    /// One component's size in bytes.
    #[must_use]
    pub const fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::U32 | Self::F32 => 4,
        }
    }

    /// Whether this type can address an element of another stream.
    ///
    /// Signed types cannot: an index is a position in a list and a
    /// negative one addresses nothing. Floats cannot either, for the
    /// same reason plus a second — a float that happens to be integral
    /// is a coincidence of the bits, not a promise about the value.
    #[must_use]
    pub const fn addresses(self) -> bool {
        matches!(self, Self::U8 | Self::U16 | Self::U32)
    }

    /// Read one component as the fraction of its own range that the
    /// format defines.
    ///
    /// Each conversion divides by the largest magnitude its type can
    /// spell rather than by its range, and the signed ones are clamped
    /// because two’s complement reaches one further down than up:
    /// `-128 / 127` is less than `-1`, and the format says the answer is
    /// `-1`.
    ///
    /// **The two widest types answer with the value unchanged**, and
    /// that arm is not dead code being humoured: `normalized` is refused
    /// on them when a view is made, so nothing reaches this through
    /// [`View::float`] — which is exactly why the conversion lives here,
    /// as something a test can call, rather than inline where the arm
    /// could only ever be exempted.
    #[must_use]
    pub fn normalize(self, value: f32) -> f32 {
        match self {
            Self::I8 => (value / 127.0).max(-1.0),
            Self::U8 => value / 255.0,
            Self::I16 => (value / 32767.0).max(-1.0),
            Self::U16 => value / 65535.0,
            Self::U32 | Self::F32 => value,
        }
    }

    /// The type a code names.
    ///
    /// # Errors
    ///
    /// [`AccessorError::UnknownComponentType`] for a code outside the
    /// table. **The refusal lives here, beside the table it is about**,
    /// rather than with the caller that read the number: a reader that
    /// mapped the code itself would be keeping a second copy of this
    /// list, and the two would drift.
    pub const fn from_code(code: u32) -> Result<Self, AccessorError> {
        match code {
            5120 => Ok(Self::I8),
            5121 => Ok(Self::U8),
            5122 => Ok(Self::I16),
            5123 => Ok(Self::U16),
            5125 => Ok(Self::U32),
            5126 => Ok(Self::F32),
            found => Err(AccessorError::UnknownComponentType { found }),
        }
    }
}

/// How many components one element holds, and in what arrangement.
///
/// **The matrix shapes are absent on purpose** — see this module's own
/// documentation. What is here is what geometry uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// One component.
    Scalar,
    /// Two.
    Vec2,
    /// Three.
    Vec3,
    /// Four.
    Vec4,
}

impl Shape {
    /// The name the format spells this shape with.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Scalar => "SCALAR",
            Self::Vec2 => "VEC2",
            Self::Vec3 => "VEC3",
            Self::Vec4 => "VEC4",
        }
    }

    /// How many components one element holds.
    #[must_use]
    pub const fn components(self) -> usize {
        match self {
            Self::Scalar => 1,
            Self::Vec2 => 2,
            Self::Vec3 => 3,
            Self::Vec4 => 4,
        }
    }
}

/// Every way an accessor can fail to describe the bytes it addresses.
///
/// **Closed on purpose**, as this crate's other refusal vocabularies
/// are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessorError {
    /// A component type code outside the format's table.
    UnknownComponentType {
        /// The code the file spelled.
        found: u32,
    },

    /// An accessor of no elements.
    ///
    /// The format asks for a finite **non-empty** sequence, and an empty
    /// one is a field nobody filled in far more often than it is a
    /// deliberate statement that there is nothing to draw.
    EmptyAccessor,

    /// Integers marked normalised where normalisation has no meaning.
    ///
    /// The flag says "read these integers as fractions of their own
    /// range". A 32-bit unsigned integer and a float have no such
    /// reading, and a file that sets the flag on one is describing a
    /// conversion that does not exist.
    NormalizedIsMeaningless {
        /// The component type it was set on.
        found: u32,
    },

    /// A stride that is not a multiple of four.
    StrideNotAligned {
        /// The stride the file declared.
        stride: usize,
    },

    /// A stride outside the range the format permits.
    StrideOutOfRange {
        /// The stride the file declared.
        stride: usize,
        /// The smallest the format allows.
        least: usize,
        /// The largest.
        most: usize,
    },

    /// A stride that would overlap consecutive elements.
    ///
    /// Distinct from a stride out of range, because it is a
    /// disagreement rather than a bad value: the stride is legal on its
    /// own and too small for the elements this accessor says it holds.
    StrideSmallerThanElement {
        /// The stride the file declared.
        stride: usize,
        /// What one element of this accessor occupies.
        element: usize,
    },

    /// An offset that is not a multiple of the component size.
    ///
    /// The format requires it, and the reason is that a reader is
    /// entitled to take an aligned view of the bytes rather than copy
    /// them out one at a time.
    OffsetNotAligned {
        /// The offset the file declared.
        offset: usize,
        /// The component size it had to be a multiple of.
        component: usize,
    },

    /// The elements do not fit in the bytes that carry them.
    ///
    /// **This is the refusal the whole module exists for.** The count,
    /// the stride and the region are three separate claims, and a reader
    /// that checks only the region's own bounds never compares them.
    OutOfRange {
        /// Bytes the accessor's own numbers require.
        needs: u64,
        /// Bytes it was given.
        available: usize,
    },

    /// A number whose arithmetic overflowed before it could be checked.
    TooLarge {
        /// Which of the accessor's numbers.
        field: &'static str,
        /// The value, widened so the message reads the same everywhere.
        value: u64,
    },

    /// An accessor asked for indices whose components cannot be one.
    NotAnIndexType {
        /// The component type it declared.
        found: u32,
    },

    /// An index accessor marked normalised.
    ///
    /// Separate from the meaningless-normalisation refusal above,
    /// because this one is legal in the format and wrong for the use: an
    /// unsigned byte marked normalised is a fraction between zero and
    /// one, and nothing addresses an element of a list with a fraction.
    NormalizedIndices,
}

impl AccessorError {
    /// The variant's own name, for a caller acting on which refusal this
    /// is rather than reading it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::UnknownComponentType { .. } => "UnknownComponentType",
            Self::EmptyAccessor => "EmptyAccessor",
            Self::NormalizedIsMeaningless { .. } => "NormalizedIsMeaningless",
            Self::StrideNotAligned { .. } => "StrideNotAligned",
            Self::StrideOutOfRange { .. } => "StrideOutOfRange",
            Self::StrideSmallerThanElement { .. } => "StrideSmallerThanElement",
            Self::OffsetNotAligned { .. } => "OffsetNotAligned",
            Self::OutOfRange { .. } => "OutOfRange",
            Self::TooLarge { .. } => "TooLarge",
            Self::NotAnIndexType { .. } => "NotAnIndexType",
            Self::NormalizedIndices => "NormalizedIndices",
        }
    }
}

impl fmt::Display for AccessorError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownComponentType { found } => {
                write!(out, "component type {found} is not one this format defines")
            }
            Self::EmptyAccessor => write!(out, "an accessor of no elements describes nothing"),
            Self::NormalizedIsMeaningless { found } => write!(
                out,
                "component type {found} is marked normalised and has no normalised reading"
            ),
            Self::StrideNotAligned { stride } => {
                write!(out, "a stride of {stride} is not a multiple of {ALIGN}")
            }
            Self::StrideOutOfRange {
                stride,
                least,
                most,
            } => write!(out, "a stride of {stride} is outside {least} to {most}"),
            Self::StrideSmallerThanElement { stride, element } => write!(
                out,
                "a stride of {stride} cannot separate elements of {element} bytes"
            ),
            Self::OffsetNotAligned { offset, component } => write!(
                out,
                "an offset of {offset} is not a multiple of the {component}-byte component"
            ),
            Self::OutOfRange { needs, available } => write!(
                out,
                "these elements need {needs} bytes and {available} are present"
            ),
            Self::TooLarge { field, value } => {
                write!(out, "{field} is {value}, which no arithmetic here holds")
            }
            Self::NotAnIndexType { found } => write!(
                out,
                "component type {found} addresses nothing: an index is a position in a list"
            ),
            Self::NormalizedIndices => write!(
                out,
                "indices marked normalised would be fractions, and nothing is at element 0.5"
            ),
        }
    }
}

impl core::error::Error for AccessorError {}

/// The alignment the format requires of a declared stride.
const ALIGN: usize = 4;

/// The smallest and largest stride the format permits.
const LEAST_STRIDE: usize = 4;
const MOST_STRIDE: usize = 252;

/// What an accessor claims about a range of bytes.
///
/// Constructed by whatever reads the document; validated against actual
/// bytes by [`Accessor::view`] or [`Accessor::indices`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accessor {
    /// The data type of one component.
    pub component: Component,
    /// How many components one element holds.
    pub shape: Shape,
    /// How many elements. Never zero in a file that is one.
    pub count: usize,
    /// Where the first element starts, within the bytes handed over.
    pub byte_offset: usize,
    /// The distance between consecutive elements, when the data is
    /// interleaved with something else.
    ///
    /// `None` means tightly packed, which is the format's own default
    /// and not a value this crate invents.
    pub byte_stride: Option<usize>,
    /// Whether integer components are fractions of their own range.
    pub normalized: bool,
}

impl Accessor {
    /// What one element occupies.
    #[must_use]
    pub const fn element_size(self) -> usize {
        self.component.size() * self.shape.components()
    }

    /// The distance between consecutive elements.
    ///
    /// The declared stride when there is one, and the element size
    /// otherwise — the format's own rule, written once here so that no
    /// caller has to remember which default applies.
    #[must_use]
    pub const fn stride(self) -> usize {
        match self.byte_stride {
            Some(stride) => stride,
            None => self.element_size(),
        }
    }

    /// Check every claim this accessor makes against `bytes`.
    ///
    /// Shared by both entry points, because the arithmetic does not care
    /// what the elements will be read as.
    fn check(self, bytes: &[u8]) -> Result<(), AccessorError> {
        if self.count == 0 {
            return Err(AccessorError::EmptyAccessor);
        }
        // The four narrow integer types each have a normalised reading;
        // the other two do not.
        if self.normalized && matches!(self.component, Component::U32 | Component::F32) {
            return Err(AccessorError::NormalizedIsMeaningless {
                found: self.component.code(),
            });
        }

        let element = self.element_size();
        if let Some(stride) = self.byte_stride {
            if !stride.is_multiple_of(ALIGN) {
                return Err(AccessorError::StrideNotAligned { stride });
            }
            if !(LEAST_STRIDE..=MOST_STRIDE).contains(&stride) {
                return Err(AccessorError::StrideOutOfRange {
                    stride,
                    least: LEAST_STRIDE,
                    most: MOST_STRIDE,
                });
            }
            if stride < element {
                return Err(AccessorError::StrideSmallerThanElement { stride, element });
            }
        }

        if !self.byte_offset.is_multiple_of(self.component.size()) {
            return Err(AccessorError::OffsetNotAligned {
                offset: self.byte_offset,
                component: self.component.size(),
            });
        }

        // **The last element needs its own size, not a whole stride.**
        // Widened to 64 bits so a hostile count cannot wrap the product
        // on a 32-bit target and land back inside the region.
        let span = (self.count as u64)
            .saturating_sub(1)
            .checked_mul(self.stride() as u64)
            .ok_or(AccessorError::TooLarge {
                field: "count times stride",
                value: self.count as u64,
            })?;
        let needs = span
            .checked_add(element as u64)
            .and_then(|sum| sum.checked_add(self.byte_offset as u64))
            .ok_or(AccessorError::TooLarge {
                field: "offset plus span",
                value: span,
            })?;
        if needs > bytes.len() as u64 {
            return Err(AccessorError::OutOfRange {
                needs,
                available: bytes.len(),
            });
        }
        Ok(())
    }

    /// Validate this accessor against `bytes` and borrow them as
    /// numbers.
    ///
    /// # Errors
    ///
    /// An [`AccessorError`] naming which of the accessor's claims failed
    /// and the numbers behind it.
    pub fn view(self, bytes: &[u8]) -> Result<View<'_>, AccessorError> {
        self.check(bytes)?;
        Ok(View {
            accessor: self,
            bytes,
        })
    }

    /// Validate this accessor as a list of positions in another stream.
    ///
    /// # Errors
    ///
    /// Everything [`Accessor::view`] refuses, plus
    /// [`AccessorError::NotAnIndexType`] for a component that cannot
    /// address anything and [`AccessorError::NormalizedIndices`] for one
    /// that would be read as a fraction.
    pub fn indices(self, bytes: &[u8]) -> Result<Indices<'_>, AccessorError> {
        if !self.component.addresses() {
            return Err(AccessorError::NotAnIndexType {
                found: self.component.code(),
            });
        }
        if self.normalized {
            return Err(AccessorError::NormalizedIndices);
        }
        self.check(bytes)?;
        Ok(Indices {
            accessor: self,
            bytes,
        })
    }
}

/// Where one component sits, or `None` past the end of the accessor.
fn offset_of(accessor: Accessor, element: usize, component: usize) -> Option<usize> {
    if element >= accessor.count || component >= accessor.shape.components() {
        return None;
    }
    let within = component.checked_mul(accessor.component.size())?;
    accessor
        .byte_offset
        .checked_add(element.checked_mul(accessor.stride())?)?
        .checked_add(within)
}

/// Read one component's raw value, as the integer its bits spell.
fn raw_at(accessor: Accessor, bytes: &[u8], at: usize) -> Option<i64> {
    let size = accessor.component.size();
    let slice = bytes.get(at..at.checked_add(size)?)?;
    let value = match accessor.component {
        // `from_le_bytes` rather than a cast: the reinterpretation is
        // the whole point, and spelling it as a conversion says so
        // without asking a lint to be told the wrap is intended.
        Component::I8 => {
            let one: [u8; 1] = slice.try_into().ok()?;
            i64::from(i8::from_le_bytes(one))
        }
        Component::U8 => i64::from(slice.first().copied()?),
        Component::I16 => {
            let pair: [u8; 2] = slice.try_into().ok()?;
            i64::from(i16::from_le_bytes(pair))
        }
        Component::U16 => {
            let pair: [u8; 2] = slice.try_into().ok()?;
            i64::from(u16::from_le_bytes(pair))
        }
        Component::U32 => {
            let quad: [u8; 4] = slice.try_into().ok()?;
            i64::from(u32::from_le_bytes(quad))
        }
        // A float's bits are not an integer, and this path never asks
        // for one: `float` reads it directly below.
        Component::F32 => return None,
    };
    Some(value)
}

/// A validated accessor, borrowing the bytes it addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View<'a> {
    accessor: Accessor,
    bytes: &'a [u8],
}

impl View<'_> {
    /// How many elements this view holds.
    #[must_use]
    pub const fn len(self) -> usize {
        self.accessor.count
    }

    /// Never true: an accessor of no elements is refused rather than
    /// borrowed.
    ///
    /// Present because a type with `len` and no `is_empty` is a lint,
    /// and answering honestly is better than suppressing it.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        false
    }

    /// What this view was validated against.
    #[must_use]
    pub const fn accessor(self) -> Accessor {
        self.accessor
    }

    /// One component, as the number a consumer of geometry wants.
    ///
    /// Integers marked normalised become fractions of their own range,
    /// by the conversions the format defines; integers that are not
    /// become themselves. **`None` means out of range and nothing
    /// else** — every other way this could fail was refused when the
    /// view was made.
    #[must_use]
    pub fn float(self, element: usize, component: usize) -> Option<f32> {
        let at = offset_of(self.accessor, element, component)?;
        if self.accessor.component == Component::F32 {
            let quad: [u8; 4] = self.bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
            return Some(f32::from_le_bytes(quad));
        }
        let raw = raw_at(self.accessor, self.bytes, at)?;
        #[expect(
            clippy::cast_precision_loss,
            reason = "the widest integer component is 32 bits and this is the conversion the \
                      format defines; a u32 past 2^24 loses precision here exactly as it does \
                      in every renderer that reads one"
        )]
        let value = raw as f32;
        if !self.accessor.normalized {
            return Some(value);
        }
        Some(self.accessor.component.normalize(value))
    }
}

/// A validated accessor whose elements address another stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Indices<'a> {
    accessor: Accessor,
    bytes: &'a [u8],
}

impl Indices<'_> {
    /// How many indices this view holds.
    #[must_use]
    pub const fn len(self) -> usize {
        self.accessor.count
    }

    /// Never true: an accessor of no elements is refused rather than
    /// borrowed.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        false
    }

    /// One index.
    ///
    /// **`None` means out of range and nothing else.** A component type
    /// that cannot address anything was refused when this view was made,
    /// so there is no second reading of a missing answer here.
    #[must_use]
    pub fn at(self, element: usize) -> Option<u32> {
        let at = offset_of(self.accessor, element, 0)?;
        let raw = raw_at(self.accessor, self.bytes, at)?;
        u32::try_from(raw).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::{Accessor, AccessorError, Component, Shape, offset_of, raw_at};

    fn packed(component: Component, shape: Shape, count: usize) -> Accessor {
        Accessor {
            component,
            shape,
            count,
            byte_offset: 0,
            byte_stride: None,
            normalized: false,
        }
    }

    /// The component table is the format's, in both directions.
    #[test]
    fn every_component_code_round_trips() {
        for component in [
            Component::I8,
            Component::U8,
            Component::I16,
            Component::U16,
            Component::U32,
            Component::F32,
        ] {
            assert_eq!(Component::from_code(component.code()), Ok(component));
        }
        assert_eq!(Component::I8.size(), 1);
        assert_eq!(Component::U16.size(), 2);
        assert_eq!(Component::F32.size(), 4);
        // 5124 is `INT`, which the format leaves out on purpose.
        assert_eq!(
            Component::from_code(5124),
            Err(AccessorError::UnknownComponentType { found: 5124 })
        );
        assert_eq!(
            Component::from_code(0),
            Err(AccessorError::UnknownComponentType { found: 0 })
        );
    }

    /// **Every arm of the normalising conversion, including the two
    /// nothing reaches through a view.**
    ///
    /// `normalized` is refused on the widest two when a view is made, so
    /// their arm is unreachable from the outside — and calling it here
    /// is the difference between a line that is covered and a line that
    /// is exempted with a promise.
    #[test]
    fn every_component_normalises_the_way_the_format_says() {
        // **Compared by bits, and that is the honest comparison here
        // rather than a way around the lint.** Every value below is
        // exact: a number divided by itself is one, a clamp returns its
        // own bound, and the identity arm returns what it was handed. A
        // tolerance would be admitting doubt this function does not have.
        let exact = |got: f32, want: f32, what: &str| {
            assert_eq!(got.to_bits(), want.to_bits(), "{what}: {got} is not {want}");
        };

        exact(Component::U8.normalize(255.0), 1.0, "unorm8 ceiling");
        exact(Component::U8.normalize(0.0), 0.0, "unorm8 floor");
        exact(Component::U16.normalize(65535.0), 1.0, "unorm16 ceiling");
        exact(Component::I8.normalize(127.0), 1.0, "snorm8 ceiling");
        exact(Component::I16.normalize(32767.0), 1.0, "snorm16 ceiling");
        // Clamped: the floor divides to slightly less than -1.
        exact(Component::I8.normalize(-128.0), -1.0, "snorm8 floor");
        exact(Component::I16.normalize(-32768.0), -1.0, "snorm16 floor");
        // And the two that have no normalised reading answer with what
        // they were given.
        exact(Component::U32.normalize(7.0), 7.0, "u32 is unchanged");
        exact(Component::F32.normalize(-2.5), -2.5, "f32 is unchanged");
    }

    /// Element sizes are the table the format prints.
    #[test]
    fn element_sizes_are_the_published_ones() {
        assert_eq!(packed(Component::U8, Shape::Scalar, 1).element_size(), 1);
        assert_eq!(packed(Component::U16, Shape::Vec2, 1).element_size(), 4);
        assert_eq!(packed(Component::F32, Shape::Vec3, 1).element_size(), 12);
        assert_eq!(packed(Component::F32, Shape::Vec4, 1).element_size(), 16);
        assert_eq!(packed(Component::U8, Shape::Vec3, 1).element_size(), 3);
        // All four names, not one: the other three arms were uncovered
        // and the gate said so.
        assert_eq!(Shape::Scalar.name(), "SCALAR");
        assert_eq!(Shape::Vec2.name(), "VEC2");
        assert_eq!(Shape::Vec3.name(), "VEC3");
        assert_eq!(Shape::Vec4.name(), "VEC4");
        assert_eq!(Shape::Scalar.components(), 1);
        assert_eq!(Shape::Vec4.components(), 4);
    }

    /// A tightly packed accessor strides by its element size, and one
    /// with a declared stride strides by that.
    #[test]
    fn the_stride_defaults_to_the_element_size() {
        let tight = packed(Component::F32, Shape::Vec3, 4);
        assert_eq!(tight.stride(), 12);
        let interleaved = Accessor {
            byte_stride: Some(32),
            ..tight
        };
        assert_eq!(interleaved.stride(), 32);
    }

    /// The helpers answer rather than panicking past the end.
    ///
    /// Exercised here because a validated view cannot reach these
    /// answers: `float` and `at` bound their arguments first. A helper
    /// whose failure arms nothing runs is a helper nobody has checked.
    #[test]
    fn the_offset_and_raw_helpers_are_total() {
        let accessor = packed(Component::U16, Shape::Vec2, 2);
        assert_eq!(offset_of(accessor, 0, 0), Some(0));
        assert_eq!(offset_of(accessor, 1, 1), Some(6));
        assert_eq!(offset_of(accessor, 2, 0), None, "past the count");
        assert_eq!(offset_of(accessor, 0, 2), None, "past the shape");

        let bytes = [1u8, 0, 2, 0, 3, 0, 4, 0];
        assert_eq!(raw_at(accessor, &bytes, 0), Some(1));
        assert_eq!(raw_at(accessor, &bytes, 7), None, "half a component");
        assert_eq!(raw_at(accessor, &bytes, 99), None, "past the end");
        // A float's bits are not an integer, and this path says so
        // rather than reinterpreting them.
        let floats = packed(Component::F32, Shape::Scalar, 1);
        assert_eq!(raw_at(floats, &[0, 0, 0, 0], 0), None);
    }
}
