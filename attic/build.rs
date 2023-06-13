//! Build script.
//!
//! We link against libnixstore to perform actions on the Nix Store.

fn main() {
    #[cfg(feature = "nix_store")]
    build_bridge();
}

#[cfg(feature = "nix_store")]
fn build_bridge() {
    cxx_build::bridge("src/nix_store/bindings/mod.rs")
        .file("src/nix_store/bindings/nix.cpp")
        .flag("-std=c++17")
        .flag("-O2")
        .flag("-include")
        .flag("nix/config.h")
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
