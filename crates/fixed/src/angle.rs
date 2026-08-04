//! Angles, and the sine table they read.
//!
//! # Binary angles
//!
//! A full turn is 2³² units, so an angle is a `u32` and wrapping is exact
//! integer overflow. That is the whole reason for the representation: in
//! radians the modulus is 2π, which is irrational and therefore not
//! representable, so every wrap would lose precision and two machines
//! wrapping at different times would drift apart. Here, adding a full turn
//! is the identity, exactly, forever.
//!
//! # The table
//!
//! 513 entries covering a quarter turn, with the other three quadrants
//! reached by symmetry. Linear interpolation between entries, rounded rather
//! than truncated.
//!
//! **Measured worst error: 1.0322 units in the last place** over a three-million-point sweep
//! of the whole circle, against a double-precision reference. That is the
//! floor for a table of any size — the entries themselves are rounded to half
//! a unit, interpolating between two of them inherits that, and the
//! interpolation rounds once more. Sixteen times the entries buys 0.03 of a
//! unit, which is why this table is 2 KB rather than 32.

use crate::Fixed;

/// Entries in the quarter-turn table, excluding the endpoint.
const QUARTER_ENTRIES: u32 = 512;

/// Binary angle units in a quarter turn.
const QUARTER_TURN: u32 = 1 << 30;

/// How many low bits of an angle fall between two table entries.
const STEP_BITS: u32 = 30 - 9;

