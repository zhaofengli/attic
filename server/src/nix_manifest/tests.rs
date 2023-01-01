use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A hypothetical manifest.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
struct HypotheticalManifest {
    #[serde(rename = "StoreDir")]
    store_dir: PathBuf,

    #[serde(rename = "WantMassQuery")]
    want_mass_query: bool,
}

#[test]
fn test_basic() {
    let manifest = r#"
StoreDir: /nix/store
WantMassQuery: 1
    "#;

    let expected = HypotheticalManifest {
        store_dir: PathBuf::from("/nix/store"),
        want_mass_query: true,
    };

    let parsed = super::from_str::<HypotheticalManifest>(manifest).unwrap();
    assert_eq!(parsed, expected);

    // TODO: Use the actual Nix parser to reparse the resulting manifest?
    let round_trip = super::to_string(&parsed).unwrap();

    // FIXME: This is pretty fragile. Just testing that it can be parsed again should
    // be enough.
    assert_eq!(manifest.trim(), round_trip.trim());

    let parsed2 = super::from_str::<HypotheticalManifest>(&round_trip).unwrap();
    assert_eq!(parsed2, expected);
}

#[test]
fn test_unquoted_number() {
    let manifest = r#"
StoreDir: 12345
WantMassQuery: 1
    "#;

    let expected = HypotheticalManifest {
        store_dir: PathBuf::from("12345"),
        want_mass_query: true,
    };

    let parsed = super::from_str::<HypotheticalManifest>(manifest).unwrap();
    assert_eq!(parsed, expected);
}
