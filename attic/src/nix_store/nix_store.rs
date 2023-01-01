//! High-level Nix Store interface.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::task::spawn_blocking;

use super::bindings::{open_nix_store, AsyncWriteAdapter, FfiNixStore};
use super::{to_base_name, StorePath, ValidPathInfo};
use crate::error::AtticResult;

/// High-level wrapper for the Unix Domain Socket Nix Store.
pub struct NixStore {
    /// The Nix store FFI.
    inner: Arc<FfiNixStore>,

    /// Path to the Nix store itself.
    store_dir: PathBuf,
}

#[cfg(feature = "nix_store")]
impl NixStore {
    pub fn connect() -> AtticResult<Self> {
        #[allow(unsafe_code)]
        let inner = unsafe { open_nix_store()? };
        let store_dir = PathBuf::from(inner.store().store_dir());

        Ok(Self {
            inner: Arc::new(inner),
            store_dir,
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
    pub fn nar_from_path(&self, store_path: StorePath) -> AsyncWriteAdapter {
        let inner = self.inner.clone();
        let (adapter, mut sender) = AsyncWriteAdapter::new();
        let base_name = Vec::from(store_path.as_base_name_bytes());

        spawn_blocking(move || {
            // Send all exceptions through the channel, and ignore errors
            // during sending (the channel may have been closed).
            if let Err(e) = inner.store().nar_from_path(base_name, sender.clone()) {
                let _ = sender.rust_error(e);
            }
        });

        adapter
    }

    /// Returns the closure of a valid path.
    ///
    /// If `flip_directions` is true, the set of paths that can reach `store_path` is
    /// returned.
    pub async fn compute_fs_closure(
        &self,
        store_path: StorePath,
        flip_directions: bool,
        include_outputs: bool,
        include_derivers: bool,
    ) -> AtticResult<Vec<StorePath>> {
        let inner = self.inner.clone();

        spawn_blocking(move || {
            let base_name = store_path.as_base_name_bytes();

            let cxx_vector = inner.store().compute_fs_closure(
                base_name,
                flip_directions,
                include_outputs,
                include_derivers,
            )?;

            Ok(cxx_vector
                .iter()
                .map(|s| {
                    let osstr = OsStr::from_bytes(s.as_bytes());
                    let pb = PathBuf::from(osstr);

                    // Safety: The C++ implementation already checks the StorePath
                    // for correct format (which also implies valid UTF-8)
                    #[allow(unsafe_code)]
                    unsafe {
                        StorePath::from_base_name_unchecked(pb)
                    }
                })
                .collect())
        })
        .await
        .unwrap()
    }

    /// Returns the closure of a set of valid paths.
    ///
    /// This is the multi-path variant of `compute_fs_closure`.
    /// If `flip_directions` is true, the set of paths that can reach `store_path` is
    /// returned.
    pub async fn compute_fs_closure_multi(
        &self,
        store_paths: Vec<StorePath>,
        flip_directions: bool,
        include_outputs: bool,
        include_derivers: bool,
    ) -> AtticResult<Vec<StorePath>> {
        let inner = self.inner.clone();

        spawn_blocking(move || {
            let plain_base_names: Vec<&[u8]> = store_paths
                .iter()
                .map(|sp| sp.as_base_name_bytes())
                .collect();

            let cxx_vector = inner.store().compute_fs_closure_multi(
                &plain_base_names,
                flip_directions,
                include_outputs,
                include_derivers,
            )?;

            Ok(cxx_vector
                .iter()
                .map(|s| {
                    let osstr = OsStr::from_bytes(s.as_bytes());
                    let pb = PathBuf::from(osstr);

                    // Safety: The C++ implementation already checks the StorePath
                    // for correct format (which also implies valid UTF-8)
                    #[allow(unsafe_code)]
                    unsafe {
                        StorePath::from_base_name_unchecked(pb)
                    }
                })
                .collect())
        })
        .await
        .unwrap()
    }

    /// Returns detailed information on a path.
    pub async fn query_path_info(&self, store_path: StorePath) -> AtticResult<ValidPathInfo> {
        let inner = self.inner.clone();

        spawn_blocking(move || {
            let base_name = store_path.as_base_name_bytes();
            let mut c_path_info = inner.store().query_path_info(base_name)?;

            // FIXME: Make this more ergonomic and efficient
            let nar_size = c_path_info.pin_mut().nar_size();
            let nar_hash = c_path_info.pin_mut().nar_hash();
            let references = c_path_info
                .pin_mut()
                .references()
                .iter()
                .map(|s| {
                    let osstr = OsStr::from_bytes(s.as_bytes());
                    PathBuf::from(osstr)
                })
                .collect();
            let sigs = c_path_info
                .pin_mut()
                .sigs()
                .iter()
                .map(|s| {
                    let osstr = OsStr::from_bytes(s.as_bytes());
                    osstr.to_str().unwrap().to_string()
                })
                .collect();
            let ca = c_path_info.pin_mut().ca();

            Ok(ValidPathInfo {
                path: store_path,
                nar_size,
                nar_hash: nar_hash.into_rust()?,
                references,
                sigs,
                ca: if ca.is_empty() { None } else { Some(ca) },
            })
        })
        .await
        .unwrap()
    }
}
