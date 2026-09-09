//! An accessor against the bytes it claims to describe.
//!
//! **The test this file exists for is the bound**, and it is the one an
//! implementer gets wrong: the last element needs its own size, not a
//! whole stride, so a region of exactly `offset + (count - 1) * stride +
//! size` bytes is enough. Writing `count * stride` instead refuses files
//! that are correct, and only whenever the data is interleaved — which
//! is to say, only on the files worth reading well.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` — which apply to `#[cfg(test)]` modules — do
// not reach it.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use renew_mesh::accessor::{Accessor, AccessorError, BufferView, Component, Shape};

/// A tightly packed accessor over `count` elements.
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

/// The refusal a claim gets, or a panic naming what it accepted.
fn refusal(accessor: Accessor, bytes: &[u8]) -> AccessorError {
    match accessor.view(bytes) {
        Ok(view) => panic!(
            "this accessor was accepted over {} bytes: {view:?}",
            bytes.len()
        ),
        Err(refused) => refused,
    }
}

/// **The last element needs its own size and not a whole stride.**
///
/// Three 32-bit floats interleaved at a stride of thirty-two: two
/// elements reach byte 32 and occupy twelve, so forty-four bytes are
/// enough and forty-three are not.
///
/// Probed by writing the bound as `count * stride`: red here, and green
/// on every tightly packed accessor in this file — which is why the
/// probe matters. A suite without an interleaved case cannot tell the
/// two expressions apart at all.
#[test]
fn the_last_element_needs_its_size_rather_than_a_stride() {
    let interleaved = Accessor {
        byte_stride: Some(32),
        ..packed(Component::F32, Shape::Vec3, 2)
    };
    assert_eq!(interleaved.stride(), 32);
    assert_eq!(interleaved.element_size(), 12);

    let exact = vec![0u8; 44];
    let view = interleaved
        .view(&exact)
        .expect("44 = 0 + (2 - 1) * 32 + 12, which is exactly enough");
    assert_eq!(view.len(), 2);
    assert!(!view.is_empty());

    let short = vec![0u8; 43];
    assert_eq!(
        refusal(interleaved, &short),
        AccessorError::OutOfRange {
            needs: 44,
            available: 43,
        }
    );

    // And the wrong bound would have wanted this many.
    assert_eq!(
        interleaved.count * interleaved.stride(),
        64,
        "the expression that refuses correct files"
    );
}

/// A tightly packed accessor fits its elements exactly.
#[test]
fn a_packed_accessor_fits_its_own_elements() {
    let accessor = packed(Component::F32, Shape::Vec3, 3);
    let bytes = vec![0u8; 36];
    assert_eq!(accessor.view(&bytes).expect("exactly enough").len(), 3);
    assert_eq!(
        refusal(accessor, &bytes[..35]),
        AccessorError::OutOfRange {
            needs: 36,
            available: 35,
        }
    );
}

/// An offset moves every element, and is counted in the bound.
#[test]
fn an_offset_is_part_of_what_the_region_must_hold() {
    let accessor = Accessor {
        byte_offset: 8,
        ..packed(Component::F32, Shape::Vec2, 2)
    };
    let bytes = vec![0u8; 24];
    assert_eq!(accessor.view(&bytes).expect("8 + 8 + 8").len(), 2);
    assert_eq!(
        refusal(accessor, &bytes[..23]),
        AccessorError::OutOfRange {
            needs: 24,
            available: 23,
        }
    );
}

/// Floats come back as they were written, element by element.
#[test]
fn floats_are_read_at_their_own_offsets() {
    let accessor = packed(Component::F32, Shape::Vec3, 2);
    let mut bytes = Vec::new();
    for value in [1.0f32, 2.0, 3.0, -4.5, 0.25, 1e10] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let view = accessor.view(&bytes).expect("two vectors");
    assert_eq!(view.float(0, 0), Some(1.0));
    assert_eq!(view.float(0, 2), Some(3.0));
    assert_eq!(view.float(1, 0), Some(-4.5));
    assert_eq!(view.float(1, 2), Some(1e10));
    assert_eq!(view.float(2, 0), None, "past the count");
    assert_eq!(view.float(0, 3), None, "past the shape");
    assert_eq!(view.accessor(), accessor);
}

