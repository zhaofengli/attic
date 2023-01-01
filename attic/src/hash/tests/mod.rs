use super::*;

use crate::error::AtticError;
use crate::nix_store::tests::test_nar;

const BLOB: &[u8] = include_bytes!("blob");

#[test]
fn test_basic() {
    let hash = Hash::sha256_from_bytes(BLOB);

    let expected_base16 = "sha256:df3404eaf1481506db9ca155e0a871d5b4d22e62a96961e8bf4ad1a8ca525330";
    assert_eq!(expected_base16, hash.to_typed_base16());

    let expected_base32 = "sha256:0c2kab5ailaapzl62sd9c8pd5d6mf6lf0md1kkdhc5a8y7m08d6z";
    assert_eq!(expected_base32, hash.to_typed_base32());
}

#[test]
fn test_nar_hash() {
    let nar = test_nar::NO_DEPS;
    let hash = Hash::sha256_from_bytes(nar.nar());

    let expected_base32 = "sha256:0hjszid30ak3rkzvc3m94c3risg8wz2hayy100c1fg92bjvvvsms";
    assert_eq!(expected_base32, hash.to_typed_base32());
}

#[test]
fn test_from_typed() {
    let base16 = "sha256:baeabdb75c223d171800c17b05c5e7e8e9980723a90eb6ffcc632a305afc5a42";
    let base32 = "sha256:0hjszid30ak3rkzvc3m94c3risg8wz2hayy100c1fg92bjvvvsms";

    assert_eq!(
        Hash::from_typed(base16).unwrap(),
        Hash::from_typed(base32).unwrap()
    );

    assert!(matches!(
        Hash::from_typed("sha256"),
        Err(AtticError::HashError(Error::NoColonSeparator))
    ));

    assert!(matches!(
        Hash::from_typed("sha256:"),
        Err(AtticError::HashError(Error::InvalidHashStringLength { .. }))
    ));

    assert!(matches!(
        Hash::from_typed("sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
        Err(AtticError::HashError(Error::InvalidBase32Hash))
    ));

    assert!(matches!(
        Hash::from_typed("sha256:gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg"),
        Err(AtticError::HashError(Error::InvalidBase16Hash(_)))
    ));

    assert!(matches!(
        Hash::from_typed("md5:invalid"),
        Err(AtticError::HashError(Error::UnsupportedHashAlgorithm(alg))) if alg == "md5"
    ));
}
