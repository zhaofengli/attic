//! get-missing-paths v1
//!
//! `POST /_api/v1/get-missing-paths`
//!
//! Requires "push" permission.

use serde::{Deserialize, Serialize};

use crate::cache::CacheName;
use crate::nix_store::StorePathHash;

#[derive(Debug, Serialize, Deserialize)]
pub struct GetMissingPathsRequest {
    /// The name of the cache.
    pub cache: CacheName,

    /// The list of store paths.
    pub store_path_hashes: Vec<StorePathHash>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetMissingPathsResponse {
    /// A list of paths that are not in the cache.
    pub missing_paths: Vec<StorePathHash>,
}
