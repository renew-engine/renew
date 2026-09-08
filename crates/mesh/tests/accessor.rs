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

use renew_mesh::accessor::{Accessor, AccessorError, Component, Shape};

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
        | AccessorError::NormalizedIndices => None,
        AccessorError::TooLarge { .. } => Some(
            "the arithmetic that would overflow is 64-bit, and on a 64-bit target the region \
             check refuses first; the test above accepts either answer for that reason",
        ),
    }
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
    for (name, refused) in &provocations() {
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

    assert!(
        accessor_cannot_reach(&AccessorError::TooLarge {
            field: "count times stride",
            value: 0,
        })
        .is_some(),
        "the one the census calls unreachable from here"
    );
    assert!(
        !AccessorError::TooLarge {
            field: "count times stride",
            value: 7,
        }
        .to_string()
        .is_empty(),
        "and it still says something, because a 32-bit target reaches it"
    );
}
