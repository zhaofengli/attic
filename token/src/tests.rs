use super::*;

use attic::cache::CacheName;

macro_rules! cache {
    ($n:expr) => {
        CacheName::new($n.to_string()).unwrap()
    };
}

#[test]
fn test_basic() {
    /*
    $ cat json
      {
        "sub": "meow",
        "exp": 4102324986,
        "nbf": 0,
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
    // nix shell nixpkgs#jwt-cli
    // openssl genpkey -out rs256 -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -outform der
    // BASE64_SECRET=$(openssl rsa -in rs256 -outform PEM -traditional | base64 -w0)
    let base64_secret = "LS0tLS1CRUdJTiBSU0EgUFJJVkFURSBLRVktLS0tLQpNSUlFcEFJQkFBS0NBUUVBNUZranRMRzV5eS9pMFlnYkQxeUJBK21GckNmLzZiQ2F0TDFFQ3ppNG1tZWhSZTcwCkFEL0dSSHhTVUErc0pZeCtZNjlyL0RqQWs2OFJlQ1c4b2FQWXhtc21RNG5VM2ZwZ2E3WWFqZ3ZoWmVsa3JtaC8KZ1ZURWtFTG1IZlJtQkwvOWlsT20yRHNtYTVhUFo0SFl6ellpdjJvcFF5UGRndXcyWXFtbzE3Nk5MdllCMmpJTwovR3FkdE55K3NPV296NktVSVlJa0hWWU5HMENVcFNzdXBqUTJ6VTVZMFc2UXlNQWFWd1BONElJT3lXWUNwZXRECjFJbWxYekhROXM4NXFSWnlLa21iZFhtTVBVWmUvekRxc2FFd3lscFlpT0RjbDdRYU5QTzEzZnk3UGtQMmVwdUkKTk5tZ1E0WEF0MkF4ZXNKck5ibUs4aG1iM3doRXZkNjRFMGdEV1FJREFRQUJBb0lCQUJEemNRd2IyVi8wK1JCMgoyeE5qMll2eHpPTi93S2FYWHBTbUxDUHRIUDhSVEU2RnM0VkZOckdrelBOMmhsL3ZNdjZ4YWdHNk1NbUZ5SFV6CnovSHIyTTY1NjRnOTloaFlXc29FSmFwL3hVYXNjYlhrdWZwZTBZeW4rcThra21JdDRtTmZYRlpXNWI0ODJmNWsKRERVdG5weTVBOEVoSzNOcGw0dnhia0E5dS90TlVlT1NHTkhPYVZjcHdERVhDNXJ4bmFxTm5wMkMwa1A4ODRINgpSb2lZVkF4bytHaVpNVzhIOFRmSXVsenh3c04yQnVNcUNmOGVhNG1EM0pRVHZ2REhhUHM4eVJTUlB3UmlHYUkzCnVybFRmdjg4U20va09oL0N2SkpoRnhCVkVNVjIydWRNUmU3L3NpTWtlbVlvUnhaTWJjRGVQK2h1RktJWTRSMEoKNnRJUHQ3VUNnWUVBOTlhL2IzeFBsQWh0ck02dUlUUXNQd0FYQUg3Q1NXL1FSdVJUTWVhYXVIMk9sRitjZmpMNApJS1Nsdy9QaUtaUEk1TFRWM2ZVZk5WNTVsOFZHTytsT2ViTFhnaXBYM3BqSDBma3AyY3Q2Smk3aGw0aUlXK0h0ClpJNE9KYkYwTTBETHdySkd3T25QL2trRHNxSW9IbC9MdTBRM2FxSm1RVCsvcG54R083R21kbDhDZ1lFQTY5NFcKZHF2NnF4VjF5V0Z4QWZOOE1hZStpTC9xY1VhTm85ZzMva2YvOXZ3VXdtcERvR0xnaVVLMWZKb3BUYlBjcWgwRwptbUZEQ3V2M1Q0OS9yU2k5dU4zYm82cmlXRUl4VFg1YUtFSjlpSEFMWDJGWDdGSDJRdUZGWEwzQ2c0ckdvL1pDCmdjUkxuS3dma3JUVnRxeEdaNjN4YmsvcFpHWjZtTW01VkNDck1VY0NnWUVBc3JUT1pQMG1CSC92VldQU2UyNjcKV05JZncrT2pCSUR6bGFxZHNxV3Rlc3BPUFA2VVFRdFBqM29wYlJvMlFmU21Md09XRXUzbEN2Nk1mcnRvNFZwaAprNjg1WmtwU0FkZjRmWmRFYmg4aWZOWGhKUHIyR0FyWXVtRVVJbW5LZUFxSTRtTGFVZEJHZ2Z6MEJhS1hldzlvClFDZjRMWlBjVjhBMzJUeFRDRWdZMTlFQ2dZQU04U2F5WkVWZzFkQ2N1Q2dIUDJEMUtJc2YzY2Z6WnplbVlkclEKclFxeWRxcDg4Rys5Z1M5bzJLdzBwaERXSHFSaEFTNjNrZGFuNXNLdkx1U0dqOUc1THhNNks4bzNwWW9uQW1QWQpDYTN4cXBRMUs1WXpkVnZaMTVxQ3VEYlFHUEZGVmVIWVZQa0JJOENud0J4cDVaSUhabGYxQVpXQTJNNnBTNGhMCndXOGpTUUtCZ1FDQmNJbjU4Y0lmZkhmMjM4SUJvZnR1UVVzREZGcnkzaUVpaWpTYmJ1WnB1Vm8zL2pWbUsyaEYKS2xUL2xoRDdWdGJ1V3phMG9WQmZDaWZqMnZ2S2pmZ0l6NnF3Um1UbC9DSjlWdUNHTUI1VG55cGl3OEtodXorSAo0L2twdDdNcW9WQ0dRSjd1WVQyQzY1K0JqNklnUnBQT09za3VKNW1RZ0FlbTQ3eDBrVnRSemc9PQotLS0tLUVORCBSU0EgUFJJVkFURSBLRVktLS0tLQo=";

    let dec_key = decode_token_rs256_secret_base64(base64_secret).unwrap();

    // TOKEN=$(jq -c < json | jwt encode --alg RS256 --secret @./rs256 -)
    let token = "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.eyJleHAiOjQxMDIzMjQ5ODYsImh0dHBzOi8vand0LmF0dGljLnJzL3YxIjp7ImNhY2hlcyI6eyJhbGwtKiI6eyJyIjoxfSwiYWxsLWNpLSoiOnsidyI6MX0sImNhY2hlLXJvIjp7InIiOjF9LCJjYWNoZS1ydyI6eyJyIjoxLCJ3IjoxfSwidGVhbS0qIjp7ImNjIjoxLCJyIjoxLCJ3IjoxfX19LCJpYXQiOjE3MjIwMDUwNzksIm5iZiI6MCwic3ViIjoibWVvdyJ9.Zs24IUbQOpOjhEe0sfsoSSJhDrzf4v-_wX_ceKqHeb2MERY8XSIQ1RPTNVeOW4LfJHumJj_rxh8Wv2BRGZSMldrTt0Ab_N7FnkhA37_jnRvgvEjSG3V4fC8aA4KoOa-43NRpg4HmPxiXte5-6LneBOR94Wss868wC1b_2yX2zCc1wQoZA3LNo-CRLnL4Yp5wY4Bbgyguv_9mfqXVYZykZnxumyGwVFD-Rub3KQ9d53Rf9tKcvRk9qxO2q8F2PKjeaUBG2xZtGwkWTMvSmwR1dKtkPUyPggOzbLoUG-6fxfo7D3NyL5qWCSN_7CkI-xlsRSLY1gTq-FqXvcpHeZbc8w";

    // NOTE(cole-h): check that we get a consistent iteration order when getting permissions for
    // caches -- this depends on the order of the fields in the token, but should otherwise be
    // consistent between iterations
    let mut was_ever_wrong = false;
    for _ in 0..=1_000 {
        // NOTE(cole-h): we construct a new Token every iteration in order to get different "random
        // state"
        let decoded =
            Token::from_jwt(token, &SignatureType::RS256(dec_key.clone()), &None, &None).unwrap();
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

    let decoded = Token::from_jwt(token, &SignatureType::RS256(dec_key), &None, &None).unwrap();

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