/// An interleaved accessor skips what is not its own.
#[test]
fn an_interleaved_accessor_steps_over_its_neighbours() {
    let accessor = Accessor {
        byte_stride: Some(16),
        byte_offset: 4,
        ..packed(Component::F32, Shape::Vec2, 2)
    };
    let mut bytes = vec![0u8; 4];
    bytes.extend_from_slice(&7.0f32.to_le_bytes());
    bytes.extend_from_slice(&8.0f32.to_le_bytes());
    bytes.extend_from_slice(&[0xFF; 4]); // A neighbour's bytes.
    bytes.extend_from_slice(&[0xFF; 4]);
    bytes.extend_from_slice(&9.0f32.to_le_bytes());
    bytes.extend_from_slice(&10.0f32.to_le_bytes());

    let view = accessor.view(&bytes).expect("4 + 16 + 8 = 28");
    assert_eq!(view.float(0, 0), Some(7.0));
    assert_eq!(view.float(0, 1), Some(8.0));
    assert_eq!(
        view.float(1, 0),
        Some(9.0),
        "the neighbour was stepped over"
    );
    assert_eq!(view.float(1, 1), Some(10.0));
}

/// **Every normalised conversion the format defines, at both ends of
/// its range.**
///
/// Each divides by the largest magnitude its type can spell rather than
/// by its range, and the signed ones are clamped because two's
/// complement reaches one further down than up: `-128 / 127` is less
/// than `-1`, and the format says the answer is `-1`.
#[test]
fn normalised_integers_become_the_fractions_the_format_names() {
    let cases: [(Component, &[u8], f32, f32); 4] = [
        (Component::U8, &[0, 255], 0.0, 1.0),
        (Component::I8, &[0x80, 127], -1.0, 1.0),
        (Component::U16, &[0, 0, 255, 255], 0.0, 1.0),
        (Component::I16, &[0x00, 0x80, 0xFF, 0x7F], -1.0, 1.0),
    ];
    for (component, bytes, low, high) in cases {
        let accessor = Accessor {
            normalized: true,
            ..packed(component, Shape::Scalar, 2)
        };
        let view = accessor.view(bytes).expect("two scalars");
        assert_eq!(
            view.float(0, 0),
            Some(low),
            "{component:?} at its floor, clamped where the format clamps"
        );
        assert_eq!(view.float(1, 0), Some(high), "{component:?} at its ceiling");
    }
}

/// An integer that is not normalised is itself.
#[test]
fn an_unnormalised_integer_keeps_its_value() {
    let accessor = packed(Component::U16, Shape::Scalar, 2);
    let view = accessor
        .view(&[0x10, 0x00, 0xFF, 0xFF])
        .expect("two scalars");
    assert_eq!(view.float(0, 0), Some(16.0));
    assert_eq!(view.float(1, 0), Some(65535.0), "not a fraction");
}

/// Indices come back as positions, from each type that can be one.
#[test]
fn indices_are_read_from_every_type_that_can_address() {
    let eight = packed(Component::U8, Shape::Scalar, 3);
    let view = eight.indices(&[0, 1, 2]).expect("three indices");
    assert_eq!(view.len(), 3);
    assert!(!view.is_empty());
    assert_eq!(view.at(0), Some(0));
    assert_eq!(view.at(2), Some(2));
    assert_eq!(view.at(3), None, "past the count");

    let sixteen = packed(Component::U16, Shape::Scalar, 2);
    let view = sixteen.indices(&[0x01, 0x00, 0xFF, 0xFF]).expect("two");
    assert_eq!(view.at(0), Some(1));
    assert_eq!(view.at(1), Some(65535));

    let thirty_two = packed(Component::U32, Shape::Scalar, 1);
    let view = thirty_two.indices(&[0xFF, 0xFF, 0xFF, 0xFF]).expect("one");
    assert_eq!(view.at(0), Some(u32::MAX));
}

/// **A type that cannot address anything is refused as indices, and the
/// same accessor is a perfectly good view.**
///
/// Which is the point of two entry points: the accessor is not wrong,
/// the use of it is.
#[test]
fn a_type_that_addresses_nothing_is_refused_as_indices() {
    for component in [Component::I8, Component::I16, Component::F32] {
        let accessor = packed(component, Shape::Scalar, 1);
        let bytes = vec![0u8; component.size()];
        assert_eq!(
            accessor.indices(&bytes).expect_err("not an index type"),
            AccessorError::NotAnIndexType {
                found: component.code()
            }
        );
        assert!(
            accessor.view(&bytes).is_ok(),
            "{component:?} is a fine attribute and only a bad index"
        );
    }
}

/// Indices marked normalised would be fractions.
#[test]
fn normalised_indices_are_refused() {
    let accessor = Accessor {
        normalized: true,
        ..packed(Component::U16, Shape::Scalar, 2)
    };
    assert_eq!(
        accessor
            .indices(&[0, 0, 1, 0])
            .expect_err("a fraction addresses nothing"),
        AccessorError::NormalizedIndices
    );
    assert!(
        accessor.view(&[0, 0, 1, 0]).is_ok(),
        "and the same accessor is a legal normalised attribute"
    );
}

