//! Error handling.

use std::error::Error as StdError;
use std::io;
use std::path::PathBuf;

use displaydoc::Display;

pub type AtticResult<T> = Result<T, AtticError>;

/// An error.
#[derive(Debug, Display)]
pub enum AtticError {
    /// Failed to connect to the Nix store: {reason}
    StoreConnectError { reason: String },

    /// Invalid store path {path:?}: {reason}
    InvalidStorePath { path: PathBuf, reason: &'static str },

    /// Invalid store path base name {base_name:?}: {reason}
    InvalidStorePathName {
        base_name: PathBuf,
        reason: &'static str,
    },

    /// Invalid store path hash "{hash}": {reason}
    InvalidStorePathHash { hash: String, reason: &'static str },

    /// Invalid cache name "{name}"
    InvalidCacheName { name: String },

    /// Signing error: {0}
    SigningError(super::signing::Error),

    /// Hashing error: {0}
    HashError(super::hash::Error),

    /// I/O error: {error}.
    IoError { error: io::Error },

    /// Invalid path info: {error}
    InvalidPathInfo { error: serde_json::Error },
}

impl AtticError {
    pub fn name(&self) -> &'static str {
        match self {
            Self::StoreConnectError { .. } => "StoreConnectError",
            Self::InvalidStorePath { .. } => "InvalidStorePath",
            Self::InvalidStorePathName { .. } => "InvalidStorePathName",
            Self::InvalidStorePathHash { .. } => "InvalidStorePathHash",
            Self::InvalidCacheName { .. } => "InvalidCacheName",
            Self::SigningError(_) => "SigningError",
            Self::HashError(_) => "HashError",
            Self::IoError { .. } => "IoError",
            Self::InvalidPathInfo { .. } => "InvalidPathInfo",
        }
    }
}

impl StdError for AtticError {}

impl From<io::Error> for AtticError {
    fn from(error: io::Error) -> Self {
        Self::IoError { error }
    }
}

impl From<super::signing::Error> for AtticError {
    fn from(error: super::signing::Error) -> Self {
        Self::SigningError(error)
    }
}

impl From<super::hash::Error> for AtticError {
    fn from(error: super::hash::Error) -> Self {
        Self::HashError(error)
    }
}

impl From<serde_json::Error> for AtticError {
    fn from(error: serde_json::Error) -> Self {
        Self::InvalidPathInfo { error }
    }
}
