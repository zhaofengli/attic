//! Shadow Nix store.
//!
//! Since Nix 2.0, Nix can use an alternative root for the store via
//! `--store` while keeping the same `storeDir`. To test pulling from
//! an Attic server with vanilla Nix, we create a temporary root
//! for the store, as well as `nix.conf` and `netrc` configurations
//! required to connect to an Attic server.
//!
//! ## Manual example
//!
//! ```bash
//! NIX_CONF_DIR="$SHADOW/etc/nix" NIX_USER_CONF_FILES="" NIX_REMOTE="" \
//!     nix-store --store "$SHADOW" -r /nix/store/h8fxhm945jlsfxlr4rvkkqlws771l07c-nix-2.7pre20220127_558c4ee -v
//! ```
//!
//! `nix.conf`:
//!
//! ```text
//! substituters = http://localhost:8080/attic-test
//! trusted-public-keys = attic-test:KmfKk/KwUscRJ8obZd4w6LgaqHZcn6uhfh7FYW02DzA=
//! ```
//!
//! `netrc`:
//!
//! ```text
//! machine localhost password eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyLCJleHAiOjQwNzA5MDg4MDAsImh0dHBzOi8vemhhb2ZlbmdsaS5naXRodWIuaW8vYXR0aWMiOnsieC1hdHRpYy1hY2Nlc3MiOnsiY2FjaGVzIjp7IioiOnsicHVzaCI6dHJ1ZSwicHVsbCI6dHJ1ZX19fX19.58WIuL8H_fQGEPmUG7U61FUHtAmsHXanYtQFSgqni6U
//! ```

use std::ffi::OsString;
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use tempfile::{Builder as TempfileBuilder, TempDir};

const WRAPPER_TEMPLATE: &str = include_str!("nix-wrapper.sh");

/// A shadow Nix store.
///
/// After creation, wrappers of common Nix executables will be
/// available under `bin`, allowing you to easily interact with
/// the shadow store.
pub struct ShadowStore {
    store_root: TempDir,
}

impl ShadowStore {
    pub fn new() -> Self {
        let store_root = TempfileBuilder::new()
            .prefix("shadow-store-")
            .tempdir()
            .expect("failed to create temporary root");

        fs::create_dir_all(store_root.path().join("etc/nix"))
            .expect("failed to create temporary config dir");

        fs::create_dir_all(store_root.path().join("bin"))
            .expect("failed to create temporary wrapper dir");

        let store = Self { store_root };
        store.create_wrapper("nix-store");

        store
    }

    /// Returns the path to the store root.
    pub fn path(&self) -> &Path {
        self.store_root.path()
    }

    /// Returns the path to the `nix-store` wrapper.
    pub fn nix_store_cmd(&self) -> OsString {
        self.store_root
            .path()
            .join("bin/nix-store")
            .as_os_str()
            .to_owned()
    }

    /// Creates a wrapper script for a Nix command.
    fn create_wrapper(&self, command: &str) {
        let path = self.store_root.path().join("bin").join(command);
        let permissions = Permissions::from_mode(0o755);
        let wrapper = WRAPPER_TEMPLATE
            .replace("%command%", command)
            .replace("%store_root%", &self.store_root.path().to_string_lossy());

        fs::write(&path, wrapper).expect("failed to write wrapper script");

        fs::set_permissions(&path, permissions).expect("failed to set wrapper permissions");
    }
}

impl Drop for ShadowStore {
    fn drop(&mut self) {
        // recursively set write permissions on directories so we can
        // cleanly delete the entire store

        fn walk(dir: &Path) {
            // excuse the unwraps
            let metadata = fs::metadata(dir).unwrap();
            let mut permissions = metadata.permissions();
            permissions.set_mode(permissions.mode() | 0o200);
            fs::set_permissions(dir, permissions).unwrap();

            for entry in fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();

                if entry.file_type().unwrap().is_dir() {
                    walk(&entry.path());
                }
            }
        }

        walk(self.store_root.path());
    }
}
