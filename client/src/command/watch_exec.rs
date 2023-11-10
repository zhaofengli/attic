use crate::cache::CacheRef;
use crate::cli::Opts;
use crate::command::queue::{run_daemon, Daemon};
use anyhow::Result;
use clap::Parser;
use indoc::formatdoc;
use std::env;
use std::env::current_exe;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::exit;
use std::time::Duration;
use tokio::fs;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Parser)]
#[command(about = "Execute the passed command and upload all meanwhile built derivations")]
pub struct WatchExec {
    #[arg(help = "Name of cache to push build artifacts to")]
    cache: CacheRef,
    #[arg(help = "Command to wrap and wait for", trailing_var_arg = true)]
    command: Vec<String>,
}

fn build_command(args: &Vec<String>) -> Command {
    let mut command = Command::new(args[0].clone());
    command.args(args.into_iter().skip(1));
    command
}

async fn create_post_build_hook_file(path: PathBuf) {
    let current_exe_path = current_exe().expect("failed to get own executable path");
    let mut file = File::create(path)
        .await
        .expect("failed to create post-build-hook file");
    let contents = formatdoc! {"
        #!/bin/sh
        set -eu
        set -f # disable globbing
        export IFS=' '
        exec {current_exe_path} queue relay
    ", current_exe_path = current_exe_path.display()};
    file.write_all(contents.as_ref())
        .await
        .expect("failed to write nix post-build-hook");
    file.set_permissions(Permissions::from_mode(0o755))
        .await
        .expect("failed to chmod 755 post-build-hook");
}

async fn create_nix_config(path: PathBuf, hook_path: PathBuf) {
    let mut file = File::create(path)
        .await
        .expect("failed to create nix config file");
    file.write_all(format!("post-build-hook = {}", hook_path.display()).as_ref())
        .await
        .expect("failed to write nix config");
}

pub async fn run(options: Opts) -> Result<()> {
    if let Some(watch_exec) = options.command.as_watch_exec() {
        let cancellation_token = CancellationToken::new();
        let cache = &watch_exec.cache;

        let daemon_handle = tokio::spawn(run_daemon(
            Daemon {
                cache: cache.clone(),
            },
            cancellation_token.clone(),
        ));
        // wait a bit for daemon to become ready
        tokio::time::sleep(Duration::from_millis(400)).await;

        let runtime_dir =
            PathBuf::from(env::var("XDG_RUNTIME_DIR").unwrap_or("/tmp".into())).join("attic");
        fs::create_dir_all(runtime_dir.clone()).await?;

        // create nix.conf and post_build_hook.sh
        let hook_path = runtime_dir.join("post_build_hook.sh");
        let nix_config_path = runtime_dir.join("nix.conf");
        create_post_build_hook_file(hook_path.clone()).await;
        create_nix_config(nix_config_path.clone(), hook_path.clone()).await;

        let mut command = build_command(&watch_exec.command);

        // prepend our temporary config to NIX_USER_CONF_FILES
        let existing_conf_files = env::var("NIX_USER_CONF_FILES").unwrap_or("".into());
        command.env(
            "NIX_USER_CONF_FILES",
            format!("{}:{}", nix_config_path.display(), existing_conf_files),
        );

        let status = command.status().await.expect("failed to run subcommand");
        let exit_code = status.code().unwrap_or(1);
        eprintln!("Command exited with {exit_code}, waiting for potential uploads to finish...");

        cancellation_token.cancel();
        // wait for daemon to finish uploading
        daemon_handle
            .await
            .unwrap()
            .expect("error waiting for daemon to finish uploading");
        exit(exit_code);
    }

    Ok(())
}
