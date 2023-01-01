use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use crate::cli::Opts;
use attic::nix_store::NixStore;

/// Returns the closure of a store path (test).
///
/// This is similar to `nix-store -qR`.
#[derive(Debug, Parser)]
pub struct GetClosure {
    store_path: PathBuf,
}

pub async fn run(opts: Opts) -> Result<()> {
    let sub = opts.command.as_get_closure().unwrap();

    let store = NixStore::connect()?;
    let store_path = store.follow_store_path(&sub.store_path)?;
    let closure = store
        .compute_fs_closure(store_path, false, false, false)
        .await?;

    for path in &closure {
        println!("{}", store.get_full_path(path).to_str().unwrap());
    }

    Ok(())
}
