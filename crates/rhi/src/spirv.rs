//! Structural SPIR-V checks: cheap plausibility gates on shader bytes
//! before they reach the driver. Not a validator — the wire-format
//! sanity a doorway owes itself even for first-party bytes, and the
//! seam where real validation attaches when shader bytes ever arrive
//! from outside the build.

#![deny(unsafe_code)]

use crate::error::PipelineError;

const SPIRV_MAGIC: u32 = 0x0723_0203;

/// Check shape and copy into the aligned word buffer the driver wants.
///
/// # Errors
///
/// [`PipelineError::InvalidSpirv`] naming the failed check.
pub fn words_from_bytes(stage: &'static str, bytes: &[u8]) -> Result<Vec<u32>, PipelineError> {
    if bytes.is_empty() {
        return Err(PipelineError::InvalidSpirv {
            stage,
            reason: "empty",
        });
    }
    if !bytes.len().is_multiple_of(4) {
        return Err(PipelineError::InvalidSpirv {
            stage,
            reason: "length not a multiple of four",
        });
    }
    // `as_chunks` rather than `chunks_exact(4)`: the length is already
    // known to be a multiple of four, so the remainder is empty by the
    // check above, and a fixed-size chunk needs no indexing to unpack.
    let (quads, _) = bytes.as_chunks::<4>();
    let words: Vec<u32> = quads.iter().copied().map(u32::from_le_bytes).collect();
    if words[0] != SPIRV_MAGIC {
        return Err(PipelineError::InvalidSpirv {
            stage,
            reason: "bad magic number",
        });
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_header() -> Vec<u8> {
        let mut bytes = SPIRV_MAGIC.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0; 16]);
        bytes
    }

    #[test]
    fn plausible_bytes_pass_and_round_trip_words() {
        let words = words_from_bytes("vertex", &valid_header()).expect("plausible");
        assert_eq!(words[0], SPIRV_MAGIC);
        assert_eq!(words.len(), 5);
    }

    #[test]
    fn each_structural_check_rejects_with_its_own_reason() {
        // One table asserted through `matches!` rather than three
        // `match`es with a `panic!` fallback: a fallback arm is dead on
        // every passing run, and the pattern still pins the variant —
        // the stage now too, which the fallback form dropped.
        let cases: [(&'static str, &[u8], &'static str); 3] = [
            ("vertex", &[], "empty"),
            ("fragment", &[1, 2, 3], "length not a multiple of four"),
            ("vertex", &[0xEF, 0xBE, 0xAD, 0xDE], "bad magic number"),
        ];
        for (stage, bytes, expected) in cases {
            let rejection = words_from_bytes(stage, bytes).expect_err("must be rejected");
            assert!(
                matches!(
                    &rejection,
                    PipelineError::InvalidSpirv { stage: got, reason }
                        if *got == stage && *reason == expected
                ),
                "expected `{expected}` rejection of {stage} bytes, got {rejection:?}"
            );
        }
    }
}
