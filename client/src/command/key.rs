use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::cli::Opts;
use attic::signing::NixKeypair;

/// Manage signing keys.
#[derive(Debug, Parser)]
pub struct Key {
    #[clap(subcommand)]
    command: KeyCommand,
}

#[derive(Debug, Subcommand)]
enum KeyCommand {
    Generate(Generate),
}

/// Generate a key.
#[derive(Debug, Clone, Parser)]
pub struct Generate {
    /// Name of the key (must not contain colons).
    name: String,
}

pub async fn run(opts: Opts) -> Result<()> {
    let sub = opts.command.as_key().unwrap();
    match &sub.command {
        KeyCommand::Generate(sub) => generate_key(sub).await,
    }
}

async fn generate_key(sub: &Generate) -> Result<()> {
    let keypair = NixKeypair::generate(&sub.name)?;

    println!("🔑 Generated keypair \"{}\"", sub.name);
    println!();
    println!("    Private key: {}", keypair.export_keypair());
    println!("     Public key: {}", keypair.export_public_key());

    Ok(())
}
