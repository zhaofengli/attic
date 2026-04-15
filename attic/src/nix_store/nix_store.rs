//! High-level Nix Store interface.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str::FromStr as _;

use async_stream::try_stream;
use futures::Stream;
use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;
use tokio::process::Command;

use super::{to_base_name, StorePath, ValidPathInfo};
use crate::error::AtticResult;
use crate::hash::Hash;
use crate::AtticError;

/// High-level wrapper for the Unix Domain Socket Nix Store.
pub struct NixStore {
    daemon: nix_daemon::nix::DaemonStore<UnixStream>,

    /// Path to the Nix store itself.
    store_dir: PathBuf,
}

/// The output of `nix path-info --json`.
#[derive(Debug, Clone, Deserialize)]
struct NixPathInfoJson {
    // Depending on the Nix version this might or might not be there.
    path: Option<PathBuf>,

    #[serde(rename = "narHash")]
    nar_hash: String,

    #[serde(rename = "narSize")]
    nar_size: u64,

    #[serde(default)]
    references: Vec<PathBuf>,

    #[serde(default)]
    signatures: Vec<String>,
}

impl NixStore {
    pub async fn connect() -> AtticResult<Self> {
        Ok(Self {
            daemon: nix_daemon::nix::DaemonStore::builder()
                .connect_unix("/nix/var/nix/daemon-socket/socket")
                .await
                .map_err(|e| AtticError::StoreConnectError {
                    reason: e.to_string(),
                })?,
            // TODO: Make this method async and call nix-instantiate --raw --eval -E 'builtins.storeDir'
            store_dir: PathBuf::from_str("/nix/store").unwrap(),
        })
    }

    /// Returns the Nix store directory.
    pub fn store_dir(&self) -> &Path {
        &self.store_dir
    }

    /// Returns the base store path of a path, following any symlinks.
    ///
    /// This is a simple wrapper over `parse_store_path` that also
    /// follows symlinks.
    pub fn follow_store_path<P: AsRef<Path>>(&self, path: P) -> AtticResult<StorePath> {
        // Some cases to consider:
        //
        // - `/nix/store/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-nixos-system-x/sw` (a symlink to sw)
        //    - `eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-nixos-system-x`
        //    - We don't resolve the `sw` symlink since the full store path is specified
        //      (this is a design decision)
        // - `/run/current-system` (a symlink to profile)
        //    - `eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-nixos-system-x`
        // - `/run/current-system/` (with a trailing slash)
        //    - `eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-nixos-system-x`
        // - `/run/current-system/sw` (a symlink to sw)
        //    - `eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-system-path` (!)
        let path = path.as_ref();
        if path.strip_prefix(&self.store_dir).is_ok() {
            // Is in the store - directly strip regardless of being a symlink or not
            self.parse_store_path(path)
        } else {
            // Canonicalize then parse
            let canon = path.canonicalize()?;
            self.parse_store_path(canon)
        }
    }

    /// Returns the base store path of a path.
    ///
    /// This function does not validate whether the path is actually in the
    /// Nix store or not.
    ///
    /// The path must be under the store directory. See `follow_store_path`
    /// for an alternative that follows symlinks.
    pub fn parse_store_path<P: AsRef<Path>>(&self, path: P) -> AtticResult<StorePath> {
        let base_name = to_base_name(&self.store_dir, path.as_ref())?;
        StorePath::from_base_name(base_name)
    }

    /// Returns the full path for a base store path.
    pub fn get_full_path(&self, store_path: &StorePath) -> PathBuf {
        self.store_dir.join(&store_path.base_name)
    }

