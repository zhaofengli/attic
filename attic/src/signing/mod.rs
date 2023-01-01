//! Object Signing and Verification.
//!
//! Nix utilitizes Ed25519 to generate signatures on NAR hashes. Currently
//! we can either generate signatures on the fly per request, or cache them
//! in the data store.
//!
//! ## String format
//!
//! All signing-related strings in Nix follow the same format (henceforth
//! "the canonical format"):
//!
//! ```text
//! {keyName}:{base64Payload}
//! ```
//!
//! We follow the same format, so keys generated using the Nix CLI will
//! simply work.
//!
//! ## Serde
//!
//! `Serialize` and `Deserialize` are implemented to convert the structs
//! from and to the canonical format.

use std::convert::TryInto;

use serde::{de, ser, Deserialize, Serialize};

use base64::DecodeError;
use displaydoc::Display;
use ed25519_compact::{Error as SignatureError, KeyPair, PublicKey, Signature};

use crate::error::AtticResult;

#[cfg(test)]
mod tests;

/// An ed25519 keypair for signing.
#[derive(Debug)]
pub struct NixKeypair {
    /// Name of this key.
    name: String,

    /// The keypair.
    keypair: KeyPair,
}

/// An ed25519 public key for verification.
#[derive(Debug, Clone)]
pub struct NixPublicKey {
    /// Name of this key.
    name: String,

    /// The public key.
    public: PublicKey,
}

/// A signing error.
#[derive(Debug, Display)]
#[ignore_extra_doc_attributes]
pub enum Error {
    /// Signature error: {0}
    SignatureError(SignatureError),

    /// The string has a wrong key name attached to it: Our name is "{our_name}" and the string has "{string_name}"
    WrongKeyName {
        our_name: String,
        string_name: String,
    },

    /// The string lacks a colon separator.
    NoColonSeparator,

    /// The name portion of the string is blank.
    BlankKeyName,

    /// The payload portion of the string is blank.
    BlankPayload,

    /// Base64 decode error: {0}
    Base64DecodeError(DecodeError),

    /// Invalid base64 payload length: Expected {expected} ({usage}), got {actual}
    InvalidPayloadLength {
        expected: usize,
        actual: usize,
        usage: &'static str,
    },

    /// Invalid signing key name "{0}".
    ///
    /// A valid name cannot be empty and must be contain colons (:).
    InvalidSigningKeyName(String),
}

impl NixKeypair {
    /// Generates a new keypair.
    pub fn generate(name: &str) -> AtticResult<Self> {
        // TODO: Make this configurable?
        let keypair = KeyPair::generate();

        validate_name(name)?;

        Ok(Self {
            name: name.to_string(),
            keypair,
        })
    }

    /// Imports an existing keypair from its canonical representation.
    pub fn from_str(keypair: &str) -> AtticResult<Self> {
        let (name, bytes) = decode_string(keypair, "keypair", KeyPair::BYTES, None)?;

        let keypair = KeyPair::from_slice(&bytes).map_err(Error::SignatureError)?;

        Ok(Self {
            name: name.to_string(),
            keypair,
        })
    }

    /// Returns the canonical representation of the keypair.
    ///
    /// This results in a 64-byte base64 payload that contains both the private
    /// key and the public key, in that order.
    ///
    /// For example, it can look like:
    ///     attic-test:msdoldbtlongtt0/xkzmcbqihd7yvy8iomajqhnkutsl3b1pyyyc0mgg2rs0ttzzuyuk9rb2zphvtpes71mlha==
    pub fn export_keypair(&self) -> String {
        format!("{}:{}", self.name, base64::encode(*self.keypair))
    }

    /// Returns the canonical representation of the public key.
    ///
    /// For example, it can look like:
    ///     attic-test:C929acssgtJoINkUtLbc81GFJPUW9maR77TxEu9ZpRw=
    pub fn export_public_key(&self) -> String {
        format!("{}:{}", self.name, base64::encode(*self.keypair.pk))
    }