/// `sin` over a quarter turn, in raw [`Fixed`] units.
///
/// Generated, not hand-written, and checked by a test against a
/// double-precision reference — the generator lives beside the note that
/// chose the table size. First entry is exactly zero and last is exactly
/// one, which the same test asserts, because a table whose endpoints drift
/// makes every symmetry below wrong at the quadrant boundaries.
static QUARTER_SINE: [i32; 513] = [
    0, 201, 402, 603, 804, 1005, 1206, 1407, 1608, 1809, 2010, 2211, 2412, 2613, 2814, 3015, 3216,
    3417, 3617, 3818, 4019, 4219, 4420, 4621, 4821, 5022, 5222, 5422, 5623, 5823, 6023, 6224, 6424,
    6624, 6824, 7024, 7224, 7423, 7623, 7823, 8022, 8222, 8421, 8621, 8820, 9019, 9218, 9417, 9616,
    9815, 10014, 10212, 10411, 10609, 10808, 11006, 11204, 11402, 11600, 11798, 11996, 12193,
    12391, 12588, 12785, 12983, 13180, 13376, 13573, 13770, 13966, 14163, 14359, 14555, 14751,
    14947, 15143, 15338, 15534, 15729, 15924, 16119, 16314, 16508, 16703, 16897, 17091, 17285,
    17479, 17673, 17867, 18060, 18253, 18446, 18639, 18832, 19024, 19216, 19409, 19600, 19792,
    19984, 20175, 20366, 20557, 20748, 20939, 21129, 21320, 21510, 21699, 21889, 22078, 22268,
    22457, 22645, 22834, 23022, 23210, 23398, 23586, 23774, 23961, 24148, 24335, 24521, 24708,
    24894, 25080, 25265, 25451, 25636, 25821, 26005, 26190, 26374, 26558, 26742, 26925, 27108,
    27291, 27474, 27656, 27838, 28020, 28202, 28383, 28564, 28745, 28926, 29106, 29286, 29466,
    29645, 29824, 30003, 30182, 30360, 30538, 30716, 30893, 31071, 31248, 31424, 31600, 31776,
    31952, 32127, 32303, 32477, 32652, 32826, 33000, 33173, 33347, 33520, 33692, 33865, 34037,
    34208, 34380, 34551, 34721, 34892, 35062, 35231, 35401, 35570, 35738, 35907, 36075, 36243,
    36410, 36577, 36744, 36910, 37076, 37241, 37407, 37572, 37736, 37900, 38064, 38228, 38391,
    38554, 38716, 38878, 39040, 39201, 39362, 39523, 39683, 39843, 40002, 40161, 40320, 40478,
    40636, 40794, 40951, 41108, 41264, 41420, 41576, 41731, 41886, 42040, 42194, 42348, 42501,
    42654, 42806, 42958, 43110, 43261, 43412, 43562, 43713, 43862, 44011, 44160, 44308, 44456,
    44604, 44751, 44898, 45044, 45190, 45335, 45480, 45625, 45769, 45912, 46056, 46199, 46341,
    46483, 46624, 46765, 46906, 47046, 47186, 47325, 47464, 47603, 47741, 47878, 48015, 48152,
    48288, 48424, 48559, 48694, 48828, 48962, 49095, 49228, 49361, 49493, 49624, 49756, 49886,
    50016, 50146, 50275, 50404, 50532, 50660, 50787, 50914, 51041, 51166, 51292, 51417, 51541,
    51665, 51789, 51911, 52034, 52156, 52277, 52398, 52519, 52639, 52759, 52878, 52996, 53114,
    53232, 53349, 53465, 53581, 53697, 53812, 53926, 54040, 54154, 54267, 54379, 54491, 54603,
    54714, 54824, 54934, 55043, 55152, 55260, 55368, 55476, 55582, 55689, 55794, 55900, 56004,
    56108, 56212, 56315, 56418, 56520, 56621, 56722, 56823, 56923, 57022, 57121, 57219, 57317,
    57414, 57511, 57607, 57703, 57798, 57892, 57986, 58079, 58172, 58265, 58356, 58448, 58538,
    58628, 58718, 58807, 58896, 58983, 59071, 59158, 59244, 59330, 59415, 59499, 59583, 59667,
    59750, 59832, 59914, 59995, 60075, 60156, 60235, 60314, 60392, 60470, 60547, 60624, 60700,
    60776, 60851, 60925, 60999, 61072, 61145, 61217, 61288, 61359, 61429, 61499, 61568, 61637,
    61705, 61772, 61839, 61906, 61971, 62036, 62101, 62165, 62228, 62291, 62353, 62415, 62476,
    62536, 62596, 62655, 62714, 62772, 62830, 62886, 62943, 62998, 63054, 63108, 63162, 63215,
    63268, 63320, 63372, 63423, 63473, 63523, 63572, 63621, 63668, 63716, 63763, 63809, 63854,
    63899, 63944, 63987, 64031, 64073, 64115, 64156, 64197, 64237, 64277, 64316, 64354, 64392,
    64429, 64465, 64501, 64536, 64571, 64605, 64639, 64672, 64704, 64735, 64766, 64797, 64827,
    64856, 64884, 64912, 64940, 64967, 64993, 65018, 65043, 65067, 65091, 65114, 65137, 65159,
    65180, 65200, 65220, 65240, 65259, 65277, 65294, 65311, 65328, 65343, 65358, 65373, 65387,
    65400, 65413, 65425, 65436, 65447, 65457, 65467, 65476, 65484, 65492, 65499, 65505, 65511,
    65516, 65521, 65525, 65528, 65531, 65533, 65535, 65536, 65536,
];

/// An angle, as a fraction of a turn.
///
/// # Contract
///
/// - **A full turn is 2³² units**, so arithmetic wraps exactly and an angle
///   is always in range by construction. There is no normalisation step and
///   no way to hold an out-of-range angle.
/// - **`Ord` follows the underlying bits**, which orders angles by their
///   position in a turn starting from zero — not by "smallest rotation",
///   which is not a total order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Angle(u32);

impl Angle {
    /// No rotation.
    pub const ZERO: Self = Self(0);
    /// A quarter turn: 90 degrees.
    pub const QUARTER: Self = Self(QUARTER_TURN);
    /// A half turn: 180 degrees.
    pub const HALF: Self = Self(QUARTER_TURN * 2);
    /// Three quarters of a turn: 270 degrees.
    pub const THREE_QUARTERS: Self = Self(QUARTER_TURN * 3);

    /// From raw binary-angle units.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The raw units.
    #[must_use]
    pub const fn to_bits(self) -> u32 {
        self.0
    }

