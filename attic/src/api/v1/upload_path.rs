use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DefaultOnError};
use uuid::Uuid;

use crate::cache::CacheName;
use crate::hash::Hash;
use crate::nix_store::StorePathHash;

/// Header containing the upload info.
pub const ATTIC_NAR_INFO: &str = "X-Attic-Nar-Info";

/// Header containing the size of the upload info at the beginning of the body.
pub const ATTIC_NAR_INFO_PREAMBLE_SIZE: &str = "X-Attic-Nar-Info-Preamble-Size";

/// NAR information associated with a upload.
///
/// There are two ways for the client to supply the NAR information:
///
/// 1. At the beginning of the PUT body. The `X-Attic-Nar-Info-Preamble-Size`
///    header must be set to the size of the JSON.
/// 2. Through the `X-Attic-Nar-Info` header.
///
/// The client is advised to use the first method if the serialized
/// JSON is large (>4K).
///
/// Regardless of client compression, the server will always decompress
/// the NAR to validate the NAR hash before applying the server-configured
/// compression again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Server-advertised configuration for chunked transport uploads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadChunkingConfig {
    /// Maximum transport part size in bytes.
    pub max_chunk_size: usize,
}

/// Request to create a chunked transport upload session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartUploadPathSessionRequest {
    /// NAR information for the object being uploaded.
    pub nar_info: UploadPathNarInfo,

    /// Requested transport part size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_size: Option<usize>,
}

/// Response returned when starting a chunked transport upload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StartUploadPathSessionResponse {
    /// A new upload session was created and the client should upload parts.
    Session {
        /// Upload session ID.
        session_id: Uuid,

        /// Transport part size selected by the server.
        chunk_size: usize,
    },

    /// The upload completed immediately without creating a session.
    Completed {
        /// Upload result.
        result: UploadPathResult,
    },
}

/// Response returned when finalizing a chunked transport upload session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FinalizeUploadSessionResponse {
    /// Finalization is running in the background. The client should poll again.
    Pending,

    /// Finalization failed permanently.
    Failed {
        /// Human-readable reason.
        message: String,
    },

    /// Finalization completed.
    Completed {
        /// Upload result.
        result: UploadPathResult,
    },
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadPathResult {
    #[serde_as(deserialize_as = "DefaultOnError")]
    pub kind: UploadPathResultKind,

    /// The compressed size of the NAR, in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_size: Option<usize>,

    /// The fraction of data that was deduplicated, from 0 to 1.
    pub frac_deduplicated: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UploadPathResultKind {
    /// The path was uploaded.
    ///
    /// This is purely informational and servers may return
    /// this variant even when the NAR is deduplicated.
    Uploaded,

    /// The path was globally deduplicated.
    ///
    /// The exact semantics of what counts as deduplicated
    /// is opaque to the client.
    Deduplicated,
}

impl Default for UploadPathResultKind {
    fn default() -> Self {
        Self::Uploaded
    }
}