/// An accessor of nothing is refused rather than borrowed empty.
#[test]
fn an_accessor_of_no_elements_is_refused() {
    let accessor = packed(Component::F32, Shape::Vec3, 0);
    assert_eq!(refusal(accessor, &[]), AccessorError::EmptyAccessor);
}

/// Normalisation has no meaning on the two widest types.
#[test]
fn normalisation_is_refused_where_it_has_no_reading() {
    for component in [Component::U32, Component::F32] {
        let accessor = Accessor {
            normalized: true,
            ..packed(component, Shape::Scalar, 1)
        };
        assert_eq!(
            refusal(accessor, &[0, 0, 0, 0]),
            AccessorError::NormalizedIsMeaningless {
                found: component.code()
            }
        );
    }
}

/// Every rule the format puts on a declared stride.
#[test]
fn a_declared_stride_obeys_its_three_rules() {
    let base = packed(Component::F32, Shape::Vec3, 2);
    let bytes = vec![0u8; 256];

    let unaligned = Accessor {
        byte_stride: Some(13),
        ..base
    };
    assert_eq!(
        refusal(unaligned, &bytes),
        AccessorError::StrideNotAligned { stride: 13 }
    );

    let enormous = Accessor {
        byte_stride: Some(256),
        ..base
    };
    assert_eq!(
        refusal(enormous, &bytes),
        AccessorError::StrideOutOfRange {
            stride: 256,
            least: 4,
            most: 252,
        }
    );

    // **Legal on its own and too small for these elements**, which is a
    // disagreement rather than a bad value and gets its own refusal.
    let overlapping = Accessor {
        byte_stride: Some(8),
        ..base
    };
    assert_eq!(
        refusal(overlapping, &bytes),
        AccessorError::StrideSmallerThanElement {
            stride: 8,
            element: 12,
        }
    );

    // Zero is out of range rather than unaligned: it is a multiple of
    // four, and the format's floor is four.
    let nothing = Accessor {
        byte_stride: Some(0),
        ..base
    };
    assert_eq!(
        refusal(nothing, &bytes),
        AccessorError::StrideOutOfRange {
            stride: 0,
            least: 4,
            most: 252,
        }
    );
}

/// An offset must be a multiple of the component size.
#[test]
fn an_offset_is_aligned_to_its_component() {
    let accessor = Accessor {
        byte_offset: 2,
        ..packed(Component::F32, Shape::Scalar, 1)
    };
    assert_eq!(
        refusal(accessor, &[0u8; 64]),
        AccessorError::OffsetNotAligned {
            offset: 2,
            component: 4,
        }
    );

    // The same offset is fine for a two-byte component.
    let narrow = Accessor {
        byte_offset: 2,
        ..packed(Component::U16, Shape::Scalar, 1)
    };
    assert!(narrow.view(&[0u8; 64]).is_ok());
}

/// A count whose arithmetic overflows is refused before it is believed.
#[test]
fn a_count_that_overflows_the_span_is_refused() {
    let accessor = Accessor {
        byte_stride: Some(252),
        ..packed(Component::F32, Shape::Vec3, usize::MAX)
    };
    let refused = refusal(accessor, &[0u8; 64]);
    // On a 64-bit target the product is representable and the region
    // check catches it; on a narrower one the multiplication is what
    // catches it. Both are refusals with numbers, and which arrives
    // depends on the target rather than on the file.
    assert!(
        matches!(
            refused,
            AccessorError::TooLarge { .. } | AccessorError::OutOfRange { .. }
        ),
        "a hostile count is refused one way or the other: {refused:?}"
    );
}

/// A view hands back its own region of a buffer, and nothing else.
#[test]
fn a_view_borrows_exactly_its_own_region() {
    let buffer: Vec<u8> = (0u8..64).collect();
    let view = BufferView {
        byte_offset: 8,
        byte_length: 16,
        byte_stride: None,
    };
    let region = view.resolve(&buffer).expect("8 + 16 is inside 64");
    assert_eq!(region.len(), 16);
    assert_eq!(region[0], 8, "it starts where the offset says");
    assert_eq!(region[15], 23, "and ends where the length says");
}

