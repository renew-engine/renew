//! Turning the characters of a number into the type a caller asked for.
//!
//! **A whole number is read by decimal arithmetic over its own
//! characters, never through a float.** That is the part worth arguing
//! for, because the short version — parse it as a double, check the
//! fraction is zero — is one line and is wrong in two ways at once.
//!
//! It is inexact above 2^53, so a length or an offset near the top of a
//! `u64` comes back as the nearest double rather than as itself, and
//! nothing says so. And it drags float arithmetic into a crate that
//! otherwise has none, which matters here beyond taste: this reader is
//! meant to be usable from code that has to produce the same answer on
//! every machine it runs on, and a rounding mode is not something such
//! code should inherit by accident.
//!
//! The arithmetic below is exact at every magnitude, refuses what does
//! not fit by name, and touches no float at all. Floats appear in this
//! file only where a caller explicitly asked for one.

use crate::error::{JsonError, JsonErrorKind};

/// A whole number, taken apart from how it was written.
struct Whole {
    negative: bool,
    magnitude: u128,
}

/// Read `lexeme` as a whole number, whatever spelling it arrived in.
///
/// `3`, `3.0`, `3e0`, `30e-1` and `0.03e2` are all three. What is
/// refused is a fraction that is not zero — a value the caller cannot
/// have meant to ask for as a whole number — and a magnitude past
/// `u128`, which nothing fits.
///
/// The method is the one a person would use on paper. Take the digits of
/// the integer and fraction parts as one run; work out where the decimal
/// point ends up once the exponent has moved it; refuse if any non-zero
/// digit lands to the right of it; then accumulate what is to the left,
/// ten at a time, with every step checked.
fn whole(lexeme: &str, at: usize, target: &'static str) -> Result<Whole, JsonError> {
    let out_of_range = || JsonError::new(at, JsonErrorKind::IntegerOutOfRange { target });

    let (negative, unsigned) = lexeme
        .strip_prefix('-')
        .map_or((false, lexeme), |rest| (true, rest));
    let (mantissa, exponent) = unsigned.find(['e', 'E']).map_or((unsigned, ""), |marker| {
        (
            unsigned.get(..marker).unwrap_or(""),
            unsigned.get(marker.saturating_add(1)..).unwrap_or(""),
        )
    });
    let (integer, fraction) = mantissa.find('.').map_or((mantissa, ""), |point| {
        (
            mantissa.get(..point).unwrap_or(""),
            mantissa.get(point.saturating_add(1)..).unwrap_or(""),
        )
    });

    // The digits of both parts read as one run, so that moving the
    // decimal point is arithmetic on an index rather than a second case.
    let integer = integer.as_bytes();
    let fraction = fraction.as_bytes();
    let digits = integer.len().saturating_add(fraction.len());
    let digit = |index: usize| -> u32 {
        integer
            .get(index)
            .or_else(|| fraction.get(index.saturating_sub(integer.len())))
            .map_or(0, |byte| u32::from(byte.saturating_sub(b'0')))
    };

    // Where the point sits after the exponent has moved it, counted in
    // digits from the left of that run. At or before zero means every
    // digit is fractional, which is what `1e-1` is.
    let point = i64::try_from(integer.len())
        .unwrap_or(i64::MAX)
        .saturating_add(exponent_value(exponent));
    let integral = usize::try_from(point.max(0))
        .unwrap_or(usize::MAX)
        .min(digits);

    for index in integral..digits {
        if digit(index) != 0 {
            return Err(JsonError::new(at, JsonErrorKind::FractionalInteger));
        }
    }

    let mut magnitude = 0u128;
    for index in 0..integral {
        magnitude = magnitude
            .checked_mul(10)
            .and_then(|scaled| scaled.checked_add(u128::from(digit(index))))
            .ok_or_else(out_of_range)?;
    }
    // Digits the exponent added past the end of what was written. Zero
    // stays zero however far it is shifted, and skipping the loop for it
    // is what keeps `0e9999999999` an answer rather than a walk.
    if magnitude != 0 {
        let trailing = usize::try_from(point.max(0))
            .unwrap_or(usize::MAX)
            .saturating_sub(digits);
        for _ in 0..trailing {
            magnitude = magnitude.checked_mul(10).ok_or_else(out_of_range)?;
        }
    }

    Ok(Whole {
        negative,
        magnitude,
    })
}

/// The exponent as a number, saturating rather than wrapping.
///
/// A number may be a hundred and twenty-eight characters long, so its
/// exponent digits can spell something far past `i64`. Saturating is
/// exactly right for what happens next: an exponent that large pushes
/// every digit off one end, where the answer is zero or a refusal, or
/// off the other, where the answer is a refusal either way.
fn exponent_value(exponent: &str) -> i64 {
    let (negative, digits) = exponent.strip_prefix('-').map_or_else(
        || (false, exponent.strip_prefix('+').unwrap_or(exponent)),
        |rest| (true, rest),
    );
    let mut value = 0i64;
    for byte in digits.as_bytes() {
        value = value
            .saturating_mul(10)
            .saturating_add(i64::from(byte.saturating_sub(b'0')));
    }
    if negative {
        0i64.saturating_sub(value)
    } else {
        value
    }
}

