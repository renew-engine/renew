//! The content digest.
//!
//! FNV-1a, 64-bit. Chosen because it needs no dependency, is a dozen
//! lines, and is the same function three other places in this tree
//! already use — a duplication that is written down elsewhere rather
//! than pretended away.
//!
//! **It is not collision-resistant, and the distinction matters.** This
//! is content *addressing*: telling whether two blobs are the same one,
//! for change detection and deduplication. It is not integrity against
//! someone who gets to choose the bytes. A pack from an untrusted source
//! is validated structurally by the reader; its hashes say what the
//! writer believed, not what an adversary could not forge.

/// FNV-1a offset basis and prime, 64-bit.
const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// The 64-bit FNV-1a digest of a byte string.
#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Published FNV-1a-64 vectors. Frozen constants rather than a
    /// property, because a hash that changes silently changes every
    /// pack's bytes, and a reimplementation that is merely
    /// self-consistent would pass any property test written against it.
    #[test]
    fn it_matches_the_published_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    /// The empty digest is the basis, which is what makes a zero-length
    /// entry hash to something rather than to nothing.
    #[test]
    fn the_empty_digest_is_the_offset_basis() {
        assert_eq!(fnv1a64(b""), OFFSET_BASIS);
    }

    /// Order matters — the classic way a folded hash goes wrong is by
    /// accidentally becoming commutative.
    #[test]
    fn it_is_not_commutative() {
        assert_ne!(fnv1a64(b"ab"), fnv1a64(b"ba"));
    }

    /// A one-bit change moves the digest. Not a strength claim; a
    /// smoke test that the input reaches the output at all.
    #[test]
    fn one_flipped_bit_changes_the_digest() {
        assert_ne!(fnv1a64(&[0b0000_0001]), fnv1a64(&[0b0000_0011]));
    }
}
