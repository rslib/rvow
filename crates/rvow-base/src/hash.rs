//! The SHA-256 digests that names, objects and proofs are keyed by.

use sha2::{Digest, Sha256};

/// The lowercase hex digit of each half byte.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// The lowercase hex SHA-256 of `bytes`, written into one string.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_of(&Sha256::digest(bytes))
}

/// The bytes `sha256_file_hex` reads at a time, so a large file costs no more memory.
const FILE_CHUNK: usize = 64 * 1024;

/// `sha256_hex` of the whole file at `path`, read in fixed chunks.
pub fn sha256_file_hex(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; FILE_CHUNK];
    let mut sha = Sha256::new();
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => sha.update(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(hex_of(&sha.finalize()))
}

fn hex_of(digest: &[u8]) -> String {
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
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
    fn sha256_file_hex_matches_the_whole_text() {
        let path = std::env::temp_dir().join(format!("rvow-file-hash-{}", std::process::id()));
        for len in [0, 1, FILE_CHUNK - 1, FILE_CHUNK, FILE_CHUNK + 1, 3 * FILE_CHUNK + 5] {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 7 % 253) as u8).collect();
            std::fs::write(&path, &bytes).unwrap();
            assert_eq!(sha256_file_hex(&path).unwrap(), sha256_hex(&bytes), "{len} bytes");
        }
        let _ = std::fs::remove_file(&path);
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
