#![deny(
    asm_sub_register,
    deprecated,
    missing_abi,
    unsafe_code,
    unused_macros,
    unused_must_use,
    unused_unsafe
)]
#![deny(clippy::from_over_into, clippy::needless_question_mark)]
#![cfg_attr(
    not(debug_assertions),
    deny(unused_imports, unused_mut, unused_variables,)
)]

mod api;
mod cache;
mod cli;
mod command;
mod compression;
mod config;
mod nix_config;
mod nix_netrc;
mod push;
mod version;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    init_logging()?;
    cli::run().await
}

fn init_logging() -> Result<()> {
    tracing_subscriber::fmt::init();
    Ok(())
}
