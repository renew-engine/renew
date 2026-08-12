//! The sRGB transfer function, as two tables.
//!
//! # Why a colour needs converting at all
//!
//! An authored colour — a value picked in an image editor, or written as
//! a hex literal — is **sRGB-encoded**: the byte is roughly the square
//! root of the light it stands for, which is how eight bits are made to
//! cover a range the eye reads as even. Arithmetic on those bytes is
//! arithmetic on square roots, so averaging two of them does not average
//! the light, and blending half-transparent black over white lands
//! visibly far from grey.
//!
//! Everything that computes with colour therefore wants **linear** values,
//! and everything that stores or displays it wants encoded ones. This
//! module is the boundary between the two, in the direction the CPU needs:
//! authored bytes in, linear floats out.
//!
//! # The other direction is the hardware's job
//!
//! Encoding back happens on write, in the attachment, because that is the
//! only place it can happen *after* blending. A shader that encoded its
//! own output would fix its own shading and leave every blend and every
//! interpolation still averaging square roots — which is the whole defect
//! this exists to remove.
//!
//! [`encode_u8`] is therefore not part of that path. It is here so a test
//! can state what the hardware should have produced, and a tool can
//! report it; nothing in a frame calls it.
//!
//! # Contract
//!
//! - **The round trip is exact.** `encode_u8(decode(k)) == k` for all 256
//!   bytes. An authored colour decoded here and encoded by the attachment
//!   lands on the byte it started from, so constants keep their meaning
//!   and only the values that genuinely blend or interpolate change.
//! - **No arithmetic at runtime.** Decoding is one table index; encoding
//!   is a binary search over precomputed thresholds. There is no `powf`
//!   here and no floating-point work whose result could differ between
//!   targets.
//! - **Both directions are monotone**, and 0 and 255 are fixed points.
//!
//! # What this is not
//!
//! It is not a colour-management system. There is one transfer function
//! here, the one IEC 61966-2-1 defines, and no notion of a colour space
//! beyond it — no primaries, no white point, no profiles.

