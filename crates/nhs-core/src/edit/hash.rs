//! Content hashing for the write guard.
//!
//! The hash is the only thing that stands between a reviewed plan and a file
//! that somebody else changed while the user was reading the diff, so it is a
//! dedicated type rather than a bare `String`.

use std::fmt;
use std::io;

use sha2::{Digest, Sha256 as DigestSha256};

/// SHA-256 of a file's exact bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256([u8; 32]);

impl Sha256 {
    /// Hash raw bytes.
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let mut hasher = DigestSha256::new();
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    /// Read `path` and hash its bytes.
    pub fn of_file(fs: &dyn crate::fs::FsView, path: &std::path::Path) -> io::Result<Self> {
        let bytes = fs.read_bytes(path)?;
        Ok(Self::of_bytes(&bytes))
    }

    /// Parse a 64-character hex digest.
    #[must_use]
    pub fn parse_hex(text: &str) -> Option<Self> {
        if text.len() != 64 {
            return None;
        }
        let mut out = [0_u8; 32];
        let (pairs, _) = text.as_bytes().as_chunks::<2>();
        for (slot, [high, low]) in out.iter_mut().zip(pairs) {
            *slot = (hex_value(*high)? << 4) | hex_value(*low)?;
        }
        Some(Self(out))
    }

    /// Lowercase hex digest, the form persisted in snapshots.
    #[must_use]
    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0f));
        }
        out
    }
}

impl fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.to_hex())
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        _ => char::from(b'a' + (value - 10)),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used)]

    use super::Sha256;

    #[test]
    fn hex_round_trips() {
        let hash = Sha256::of_bytes(b"nix-home-studio");
        let Some(parsed) = Sha256::parse_hex(&hash.to_hex()) else {
            panic!("valid hex must parse");
        };
        assert_eq!(hash, parsed);
    }

    #[test]
    fn malformed_hex_is_rejected() {
        assert!(Sha256::parse_hex("").is_none());
        assert!(Sha256::parse_hex("zz").is_none());
        assert!(Sha256::parse_hex(&"a".repeat(63)).is_none());
    }

    #[test]
    fn distinct_input_hashes_differently() {
        assert_ne!(Sha256::of_bytes(b"a"), Sha256::of_bytes(b"b"));
    }
}
