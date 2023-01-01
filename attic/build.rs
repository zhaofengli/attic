//! Build script.
//!
//! We link against libnixstore to perform actions on the Nix Store.

#[cfg(feature = "nix_store")]
use bindgen::callbacks::{EnumVariantValue, ParseCallbacks};

fn main() {
    #[cfg(feature = "nix_store")]
    build_bridge();

    #[cfg(feature = "nix_store")]
    run_bindgen();
}

#[cfg(feature = "nix_store")]
#[derive(Debug)]
struct TransformNix;

#[cfg(feature = "nix_store")]
impl ParseCallbacks for TransformNix {
    fn enum_variant_name(
        &self,
        enum_name: Option<&str>,
        original_variant_name: &str,
        _variant_value: EnumVariantValue,
    ) -> Option<String> {
        match enum_name {
            Some("HashType") => {
                let t = match original_variant_name {
                    "htUnknown" => "Unknown",
                    "htMD5" => "Md5",
                    "htSHA1" => "Sha1",
                    "htSHA256" => "Sha256",
                    "htSHA512" => "Sha512",
                    x => panic!("Unknown hash type {} - Add it in build.rs", x),
                };
                Some(t.to_owned())
            }
            _ => None,
        }
    }

    fn include_file(&self, filename: &str) {
        println!("cargo:rerun-if-changed={}", filename);
    }
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
}

#[cfg(feature = "nix_store")]
fn run_bindgen() {
    use std::env;
    use std::path::PathBuf;

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());

    let headers = vec!["src/nix_store/bindings/bindgen.hpp"];

    let mut builder = bindgen::Builder::default()
        .clang_arg("-std=c++17")
        .clang_arg("-include")
        .clang_arg("nix/config.h")
        .opaque_type("std::.*")
        .allowlist_type("nix::Hash")
        .rustified_enum("nix::HashType")
        .disable_name_namespacing()
        .layout_tests(false)
        .parse_callbacks(Box::new(TransformNix));

    for header in headers {
        builder = builder.header(header);
        println!("cargo:rerun-if-changed={}", header);
    }

    let bindings = builder.generate().expect("Failed to generate Nix bindings");

    bindings
        .write_to_file(out_path.join("bindgen.rs"))
        .expect("Failed to write bindings");

    // the -l flags must be after -lnixbinding
    pkg_config::Config::new()
        .atleast_version("2.4")
        .probe("nix-store")
        .unwrap();
}
