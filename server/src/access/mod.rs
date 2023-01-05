//! Access control.
//!
//! Access control in Attic is simple and stateless [0] - The server validates
//! the JWT against a trusted public key and allows access based on the
//! `x-attic-access` claim.
//!
//! One primary goal of the Attic Server is easy scalability. It's designed
//! to be deployed to serverless platforms like AWS Lambda and have fast
//! cold-start times. Instances are created and destoyed rapidly in response
//! to requests.
//!
//! [0] We may revisit this later :)
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

pub mod http;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use displaydoc::Display;
pub use jsonwebtoken::{
    Algorithm as JwtAlgorithm, DecodingKey as JwtDecodingKey, EncodingKey as JwtEncodingKey,
    Header as JwtHeader, Validation as JwtValidation,
};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, BoolFromInt};

use crate::error::ServerResult;
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
        pub fn $name(&self) -> ServerResult<()> {
            if !self.$member {
                tracing::debug!("Client has no {} permission", $descr);
                if self.can_discover() {
                    Err(Error::PermissionDenied.into())
                } else {
                    Err(Error::NoDiscoveryPermission.into())
                }
            } else {
                Ok(())
            }
        }
    };
}

/// A validated JSON Web Token.
#[derive(Debug)]
pub struct Token(jsonwebtoken::TokenData<TokenClaims>);

/// Claims of a JSON Web Token.
#[derive(Debug, Serialize, Deserialize)]
struct TokenClaims {
    /// Subject.
    sub: String,

    /// Expiration timestamp.
    exp: usize,

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
    caches: HashMap<CacheNamePattern, CachePermission>,
}

/// Permission to a single cache.
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachePermission {
    /// Can pull objects from the cache.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "is_false")]
    #[serde(rename = "r")]
    #[serde_as(as = "BoolFromInt")]
    pub pull: bool,

    /// Can push objects to the cache.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "is_false")]
    #[serde(rename = "w")]
    #[serde_as(as = "BoolFromInt")]
    pub push: bool,

    /// Can delete objects from the cache.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "is_false")]
    #[serde(rename = "d")]
    #[serde_as(as = "BoolFromInt")]
    pub delete: bool,

    /// Can create the cache itself.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "is_false")]
    #[serde(rename = "cc")]
    #[serde_as(as = "BoolFromInt")]
    pub create_cache: bool,

    /// Can reconfigure the cache.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "is_false")]
    #[serde(rename = "cr")]
    #[serde_as(as = "BoolFromInt")]
    pub configure_cache: bool,

    /// Can configure retention/quota settings.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "is_false")]
    #[serde(rename = "cq")]
    #[serde_as(as = "BoolFromInt")]
    pub configure_cache_retention: bool,

    /// Can destroy the cache itself.
    #[serde(default = "CachePermission::permission_default")]
    #[serde(skip_serializing_if = "is_false")]
    #[serde(rename = "cd")]
    #[serde_as(as = "BoolFromInt")]
    pub destroy_cache: bool,
}

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
    TokenError(jsonwebtoken::errors::Error),
}

impl Token {
    /// Verifies and decodes a token.
    pub fn from_jwt(token: &str, key: &JwtDecodingKey) -> ServerResult<Self> {
        let validation = JwtValidation::default();
        jsonwebtoken::decode::<TokenClaims>(token, key, &validation)
            .map_err(|e| Error::TokenError(e).into())
            .map(Token)
    }

    /// Creates a new token with an expiration timestamp.
    pub fn new(sub: String, exp: &DateTime<Utc>) -> Self {
        let claims = TokenClaims {
            sub,
            exp: exp.timestamp() as usize,
            attic_ns: Default::default(),
        };

        Self(jsonwebtoken::TokenData {
            header: JwtHeader::new(JwtAlgorithm::HS256),
            claims,
        })
    }

    /// Encodes the token.
    pub fn encode(&self, key: &JwtEncodingKey) -> ServerResult<String> {
        jsonwebtoken::encode(&self.0.header, &self.0.claims, key)
            .map_err(|e| Error::TokenError(e).into())
    }

    /// Returns the subject of the token.
    pub fn sub(&self) -> &str {
        self.0.claims.sub.as_str()
    }

    /// Returns the claims as a serializable value.
    pub fn opaque_claims(&self) -> &impl Serialize {
        &self.0.claims
    }

    /// Returns a mutable reference to a permission entry.
    pub fn get_or_insert_permission_mut(
        &mut self,
        pattern: CacheNamePattern,
    ) -> &mut CachePermission {
        use std::collections::hash_map::Entry;

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
        &self.0.claims.attic_ns
    }

    fn attic_access_mut(&mut self) -> &mut AtticAccess {
        &mut self.0.claims.attic_ns
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

    pub fn require_discover(&self) -> ServerResult<()> {
        if !self.can_discover() {
            Err(Error::NoDiscoveryPermission.into())
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

impl Default for CachePermission {
    fn default() -> Self {
        Self {
            pull: false,
            push: false,
            delete: false,
            create_cache: false,
            configure_cache: false,
            configure_cache_retention: false,
            destroy_cache: false,
        }
    }
}

// bruh
fn is_false(b: &bool) -> bool {
    !b
}
