//! The SHA-256 digests that names, objects and proofs are keyed by.

use sha2::{Digest, Sha256};

/// The lowercase hex digit of each half byte.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// The lowercase hex SHA-256 of `bytes`, written into one string.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        hex.push(HEX[usize::from(b >> 4)] as char);
        hex.push(HEX[usize::from(b & 0x0f)] as char);
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The per-byte `format!` that `sha256_hex` replaced; the tests check it is
    /// the same output.
    fn sha256_hex_format(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha256_hex_matches_the_known_digests() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn sha256_hex_matches_the_format_implementation() {
        for len in 0..1000usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i.wrapping_mul(31).wrapping_add(len)) as u8).collect();
            assert_eq!(sha256_hex(&bytes), sha256_hex_format(&bytes), "{len} bytes");
        }
    }
}
