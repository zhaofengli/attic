//! Access control.
//!
//! Access control in Attic is simple and stateless [0] - The server validates
//! the JWT against the configured key and allows access based on the `https://jwt.attic.rs/v1`
//! claim.
//!
//! One primary goal of the Attic Server is easy scalability. It's designed
//! to be deployed to serverless platforms like fly.io and have fast
//! cold-start times. Instances are created and destoyed rapidly in response
//! to requests.
//!
//! [0] We may revisit this later :)
//!
//! ## Opaqueness
//!
//! The token format is unstable and claims beyond the standard ones defined
//! in RFC 7519 should never be interpreted by the client. The token might not
//! even be a valid JWT, in which case the client must not throw an error.
//!
//! ## Cache discovery
//!
//! If the JWT grants any permission at all to the requested cache name,
//! then the bearer is able to discover the presence of the cache, meaning
//! that NoSuchCache or Forbidden can be returned depending on the scenario.
//! Otherwise, the user will get a generic 401 response (Unauthorized)
//! regardless of the request (or whether the cache exists or not).
//!
//! ## Supplying the token
//!
//! The JWT can be supplied to the server in one of two ways:
//!
//! - As a normal Bearer token.
//! - As the password in Basic Auth (used by Nix). The username is ignored.
//!
//! To add the token to Nix, use the following format in `~/.config/nix/netrc`:
//!
//! ```text
//! machine attic.server.tld password eyJhb...
//! ```
//!
//! ## Example token
//!
//! ```json
//! {
//!   "sub": "meow",
//!   "exp": 4102324986,
//!   "https://jwt.attic.rs/v1": {
//!     "caches": {
//!       "cache-rw": {
//!         "w": 1,
//!         "r": 1
//!       },
//!       "cache-ro": {
//!         "r": 1
//!       },
//!       "team-*": {
//!         "w": 1,
//!         "r": 1,
//!         "cc": 1
//!       }
//!     }
//!   }
//! }
//! ```

#![deny(
    asm_sub_register,
    deprecated,
    missing_abi,
    unsafe_code,
    unused_macros,
    unused_must_use,
    unused_unsafe
)]
#![deny(clippy::from_over_into, clippy::needless_question_mark)]
#![cfg_attr(
    not(debug_assertions),
    deny(unused_imports, unused_mut, unused_variables)
)]

pub mod util;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::error::Error as StdError;

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use chrono::{DateTime, Utc};
use displaydoc::Display;
use indexmap::IndexMap;
use jwt_simple::prelude::{Duration, RSAKeyPairLike, RSAPublicKeyLike, VerificationOptions};
pub use jwt_simple::{
    algorithms::{HS256Key, MACLike, RS256KeyPair, RS256PublicKey},
    claims::{Claims, JWTClaims},
    prelude::UnixTimeStamp,
};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, BoolFromInt};

use attic::cache::{CacheName, CacheNamePattern};

/// Custom claim namespace for the AtticAccess information.
///
/// Custom claim namespaces are required by platforms like Auth0, and
/// custom claims without one will be silently dropped.
///
/// <https://auth0.com/docs/security/tokens/json-web-tokens/create-namespaced-custom-claims>
///
/// Also change the `#[serde(rename)]` below if you change this.
pub const CLAIM_NAMESPACE: &str = "https://jwt.attic.rs/v1";

macro_rules! require_permission_function {
    ($name:ident, $descr:literal, $member:ident) => {
        pub fn $name(&self) -> Result<()> {
            if !self.$member {
                tracing::debug!("Client has no {} permission", $descr);
                if self.can_discover() {
                    Err(Error::PermissionDenied)
                } else {
                    Err(Error::NoDiscoveryPermission)
                }
            } else {
                Ok(())
            }
        }
    };
}

/// A validated JSON Web Token.
#[derive(Debug)]
pub struct Token(JWTClaims<TokenClaims>);

/// Claims of a JSON Web Token.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenClaims {
    /// Attic namespace.
    #[serde(rename = "https://jwt.attic.rs/v1")]
    attic_ns: AtticAccess,
}

/// Permissions granted to a client.
///
/// This is the content of the `attic-access` claim in JWTs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AtticAccess {
    /// Cache permissions.
    ///
    /// Keys here may include wildcards.
    caches: IndexMap<CacheNamePattern, CachePermission>,
}

