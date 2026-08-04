//! The render interpolation factor.

use core::num::NonZeroU64;

/// The largest `f32` strictly below one.
///
/// The bound is load-bearing and not defensive. Measured: the naive
/// `remainder as f32 / step as f32` returns exactly `1.0` at 30 Hz
/// (step = `33_333_333` ns, remainder = step − 1), and even the `f64`
/// division rounds up to `1.0` at 1 Hz. An alpha of one is a renderer
/// drawing a full tick ahead of the state it interpolates *from* — a bug
/// that would have been hunted in the renderer for a week.
const LARGEST_BELOW_ONE: f32 = f32::from_bits(0x3F7F_FFFF);

/// How far past the last executed simulation step a frame stands, as a
/// fraction of the timestep: a value in `[0, 1)`.
///
/// # Contract
///
/// - **Always in `[0, 1)`.** Never exactly one, by construction — see
///   [`Alpha::new`]. A renderer may use it as a blend weight without
///   bounds-checking it.
/// - **A hint for presentation, never an input to simulation.** A
///   newtype with no arithmetic surface: the name is the contract.
///   Nothing here can be added to, multiplied by, or accumulated into
///   anything; a caller must ask for the `f32` explicitly.
///
/// # Why this type lives in the maths crate
///
/// It is the presentation half of the fixed-timestep loop, and it used
/// to live in the frame crate beside the accumulator that produces its
/// inputs. That crate is simulation-designated, and computing this ratio
/// was the only floating-point arithmetic anywhere in a crate whose
/// output has to reproduce bit-for-bit on three platforms. The
/// computation was provably inert — derived from two digested integers,
/// consumed only by rendering — and it was carried as a named exemption
/// with an `allow` at the expression.
///
/// **An exemption defended by the cost of removing it is an exemption
/// that becomes permanent.** So it moved, and the frame crate now
/// contains no float at all. Here rather than in a new presentation
/// crate for one reason that matters more than tidiness: a simulation
/// crate is *mechanically forbidden* from reaching this one — the
/// structure checker's float-closure rule fails any simulation whose
/// shipping closure includes a crate that does not deny float
/// arithmetic, and this crate does not deny it. A fresh crate would have
/// needed that same guard to mean anything. The strongest place to put a
/// float a simulation must not touch is the crate a simulation already
/// cannot reach.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Alpha(f32);

impl Alpha {
    /// Exactly on a step boundary: nothing to interpolate.
    pub const ZERO: Self = Self(0.0);

    /// The fraction `remainder / step`, clamped below one.
    ///
    /// Both arguments are nanoseconds, and both are integers on purpose:
    /// the caller holds the exact rational and hands it over unrounded,
    /// so the one rounding that happens is the one here. A consumer that
    /// wants the exact ratio never needs to call this at all.
    ///
    /// The `f64` intermediate is deliberate — see [`LARGEST_BELOW_ONE`].
    /// A `remainder` at or past `step` saturates rather than exceeding
    /// one; the frame loop never produces that, and a type whose whole
    /// contract is a range should not depend on its caller to hold it.
    #[must_use]
    pub fn new(remainder_nanos: u64, step_nanos: NonZeroU64) -> Self {
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            reason = "the f64 division is exact well past any timestep a loop uses, and the \
                      narrowing to f32 is what the renderer consumes"
        )]
        let ratio = (remainder_nanos as f64 / step_nanos.get() as f64) as f32;
        Self(if ratio < LARGEST_BELOW_ONE {
            ratio
        } else {
            LARGEST_BELOW_ONE
        })
    }

    /// The factor itself, for a renderer that is about to blend with it.
    #[must_use]
    pub const fn get(self) -> f32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{Alpha, LARGEST_BELOW_ONE};
    use core::num::NonZeroU64;

    fn step(nanos: u64) -> NonZeroU64 {
        NonZeroU64::new(nanos).expect("a positive timestep")
    }

    #[test]
    fn a_zero_remainder_is_exactly_zero() {
        assert_eq!(
            Alpha::new(0, step(16_666_667)).get().to_bits(),
            Alpha::ZERO.get().to_bits()
        );
        assert!(Alpha::new(0, step(16_666_667)).get().abs() < f32::EPSILON);
    }

    #[test]
    fn a_half_step_is_a_half() {
        let alpha = Alpha::new(8_333_333, step(16_666_667)).get();
        assert!(
            (alpha - 0.5).abs() < 1e-6,
            "half a step should read as a half, got {alpha}"
        );
    }

    /// The reason the `f64` intermediate and the bound both exist. Each
    /// of these returns exactly `1.0` under some naive formulation, and
    /// an alpha of one is a renderer a whole tick ahead of its state.
    #[test]
    fn one_nanosecond_short_of_a_step_never_reaches_one() {
        for step_nanos in [16_666_667u64, 33_333_333, 1_000_000_000, 4] {
            let alpha = Alpha::new(step_nanos - 1, step(step_nanos));
            assert!(
                alpha.get() < 1.0,
                "at a {step_nanos} ns step, alpha reached {} — a renderer would draw a \
                 full tick ahead of the state it interpolates from",
                alpha.get()
            );
            assert!(alpha.get() >= 0.0);
        }
    }

    /// The type promises a range; it does not promise its caller is
    /// careful. The frame loop cannot produce this, and the contract
    /// holds anyway.
    #[test]
    fn a_remainder_past_the_step_saturates_rather_than_escaping_the_range() {
        // Bit patterns, not values: "clamped to exactly the bound" is a
        // claim about the representation, and `==` on floats is the lint
        // this crate keeps for good reason everywhere else.
        assert_eq!(
            Alpha::new(999, step(10)).get().to_bits(),
            LARGEST_BELOW_ONE.to_bits()
        );
        assert_eq!(
            Alpha::new(10, step(10)).get().to_bits(),
            LARGEST_BELOW_ONE.to_bits()
        );
        assert!(Alpha::new(u64::MAX, step(1)).get() < 1.0);
    }

    #[test]
    fn the_bound_really_is_below_one_and_the_next_value_up_is_not() {
        // Asserted on bit patterns throughout: the next representable
        // `f32` above the bound is exactly one, so nothing lies between
        // them and the clamp costs a renderer no resolution it could
        // use. Stated this way rather than as `bound < 1.0`, which is a
        // constant the compiler folds and the linter rightly calls out
        // as an assertion that cannot fail.
        assert_eq!(
            f32::from_bits(LARGEST_BELOW_ONE.to_bits() + 1).to_bits(),
            1.0_f32.to_bits()
        );
        assert_eq!(LARGEST_BELOW_ONE.to_bits(), 0x3F7F_FFFF);
    }
}
