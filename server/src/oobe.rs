//! Guided out-of-box experience.
//!
//! This performs automatic setup for people running `atticd`
//! directly without specifying any configurations. The goal is
//! to let them quickly have a taste of Attic with a config
//! template that provide guidance for them to achieve a more
//! permanent setup.
//!
//! Paths:
//! - Config: `~/.config/attic/server.yaml`
//! - SQLite: `~/.local/share/attic/server.db`
//! - NARs: `~/.local/share/attic/storage`

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use chrono::{Months, Utc};
use rand::distributions::Alphanumeric;
use rand::Rng;
use tokio::fs::{self, OpenOptions};

use crate::access::{decode_token_rs256_secret, Token};
use crate::config;
use attic::cache::CacheNamePattern;

const CONFIG_TEMPLATE: &str = include_str!("config-template.toml");

pub async fn run_oobe() -> Result<()> {
    let config_path = config::get_xdg_config_path()?;

    if config_path.exists() {
        return Ok(());
    }

    let data_path = config::get_xdg_data_path()?;

    // Generate a simple config
    let database_path = data_path.join("server.db");
    let database_url = format!("sqlite://{}", database_path.to_str().unwrap());
    OpenOptions::new()
        .create(true)
        .write(true)
        .open(&database_path)
        .await?;

    let storage_path = data_path.join("storage");
    fs::create_dir_all(&storage_path).await?;

    let rs256_secret = {
        let random: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(128)
            .map(char::from)
            .collect();

        BASE64_STANDARD.encode(random)
    };

    let config_content = CONFIG_TEMPLATE
        .replace("%database_url%", &database_url)
        .replace("%storage_path%", storage_path.to_str().unwrap())
        .replace("%token_rs256_secret%", &rs256_secret);

    fs::write(&config_path, config_content.as_bytes()).await?;

    // Generate a JWT token
    let root_token = {
        let in_two_years = Utc::now().checked_add_months(Months::new(24)).unwrap();
        let mut token = Token::new("root".to_string(), &in_two_years);
        let any_cache = CacheNamePattern::new("*".to_string()).unwrap();
        let perm = token.get_or_insert_permission_mut(any_cache);
        perm.pull = true;
        perm.push = true;
        perm.delete = true;
        perm.create_cache = true;
        perm.configure_cache = true;
        perm.configure_cache_retention = true;
        perm.destroy_cache = true;

        let key = decode_token_rs256_secret(&rs256_secret).unwrap();
        token.encode(&key.0)?
    };

    eprintln!();
    eprintln!("-----------------");
    eprintln!("Welcome to Attic!");
    eprintln!();
    eprintln!("A simple setup using SQLite and local storage has been configured for you in:");
    eprintln!();
    eprintln!("    {}", config_path.to_str().unwrap());
    eprintln!();
    eprintln!("Run the following command to log into this server:");
    eprintln!();
    eprintln!("    attic login local http://localhost:8080 {root_token}");
    eprintln!();
    eprintln!("Documentations and guides:");
    eprintln!();
    eprintln!("    https://docs.attic.rs");
    eprintln!();
    eprintln!("Enjoy!");
    eprintln!("-----------------");
    eprintln!();

    Ok(())
}