    /// Returns the public key portion of the keypair.
    pub fn to_public_key(&self) -> NixPublicKey {
        NixPublicKey {
            name: self.name.clone(),
            public: self.keypair.pk,
        }
    }

    /// Signs a message, returning its canonical representation.
    pub fn sign(&self, message: &[u8]) -> String {
        let bytes = self.keypair.sk.sign(message, None);
        format!("{}:{}", self.name, base64::encode(bytes))
    }

    /// Verifies a message.
    pub fn verify(&self, message: &[u8], signature: &str) -> AtticResult<()> {
        let (_, bytes) = decode_string(signature, "signature", Signature::BYTES, Some(&self.name))?;

        let bytes: [u8; Signature::BYTES] = bytes.try_into().unwrap();
        let signature = Signature::from_slice(&bytes).map_err(Error::SignatureError)?;

        self.keypair
            .pk
            .verify(message, &signature)
            .map_err(|e| Error::SignatureError(e).into())
    }
}

impl<'de> Deserialize<'de> for NixKeypair {
    /// Deserializes a potentially-invalid Nix keypair from its canonical representation.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        use de::Error;
        String::deserialize(deserializer)
            .and_then(|s| Self::from_str(&s).map_err(|e| Error::custom(e.to_string())))
    }
}

impl Serialize for NixKeypair {
    /// Serializes a Nix keypair to its canonical representation.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        serializer.serialize_str(&self.export_keypair())
    }
}

impl NixPublicKey {
    /// Imports an existing public key from its canonical representation.
    pub fn from_str(public_key: &str) -> AtticResult<Self> {
        let (name, bytes) = decode_string(public_key, "public key", PublicKey::BYTES, None)?;

        let public = PublicKey::from_slice(&bytes).map_err(Error::SignatureError)?;

        Ok(Self {
            name: name.to_string(),
            public,
        })
    }

    /// Returns the Nix-compatible textual representation of the public key.
    ///
    /// For example, it can look like:
    ///     attic-test:C929acssgtJoINkUtLbc81GFJPUW9maR77TxEu9ZpRw=
    pub fn export(&self) -> String {
        format!("{}:{}", self.name, base64::encode(*self.public))
    }

    /// Verifies a message.
    pub fn verify(&self, message: &[u8], signature: &str) -> AtticResult<()> {
        let (_, bytes) = decode_string(signature, "signature", Signature::BYTES, Some(&self.name))?;

        let bytes: [u8; Signature::BYTES] = bytes.try_into().unwrap();
        let signature = Signature::from_slice(&bytes).map_err(Error::SignatureError)?;

        self.public
            .verify(message, &signature)
            .map_err(|e| Error::SignatureError(e).into())
    }
}

/// Validates the name/label of a signing key.
///
/// A valid name cannot be empty and must not contain colons (:).
fn validate_name(name: &str) -> AtticResult<()> {
    if name.is_empty() || name.find(':').is_some() {
        Err(Error::InvalidSigningKeyName(name.to_string()).into())
    } else {
        Ok(())
    }
}

/// Decodes a colon-delimited string containing a key name and a base64 payload.
fn decode_string<'s>(
    s: &'s str,
    usage: &'static str,
    expected_payload_length: usize,
    expected_name: Option<&str>,
) -> AtticResult<(&'s str, Vec<u8>)> {
    let colon = s.find(':').ok_or(Error::NoColonSeparator)?;

    let (name, colon_and_payload) = s.split_at(colon);

    validate_name(name)?;

    // don't bother decoding base64 if the name doesn't match
    if let Some(expected_name) = expected_name {
        if expected_name != name {
            return Err(Error::WrongKeyName {
                our_name: expected_name.to_string(),
                string_name: name.to_string(),
            }
            .into());
        }
    }

    let bytes = base64::decode(&colon_and_payload[1..]).map_err(Error::Base64DecodeError)?;

    if bytes.len() != expected_payload_length {
        return Err(Error::InvalidPayloadLength {
            actual: bytes.len(),
            expected: expected_payload_length,
            usage,
        }
        .into());
    }

    Ok((name, bytes))
}
