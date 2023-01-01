//! Client-specific cache references.
//!
//! The Attic client is designed to work with multiple servers.
//! Therefore, users can refer to caches in the following forms:
//!
//! - `cachename`: Will use `cachename` on the default server
//! - `servername:cachename`: Will use `cachename` on server `servername`
//! - `https://cache.server.tld/username`: Will auto-detect
//!     - To be implemented

use std::ops::Deref;
use std::str::FromStr;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

pub use attic::cache::{CacheName, CacheNamePattern};

/// A reference to a cache.
#[derive(Debug, Clone)]
pub enum CacheRef {
    DefaultServer(CacheName),
    ServerQualified(ServerName, CacheName),
}

/// A server name.
///
/// It has the same requirements as a cache name.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ServerName(CacheName);

impl CacheRef {
    fn try_parse_cache(s: &str) -> Option<Self> {
        let name = CacheName::new(s.to_owned()).ok()?;
        Some(Self::DefaultServer(name))
    }

    fn try_parse_server_qualified(s: &str) -> Option<Self> {
        let (server, cache) = s.split_once(':')?;
        let server = CacheName::new(server.to_owned()).ok()?;
        let cache = CacheName::new(cache.to_owned()).ok()?;
        Some(Self::ServerQualified(ServerName(server), cache))
    }
}

impl FromStr for CacheRef {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        if let Some(r) = Self::try_parse_cache(s) {
            return Ok(r);
        }

        if let Some(r) = Self::try_parse_server_qualified(s) {
            return Ok(r);
        }

        Err(anyhow!("Invalid cache reference"))
    }
}

impl Deref for ServerName {
    type Target = CacheName;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromStr for ServerName {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(Self(CacheName::from_str(s)?))
    }
}
