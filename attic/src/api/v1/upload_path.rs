use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DefaultOnError};

use crate::cache::CacheName;
use crate::hash::Hash;
use crate::nix_store::StorePathHash;

/// NAR information associated with a upload.
///
/// This is JSON-serialized as the value of the `X-Attic-Nar-Info` header.
/// The (client-compressed) NAR is the PUT body.
///
/// Regardless of client compression, the server will always decompress
/// the NAR to validate the NAR hash before applying the server-configured
/// compression again.
#[derive(Debug, Serialize, Deserialize)]
pub struct UploadPathNarInfo {
    /// The name of the binary cache to upload to.
    pub cache: CacheName,

    /// The hash portion of the store path.
    pub store_path_hash: StorePathHash,

    /// The full store path being cached, including the store directory.
    pub store_path: String,

    /// Other store paths this object directly refereces.
    pub references: Vec<String>,

    /// The system this derivation is built for.
    pub system: Option<String>,

    /// The derivation that produced this object.
    pub deriver: Option<String>,

    /// The signatures of this object.
    pub sigs: Vec<String>,

    /// The CA field of this object.
    pub ca: Option<String>,

    /// The hash of the NAR.
    ///
    /// It must begin with `sha256:` with the SHA-256 hash in the
    /// hexadecimal format (64 hex characters).
    ///
    /// This is informational and the server always validates the supplied
    /// hash.
    pub nar_hash: Hash,

    /// The size of the NAR.
    pub nar_size: usize,
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
pub struct UploadPathResult {
    #[serde_as(deserialize_as = "DefaultOnError")]
    pub kind: UploadPathResultKind,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UploadPathResultKind {
    /// The path was uploaded.
    Uploaded,

    /// The path was globally deduplicated.
    Deduplicated,
}

impl Default for UploadPathResultKind {
    fn default() -> Self {
        Self::Uploaded
    }
}
