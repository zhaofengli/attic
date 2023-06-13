//! Hashing utilities.

#[cfg(test)]
mod tests;

use displaydoc::Display;
use serde::{de, ser, Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AtticResult;

/// A hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hash {
    /// An SHA-256 hash.
    Sha256([u8; 32]),
}

/// A hashing error.
#[derive(Debug, Display)]
pub enum Error {
    /// The string lacks a colon separator.
    NoColonSeparator,

    /// Hash algorithm {0} is not supported.
    UnsupportedHashAlgorithm(String),

    /// Invalid base16 hash: {0}
    InvalidBase16Hash(hex::FromHexError),

    /// Invalid base32 hash.
    InvalidBase32Hash,

    /// Invalid length for {typ} string: Must be either {base16_len} (hexadecimal) or {base32_len} (base32), got {actual}.
    InvalidHashStringLength {
        typ: &'static str,
        base16_len: usize,
        base32_len: usize,
        actual: usize,
    },
}

impl Hash {
    /// Convenience function to generate a SHA-256 hash from a slice.
    pub fn sha256_from_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self::Sha256(hasher.finalize().into())
    }

    /// Parses a typed representation of a hash.
    pub fn from_typed(s: &str) -> AtticResult<Self> {
        let colon = s.find(':').ok_or(Error::NoColonSeparator)?;

        let (typ, rest) = s.split_at(colon);
        let hash = &rest[1..];

        match typ {
            "sha256" => {
                let v = decode_hash(hash, "SHA-256", 32)?;
                Ok(Self::Sha256(v.try_into().unwrap()))
            }
            _ => Err(Error::UnsupportedHashAlgorithm(typ.to_owned()).into()),
        }
    }

    /// Returns the hash in Nix-specific Base32 format, with the hash type prepended.
    pub fn to_typed_base32(&self) -> String {
        format!("{}:{}", self.hash_type(), self.to_base32())
    }

    /// Returns the hash in hexadecimal format, with the hash type prepended.
    ///
    /// This is the canonical representation of hashes in the Attic database.
    pub fn to_typed_base16(&self) -> String {
        format!("{}:{}", self.hash_type(), hex::encode(self.data()))
    }

    fn data(&self) -> &[u8] {
        match self {
            Self::Sha256(d) => d,
        }
    }

    fn hash_type(&self) -> &'static str {
        match self {
            Self::Sha256(_) => "sha256",
        }
    }

    /// Returns the hash in Nix-specific Base32 format.
    fn to_base32(&self) -> String {
        nix_base32::to_nix_base32(self.data())
    }
}

impl<'de> Deserialize<'de> for Hash {
    /// Deserializes a typed hash string.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        use de::Error;

        String::deserialize(deserializer)
            .and_then(|s| Self::from_typed(&s).map_err(|e| Error::custom(e.to_string())))
    }
}

impl Serialize for Hash {
    /// Serializes a hash into a hexadecimal hash string.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        serializer.serialize_str(&self.to_typed_base16())
    }
}

/// Decodes a base16 or base32 encoded hash containing a specified number of bytes.
fn decode_hash<'s>(s: &'s str, typ: &'static str, expected_bytes: usize) -> AtticResult<Vec<u8>> {
    let base16_len = expected_bytes * 2;
    let base32_len = (expected_bytes * 8 - 1) / 5 + 1;

    let v = if s.len() == base16_len {
        hex::decode(s).map_err(Error::InvalidBase16Hash)?
    } else if s.len() == base32_len {
        nix_base32::from_nix_base32(s).ok_or(Error::InvalidBase32Hash)?
    } else {
        return Err(Error::InvalidHashStringLength {
            typ,
            base16_len,
            base32_len,
            actual: s.len(),
        }
        .into());
    };

    assert!(v.len() == expected_bytes);

    Ok(v)
}