#[expect(
    clippy::unreadable_literal,
    reason = "a generated table of the transfer function; separators would obscure a column of values meant to be scanned rather than read"
)]
const DECODE: [f32; 256] = [
    0.0,
    0.000303527,
    0.000607054,
    0.000910581,
    0.001214108,
    0.001517635,
    0.001821162,
    0.0021246888,
    0.002428216,
    0.0027317428,
    0.00303527,
    0.0033465358,
    0.0036765074,
    0.004024717,
    0.004391442,
    0.0047769533,
    0.0051815165,
    0.0056053917,
    0.006048833,
    0.0065120906,
    0.00699541,
    0.007499032,
    0.008023193,
    0.008568126,
    0.009134059,
    0.009721218,
    0.010329823,
    0.010960094,
    0.011612245,
    0.012286488,
    0.0129830325,
    0.013702083,
    0.014443844,
    0.015208514,
    0.015996294,
    0.016807375,
    0.017641954,
    0.01850022,
    0.019382361,
    0.020288562,
    0.02121901,
    0.022173885,
    0.023153367,
    0.024157632,
    0.02518686,
    0.026241222,
    0.027320892,
    0.02842604,
    0.029556835,
    0.030713445,
    0.031896032,
    0.033104766,
    0.034339808,
    0.035601314,
    0.03688945,
    0.038204372,
    0.039546236,
    0.0409152,
    0.04231141,
    0.04373503,
    0.045186203,
    0.046665087,
    0.048171826,
    0.049706567,
    0.051269457,
    0.052860647,
    0.054480277,
    0.05612849,
    0.05780543,
    0.059511237,
    0.061246052,
    0.063010015,
    0.064803265,
    0.06662594,
    0.06847817,
    0.070360094,
    0.07227185,
    0.07421357,
    0.07618538,
    0.07818742,
    0.08021982,
    0.08228271,
    0.08437621,
    0.08650046,
    0.08865558,
    0.09084171,
    0.093058966,
    0.09530747,
    0.09758735,
    0.099898726,
    0.10224173,
    0.104616486,
    0.107023105,
    0.10946171,
    0.11193243,
    0.114435375,
    0.116970666,
    0.11953843,
    0.122138776,
    0.12477182,
    0.12743768,
    0.13013647,
    0.13286832,
    0.13563333,
    0.13843161,
    0.14126329,
    0.14412847,
    0.14702727,
    0.14995979,
    0.15292615,
    0.15592647,
    0.15896083,
    0.16202937,
    0.1651322,
    0.1682694,
    0.17144111,
    0.1746474,
    0.17788842,
    0.18116425,
    0.18447499,
    0.18782078,
    0.19120169,
    0.19461784,
    0.19806932,
    0.20155625,
    0.20507874,
    0.20863687,
    0.21223076,
    0.2158605,
    0.2195262,
    0.22322796,
    0.22696587,
    0.23074006,
    0.23455058,
    0.23839757,
    0.24228112,
    0.24620132,
    0.25015828,
    0.2541521,
    0.25818285,
    0.26225066,
    0.2663556,
    0.2704978,
    0.2746773,
    0.27889428,
    0.28314874,
    0.28744084,
    0.29177064,
    0.29613826,
    0.30054379,
    0.3049873,
    0.30946892,
    0.31398872,
    0.31854677,
    0.3231432,
    0.3277781,
    0.33245152,
    0.33716363,
    0.34191442,
    0.34670407,
    0.3515326,
    0.35640013,
    0.3613068,
    0.3662526,
    0.3712377,
    0.37626213,
    0.38132602,
    0.38642943,
    0.39157248,
    0.39675522,
    0.40197778,
    0.4072402,
    0.4125426,
    0.41788507,
    0.42326766,
    0.4286905,
    0.43415365,
    0.43965718,
    0.4452012,
    0.4507858,
    0.45641103,
    0.462077,
    0.4677838,
    0.47353148,
    0.47932017,
    0.48514995,
    0.49102086,
    0.49693298,
    0.5028865,
    0.50888133,
    0.5149177,
    0.52099556,
    0.5271151,
    0.5332764,
    0.5394795,
    0.54572445,
    0.55201143,
    0.5583404,
    0.5647115,
    0.57112485,
    0.57758045,
    0.58407843,
    0.59061885,
    0.59720176,
    0.60382736,
    0.61049557,
    0.6172066,
    0.6239604,
    0.63075715,
    0.63759685,
    0.6444797,
    0.65140563,
    0.65837485,
    0.6653873,
    0.67244315,
    0.6795425,
    0.6866853,
    0.69387174,
    0.7011019,
    0.70837575,
    0.7156935,
    0.7230551,
    0.73046076,
    0.7379104,
    0.7454042,
    0.7529422,
    0.7605245,
    0.76815116,
    0.7758222,
    0.7835378,
    0.7912979,
    0.7991027,
    0.80695224,
    0.8148466,
    0.82278574,
    0.8307699,
    0.838799,
    0.8468732,
    0.8549926,
    0.8631572,
    0.8713671,
    0.8796224,
    0.8879231,
    0.8962694,
    0.9046612,
    0.91309863,
    0.92158186,
    0.9301109,
    0.9386857,
    0.9473065,
    0.9559733,
    0.9646863,
    0.9734453,
    0.9822506,
    0.9911021,
    1.0,
];

