//! Build script.
//!
//! We link against libnixstore to perform actions on the Nix Store.

fn main() {
    #[cfg(feature = "nix_store")]
    nix_store::build_bridge();
}

#[cfg(feature = "nix_store")]
mod nix_store {
    use cc::Build;
    use system_deps::Dependencies;
    use version_compare::{Part, Version};

    fn apply_variant_flags(build: &mut Build, deps: &Dependencies) {
        let nix_main = deps
            .get_by_name("nix-main")
            .expect("Failed to get version of nix-main");
        let version = Version::from(&nix_main.version).unwrap();

        build.define("ATTIC_VARIANT_NIX", None);

        let (major, minor) = match (version.part(0), version.part(1)) {
            (Ok(Part::Number(major)), Ok(Part::Number(minor))) if minor < 100 => (major, minor),
            _ => panic!("Nix version {version} is not supported"),
        };

        let version = major * 100 + minor;
        build.define("NIX_VERSION", &*format!("{version}"));
    }

    pub fn build_bridge() {
        let deps = system_deps::Config::new().probe().unwrap();

        let mut build = cxx_build::bridge("src/nix_store/bindings/mod.rs");
        build
            .file("src/nix_store/bindings/nix.cpp")
            .flag("-std=c++2a")
            .flag("-O2")
            .includes(deps.all_include_paths());

        apply_variant_flags(&mut build, &deps);

        build.compile("nixbinding");

        println!("cargo:rerun-if-changed=src/nix_store/bindings");

        // the -l flags must be after -lnixbinding

        // HACK: system_deps emits -lnixmain before cc emits -lnixbinding
        system_deps::Config::new().probe().unwrap();
    }
}