/// Permission to a single cache.
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CachePermission {
    /// Can pull objects from the cache.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    #[serde(rename = "r")]
    #[serde_as(as = "BoolFromInt")]
    pub pull: bool,

    /// Can push objects to the cache.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    #[serde(rename = "w")]
    #[serde_as(as = "BoolFromInt")]
    pub push: bool,

    /// Can delete objects from the cache.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    #[serde(rename = "d")]
    #[serde_as(as = "BoolFromInt")]
    pub delete: bool,

    /// Can create the cache itself.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    #[serde(rename = "cc")]
    #[serde_as(as = "BoolFromInt")]
    pub create_cache: bool,

    /// Can reconfigure the cache.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    #[serde(rename = "cr")]
    #[serde_as(as = "BoolFromInt")]
    pub configure_cache: bool,

    /// Can configure retention/quota settings.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    #[serde(rename = "cq")]
    #[serde_as(as = "BoolFromInt")]
    pub configure_cache_retention: bool,

    /// Can destroy the cache itself.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    #[serde(rename = "cd")]
    #[serde_as(as = "BoolFromInt")]
    pub destroy_cache: bool,
}

pub type Result<T> = std::result::Result<T, Error>;

/// An access error.
#[derive(Debug, Display)]
#[ignore_extra_doc_attributes]
pub enum Error {
    /// User has no permission to this cache.
    NoDiscoveryPermission,

    /// User does not have permission to complete this action.
    ///
    /// This implies that there is some permission granted to the
    /// user, so the user is authorized to discover the cache.
    PermissionDenied,

    /// JWT error: {0}
    TokenError(jwt_simple::Error),

    /// Base64 decode error: {0}
    Base64Error(base64::DecodeError),

    /// RSA Key error: {0}
    RsaKeyError(rsa::pkcs1::Error),

    /// Failure decoding the base64 layer of the base64 encoded PEM
    Utf8Error(std::str::Utf8Error),

    /// Pubkey-only JWT authentication cannot create signed JWTs
    PubkeyOnlyCannotCreateToken,
}

/// The supported JWT signature types.
pub enum SignatureType {
    HS256(HS256Key),
    RS256(RS256KeyPair),
    RS256PubkeyOnly(RS256PublicKey),
}

impl Token {
    /// Verifies and decodes a token.
    pub fn from_jwt(
        token: &str,
        signature_type: &SignatureType,
        maybe_bound_issuer: &Option<String>,
        maybe_bound_audiences: &Option<HashSet<String>>,
    ) -> Result<Self> {
        let opts = VerificationOptions {
            reject_before: None,
            accept_future: false,
            required_subject: None,
            required_key_id: None,
            required_public_key: None,
            required_nonce: None,
            allowed_issuers: maybe_bound_issuer
                .as_ref()
                .map(|s| [s.to_owned()].into())
                .to_owned(),
            allowed_audiences: maybe_bound_audiences.to_owned(),
            time_tolerance: None,
            max_validity: None,
            max_token_length: None,
            max_header_length: None,
            artificial_time: None,
        };

        match signature_type {
            SignatureType::HS256(key) => key
                .verify_token(token, Some(opts))
                .map_err(Error::TokenError)
                .map(Token),
            SignatureType::RS256(key) => {
                let public_key = key.public_key();
                public_key
                    .verify_token(token, Some(opts))
                    .map_err(Error::TokenError)
                    .map(Token)
            }
            SignatureType::RS256PubkeyOnly(key) => key
                .verify_token(token, Some(opts))
                .map_err(Error::TokenError)
                .map(Token),
        }
    }

    /// Creates a new token with an expiration timestamp.
    pub fn new(sub: String, exp: &DateTime<Utc>) -> Self {
        let claims = TokenClaims {
            attic_ns: Default::default(),
        };

        let now_epoch = Utc::now().signed_duration_since(DateTime::UNIX_EPOCH);

        Self(JWTClaims {
            issued_at: None,
            expires_at: Some(UnixTimeStamp::from_secs(
                exp.timestamp().try_into().unwrap(),
            )),
            invalid_before: Some(Duration::new(
                now_epoch.num_seconds().try_into().unwrap(),
                0,
            )),
            issuer: None,
            subject: Some(sub),
            audiences: None,
            jwt_id: None,
            nonce: None,
            custom: claims,
        })
    }