#[expect(
    clippy::unreadable_literal,
    reason = "a generated table of the transfer function; separators would obscure a column of values meant to be scanned rather than read"
)]
const THRESHOLD: [f32; 255] = [
    0.0001517635,
    0.0004552905,
    0.0007588175,
    0.0010623444,
    0.0013658714,
    0.0016693984,
    0.0019729254,
    0.0022764525,
    0.0025799794,
    0.0028835062,
    0.0031883009,
    0.0035092593,
    0.003848315,
    0.004205748,
    0.004581833,
    0.0049768374,
    0.005391024,
    0.0058246506,
    0.0062779696,
    0.0067512277,
    0.0072446684,
    0.0077585303,
    0.0082930485,
    0.008848453,
    0.0094249705,
    0.010022826,
    0.010642237,
    0.011283421,
    0.0119465925,
    0.01263196,
    0.013339732,
    0.014070112,
    0.014823303,
    0.015599503,
    0.01639891,
    0.017221715,
    0.018068114,
    0.018938294,
    0.019832443,
    0.020750744,
    0.021693382,
    0.022660539,
    0.02365239,
    0.024669115,
    0.025710888,
    0.026777882,
    0.02787027,
    0.02898822,
    0.030131903,
    0.03130148,
    0.032497123,
    0.03371899,
    0.034967244,
    0.036242045,
    0.037543554,
    0.038871925,
    0.04022732,
    0.041609887,
    0.043019786,
    0.044457164,
    0.04592217,
    0.047414962,
    0.048935685,
    0.050484486,
    0.052061506,
    0.053666897,
    0.055300802,
    0.05696336,
    0.058654718,
    0.060375012,
    0.062124383,
    0.063902974,
    0.06571092,
    0.06754835,
    0.06941541,
    0.071312234,
    0.073238954,
    0.07519571,
    0.07718261,
    0.07919982,
    0.08124744,
    0.083325624,
    0.08543449,
    0.087574154,
    0.08974477,
    0.09194644,
    0.0941793,
    0.096443474,
    0.098739095,
    0.10106627,
    0.10342513,
    0.105815805,
    0.1082384,
    0.110693045,
    0.11317986,
    0.11569897,
    0.11825048,
    0.12083452,
    0.1234512,
    0.12610064,
    0.12878296,
    0.13149826,
    0.13424668,
    0.1370283,
    0.13984327,
    0.14269169,
    0.14557366,
    0.14848931,
    0.15143873,
    0.15442206,
    0.15743938,
    0.16049083,
    0.1635765,
    0.16669649,
    0.16985093,
    0.17303991,
    0.17626357,
    0.17952198,
    0.18281525,
    0.1861435,
    0.18950683,
    0.19290535,
    0.19633915,
    0.19980834,
    0.20331304,
    0.20685335,
    0.21042934,
    0.21404114,
    0.21768884,
    0.22137256,
    0.2250924,
    0.22884843,
    0.23264076,
    0.2364695,
    0.24033478,
    0.24423663,
    0.2481752,
    0.25215057,
    0.25616285,
    0.26021212,
    0.26429847,
    0.26842204,
    0.2725829,
    0.2767811,
    0.2810168,
    0.2852901,
    0.28960103,
    0.29394972,
    0.2983363,
    0.3027608,
    0.30722335,
    0.31172404,
    0.31626296,
    0.32084018,
    0.32545584,
    0.33010998,
    0.33480275,
    0.33953416,
    0.34430438,
    0.34911346,
    0.3539615,
    0.35884857,
    0.36377478,
    0.36874023,
    0.37374496,
    0.37878913,
    0.38387278,
    0.388996,
    0.3941589,
    0.39936152,
    0.40460402,
    0.40988642,
    0.41520882,
    0.42057136,
    0.42597404,
    0.43141702,
    0.43690035,
    0.44242412,
    0.44798842,
    0.4535933,
    0.45923892,
    0.4649253,
    0.47065252,
    0.4764207,
    0.48222992,
    0.48808023,
    0.49397177,
    0.49990454,
    0.5058787,
    0.5118943,
    0.5179514,
    0.5240501,
    0.5301905,
    0.5363727,
    0.54259676,
    0.5488627,
    0.55517066,
    0.5615207,
    0.5679129,
    0.5743473,
    0.58082414,
    0.58734334,
    0.593905,
    0.6005092,
    0.6071561,
    0.6138457,
    0.6205781,
    0.62735337,
    0.6341716,
    0.6410329,
    0.64793724,
    0.6548848,
    0.66187567,
    0.6689098,
    0.67598736,
    0.68310845,
    0.6902731,
    0.69748133,
    0.7047334,
    0.71202916,
    0.7193688,
    0.72675246,
    0.73418003,
    0.7416518,
    0.7491677,
    0.7567278,
    0.7643323,
    0.7719811,
    0.7796744,
    0.7874123,
    0.79519475,
    0.8030219,
    0.81089383,
    0.8188105,
    0.8267722,
    0.8347788,
    0.8428305,
    0.8509273,
    0.8590692,
    0.8672565,
    0.87548906,
    0.88376707,
    0.89209056,
    0.9004596,
    0.9088742,
    0.91733456,
    0.9258406,
    0.9343926,
    0.94299036,
    0.95163417,
    0.96032405,
    0.96906,
    0.97784215,
    0.98667055,
    0.99554527,
];

/// The linear value of an sRGB-encoded byte.
///
/// One table index. See the module contract for why this direction is the
/// only one a frame performs.
#[must_use]
pub const fn decode(encoded: u8) -> f32 {
    DECODE[encoded as usize]
}