/// **A region that is not inside its buffer, which is the claim an
/// accessor cannot make on its own.**
///
/// An accessor is checked against the region it is handed; nothing in
/// that check can tell whether the region was really there. This is
/// catalogue entry 23's third claim, and the reason a view is its own
/// type.
#[test]
fn a_view_past_the_end_of_its_buffer_is_refused() {
    let buffer = [0u8; 32];

    let over = BufferView {
        byte_offset: 24,
        byte_length: 16,
        byte_stride: None,
    };
    assert_eq!(
        over.resolve(&buffer).expect_err("24 + 16 is past 32"),
        AccessorError::ViewOutOfRange {
            needs: 40,
            available: 32,
        }
    );

    // Exactly reaching the end is inside it, which is the boundary the
    // refusal above would get wrong in the other direction.
    let exact = BufferView {
        byte_offset: 16,
        byte_length: 16,
        byte_stride: None,
    };
    assert_eq!(exact.resolve(&buffer).expect("16 + 16 is 32").len(), 16);
}

/// A stride wider than the region that declares it.
#[test]
fn a_stride_wider_than_its_view_is_refused() {
    let buffer = [0u8; 64];
    let view = BufferView {
        byte_offset: 0,
        byte_length: 16,
        byte_stride: Some(32),
    };
    assert_eq!(
        view.resolve(&buffer)
            .expect_err("32 cannot separate anything inside 16"),
        AccessorError::StrideExceedsView {
            stride: 32,
            length: 16,
        }
    );

    // Equal is legal: a stride the width of the region separates one
    // element from nothing, which is what a single-element view is.
    let equal = BufferView {
        byte_stride: Some(16),
        ..view
    };
    assert!(equal.resolve(&buffer).is_ok());
}

/// **A zero-length view resolves, and the accessor over it refuses with
/// the numbers.**
///
/// Deliberate rather than an omission: a refusal at the view would be a
/// second answer to a question the accessor already answers better,
/// because an accessor holds at least one element and says how many
/// bytes that needed.
#[test]
fn a_zero_length_view_is_left_for_the_accessor_to_refuse() {
    let buffer = [0u8; 32];
    let empty = BufferView {
        byte_offset: 4,
        byte_length: 0,
        byte_stride: None,
    };
    let region = empty.resolve(&buffer).expect("an empty region is a region");
    assert!(region.is_empty());

    assert_eq!(
        refusal(packed(Component::F32, Shape::Vec3, 1), region),
        AccessorError::OutOfRange {
            needs: 12,
            available: 0,
        }
    );
}

/// **The stride travels from the view to the accessors over it**, which
/// is the relationship a caller assembling from a document has to
/// preserve.
///
/// Pinned because it is one number in two places. If it ever arrives
/// wrong, the fix is to take the field off the accessor rather than to
/// check the two against each other.
#[test]
fn the_stride_comes_from_the_view() {
    let buffer = [0u8; 128];
    let view = BufferView {
        byte_offset: 0,
        byte_length: 44,
        byte_stride: Some(32),
    };
    let region = view.resolve(&buffer).expect("inside the buffer");

    let accessor = Accessor {
        byte_stride: view.byte_stride,
        ..packed(Component::F32, Shape::Vec3, 2)
    };
    assert_eq!(accessor.stride(), 32, "the view's number, not a default");
    assert_eq!(
        accessor.view(region).expect("44 is exactly enough").len(),
        2
    );

    // The same accessor without it reads the same region as tightly
    // packed, and fits — which is why the two must not disagree.
    let packed_instead = packed(Component::F32, Shape::Vec3, 2);
    assert_eq!(packed_instead.stride(), 12);
    assert!(packed_instead.view(region).is_ok());
}

/// **Every refusal this layer can make is reachable, and every one it
/// cannot make says why.**
///
/// No wildcard arm, so a variant added later stops this file compiling
/// until somebody decides which it is.
fn accessor_cannot_reach(refusal: &AccessorError) -> Option<&'static str> {
    match refusal {
        AccessorError::UnknownComponentType { .. }
        | AccessorError::EmptyAccessor
        | AccessorError::NormalizedIsMeaningless { .. }
        | AccessorError::StrideNotAligned { .. }
        | AccessorError::StrideOutOfRange { .. }
        | AccessorError::StrideSmallerThanElement { .. }
        | AccessorError::OffsetNotAligned { .. }
        | AccessorError::OutOfRange { .. }
        | AccessorError::NotAnIndexType { .. }
        | AccessorError::ViewOutOfRange { .. }
        | AccessorError::StrideExceedsView { .. }
        | AccessorError::NormalizedIndices => None,
        AccessorError::UnknownShape => Some(
            "a shape here is an enum a caller already holds; the name that spells one is              read where a document is, and the refusal for a name outside the four belongs              beside that table",
        ),
        AccessorError::TooLarge { .. } => Some(
            "the arithmetic that would overflow is 64-bit, and on a 64-bit target the region \
             check refuses first; the test above accepts either answer for that reason",
        ),
    }
}