    /// Encodes the token.
    pub fn encode(
        &self,
        signature_type: &SignatureType,
        maybe_bound_issuer: &Option<String>,
        maybe_bound_audiences: &Option<HashSet<String>>,
    ) -> Result<String> {
        let mut token = self.0.clone();

        if let Some(issuer) = maybe_bound_issuer {
            token = token.with_issuer(issuer);
        }
        if let Some(audiences) = maybe_bound_audiences {
            token = token.with_audiences(audiences.to_owned());
        }

        match signature_type {
            SignatureType::HS256(key) => key.authenticate(token).map_err(Error::TokenError),
            SignatureType::RS256(key) => key.sign(token).map_err(Error::TokenError),
            SignatureType::RS256PubkeyOnly(_) => {
                return Err(Error::PubkeyOnlyCannotCreateToken);
            }
        }
    }

    /// Returns the subject of the token.
    pub fn sub(&self) -> Option<&str> {
        self.0.subject.as_deref()
    }

    /// Returns the claims as a serializable value.
    pub fn opaque_claims(&self) -> &impl Serialize {
        &self.0
    }

    /// Returns a mutable reference to a permission entry.
    pub fn get_or_insert_permission_mut(
        &mut self,
        pattern: CacheNamePattern,
    ) -> &mut CachePermission {
        use indexmap::map::Entry;

        let access = self.attic_access_mut();
        match access.caches.entry(pattern) {
            Entry::Occupied(v) => v.into_mut(),
            Entry::Vacant(v) => v.insert(CachePermission::default()),
        }
    }

    /// Returns explicit permission granted for a cache.
    pub fn get_permission_for_cache(&self, cache: &CacheName) -> CachePermission {
        let access = self.attic_access();

        let pattern_key = cache.to_pattern();
        if let Some(direct_match) = access.caches.get(&pattern_key) {
            return direct_match.clone();
        }

        for (pattern, permission) in access.caches.iter() {
            if pattern.matches(cache) {
                return permission.clone();
            }
        }

        CachePermission::default()
    }

    fn attic_access(&self) -> &AtticAccess {
        &self.0.custom.attic_ns
    }

    fn attic_access_mut(&mut self) -> &mut AtticAccess {
        &mut self.0.custom.attic_ns
    }
}

impl CachePermission {
    /// Adds implicit grants for public caches.
    pub fn add_public_permissions(&mut self) {
        self.pull = true;
    }

    /// Returns whether the user is allowed to discover this cache.
    ///
    /// This permission is implied when any permission is explicitly
    /// granted.
    pub const fn can_discover(&self) -> bool {
        self.push
            || self.pull
            || self.delete
            || self.create_cache
            || self.configure_cache
            || self.destroy_cache
            || self.configure_cache_retention
    }

    pub fn require_discover(&self) -> Result<()> {
        if !self.can_discover() {
            Err(Error::NoDiscoveryPermission)
        } else {
            Ok(())
        }
    }

    require_permission_function!(require_pull, "pull", pull);
    require_permission_function!(require_push, "push", push);
    require_permission_function!(require_delete, "delete", delete);
    require_permission_function!(require_create_cache, "create cache", create_cache);
    require_permission_function!(
        require_configure_cache,
        "reconfigure cache",
        configure_cache
    );
    require_permission_function!(
        require_configure_cache_retention,
        "configure cache retention",
        configure_cache_retention
    );
    require_permission_function!(require_destroy_cache, "destroy cache", destroy_cache);

    fn permission_default() -> bool {
        false
    }
}

impl StdError for Error {}

pub fn decode_token_hs256_secret_base64(s: &str) -> Result<HS256Key> {
    let decoded = BASE64_STANDARD.decode(s).map_err(Error::Base64Error)?;
    let secret = std::str::from_utf8(&decoded).map_err(Error::Utf8Error)?;
    Ok(HS256Key::from_bytes(&secret.as_bytes()))
}

pub fn decode_token_rs256_secret_base64(s: &str) -> Result<RS256KeyPair> {
    let decoded = BASE64_STANDARD.decode(s).map_err(Error::Base64Error)?;
    let secret = std::str::from_utf8(&decoded).map_err(Error::Utf8Error)?;
    let keypair = RS256KeyPair::from_pem(secret).map_err(Error::TokenError)?;

    Ok(keypair)
}

pub fn decode_token_rs256_pubkey_base64(s: &str) -> Result<RS256PublicKey> {
    let decoded = BASE64_STANDARD.decode(s).map_err(Error::Base64Error)?;
    let pubkey = std::str::from_utf8(&decoded).map_err(Error::Utf8Error)?;
    let pubkey = RS256PublicKey::from_pem(pubkey).map_err(Error::TokenError)?;

    Ok(pubkey)
}