/// `lexeme` as an unsigned whole number no larger than `ceiling`.
///
/// `-0` is zero and is accepted, because it is.
pub(crate) fn unsigned(
    lexeme: &str,
    at: usize,
    target: &'static str,
    ceiling: u128,
) -> Result<u128, JsonError> {
    let value = whole(lexeme, at, target)?;
    if (value.negative && value.magnitude != 0) || value.magnitude > ceiling {
        return Err(JsonError::new(
            at,
            JsonErrorKind::IntegerOutOfRange { target },
        ));
    }
    Ok(value.magnitude)
}

/// `lexeme` as a signed whole number that fits an `i64`.
pub(crate) fn signed(lexeme: &str, at: usize) -> Result<i64, JsonError> {
    let value = whole(lexeme, at, "i64")?;
    let magnitude = i128::try_from(value.magnitude).unwrap_or(i128::MAX);
    let signed = if value.negative {
        0i128.saturating_sub(magnitude)
    } else {
        magnitude
    };
    i64::try_from(signed)
        .map_err(|_| JsonError::new(at, JsonErrorKind::IntegerOutOfRange { target: "i64" }))
}

/// `lexeme` as a double.
///
/// The standard library's parser is exactly right here and wrong for
/// whole numbers, which is why it is used for one and not the other:
/// its float grammar is *wider* than JSON's, so it must never decide
/// whether a document is valid, and its rounding is correct, so it
/// should decide what a valid fraction is worth.
///
/// The parse cannot fail — the grammar was checked before this — and the
/// fallback exists so that "cannot" is not spelled as a panic.
pub(crate) fn double(lexeme: &str, at: usize) -> Result<f64, JsonError> {
    let value = lexeme.parse::<f64>().unwrap_or(f64::NAN);
    if value.is_finite() {
        Ok(value)
    } else {
        Err(JsonError::new(at, JsonErrorKind::NumberNotFinite))
    }
}

/// `lexeme` as a single.
///
/// Read from the characters rather than rounded down from a double, so a
/// value gets one rounding rather than two. A number that is finite as a
/// double and an infinity as a single — `3e300` is one — is refused here
/// for the same reason `1e999` is refused there.
pub(crate) fn single(lexeme: &str, at: usize) -> Result<f32, JsonError> {
    let value = lexeme.parse::<f32>().unwrap_or(f32::NAN);
    if value.is_finite() {
        Ok(value)
    } else {
        Err(JsonError::new(at, JsonErrorKind::NumberNotFinite))
    }
}

#[cfg(test)]
mod tests {
    use crate::{Json, JsonErrorKind};

