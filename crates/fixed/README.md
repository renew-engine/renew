# renew-fixed

Fixed-point arithmetic for simulation code whose output has to reproduce
bit-for-bit on every target.

**Status: `bootstrap`.** Interface churn expected. See
[`Cargo.toml`](Cargo.toml) for the machine-readable manifest — maturity,
dependencies and core status live there, not here.

## Why this exists

Rust guarantees IEEE 754 semantics for `f32` and `f64` *operators*, and the
guarantee stops there. `sin`, `cos` and their siblings come from the platform's
maths library and may differ between targets by an ulp. A simulation that calls
them cannot claim to reproduce across machines.

Integer arithmetic is bit-identical everywhere, with nothing to police. That is
the entire argument, and it is why physics is written in this type rather than
in floats.

## The representation

`Q47.16` in an `i64`: 16 fractional bits, resolution 2⁻¹⁶ ≈ 0.0000153, range
±2⁴⁷ ≈ ±1.4 × 10¹⁴.

Sixteen rather than thirty-two fractional bits **because physics squares
things**. A squared value has to fit the type that stores it, so the range that
matters is not what is representable but what is *squarable* — the square root
of the representable range:

| | representable | squarable |
|---|---|---|
| Q47.16 | ±1.4 × 10¹⁴ | **±1.2 × 10⁷** |
| Q32.32 | ±2.1 × 10⁹ | **±4.6 × 10⁴** |

Two hundred and fifty-six times the working room, for a resolution already
finer than anything a game perceives.

## What the API refuses

**No `f32` or `f64` in any signature.** Converting to a float is a presentation
concern; it lives in the maths crate, which depends on this one. A simulation
cannot reach that crate, so it cannot perform the conversion — enforced by the
structure checker's float-closure rule rather than by anyone remembering.

Values are constructed from integers: `from_int`, `from_ratio` (how `9.81` is
written without a float ever existing — `from_ratio(981, 100)`), and
`from_bits` for serialisation. All three are `const fn`, because game constants
are written at compile time.

## Two behaviours worth knowing before you use it

**Multiplication rounds to nearest, ties away from zero — not by shifting.**
The obvious implementation, `(a as i128 * b as i128) >> 16`, uses an arithmetic
shift, which rounds toward negative infinity and is therefore *asymmetric under
negation*: `(-a) * b` and `-(a * b)` differ for some inputs. Deterministic, and
still wrong for physics, because a body moving left and the same body moving
right would then accumulate different error. There is a property test for this,
and it fails against the shift — verified by swapping one in.

**Overflow saturates, in every build profile, and is counted.** It never wraps
and never differs between debug and release: behaviour that differed by profile
would mean the release behaviour is the one no test ever exercises. Saturation
is silent by itself, so `saturations()` reports how many times it happened on
this thread, and a test asserts zero the way the frame loop's allocation gate
does. The counter is thread-local, which is both what the threading standard
already sanctions and the right shape — simulation is single-threaded, so
per-thread is per-simulation.

## Extension points

None. This is a value type; it has no trait to implement and no runtime
polymorphism. Growing it means adding operations here.