    /// Creates a NAR archive from a path.
    ///
    /// This is akin to `nix-store --dump`.
    pub fn nar_from_path(
        &self,
        store_path: StorePath,
    ) -> impl Stream<Item = AtticResult<Vec<u8>>> + Unpin + Send {
        let full_path = self.get_full_path(&store_path);
        Box::pin(try_stream! {
            let mut child = Command::new("nix-store")
                .arg("--dump")
                .arg("--")
                .arg(&full_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?;

            let mut stdout = child.stdout.take().expect("stdout is piped");

            // This size is arbitrary. We read in "large enough" chunks.
            let mut buf = vec![0u8; 16 << 20];

            loop {
                let n = stdout.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                yield buf[..n].to_vec();
            }

            let status = child.wait().await?;
            if !status.success() {
                Err(std::io::Error::other(
                    format!("nix-store --dump exited with {status}"),
                ))?;
            }
        })
    }

    /// Returns the closure of a valid path.
    ///
    /// If `flip_directions` is true, the set of paths that can reach `store_path` is
    /// returned.
    pub async fn compute_fs_closure(
        &self,
        store_path: StorePath,
        include_outputs: bool,
    ) -> AtticResult<Vec<StorePath>> {
        self.compute_fs_closure_multi(vec![store_path], include_outputs)
            .await
    }

    /// Returns the closure of a set of valid paths.
    ///
    /// This is the multi-path variant of `compute_fs_closure`.
    pub async fn compute_fs_closure_multi(
        &self,
        store_paths: Vec<StorePath>,
        include_outputs: bool,
    ) -> AtticResult<Vec<StorePath>> {
        let to_store_path = |p: StorePath| self.store_dir().join(p.base_name);

        let child = Command::new("nix-store")
            .arg("--query")
            .arg("--requisites")
            .args(include_outputs.then_some("--include-outputs"))
            .arg("--")
            .args(store_paths.into_iter().map(to_store_path))
            .output()
            .await?;

        if !child.status.success() {
            return Err(std::io::Error::other(format!(
                "nix-store exited with {}",
                child.status
            )))?;
        }

        // TODO Better error handling
        let output = str::from_utf8(&child.stdout).map_err(|_e| AtticError::InvalidStorePath {
            path: Default::default(),
            reason: "Invalid UTF-8 output from nix-store",
        })?;

        let paths: Vec<StorePath> = output
            .lines()
            .map(|l| -> AtticResult<StorePath> { self.parse_store_path(l) })
            .collect::<AtticResult<Vec<_>>>()?;

        Ok(paths)
    }

    /// Returns detailed information on a path.
    pub async fn query_path_info(&self, store_path: StorePath) -> AtticResult<ValidPathInfo> {
        let full_store_path = self.store_dir().join(&store_path.base_name);
        let child = Command::new("nix")
            .arg("--experimental-features")
            .arg("nix-command")
            .arg("path-info")
            .arg("--json")
            .arg("--")
            .arg(&full_store_path)
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !child.status.success() {
            return Err(std::io::Error::other(format!(
                "nix path-info {} exited with {}: {}",
                full_store_path.display(),
                child.status,
                str::from_utf8(&child.stderr).unwrap_or("(invalid UTF-8)")
            )))?;
        }

        //eprintln!("{}", str::from_utf8(&child.stdout).unwrap());

        let path_info: serde_json::Value = serde_json::from_slice(&child.stdout)?;

        // We have three cases here depending on the Nix version! This is kind
        // of ugly, because we tried to be Nix version agnostic.
        //
        // TODO Find a better way to handle this.
        //
        // Either:
        // 1. The output is a single object (e.g. `{"path": ...}`)
        // 2. The output is an array of objects (e.g. `[{"path": ...}]`)
        // 3. The output is a JSON object with a single key (e.g. `{"/nix/store/...": {"path": ...}}`)

        let path_info: NixPathInfoJson = if path_info.is_array() {
            serde_json::from_value(path_info[0].clone())?
        } else if path_info.is_object() {
            let key = path_info.as_object().unwrap().keys().next().unwrap();
            let mut path_info: NixPathInfoJson = serde_json::from_value(path_info[key].clone())?;
            path_info.path = Some(key.clone().into());
            path_info
        } else {
            serde_json::from_value(path_info)?
        };

        Ok(ValidPathInfo {
            path: self.parse_store_path(path_info.path.unwrap())?,
            nar_hash: Hash::from_sri(&path_info.nar_hash)?,
            nar_size: path_info.nar_size,
            references: path_info
                .references
                .into_iter()
                .map(|p| -> AtticResult<PathBuf> { Ok(self.parse_store_path(p)?.base_name) })
                .collect::<AtticResult<Vec<_>>>()?,
            sigs: path_info.signatures,

            // TODO Remove the ca field across the codebase.
            ca: None,
        })
    }
}
