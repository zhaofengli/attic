//! Nix store operations.
//!
//! ## FFI Bindings
//!
//! For now, the FFI bindings are for use in the client. We never
//! interact with the Nix store on the server. When the `nix_store`
//! crate feature is disabled, native Rust portions of this module
//! will still function.
//!
//! We use `libnixstore` to carry out most of the operations.
//! To interface with `libnixstore`, we first construct a simpler,
//! FFI-friendly API in C++ and then integrate with it using [cxx](https://cxx.rs)
//! and [rust-bindgen](https://rust-lang.github.io/rust-bindgen).
//! The glue interface is mostly object-oriented, with no pesky
//! C-style OOP functions or manual lifetime tracking.
//!
//! The C++-side code is responsible for translating the calls
//! into actual `libnixstore` invocations which are version-specific.
//! (we target Nix 2.4 and 2.5).
//!
//! We have the following goals:
//! - Retrieval of store path information
//! - Computation of closures
//! - Streaming of NAR archives
//! - Fully `async`/`await` API with support for concurrency
//!
//! ## Alternatives?
//!
//! The Nix source tree includes [`nix-rust`](https://github.com/NixOS/nix/tree/master/nix-rust)
//! which contains a limited implementation of various store operations.
//! It [used to](https://github.com/NixOS/nix/commit/bbe97dff8b3054d96e758f486f9ce3fa09e64de3)
//! contain an implementation of `StorePath` in Rust which was used from C++
//! via FFI. It was [removed](https://github.com/NixOS/nix/commit/759947bf72c134592f0ce23d385e48095bd0a301)
//! half a year later due to memory consumption concerns. The current
//! `nix-rust` contains a set of `libnixstore` bindings, but they are low-level
//! and suffering from bitrot.
//!
//! For easier FFI, there is an attempt to make a C wrapper for `libnixstore` called
//! [libnixstore-c](https://github.com/andir/libnixstore-c). It offers
//! very limited amount of functionality.

#[cfg(feature = "nix_store")]
#[allow(unsafe_code)]
mod bindings;

#[cfg(feature = "nix_store")]
mod nix_store;

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use lazy_static::lazy_static;
use regex::Regex;
use serde::{de, Deserialize, Serialize};

use crate::error::{AtticError, AtticResult};
use crate::hash::Hash;

#[cfg(feature = "nix_store")]
pub use bindings::{FfiHash, FfiHashType};

#[cfg(feature = "nix_store")]
pub use nix_store::NixStore;

#[cfg(test)]
pub mod tests;

/// Length of the hash in a store path.
pub const STORE_PATH_HASH_LEN: usize = 32;

/// Regex that matches a store path hash, without anchors.
pub const STORE_PATH_HASH_REGEX_FRAGMENT: &str = "[0123456789abcdfghijklmnpqrsvwxyz]{32}";

lazy_static! {
    /// Regex for a valid store path hash.
    ///
    /// This is the path portion of a base name.
    static ref STORE_PATH_HASH_REGEX: Regex = {
        Regex::new(&format!("^{}$", STORE_PATH_HASH_REGEX_FRAGMENT)).unwrap()
    };

    /// Regex for a valid store base name.
    ///
    /// A base name consists of two parts: A hash and a human-readable
    /// label/name. The format of the hash is described in `StorePathHash`.
    ///
    /// The human-readable name can only contain the following characters:
    ///
    /// - A-Za-z0-9
    /// - `+-._?=`
    ///
    /// See the Nix implementation in `src/libstore/path.cc`.
    static ref STORE_BASE_NAME_REGEX: Regex = {
        Regex::new(r"^[0123456789abcdfghijklmnpqrsvwxyz]{32}-[A-Za-z0-9+-._?=]+$").unwrap()
    };
}

/// A path in a Nix store.
///
/// This must be a direct child of the store. This path may or
/// may not actually exist.
///
/// This guarantees that the base name is of valid format.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct StorePath {
    /// Base name of the store path.
    ///
    /// For example, for `/nix/store/ia70ss13m22znbl8khrf2hq72qmh5drr-ruby-2.7.5`,
    /// this would be `ia70ss13m22znbl8khrf2hq72qmh5drr-ruby-2.7.5`.
    base_name: PathBuf,
}

/// A fixed-length store path hash.
///
/// For example, for `/nix/store/ia70ss13m22znbl8khrf2hq72qmh5drr-ruby-2.7.5`,
/// this would be `ia70ss13m22znbl8khrf2hq72qmh5drr`.
///
/// It must contain exactly 32 "base-32 characters". Nix's special scheme
/// include the following valid characters: "0123456789abcdfghijklmnpqrsvwxyz"
/// ('e', 'o', 'u', 't' are banned).
///
/// Examples of invalid store path hashes:
///
/// - "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
/// - "IA70SS13M22ZNBL8KHRF2HQ72QMH5DRR"
/// - "whatevenisthisthing"
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize)]
pub struct StorePathHash(String);

