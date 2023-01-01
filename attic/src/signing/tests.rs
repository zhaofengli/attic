use super::*;

#[test]
fn test_generate_key() {
    let keypair = NixKeypair::generate("attic-test").expect("Could not generate key");

    let export_priv = keypair.export_keypair();
    let export_pub = keypair.export_public_key();

    eprintln!("Private key: {}", export_priv);
    eprintln!(" Public key: {}", export_pub);

    // re-import keypair
    let import = NixKeypair::from_str(&export_priv).expect("Could not re-import generated key");

    assert_eq!(keypair.name, import.name);
    assert_eq!(keypair.keypair, import.keypair);

    // re-import public key
    let import_pub = NixPublicKey::from_str(&export_pub).expect("Could not re-import public key");

    assert_eq!(keypair.name, import_pub.name);
    assert_eq!(keypair.keypair.pk, import_pub.public);

    // test the export functionality of NixPublicKey as well
    let export_pub2 = import_pub.export();
    let import_pub2 = NixPublicKey::from_str(&export_pub2).expect("Could not re-import public key");

    assert_eq!(keypair.name, import_pub2.name);
    assert_eq!(keypair.keypair.pk, import_pub2.public);
}

#[test]
fn test_serde() {
    let json = "\"attic-test:x326WFy/JUl+MQnN1u9NPdWQPBbcVn2mwoIqSLS3DmQqZ8qT8rBSxxEnyhtl3jDouBqodlyfq6F+HsVhbTYPMA==\"";

    let keypair: NixKeypair = serde_json::from_str(json).expect("Could not deserialize keypair");

    let export = serde_json::to_string(&keypair).expect("Could not serialize keypair");

    eprintln!("Public Key: {}", keypair.export_public_key());

    assert_eq!(json, &export);
}

#[test]
fn test_import_public_key() {
    let cache_nixos_org = "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=";
    let import = NixPublicKey::from_str(cache_nixos_org).expect("Could not import public key");

    assert_eq!(cache_nixos_org, import.export());
}

#[test]
fn test_signing() {
    let keypair = NixKeypair::generate("attic-test").expect("Could not generate key");

    let public = keypair.to_public_key();

    let message = b"hello world";

    let signature = keypair.sign(message);

    keypair.verify(message, &signature).unwrap();
    public.verify(message, &signature).unwrap();

    keypair.verify(message, "attic-test:lo9EfNIL4eGRuNh7DTbAAffWPpI2SlYC/8uP7JnhgmfRIUNGhSbFe8qEaKN0mFS02TuhPpXFPNtRkFcCp0hGAQ==").unwrap_err();
}
