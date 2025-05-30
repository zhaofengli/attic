//! Client configurations.
//!
//! Configuration files are stored under `$XDG_CONFIG_HOME/attic/config.toml`.
//! We automatically write modified configurations back for a good end-user
//! experience (e.g., `attic login`).
//!
//! Stateless configuration through environment variables is also supported (see below).
use std::collections::HashMap;
use std::env;
use std::fs::{self, read_to_string, OpenOptions, Permissions};
use std::io::Write;
use std::ops::{Deref, DerefMut};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use xdg::BaseDirectories;

use crate::cache::{CacheName, CacheRef, ServerName};

/// Application prefix in XDG base directories.
///
/// This will be concatenated into `$XDG_CONFIG_HOME/attic`.
const XDG_PREFIX: &str = "attic";

/// The permission the configuration file should have.
const FILE_MODE: u32 = 0o600;

/// Supported Environment Variables for stateless configuration:
const ATTIC_LOGIN_ENDPOINT: &str = "ATTIC_LOGIN_ENDPOINT"; // Server URL. When set, requires `ATTIC_LOGIN_NAME`.
const ATTIC_LOGIN_NAME: &str = "ATTIC_LOGIN_NAME"; // Name for the `ATTIC_LOGIN_ENDPOINT`.
const ATTIC_LOGIN_FORCE_DEFAULT: &str = "ATTIC_LOGIN_FORCE_DEFAULT"; // If set to any value, forces the server specified by `ATTIC_LOGIN_NAME` to be the default server.
const ATTIC_LOGIN_TOKEN: &str = "ATTIC_LOGIN_TOKEN"; // Inline token value.
const ATTIC_LOGIN_TOKEN_FILE: &str = "ATTIC_LOGIN_TOKEN_FILE"; // Path to a file containing the token.

/// Configuration loader from all sources.
pub struct Config {
    /// Configuration from TOML file persisted on disk.
    toml: TomlConfig,

    /// Configuration from environment variables.
    env: Option<EnvConfig>,
}

/// Configuration loader from environment variables.
///
/// During resolution, this takes precedence over `TomlConfig`.
struct EnvConfig {
    /// The server name specified by `ATTIC_LOGIN_NAME` env.
    login_server_name: ServerName,

    /// The configuration used to connect to the server specified by `ATTIC_LOGIN_ENDPOINT` env.
    login_server_config: ServerConfig,

    /// Whether to force the server specified by `ATTIC_LOGIN_NAME` to be the default server.
    login_force_default: bool,
}
/// Toml Configuration loader.
#[derive(Debug)]
struct TomlConfig {
    /// Actual configuration data.
    data: TomlConfigData,

    /// Path to write modified configurations back to.
    path: Option<PathBuf>,
}

/// Client configurations.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TomlConfigData {
    /// The default server to connect to.
    #[serde(rename = "default-server")]
    pub default_server: Option<ServerName>,

    /// A set of remote servers and access credentials.
    #[serde(default = "HashMap::new")]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub servers: HashMap<ServerName, ServerConfig>,
}

/// Configuration of a server.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    pub endpoint: String,
    #[serde(flatten)]
    pub token: Option<ServerTokenConfig>,
}

/// Copied from server/src/config.rs
fn read_non_empty_var(key: &str) -> Result<Option<String>> {
    let value = match env::var(key) {
        Err(env::VarError::NotPresent) => {
            return Ok(None);
        }
        r => r?,
    };

    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

impl ServerConfig {
    /// Create a `ServerConfig` from environment variables if `ATTIC_LOGIN_ENDPOINT` is set.
    pub fn from_env() -> Result<Option<Self>> {
        let endpoint = match read_non_empty_var(ATTIC_LOGIN_ENDPOINT)
            .with_context(|| format!("Failed to read {ATTIC_LOGIN_ENDPOINT} env"))?
        {
            Some(endpoint) => endpoint,
            None => return Ok(None),
        };

        let token = ServerTokenConfig::from_env()?;

        Ok(Some(ServerConfig { endpoint, token }))
    }
    pub fn token(&self) -> Result<Option<String>> {
        self.token.as_ref().map(|token| token.get()).transpose()
    }
}

/// Configured server token
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ServerTokenConfig {
    Raw {
        token: String,
    },
    File {
        #[serde(rename = "token-file")]
        token_file: String,
    },
}

impl ServerTokenConfig {
    /// Create a `ServerTokenConfig` from environment variables
    ///
    /// If both `ATTIC_LOGIN_TOKEN` and `ATTIC_LOGIN_TOKEN_FILE` env vars are set, the latter takes precedence.
    pub fn from_env() -> Result<Option<Self>> {
        if let Some(token_file) = read_non_empty_var(ATTIC_LOGIN_TOKEN_FILE)
            .with_context(|| format!("Failed to read {ATTIC_LOGIN_TOKEN_FILE} env"))?
        {
            return Ok(Some(ServerTokenConfig::File { token_file }));
        }

        if let Some(token) = read_non_empty_var(ATTIC_LOGIN_TOKEN)
            .with_context(|| format!("Failed to read {ATTIC_LOGIN_TOKEN} env"))?
        {
            return Ok(Some(ServerTokenConfig::Raw { token }));
        }

        Ok(None)
    }

    /// Get the token either directly from the config or through the token file
    pub fn get(&self) -> Result<String> {
        match self {
            ServerTokenConfig::Raw { token } => Ok(token.clone()),
            ServerTokenConfig::File { token_file } => Ok(read_to_string(token_file)
                .map(|t| t.trim().to_string())
                .with_context(|| format!("Failed to read token from {token_file}"))?),
        }
    }
}

/// Wrapper that automatically saves the config once dropped.
pub struct TomlConfigWriteGuard<'a>(&'a mut TomlConfig);

