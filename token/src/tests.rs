use super::*;

use attic::cache::CacheName;

macro_rules! cache {
    ($n:expr) => {
        CacheName::new($n.to_string()).unwrap()
    };
}

#[test]
fn test_basic() {
    // "very secure secret"
    let base64_secret = "dmVyeSBzZWN1cmUgc2VjcmV0";

    let dec_key = decode_token_hs256_secret_base64(base64_secret).unwrap();

    /*
      {
        "sub": "meow",
        "exp": 4102324986,
        "https://jwt.attic.rs/v1": {
          "caches": {
            "cache-rw": {"r":1,"w":1},
            "cache-ro": {"r":1},
            "team-*": {"r":1,"w":1,"cc":1}
          }
        }
      }
    */

    let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJtZW93IiwiZXhwIjo0MTAyMzI0OTg2LCJodHRwczovL2p3dC5hdHRpYy5ycy92MSI6eyJjYWNoZXMiOnsiY2FjaGUtcnciOnsiciI6MSwidyI6MX0sImNhY2hlLXJvIjp7InIiOjF9LCJ0ZWFtLSoiOnsiciI6MSwidyI6MSwiY2MiOjF9fX19.UlsIM9bQHr9SXGAcSQcoVPo9No8Zhh6Y5xfX8vCmKmA";

    let decoded = Token::from_jwt(token, &dec_key).unwrap();

    let perm_rw = decoded.get_permission_for_cache(&cache! { "cache-rw" });

    assert!(perm_rw.pull);
    assert!(perm_rw.push);
    assert!(!perm_rw.delete);
    assert!(!perm_rw.create_cache);

    assert!(perm_rw.require_pull().is_ok());
    assert!(perm_rw.require_push().is_ok());
    assert!(perm_rw.require_delete().is_err());
    assert!(perm_rw.require_create_cache().is_err());

    let perm_ro = decoded.get_permission_for_cache(&cache! { "cache-ro" });

    assert!(perm_ro.pull);
    assert!(!perm_ro.push);
    assert!(!perm_ro.delete);
    assert!(!perm_ro.create_cache);

    assert!(perm_ro.require_pull().is_ok());
    assert!(perm_ro.require_push().is_err());
    assert!(perm_ro.require_delete().is_err());
    assert!(perm_ro.require_create_cache().is_err());

    let perm_team = decoded.get_permission_for_cache(&cache! { "team-xyz" });

    assert!(perm_team.pull);
    assert!(perm_team.push);
    assert!(!perm_team.delete);
    assert!(perm_team.create_cache);

    assert!(perm_team.require_pull().is_ok());
    assert!(perm_team.require_push().is_ok());
    assert!(perm_team.require_delete().is_err());
    assert!(perm_team.require_create_cache().is_ok());

    assert!(!decoded
        .get_permission_for_cache(&cache! { "forbidden-cache" })
        .can_discover());
}
