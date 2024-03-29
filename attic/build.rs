//! Build script.
//!
//! We link against libnixstore to perform actions on the Nix Store.

fn main() {
    #[cfg(feature = "nix_store")]
    build_bridge();
}

#[cfg(feature = "nix_store")]
fn build_bridge() {
    // Temporary workaround for issue in <https://github.com/NixOS/nix/pull/8484>
    let hacky_include = {
        let dir = tempfile::tempdir().expect("Failed to create temporary directory for workaround");
        std::fs::write(dir.path().join("uds-remote-store.md"), "\"\"").unwrap();
        dir
    };

    cxx_build::bridge("src/nix_store/bindings/mod.rs")
        .file("src/nix_store/bindings/nix.cpp")
        .flag("-std=c++2a")
        .flag("-O2")
        .flag("-include")
        .flag("nix/config.h")
        .flag("-idirafter")
        .flag(hacky_include.path().to_str().unwrap())
        // In Nix 2.19+, nix/args/root.hh depends on being able to #include "root.hh" (which is in its parent directory), for some reason
        .flag("-I")
        .flag(concat!(env!("NIX_INCLUDE_PATH"), "/nix"))
        .compile("nixbinding");

    println!("cargo:rerun-if-changed=src/nix_store/bindings");

    // the -l flags must be after -lnixbinding
    pkg_config::Config::new()
        .atleast_version("2.4")
        .probe("nix-store")
        .unwrap();

    pkg_config::Config::new()
        .atleast_version("2.4")
        .probe("nix-main")
        .unwrap();
}
