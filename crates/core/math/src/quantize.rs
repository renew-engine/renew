//! The pointer quantization seam: `f64` window coordinates become the
//! integers a simulation may hear.
//!
//! Pointer events arrive from the window system as `f64` physical
//! pixels. Simulation crates are mechanically forbidden from computing
//! with floats, so somewhere a float must become an integer — and that
//! somewhere must be *one* documented function, applied by every
//! windowed driver and by the replay harness alike, or two callers
//! quantize two ways and a replayed trace stops reproducing the run it
//! recorded.
//!
//! This is that function. It lives here for [`crate::Alpha`]'s reason:
//! this is the crate a simulation is mechanically forbidden from
//! reaching, which makes it the one safe home for the exact float
//! seams the engine's edges need.

/// One axis of a pointer position, floored to the pixel it is inside
/// and saturated to the representable range.
///
/// Floor rather than round or truncate: a pointer at x = 3.7 is inside
/// pixel 3, and truncation would disagree for negative coordinates
/// (−0.5 is inside pixel −1, not pixel 0). The operations are IEEE
/// exact — `floor` on any finite `f64` and a saturating cast — so the
/// result is bit-identical on every target, which is what lets a
/// recorded `f64` trace replay into the same integers everywhere.
///
/// Edges, all deliberate: exact integers stay themselves; infinities
/// saturate to the corresponding extreme; NaN — a pointer coordinate
/// no window system should ever send — quantizes to zero, the cast's
/// defined answer, rather than to a panic in the input path.
#[must_use]
pub fn quantize_pointer(coordinate: f64) -> i32 {
    // `as` from a float saturates and maps NaN to zero by definition
    // (Rust 1.45's float-to-int semantics) — the cast IS the clamp.
    #[allow(clippy::cast_possible_truncation)]
    let quantized = coordinate.floor() as i32;
    quantized
}

#[cfg(test)]
mod tests {
    use super::quantize_pointer;

    /// The interior cases: floor, not round, not truncate.
    #[test]
    fn a_pointer_is_inside_the_pixel_below_it() {
        assert_eq!(quantize_pointer(3.7), 3);
        assert_eq!(quantize_pointer(3.0), 3);
        assert_eq!(quantize_pointer(0.999_999), 0);
        assert_eq!(quantize_pointer(0.0), 0);
        assert_eq!(
            quantize_pointer(-0.5),
            -1,
            "truncation would say 0, wrongly"
        );
        assert_eq!(quantize_pointer(-1.0), -1);
        assert_eq!(quantize_pointer(-1.000_001), -2);
    }

    /// The edges: saturation at both ends, zero for NaN, and the
    /// boundary where an f64 stops being representable as i32.
    #[test]
    fn the_edges_saturate_and_nan_is_zero() {
        assert_eq!(quantize_pointer(f64::from(i32::MAX) + 0.7), i32::MAX);
        assert_eq!(quantize_pointer(f64::from(i32::MIN) - 0.7), i32::MIN);
        assert_eq!(quantize_pointer(f64::INFINITY), i32::MAX);
        assert_eq!(quantize_pointer(f64::NEG_INFINITY), i32::MIN);
        assert_eq!(quantize_pointer(f64::NAN), 0);
        assert_eq!(quantize_pointer(f64::from(i32::MAX)), i32::MAX);
        assert_eq!(quantize_pointer(f64::from(i32::MIN)), i32::MIN);
    }

    /// Bit-exactness in the form a replay depends on: the same f64
    /// always answers the same integer, including awkward negatives.
    #[test]
    fn the_same_coordinate_always_answers_the_same_pixel() {
        for raw in [0.1_f64, -2.9, 1e9, -1e9, 1_234.499_999_999_9] {
            assert_eq!(quantize_pointer(raw), quantize_pointer(raw));
        }
    }
}