impl Config {
    pub fn load() -> Result<Self> {
        Ok(Self {
            toml: TomlConfig::load()?,
            env: EnvConfig::load()?,
        })
    }

    /// Get the server configuration by server name.
    ///
    /// If `EnvConfig` is set and `login_server_name` matches the requested server name, it takes precedence over `TomlConfig`.
    pub fn get_server<'b>(
        &'b self,
        name: &'b ServerName,
    ) -> Result<(&'b ServerName, &'b ServerConfig)> {
        if let Some(env) = &self.env {
            if name == &env.login_server_name {
                return Ok((&env.login_server_name, &env.login_server_config));
            } else {
                tracing::warn!(
                    "ATTIC_LOGIN_* environment is set for server '{}' but request targets server '{}'; using TOML configuration.",
                    env.login_server_name.as_str(),
                    name.as_str(),
                );
            }
        }
        let cfg = self
            .toml
            .data
            .servers
            .get(name)
            .ok_or_else(|| anyhow!("Server \"{}\" does not exist", name.as_str()))?;
        Ok((name, cfg))
    }

    /// Get the default server configuration.
    ///
    /// If `EnvConfig` is set and `login_force_default` is true, it takes precedence over `TomlConfig`.
    pub fn default_server(&self) -> Result<(&ServerName, &ServerConfig)> {
        if let Some(env) = &self.env {
            if env.login_force_default {
                return Ok((&env.login_server_name, &env.login_server_config));
            } else {
                tracing::warn!(
                    "Ignoring ATTIC_LOGIN_* environment as '{}' server is not default. Make it default by setting {ATTIC_LOGIN_FORCE_DEFAULT}=1",
                    env.login_server_name.as_str(),
                );
            }
        }
        self.toml.data.default_server()
    }

    pub fn resolve_cache<'b>(
        &'b self,
        r: &'b CacheRef,
    ) -> Result<(&'b ServerName, &'b ServerConfig, &'b CacheName)> {
        match r {
            CacheRef::DefaultServer(cache) => {
                let (name, config) = self.default_server()?;
                Ok((name, config, cache))
            }
            CacheRef::ServerQualified(server, cache) => {
                let (name, config) = self.get_server(server)?;
                Ok((name, config, cache))
            }
        }
    }

    // Forward a write guard for toml config persistence
    pub fn as_mut(&mut self) -> TomlConfigWriteGuard<'_> {
        self.toml.as_mut()
    }
}

impl EnvConfig {
    fn load() -> Result<Option<Self>> {
        let login_server_config = match ServerConfig::from_env()? {
            Some(cfg) => cfg,
            None => return Ok(None),
        };

        let login_server_name = match read_non_empty_var(ATTIC_LOGIN_NAME)
            .with_context(|| format!("Failed to read {ATTIC_LOGIN_NAME} env"))?
        {
            Some(name) => ServerName::from_str(&name)?,
            None => {
                return Err(anyhow!(
                "{ATTIC_LOGIN_NAME}=<name> env must be set when {ATTIC_LOGIN_ENDPOINT} is provided"
            ))
            }
        };

        let login_force_default = read_non_empty_var(ATTIC_LOGIN_FORCE_DEFAULT)
            .with_context(|| format!("Failed to read {ATTIC_LOGIN_FORCE_DEFAULT} env"))?
            .is_some();

        Ok(Some(Self {
            login_server_name,
            login_server_config,
            login_force_default,
        }))
    }
}
impl TomlConfig {
    /// Loads the configuration from the system.
    pub fn load() -> Result<Self> {
        let path = get_config_path()
            .map_err(|e| {
                tracing::warn!("Could not get config path: {}", e);
                e
            })
            .ok();

        let data = TomlConfigData::load_from_path(path.as_ref())?;

        Ok(Self { data, path })
    }

    /// Returns a mutable reference to the configuration.
    pub fn as_mut(&mut self) -> TomlConfigWriteGuard {
        TomlConfigWriteGuard(self)
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

impl Deref for TomlConfig {
    type Target = TomlConfigData;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl TomlConfigData {
    fn load_from_path(path: Option<&PathBuf>) -> Result<Self> {
        if let Some(path) = path {
            if path.exists() {
                let contents = fs::read(path)?;
                let s = std::str::from_utf8(&contents)?;
                let data = toml::from_str(s)?;
                return Ok(data);
            }
        }

        Ok(TomlConfigData::default())
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
}

impl<'a> Deref for TomlConfigWriteGuard<'a> {
    type Target = TomlConfigData;

    fn deref(&self) -> &Self::Target {
        &self.0.data
    }
}

impl<'a> DerefMut for TomlConfigWriteGuard<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0.data
    }
}

impl<'a> Drop for TomlConfigWriteGuard<'a> {
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
