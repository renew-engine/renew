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
    let mut words = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        words.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
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
        match words_from_bytes("vertex", &[]) {
            Err(PipelineError::InvalidSpirv { reason, .. }) => assert_eq!(reason, "empty"),
            other => panic!("expected empty rejection, got {other:?}"),
        }
        match words_from_bytes("fragment", &[1, 2, 3]) {
            Err(PipelineError::InvalidSpirv { reason, .. }) => {
                assert_eq!(reason, "length not a multiple of four");
            }
            other => panic!("expected length rejection, got {other:?}"),
        }
        match words_from_bytes("vertex", &[0xEF, 0xBE, 0xAD, 0xDE]) {
            Err(PipelineError::InvalidSpirv { reason, .. }) => {
                assert_eq!(reason, "bad magic number");
            }
            other => panic!("expected magic rejection, got {other:?}"),
        }
    }
}
