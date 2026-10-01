//! Pin API types.
//!
//! A pin is a named GC root in a cache: the pinned store path and everything
//! reachable from it (via narinfo references) is exempt from time-based
//! garbage collection until the pin is deleted. Prior art: Cachix "pins".

use serde::{Deserialize, Serialize};

/// Request to create or re-point a pin.
///
/// `PUT /_api/v1/pins/{cache}/{name}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePinRequest {
    /// The full store path to pin (e.g. `/nix/store/<hash>-<name>`).
    ///
    /// Must already exist as an object in the cache.
    pub store_path: String,
}

/// A single pin, as returned by the list endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinEntry {
    /// Name of the pin, unique per cache.
    pub name: String,

    /// The pinned store path.
    pub store_path: String,

    /// When the pin was created or last re-pointed.
    pub created_at: String,

    /// Who created the pin, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

/// Response of the list endpoint.
///
/// `GET /_api/v1/pins/{cache}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPinsResponse {
    pub pins: Vec<PinEntry>,
}