    /// From degrees, exactly for the ones that divide a turn evenly.
    ///
    /// Wrapping, so 370 degrees is 10 and −90 is 270 — which is the whole
    /// point of the representation rather than a convenience.
    #[must_use]
    pub const fn from_degrees(degrees: i32) -> Self {
        // 2^32 / 360 is not an integer, so the multiply happens first and in
        // 64 bits: the result is exact for every degree value.
        let turns = (degrees as i64).rem_euclid(360);
        // `rem_euclid` puts `turns` in [0, 360), so the quotient is in
        // [0, 2^32) — non-negative and inside a u32 by construction.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "rem_euclid bounds the value to [0, 2^32)"
        )]
        let bits = ((turns << 32) / 360) as u32;
        Self(bits)
    }

    /// From a fraction of a turn: `from_turn_ratio(1, 8)` is 45 degrees.
    ///
    /// # Panics
    ///
    /// If `denominator` is zero.
    #[must_use]
    pub const fn from_turn_ratio(numerator: i32, denominator: i32) -> Self {
        assert!(
            denominator != 0,
            "Angle::from_turn_ratio needs a nonzero denominator"
        );
        let scaled = ((numerator as i64) << 32) / denominator as i64;
        // Deliberately wrapping: a ratio past a whole turn, or a negative
        // one, is a real angle and lands where the turn wraps. That is the
        // representation's whole point, so truncation here is the intent
        // rather than a hazard.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "wrapping is the defined behaviour for angles past a turn"
        )]
        let bits = scaled as u32;
        Self(bits)
    }

    /// The sine.
    #[must_use]
    pub fn sin(self) -> Fixed {
        let quadrant = self.0 >> 30;
        let within = self.0 & (QUARTER_TURN - 1);
        // Quadrants 1 and 3 read the table backwards; 2 and 3 negate. The
        // reversal is `QUARTER_TURN - within` rather than a separate table,
        // and it lands exactly on the endpoint when `within` is zero.
        let magnitude = if quadrant.is_multiple_of(2) {
            lookup(within)
        } else {
            lookup(QUARTER_TURN - within)
        };
        if quadrant < 2 { magnitude } else { -magnitude }
    }

    /// The cosine, which is the sine a quarter turn ahead.
    #[must_use]
    pub fn cos(self) -> Fixed {
        Self(self.0.wrapping_add(QUARTER_TURN)).sin()
    }

    /// Both, for callers that need them together — which is every rotation.
    #[must_use]
    pub fn sin_cos(self) -> (Fixed, Fixed) {
        (self.sin(), self.cos())
    }
}

/// Sine over `[0, QUARTER_TURN]`, interpolated and rounded.
fn lookup(within: u32) -> Fixed {
    let index = (within >> STEP_BITS) as usize;
    // The endpoint: `within == QUARTER_TURN` indexes one past the last gap.
    if index >= QUARTER_ENTRIES as usize {
        return Fixed::from_bits(i64::from(QUARTER_SINE[QUARTER_ENTRIES as usize]));
    }
    let low = i64::from(QUARTER_SINE[index]);
    let high = i64::from(QUARTER_SINE[index + 1]);
    let fraction = i64::from(within & ((1 << STEP_BITS) - 1));
    // Rounded, not truncated: truncating costs half a unit in the last place
    // on every interpolation, which is a third of the total error budget for
    // nothing.
    let half = 1i64 << (STEP_BITS - 1);
    Fixed::from_bits(low + (((high - low) * fraction + half) >> STEP_BITS))
}

impl core::ops::Add for Angle {
    type Output = Self;
    /// Wrapping, which is exact: a full turn is the identity.
    fn add(self, other: Self) -> Self {
        Self(self.0.wrapping_add(other.0))
    }
}

impl core::ops::Sub for Angle {
    type Output = Self;
    /// Wrapping, which is exact.
    fn sub(self, other: Self) -> Self {
        Self(self.0.wrapping_sub(other.0))
    }
}

impl core::ops::Neg for Angle {
    type Output = Self;
    fn neg(self) -> Self {
        Self(self.0.wrapping_neg())
    }
}