    /// The root of a one-number document.
    fn number(text: &str) -> Json<'_> {
        Json::parse(text.as_bytes())
            .unwrap_or_else(|error| panic!("`{text}` is not a number: {error}"))
    }

    fn as_u64(text: &str) -> Result<u64, JsonErrorKind> {
        number(text)
            .root()
            .as_u64()
            .map_err(|error| error.kind().clone())
    }

    fn as_u32(text: &str) -> Result<u32, JsonErrorKind> {
        number(text)
            .root()
            .as_u32()
            .map_err(|error| error.kind().clone())
    }

    fn as_i64(text: &str) -> Result<i64, JsonErrorKind> {
        number(text)
            .root()
            .as_i64()
            .map_err(|error| error.kind().clone())
    }

    /// **A whole number is whole however it was spelled.**
    ///
    /// This is the rule the formats this reader was sized for state
    /// outright: an integer field *may* be written with a zero fraction
    /// or with an exponent. A reader that took only bare digits would
    /// reject documents that are entirely legal, and it would do it on
    /// files that look fine to every other tool — the quiet kind of
    /// wrong.
    ///
    /// Probed by refusing any lexeme containing a point or an exponent:
    /// `3.0` answers `FractionalInteger` where 3 was expected, which is
    /// the first of eight cases that would have failed.
    #[test]
    fn a_whole_number_is_whole_however_it_was_spelled() {
        for (text, expected) in [
            ("3", 3u64),
            ("3.0", 3),
            ("3.00000", 3),
            ("3e0", 3),
            ("3E0", 3),
            ("3e2", 300),
            ("0.03e2", 3),
            ("30e-1", 3),
            ("300e-2", 3),
            ("0", 0),
            ("-0", 0),
            ("1e-0", 1),
            ("0e999999999", 0),
            ("0.0e-999999999", 0),
        ] {
            assert_eq!(as_u64(text), Ok(expected), "`{text}`");
        }
    }

    /// **Exactness holds past where a double stops being exact.**
    ///
    /// Every value here is above 2^53, where the one-line version of
    /// this conversion — parse as a double, check the fraction is zero —
    /// silently returns the nearest representable number instead. That
    /// is the defect this arithmetic exists to avoid, and it is
    /// invisible without a case that crosses the line.
    ///
    /// Probed by routing `as_u64` through an `f64`: the first case comes
    /// back as 9007199254740992, one short of what the document says,
    /// with nothing anywhere to report that it was rounded.
    #[test]
    fn a_large_whole_number_comes_back_as_itself() {
        assert_eq!(as_u64("9007199254740993"), Ok(9_007_199_254_740_993));
        assert_eq!(
            as_u64("18446744073709550616"),
            Ok(18_446_744_073_709_550_616)
        );
        assert_eq!(as_u64("18446744073709551615"), Ok(u64::MAX));
        assert_eq!(as_i64("-9223372036854775808"), Ok(i64::MIN));
        assert_eq!(as_i64("9223372036854775807"), Ok(i64::MAX));
        assert_eq!(as_i64("-7"), Ok(-7));
        assert_eq!(as_i64("-7.0e0"), Ok(-7));
    }

    /// **What does not fit is refused by name, and so is a fraction.**
    ///
    /// The two are different faults with different fixes — one is a
    /// field that needs a wider type, the other is a document that means
    /// something else — so they are different refusals.
    ///
    /// Probed by dropping the ceiling check from `unsigned`:
    /// `4294967296` comes back as 4294967295 — the narrowing's own
    /// saturation, silently one less than the document said.
    #[test]
    fn what_does_not_fit_and_what_is_not_whole_are_told_apart() {
        for text in ["3.5", "0.1", "1e-1", "-0.5", "1.0000001"] {
            assert_eq!(
                as_u64(text),
                Err(JsonErrorKind::FractionalInteger),
                "`{text}`"
            );
        }
        for (text, target) in [
            ("4294967296", "u32"),
            ("-1", "u32"),
            ("1e999", "u32"),
            // More digits than `u128` holds, so the refusal comes from
            // the accumulation rather than from the ceiling.
            ("123456789012345678901234567890123456789012345", "u32"),
        ] {
            assert_eq!(
                as_u32(text),
                Err(JsonErrorKind::IntegerOutOfRange { target }),
                "`{text}`"
            );
        }
        assert_eq!(
            as_u64("18446744073709551616"),
            Err(JsonErrorKind::IntegerOutOfRange { target: "u64" })
        );
        assert_eq!(
            as_i64("9223372036854775808"),
            Err(JsonErrorKind::IntegerOutOfRange { target: "i64" })
        );
        assert_eq!(
            as_i64("-9223372036854775809"),
            Err(JsonErrorKind::IntegerOutOfRange { target: "i64" })
        );
        assert_eq!(
            as_i64("-99999999999999999999999999999999999999999"),
            Err(JsonErrorKind::IntegerOutOfRange { target: "i64" })
        );
    }

    /// **A number that is finite as written and not as read is
    /// refused.**
    ///
    /// An infinity poisons every sum it reaches: a bounding box built
    /// from one is silently wrong everywhere rather than loudly wrong
    /// here. The single is refused at a magnitude the double accepts,
    /// which is the case that a reader narrowing after the fact would
    /// miss.
    ///
    /// Probed by returning the double without the finiteness check:
    /// `1e999` comes back as `Ok(inf)`, which is the value that would
    /// then have poisoned everything downstream of it.
    #[test]
    fn a_number_that_rounds_to_an_infinity_is_refused() {
        let document = Json::parse(b"[1.5, -2.25, 1e308, 3e300, 1e999]").expect("five numbers");
        let values: Vec<_> = document.root().elements().expect("an array").collect();

        assert_eq!(values[0].as_f64(), Ok(1.5));
        assert_eq!(values[0].as_f32(), Ok(1.5));
        assert_eq!(values[1].as_f64(), Ok(-2.25));
        assert!(values[2].as_f64().is_ok());
        // Finite as a double, an infinity as a single.
        assert!(values[3].as_f64().is_ok());
        assert_eq!(
            values[3].as_f32().map_err(|error| error.kind().clone()),
            Err(JsonErrorKind::NumberNotFinite)
        );
        assert_eq!(
            values[4].as_f64().map_err(|error| error.kind().clone()),
            Err(JsonErrorKind::NumberNotFinite)
        );
        assert_eq!(
            values[4].as_f32().map_err(|error| error.kind().clone()),
            Err(JsonErrorKind::NumberNotFinite)
        );
        // A tiny exponent is not an infinity; it is zero, and legal.
        let small = Json::parse(b"1e-999").expect("a legal number");
        assert_eq!(small.root().as_f64(), Ok(0.0));
    }
}