/// The two a view refuses, which are a layer in front of the rest.
///
/// Their own function because they are about a different claim — whether
/// a region is inside its buffer at all — and because the list below hit
/// the line limit, which is the linter noticing the same thing.
fn view_provocations() -> [(&'static str, AccessorError); 2] {
    [
        (
            "ViewOutOfRange",
            BufferView {
                byte_offset: 24,
                byte_length: 16,
                byte_stride: None,
            }
            .resolve(&[0u8; 32])
            .expect_err("past the buffer"),
        ),
        (
            "StrideExceedsView",
            BufferView {
                byte_offset: 0,
                byte_length: 16,
                byte_stride: Some(32),
            }
            .resolve(&[0u8; 64])
            .expect_err("wider than the region"),
        ),
    ]
}

/// One claim per refusal, each wrong in exactly one way.
///
/// Its own function because the list is what makes the census long, and
/// a hundred lines of fixtures inside an assertion loop reads as one
/// long thing rather than two short ones.
fn provocations() -> [(&'static str, AccessorError); 10] {
    let bytes = [0u8; 64];
    let base = packed(Component::F32, Shape::Vec3, 2);

    [
        (
            "UnknownComponentType",
            Component::from_code(5124).expect_err("5124 is not in the table"),
        ),
        (
            "EmptyAccessor",
            refusal(packed(Component::F32, Shape::Vec3, 0), &bytes),
        ),
        (
            "NormalizedIsMeaningless",
            refusal(
                Accessor {
                    normalized: true,
                    ..packed(Component::F32, Shape::Scalar, 1)
                },
                &bytes,
            ),
        ),
        (
            "StrideNotAligned",
            refusal(
                Accessor {
                    byte_stride: Some(13),
                    ..base
                },
                &bytes,
            ),
        ),
        (
            "StrideOutOfRange",
            refusal(
                Accessor {
                    byte_stride: Some(256),
                    ..base
                },
                &bytes,
            ),
        ),
        (
            "StrideSmallerThanElement",
            refusal(
                Accessor {
                    byte_stride: Some(8),
                    ..base
                },
                &bytes,
            ),
        ),
        (
            "OffsetNotAligned",
            refusal(
                Accessor {
                    byte_offset: 2,
                    ..packed(Component::F32, Shape::Scalar, 1)
                },
                &bytes,
            ),
        ),
        (
            "OutOfRange",
            refusal(packed(Component::F32, Shape::Vec3, 99), &bytes),
        ),
        (
            "NotAnIndexType",
            packed(Component::F32, Shape::Scalar, 1)
                .indices(&bytes)
                .expect_err("a float addresses nothing"),
        ),
        (
            "NormalizedIndices",
            Accessor {
                normalized: true,
                ..packed(Component::U16, Shape::Scalar, 1)
            }
            .indices(&bytes)
            .expect_err("a fraction addresses nothing"),
        ),
    ]
}

/// The census and the claims agree, and every refusal says something.
#[test]
fn the_census_and_the_claims_agree() {
    for (name, refused) in provocations().iter().chain(&view_provocations()) {
        assert!(
            accessor_cannot_reach(refused).is_none(),
            "`{name}` is provoked here and the census calls it unreachable"
        );
        assert_eq!(
            refused.name(),
            *name,
            "the claim meant to provoke `{name}` provoked something else"
        );
        let shown = refused.to_string();
        assert!(!shown.is_empty(), "{refused:?} says nothing");
    }

    // **Every variant is asked its name, including the ones this layer
    // cannot reach.** A refusal a caller cannot key on is half a
    // refusal, and the ones refused elsewhere — a shape name read from a
    // document, an overflow only a narrow target reaches — are exactly
    // the ones no provocation here covers. Asked once, so the next
    // variant added is covered by a test that already exists.
    let unreachable = [
        AccessorError::TooLarge {
            field: "count times stride",
            value: 7,
        },
        AccessorError::UnknownShape,
    ];
    for refusal in unreachable {
        assert!(
            accessor_cannot_reach(&refusal).is_some(),
            "{refusal:?} is listed here and the census calls it reachable"
        );
        assert!(!refusal.name().is_empty(), "{refusal:?} has no name");
        assert!(!refusal.to_string().is_empty(), "{refusal:?} says nothing");
    }

    assert_eq!(
        AccessorError::TooLarge {
            field: "count times stride",
            value: 7,
        }
        .name(),
        "TooLarge",
        "still says something, because a 32-bit target reaches it"
    );
}
