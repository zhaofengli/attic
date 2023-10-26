//! Client configurations.
//!
//! Configuration files are stored under `$XDG_CONFIG_HOME/attic/config.toml`.
//! We automatically write modified configurations back for a good end-user
//! experience (e.g., `attic login`).

use std::collections::HashMap;
use std::fs::{self, OpenOptions, Permissions};
use std::io::Write;
use std::ops::{Deref, DerefMut};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use xdg::BaseDirectories;

use crate::cache::{CacheName, CacheRef, ServerName};

/// Application prefix in XDG base directories.
///
/// This will be concatenated into `$XDG_CONFIG_HOME/attic`.
const XDG_PREFIX: &str = "attic";

/// The permission the configuration file should have.
const FILE_MODE: u32 = 0o600;

/// Configuration loader.
#[derive(Debug)]
pub struct Config {
    /// Actual configuration data.
    data: ConfigData,

    /// Path to write modified configurations back to.
    path: Option<PathBuf>,
}

/// Client configurations.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ConfigData {
    /// The default server to connect to.
    #[serde(rename = "default-server")]
    pub default_server: Option<ServerName>,

    /// A set of remote servers and access credentials.
    #[serde(default = "HashMap::new")]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub servers: HashMap<ServerName, ServerConfig>,
}

/// Compression Settings
#[derive(Debug, Copy, Clone, Deserialize, Serialize, Default)]
pub enum CompressionConfig {
    /// No compression.
    None,
    /// Brotli compression.
    Brotli,
    /// Deflate compression.
    Deflate,
    /// Gzip compression.
    Gzip,
    /// Xz compression.
    Xz,
    /// Zstd compression.
    #[default]
    Zstd,
}

impl CompressionConfig {
    /// Returns the HTTP representation of this compression setting.
    pub fn http_value(self) -> &'static str {
        match self {
            Self::None => "identity",
            Self::Brotli => "br",
            Self::Deflate => "deflate",
            Self::Gzip => "gzip",
            Self::Xz => "xz",
            Self::Zstd => "zstd",
        }
    }
}

/// Configuration of a server.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    pub endpoint: String,
    pub token: Option<String>,
    #[serde(default)]
    pub compression: CompressionConfig,
}

/// Wrapper that automatically saves the config once dropped.
pub struct ConfigWriteGuard<'a>(&'a mut Config);

impl Config {
    /// Loads the configuration from the system.
    pub fn load() -> Result<Self> {
        let path = get_config_path()
            .map_err(|e| {
                tracing::warn!("Could not get config path: {}", e);
                e
            })
            .ok();

        let data = ConfigData::load_from_path(path.as_ref())?;

        Ok(Self { data, path })
    }

    /// Returns a mutable reference to the configuration.
    pub fn as_mut(&mut self) -> ConfigWriteGuard {
        ConfigWriteGuard(self)
    }

    /// Saves the configuration back to the system, if possible.
    pub fn save(&self) -> Result<()> {
        if let Some(path) = &self.path {
            let serialized = toml::to_string(&self.data)?;

            // This isn't atomic, so some other process might chmod it
            // to something else before we write. We don't handle this case.
            if path.exists() {
                let permissions = Permissions::from_mode(FILE_MODE);
                fs::set_permissions(path, permissions)?;
            }

            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(FILE_MODE)
                .open(path)?;

            file.write_all(serialized.as_bytes())?;

            tracing::debug!("Saved modified configuration to {:?}", path);
        }

        Ok(())
    }
}

impl Deref for Config {
    type Target = ConfigData;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl ConfigData {
    fn load_from_path(path: Option<&PathBuf>) -> Result<Self> {
        if let Some(path) = path {
            if path.exists() {
                let contents = fs::read(path)?;
                let s = std::str::from_utf8(&contents)?;
                let data = toml::from_str(s)?;
                return Ok(data);
            }
        }

        Ok(ConfigData::default())
    }

    pub fn default_server(&self) -> Result<(&ServerName, &ServerConfig)> {
        if let Some(name) = &self.default_server {
            let config = self.servers.get(name).ok_or_else(|| {
                anyhow!(
                    "Configured default server \"{}\" does not exist",
                    name.as_str()
                )
            })?;
            Ok((name, config))
        } else if let Some((name, config)) = self.servers.iter().next() {
            Ok((name, config))
        } else {
            Err(anyhow!("No servers are available."))
        }
    }

    pub fn resolve_cache<'a>(
        &'a self,
        r: &'a CacheRef,
    ) -> Result<(&'a ServerName, &'a ServerConfig, &'a CacheName)> {
        match r {
            CacheRef::DefaultServer(cache) => {
                let (name, config) = self.default_server()?;
                Ok((name, config, cache))
            }
            CacheRef::ServerQualified(server, cache) => {
                let config = self
                    .servers
                    .get(server)
                    .ok_or_else(|| anyhow!("Server \"{}\" does not exist", server.as_str()))?;
                Ok((server, config, cache))
            }
        }
    }
}

impl<'a> Deref for ConfigWriteGuard<'a> {
    type Target = ConfigData;

    fn deref(&self) -> &Self::Target {
        &self.0.data
    }
}

impl<'a> DerefMut for ConfigWriteGuard<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0.data
    }
}

impl<'a> Drop for ConfigWriteGuard<'a> {
    fn drop(&mut self) {
        if let Err(e) = self.0.save() {
            tracing::error!("Could not save modified configuration: {}", e);
        }
    }
}

fn get_config_path() -> Result<PathBuf> {
    let xdg_dirs = BaseDirectories::with_prefix(XDG_PREFIX)?;
    let config_path = xdg_dirs.place_config_file("config.toml")?;

    Ok(config_path)
}
