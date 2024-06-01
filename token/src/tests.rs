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
            "all-*": {"r":1},
            "all-ci-*": {"w":1},
            "cache-rw": {"r":1,"w":1},
            "cache-ro": {"r":1},
            "team-*": {"r":1,"w":1,"cc":1}
          }
        }
      }
    */

    let token = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJleHAiOjQxMDIzMjQ5ODYsImh0dHBzOi8vand0LmF0dGljLnJzL3YxIjp7ImNhY2hlcyI6eyJhbGwtKiI6eyJyIjoxfSwiYWxsLWNpLSoiOnsidyI6MX0sImNhY2hlLXJvIjp7InIiOjF9LCJjYWNoZS1ydyI6eyJyIjoxLCJ3IjoxfSwidGVhbS0qIjp7ImNjIjoxLCJyIjoxLCJ3IjoxfX19LCJpYXQiOjE3MTY2NjA1ODksInN1YiI6Im1lb3cifQ.8vtxp_1OEYdcnkGPM4c9ORXooJZV7DOTS4NRkMKN8mw";

    // NOTE(cole-h): check that we get a consistent iteration order when getting permissions for
    // caches -- this depends on the order of the fields in the token, but should otherwise be
    // consistent between iterations
    let mut was_ever_wrong = false;
    for _ in 0..=1_000 {
        // NOTE(cole-h): we construct a new Token every iteration in order to get different "random
        // state"
        let decoded = Token::from_jwt(token, &dec_key).unwrap();
        let perm_all_ci = decoded.get_permission_for_cache(&cache! { "all-ci-abc" });

        // NOTE(cole-h): if the iteration order of the token is inconsistent, the permissions may be
        // retrieved from the `all-ci-*` pattern (which only allows writing/pushing), even though
        // the `all-*` pattern (which only allows reading/pulling) is specified first
        if perm_all_ci.require_pull().is_err() || perm_all_ci.require_push().is_ok() {
            was_ever_wrong = true;
        }
    }
    assert!(
        !was_ever_wrong,
        "Iteration order should be consistent to prevent random auth failures (and successes)"
    );

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