/// The sRGB-encoded byte nearest a linear value, saturating at both ends.
///
/// **Not part of the render path** — the attachment encodes, after
/// blending, which is the only place the result is right. This exists so
/// a test can say what the hardware should have written.
///
/// A binary search over the rounding thresholds, so the answer is exactly
/// `round(255 · encode(linear))` by construction rather than by a
/// floating-point coincidence.
#[must_use]
pub fn encode_u8(linear: f32) -> u8 {
    // NaN compares false against everything, so it is named rather than
    // left to fall through a comparison: a NaN reaching here is a defect
    // upstream, and a dark surprise pixel is a better way to learn about
    // it than a bright one.
    if linear.is_nan() || linear <= 0.0 {
        return 0;
    }
    // `partition_point` counts the thresholds this value has passed,
    // which is the encoded byte: with none passed it is 0, with all 255
    // passed it is 255.
    let index = THRESHOLD.partition_point(|&threshold| threshold <= linear);
    // The count cannot exceed the table length, which is 255.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "partition_point over a 255-entry table returns 0..=255"
    )]
    let byte = index as u8;
    byte
}

#[cfg(test)]
mod tests {
    use super::{decode, encode_u8};

    /// **The claim every authored constant rests on.** A colour decoded
    /// here and re-encoded by the attachment must land on the byte it
    /// started from, or every clear value and every tint in the tree
    /// shifts the day the attachments change.
    #[test]
    fn every_byte_survives_the_round_trip() {
        for encoded in 0..=u8::MAX {
            assert_eq!(
                encode_u8(decode(encoded)),
                encoded,
                "byte {encoded} did not survive decode then encode"
            );
        }
    }

    #[test]
    fn the_endpoints_are_exact() {
        assert_eq!(decode(0).to_bits(), 0.0f32.to_bits());
        assert_eq!(decode(255).to_bits(), 1.0f32.to_bits());
        assert_eq!(encode_u8(0.0), 0);
        assert_eq!(encode_u8(1.0), 255);
    }

    #[test]
    fn decoding_is_strictly_increasing() {
        for encoded in 1..=u8::MAX {
            assert!(
                decode(encoded) > decode(encoded - 1),
                "decode is not strictly increasing at {encoded}"
            );
        }
    }

    #[test]
    fn encoding_is_monotone_across_the_whole_range() {
        let mut previous = 0u8;
        for step in 0..=2048u32 {
            let linear = f32::from(u16::try_from(step).expect("in range")) / 2048.0;
            let encoded = encode_u8(linear);
            assert!(
                encoded >= previous,
                "encode went backwards at {linear}: {encoded} after {previous}"
            );
            previous = encoded;
        }
    }

    /// Out-of-range and non-finite inputs saturate rather than wrapping
    /// or panicking: a blend can overshoot, and a NaN is an upstream
    /// defect this must not amplify into a bright pixel.
    #[test]
    fn values_outside_the_range_saturate() {
        assert_eq!(encode_u8(-1.0), 0);
        assert_eq!(encode_u8(2.0), 255);
        assert_eq!(encode_u8(f32::INFINITY), 255);
        assert_eq!(encode_u8(f32::NEG_INFINITY), 0);
        assert_eq!(encode_u8(f32::NAN), 0);
        assert_eq!(encode_u8(-0.0), 0);
    }

    /// The curve is piecewise: a short linear segment near black, then a
    /// power curve. The join is at encoded 0.04045, which falls between
    /// bytes 10 and 11, and both sides must still round-trip — this is
    /// where an implementation that used one branch for everything would
    /// come apart.
    #[test]
    fn the_piecewise_join_round_trips_from_both_sides() {
        for encoded in [9u8, 10, 11, 12] {
            assert_eq!(encode_u8(decode(encoded)), encoded);
        }
        // Below the join the curve is a straight line through the
        // origin, so doubling the byte doubles the light.
        let one = f64::from(decode(1));
        let two = f64::from(decode(2));
        assert!(
            (two - 2.0 * one).abs() < 1e-9,
            "the segment below the join is not linear: {one} and {two}"
        );
    }

    /// The encode is not the decode: a mid-grey byte stands for far less
    /// than half the light. Without this, a table of all zeros or an
    /// identity would pass every other test here.
    #[test]
    fn the_curve_is_not_the_identity() {
        let mid = decode(128);
        assert!(
            (0.21..0.22).contains(&mid),
            "byte 128 should be about 21.6% of the light, got {mid}"
        );
    }
}