/// Information on a valid store path.
#[derive(Debug)]
pub struct ValidPathInfo {
    /// The store path.
    pub path: StorePath,

    /// Hash of the NAR.
    pub nar_hash: Hash,

    /// Size of the NAR.
    pub nar_size: u64,

    /// References.
    ///
    /// This list only contains base names of the paths.
    pub references: Vec<PathBuf>,

    /// Signatures.
    pub sigs: Vec<String>,

    /// Content Address.
    pub ca: Option<String>,
}

#[cfg_attr(not(feature = "nix_store"), allow(dead_code))]
impl StorePath {
    /// Creates a StorePath with a base name.
    fn from_base_name(base_name: PathBuf) -> AtticResult<Self> {
        let s = base_name
            .as_os_str()
            .to_str()
            .ok_or_else(|| AtticError::InvalidStorePathName {
                base_name: base_name.clone(),
                reason: "Name contains non-UTF-8 characters",
            })?;

        if !STORE_BASE_NAME_REGEX.is_match(s) {
            return Err(AtticError::InvalidStorePathName {
                base_name,
                reason: "Name is of invalid format",
            });
        }

        Ok(Self { base_name })
    }

    /// Creates a StorePath with a known valid base name.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the name is of a valid format (refer
    /// to the documentations for `STORE_BASE_NAME_REGEX`). Other operations
    /// with this object will assume it's valid.
    #[allow(unsafe_code)]
    unsafe fn from_base_name_unchecked(base_name: PathBuf) -> Self {
        Self { base_name }
    }

    /// Gets the hash portion of the store path.
    pub fn to_hash(&self) -> StorePathHash {
        // Safety: We have already validated the format of the base name,
        // including the hash part. The name is guaranteed valid UTF-8.
        #[allow(unsafe_code)]
        unsafe {
            let s = std::str::from_utf8_unchecked(self.base_name.as_os_str().as_bytes());
            let hash = s[..STORE_PATH_HASH_LEN].to_string();
            StorePathHash::new_unchecked(hash)
        }
    }

    /// Returns the human-readable name.
    pub fn name(&self) -> String {
        // Safety: Already checked
        #[allow(unsafe_code)]
        unsafe {
            let s = std::str::from_utf8_unchecked(self.base_name.as_os_str().as_bytes());
            s[STORE_PATH_HASH_LEN + 1..].to_string()
        }
    }

    pub fn as_os_str(&self) -> &OsStr {
        self.base_name.as_os_str()
    }

    #[cfg_attr(not(feature = "nix_store"), allow(dead_code))]
    fn as_base_name_bytes(&self) -> &[u8] {
        self.base_name.as_os_str().as_bytes()
    }
}

impl StorePathHash {
    /// Creates a store path hash from a string.
    pub fn new(hash: String) -> AtticResult<Self> {
        if hash.as_bytes().len() != STORE_PATH_HASH_LEN {
            return Err(AtticError::InvalidStorePathHash {
                hash,
                reason: "Hash is of invalid length",
            });
        }

        if !STORE_PATH_HASH_REGEX.is_match(&hash) {
            return Err(AtticError::InvalidStorePathHash {
                hash,
                reason: "Hash is of invalid format",
            });
        }

        Ok(Self(hash))
    }

    /// Creates a store path hash from a string, without checking its validity.
    ///
    /// # Safety
    ///
    /// The caller must make sure that it is of expected length and format.
    #[allow(unsafe_code)]
    pub unsafe fn new_unchecked(hash: String) -> Self {
        Self(hash)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_string(&self) -> String {
        self.0.clone()
    }
}

impl<'de> Deserialize<'de> for StorePathHash {
    /// Deserializes a potentially-invalid store path hash.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        use de::Error;
        String::deserialize(deserializer)
            .and_then(|s| Self::new(s).map_err(|e| Error::custom(e.to_string())))
    }
}

/// Returns the base store name of a path relative to a store root.
#[cfg_attr(not(feature = "nix_store"), allow(dead_code))]
fn to_base_name(store_dir: &Path, path: &Path) -> AtticResult<PathBuf> {
    if let Ok(remaining) = path.strip_prefix(store_dir) {
        let first = remaining
            .iter()
            .next()
            .ok_or_else(|| AtticError::InvalidStorePath {
                path: path.to_owned(),
                reason: "Path is store directory itself",
            })?;

        if first.len() < STORE_PATH_HASH_LEN {
            Err(AtticError::InvalidStorePath {
                path: path.to_owned(),
                reason: "Path is too short",
            })
        } else {
            Ok(PathBuf::from(first))
        }
    } else {
        Err(AtticError::InvalidStorePath {
            path: path.to_owned(),
            reason: "Path is not in store directory",
        })
    }
}
