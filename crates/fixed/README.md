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

## Vectors

`Vec2` and `Vec3` over `Fixed`, as two concrete types rather than one generic
over dimension — the same choice the physics contract makes for the same
reason: a dimension-generic vocabulary infects every signature with a bound,
and writing `dot` twice costs less than every caller reading one.

Two operations are worth knowing about because physics leans on them.

`slide_along` removes a displacement's component along a unit normal. That is
the whole of move-and-slide's inner step, named here so an implementation does
not spell it out at each call site and get the sign wrong at one of them.

`perpendicular` is a quarter turn, and it is **exact** — a swap and a negation,
no trigonometry, no rounding. General rotation is not available: it needs
trigonometric functions this type does not have, which is why the physics
contract defers rotated shapes.

`normalize` is fallible, because the zero vector is a value a simulation
legitimately produces and an assertion on a path that runs every frame is the
wrong shape. **The result is unit-length only to the type's resolution** — a
four parts in 65536 — so callers wanting exact equality should compare
squared lengths against a tolerance rather than expecting exactly one.

## Angles and rotation

`Angle` is a binary angle: **a full turn is 2³² units**, so an angle is a
`u32` and wrapping is exact integer overflow. That is the reason for the
representation rather than a convenience. In radians the modulus is 2π, which
is irrational and therefore not representable, so every wrap would lose
precision and two machines wrapping at different moments would drift apart.
Here, adding a full turn is the identity, exactly, forever — and `from_degrees`
lands on the cardinal angles exactly.

`sin` and `cos` read a 513-entry quarter-turn table with rounded linear
interpolation, the other three quadrants coming from symmetry. **Measured
worst error: 1.0322 units in the last place**, over a sweep of the whole
circle against a double-precision reference. That is the floor for a table of
any size — its entries are each rounded to half a unit, interpolating between
two inherits that, and the interpolation rounds once more. Sixteen times the
entries buys 0.03 of a unit, which is why the table is 2 KB rather than 32.

The table is generated, not hand-written, and a test checks every entry
against the reference. That test is the only place in this crate's world that
uses floating point, and deliberately: the shipped code has none, and the
proof that its table is right needs one.

`Vec2::rotate` follows. Note that `perpendicular` is *not* the same as
rotating by a quarter turn — it is exact, where rotation rounds — so the
quarter turn keeps its own operation.

## Extension points

None. This is a value type; it has no trait to implement and no runtime
polymorphism. Growing it means adding operations here.
