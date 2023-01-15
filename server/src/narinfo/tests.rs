use super::*;

use std::path::Path;

use attic::signing::NixPublicKey;

#[test]
fn test_basic() {
    let s = r#"
StorePath: /nix/store/xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10
URL: nar/0nqgf15qfiacfxrgm2wkw0gwwncjqqzzalj8rs14w9srkydkjsk9.nar.xz
Compression: xz
FileHash: sha256:0nqgf15qfiacfxrgm2wkw0gwwncjqqzzalj8rs14w9srkydkjsk9
FileSize: 41104
NarHash: sha256:16mvl7v0ylzcg2n3xzjn41qhzbmgcn5iyarx16nn5l2r36n2kqci
NarSize: 206104
References: 563528481rvhc5kxwipjmg6rqrl95mdx-glibc-2.33-56 xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10
Deriver: vvb4wxmnjixmrkhmj2xb75z62hrr41i7-hello-2.10.drv
Sig: cache.nixos.org-1:lo9EfNIL4eGRuNh7DTbAAffWPpI2SlYC/8uP7JnhgmfRIUNGhSbFe8qEaKN0mFS02TuhPpXFPNtRkFcCp0hGAQ==
    "#;

    let narinfo = NarInfo::from_str(s).expect("Could not parse narinfo");

    fn verify_narinfo(narinfo: &NarInfo) {
        assert_eq!(
            Path::new("/nix/store/xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10"),
            narinfo.store_path
        );
        assert_eq!(Path::new("/nix/store"), narinfo.store_dir());
        assert_eq!(
            "nar/0nqgf15qfiacfxrgm2wkw0gwwncjqqzzalj8rs14w9srkydkjsk9.nar.xz",
            narinfo.url
        );
        assert_eq!(Compression::Xz, narinfo.compression);
        assert_eq!(
            "sha256:0nqgf15qfiacfxrgm2wkw0gwwncjqqzzalj8rs14w9srkydkjsk9",
            narinfo.file_hash.as_ref().unwrap().to_typed_base32()
        );
        assert_eq!(Some(41104), narinfo.file_size);
        assert_eq!(
            "sha256:16mvl7v0ylzcg2n3xzjn41qhzbmgcn5iyarx16nn5l2r36n2kqci",
            narinfo.nar_hash.to_typed_base32()
        );
        assert_eq!(206104, narinfo.nar_size);
        assert_eq!(
            vec![
                "563528481rvhc5kxwipjmg6rqrl95mdx-glibc-2.33-56",
                "xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10",
            ],
            narinfo.references
        );
        assert_eq!(
            Some("vvb4wxmnjixmrkhmj2xb75z62hrr41i7-hello-2.10.drv".to_string()),
            narinfo.deriver
        );
        assert_eq!(Some("cache.nixos.org-1:lo9EfNIL4eGRuNh7DTbAAffWPpI2SlYC/8uP7JnhgmfRIUNGhSbFe8qEaKN0mFS02TuhPpXFPNtRkFcCp0hGAQ==".to_string()), narinfo.signature);
    }

    verify_narinfo(&narinfo);

    let round_trip = narinfo.to_string().expect("Could not serialize narinfo");

    eprintln!("{}", round_trip);

    let reparse = NarInfo::from_str(&round_trip).expect("Could not re-parse serialized narinfo");

    verify_narinfo(&reparse);
}

#[test]
fn test_deriver() {
    let s = r#"
StorePath: /nix/store/xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10
URL: nar/0nqgf15qfiacfxrgm2wkw0gwwncjqqzzalj8rs14w9srkydkjsk9.nar.xz
Compression: xz
FileHash: sha256:0nqgf15qfiacfxrgm2wkw0gwwncjqqzzalj8rs14w9srkydkjsk9
FileSize: 41104
NarHash: sha256:16mvl7v0ylzcg2n3xzjn41qhzbmgcn5iyarx16nn5l2r36n2kqci
NarSize: 206104
References: 563528481rvhc5kxwipjmg6rqrl95mdx-glibc-2.33-56 xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10
Deriver: unknown-deriver
    "#;

    let narinfo = NarInfo::from_str(s).expect("Could not parse narinfo");

    assert_eq!(None, narinfo.deriver);
}

#[test]
fn test_fingerprint() {
    let s = r#"
StorePath: /nix/store/xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10
URL: nar/0nqgf15qfiacfxrgm2wkw0gwwncjqqzzalj8rs14w9srkydkjsk9.nar.xz
Compression: xz
FileHash: sha256:0nqgf15qfiacfxrgm2wkw0gwwncjqqzzalj8rs14w9srkydkjsk9
FileSize: 41104
NarHash: sha256:91e129ac1959d062ad093d2b1f8b65afae0f712056fe3eac78ec530ff6a1bb9a
NarSize: 206104
References: 563528481rvhc5kxwipjmg6rqrl95mdx-glibc-2.33-56 xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10
Deriver: vvb4wxmnjixmrkhmj2xb75z62hrr41i7-hello-2.10.drv
Sig: cache.nixos.org-1:lo9EfNIL4eGRuNh7DTbAAffWPpI2SlYC/8uP7JnhgmfRIUNGhSbFe8qEaKN0mFS02TuhPpXFPNtRkFcCp0hGAQ==
    "#;

    let correct_fingerprint = b"1;/nix/store/xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10;sha256:16mvl7v0ylzcg2n3xzjn41qhzbmgcn5iyarx16nn5l2r36n2kqci;206104;/nix/store/563528481rvhc5kxwipjmg6rqrl95mdx-glibc-2.33-56,/nix/store/xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10";

    let public_key =
        NixPublicKey::from_str("cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=")
            .expect("Could not import cache.nixos.org public key");

    let narinfo = NarInfo::from_str(s).expect("Could not parse narinfo");

    let fingerprint = narinfo.fingerprint();

    eprintln!(
        "Expected: {}",
        String::from_utf8(correct_fingerprint.to_vec()).unwrap()
    );
    eprintln!(
        "  Actual: {}",
        String::from_utf8(fingerprint.clone()).unwrap()
    );

    assert_eq!(correct_fingerprint, fingerprint.as_slice());

    public_key
        .verify(&narinfo.fingerprint(), narinfo.signature().unwrap())
        .expect("Could not verify signature");
}
