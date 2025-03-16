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
    use version_compare::Version;

    struct NixDependency {
        version: String,
    }

    impl NixDependency {
        fn detect() -> Self {
            let library = pkg_config::Config::new()
                .cargo_metadata(false)
                .atleast_version("2.4")
                .probe("nix-main")
                .expect("Failed to find nix-main >=2.4 through pkg-config");

            Self {
                version: library.version,
            }
        }

        fn apply_version_flags(&self, build: &mut Build) {
            let version = Version::from(&self.version).unwrap();

            if version >= Version::from("2.20").unwrap() {
                build.flag("-DATTIC_NIX_2_20");
            }
        }

        fn emit_cargo_metadata(&self) {
            pkg_config::Config::new()
                .atleast_version("2.4")
                .probe("nix-store")
                .unwrap();

            pkg_config::Config::new()
                .atleast_version("2.4")
                .probe("nix-main")
                .unwrap();
        }
    }

    pub fn build_bridge() {
        let nix_dep = NixDependency::detect();

        let mut build = cxx_build::bridge("src/nix_store/bindings/mod.rs");
        build
            .file("src/nix_store/bindings/nix.cpp")
            .flag("-std=c++2a")
            .flag("-O2")
            .flag("-include")
            .flag("nix/config.h")
            // In Nix 2.19+, nix/args/root.hh depends on being able to #include "args.hh" (which is in its parent directory), for some reason
            .flag("-I")
            .flag(concat!(env!("NIX_INCLUDE_PATH"), "/nix"));

        nix_dep.apply_version_flags(&mut build);

        build.compile("nixbinding");

        println!("cargo:rerun-if-changed=src/nix_store/bindings");

        // the -l flags must be after -lnixbinding
        nix_dep.emit_cargo_metadata();
    }
}
