//! Build script.
//!
//! We link against libnixstore to perform actions on the Nix Store.

fn main() {
    #[cfg(feature = "nix_store")]
    nix_store::build_bridge();
}

#[cfg(feature = "nix_store")]
mod nix_store {
    use version_compare::Version;

    pub fn build_bridge() {
        let deps = system_deps::Config::new().probe().unwrap();

        let mut build = cxx_build::bridge("src/nix_store/bindings/mod.rs");
        build
            .file("src/nix_store/bindings/nix.cpp")
            .flag("-std=c++2a")
            .flag("-O2")
            .flag("-include")
            .flag("nix/config.h")
            .includes(deps.all_include_paths());

        build.compile("nixbinding");

        println!("cargo:rerun-if-changed=src/nix_store/bindings");

        // the -l flags must be after -lnixbinding

        // HACK: system_deps emits -lnixmain before cc emits -lnixbinding
        system_deps::Config::new().probe().unwrap();
    }
}
