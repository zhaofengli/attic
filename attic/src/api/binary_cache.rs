//! Nix Binary Cache server.
//!
//! This module contains Attic-specific extensions to the
//! Nix Binary Cache API.

/// Header indicating a cache's visibility.
pub const ATTIC_CACHE_VISIBILITY: &str = "X-Attic-Cache-Visibility";
