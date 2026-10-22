//! Shared hashing, MAC, and random byte helpers.

use std::{fmt, ops::Deref};

use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::{Digest as _, Sha256 as Sha2_256};

/// SHA-256 digest bytes.
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    /// Returns the digest bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the digest byte length.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns whether the digest is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }
}

impl AsRef<[u8]> for Sha256Digest {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl Deref for Sha256Digest {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}

impl IntoIterator for Sha256Digest {
    type IntoIter = std::array::IntoIter<u8, 32>;
    type Item = u8;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl fmt::LowerHex for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.as_bytes() {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Incremental SHA-256 hasher.
pub struct Sha256 {
    hasher: Sha2_256,
}

impl Sha256 {
    /// Creates a SHA-256 hasher.
    #[must_use]
    pub fn new() -> Self {
        Self {
            hasher: Sha2_256::new(),
        }
    }

    /// Returns the digest bytes for a single byte slice.
    #[must_use]
    pub fn digest(bytes: impl AsRef<[u8]>) -> Sha256Digest {
        Sha256Digest(Sha2_256::digest(bytes.as_ref()).into())
    }

    /// Adds bytes to the digest input.
    pub fn update(&mut self, bytes: impl AsRef<[u8]>) {
        self.hasher.update(bytes.as_ref());
    }

    /// Finalizes the digest bytes.
    #[must_use]
    pub fn finalize(self) -> Sha256Digest {
        Sha256Digest(self.hasher.finalize().into())
    }
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

/// Incremental SHA-256 hasher with hex finalizers.
pub struct Sha256Hex {
    hasher: Sha2_256,
}

impl Sha256Hex {
    /// Creates a SHA-256 hasher.
    #[must_use]
    pub fn new() -> Self {
        Self {
            hasher: Sha2_256::new(),
        }
    }

    /// Adds bytes to the digest input.
    pub fn update(&mut self, bytes: impl AsRef<[u8]>) {
        self.hasher.update(bytes.as_ref());
    }

    /// Finalizes the digest as lowercase hexadecimal.
    #[must_use]
    pub fn finalize_hex(self) -> String {
        hex::encode(self.hasher.finalize())
    }

    /// Finalizes the digest bytes.
    #[must_use]
    pub fn finalize(self) -> Sha256Digest {
        Sha256Digest(self.hasher.finalize().into())
    }

    /// Finalizes the digest as `sha256:<hex>`.
    #[must_use]
    pub fn finalize_tagged_hex(self) -> String {
        format!("sha256:{}", self.finalize_hex())
    }
}

impl Default for Sha256Hex {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns the SHA-256 digest as lowercase hexadecimal.
#[must_use]
pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha2_256::digest(bytes.as_ref()))
}

/// Returns the leading characters of the SHA-256 digest as lowercase hexadecimal.
#[must_use]
pub fn sha256_hex_prefix(bytes: impl AsRef<[u8]>, len: usize) -> String {
    let digest = sha256_hex(bytes);
    digest[..len.min(digest.len())].to_owned()
}

/// Returns the SHA-256 digest bytes.
#[must_use]
pub fn sha256_bytes(bytes: impl AsRef<[u8]>) -> Sha256Digest {
    Sha256::digest(bytes)
}

/// Returns the SHA-256 digest as `sha256:<hex>`.
#[must_use]
pub fn sha256_tagged_hex(bytes: impl AsRef<[u8]>) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

/// Returns `<prefix>:sha256:<hex>` for a stable textual reference.
#[must_use]
pub fn sha256_tagged_ref(prefix: &str, bytes: impl AsRef<[u8]>) -> String {
    format!("{prefix}:{}", sha256_tagged_hex(bytes))
}

/// Returns arbitrary bytes as lowercase hexadecimal.
#[must_use]
pub fn hex_bytes(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(bytes)
}

/// Decodes lowercase or uppercase hexadecimal bytes with an exact output length.
#[must_use]
pub fn fixed_hex_bytes(value: &str, expected_len: usize) -> Option<Vec<u8>> {
    let value = value.trim();
    if value.len() != expected_len * 2 || !value.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    hex::decode(value)
        .ok()
        .filter(|bytes| bytes.len() == expected_len)
}

/// Returns HMAC-SHA256 as lowercase hexadecimal.
pub fn hmac_sha256_hex(
    key: impl AsRef<[u8]>,
    payload: impl AsRef<[u8]>,
) -> Result<String, hmac::digest::InvalidLength> {
    let mut mac = Hmac::<Sha2_256>::new_from_slice(key.as_ref())?;
    mac.update(payload.as_ref());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Fills a fixed-size random byte array from the operating system RNG.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], getrandom::Error> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes)?;
    Ok(bytes)
}
