//! HTTP middlewares for access control.

use std::str;

use axum::{http::Request, middleware::Next, response::Response};
use lazy_static::lazy_static;
use regex::Regex;
use sea_orm::DatabaseConnection;
use tokio::sync::OnceCell;

use crate::access::{CachePermission, Token};
use crate::database::{entity::cache::CacheModel, AtticDatabase};
use crate::error::ServerResult;
use crate::{RequestState, State};
use attic::cache::CacheName;

lazy_static! {
    static ref AUTHORIZATION_REGEX: Regex =
        Regex::new(r"^(?i)((?P<bearer>bearer)|(?P<basic>basic))(?-i) (?P<rest>(.*))$").unwrap();
}

/// Auth state.
#[derive(Debug)]
pub struct AuthState {
    /// The JWT token.
    pub token: OnceCell<Token>,
}

impl AuthState {
    /// Returns an auth state with no authenticated user and no permissions.
    pub fn new() -> Self {
        Self {
            token: OnceCell::new(),
        }
    }

    /// Returns the username if it exists.
    ///
    /// Currently it's the `sub` claim of the JWT.
    pub fn username(&self) -> Option<&str> {
        self.token.get().map(|token| token.sub())
    }

    /// Finds and performs authorization for a cache.
    pub async fn auth_cache<F, T>(
        &self,
        database: &DatabaseConnection,
        cache_name: &CacheName,
        f: F,
    ) -> ServerResult<T>
    where
        F: FnOnce(CacheModel, &mut CachePermission) -> ServerResult<T>,
    {
        let mut permission = if let Some(token) = self.token.get() {
            token.get_permission_for_cache(cache_name)
        } else {
            CachePermission::default()
        };

        let cache = match database.find_cache(cache_name).await {
            Ok(d) => {
                if d.is_public {
                    permission.add_public_permissions();
                }

                d
            }
            Err(mut e) => {
                e.set_discovery_permission(permission.can_discover());
                return Err(e);
            }
        };

        match f(cache, &mut permission) {
            Ok(t) => Ok(t),
            Err(mut e) => {
                e.set_discovery_permission(permission.can_discover());
                Err(e)
            }
        }
    }

    /// Returns permission granted for a cache.
    pub fn get_permission_for_cache(
        &self,
        cache: &CacheName,
        grant_public_permissions: bool,
    ) -> CachePermission {
        let mut permission = if let Some(token) = self.token.get() {
            token.get_permission_for_cache(cache)
        } else {
            CachePermission::default()
        };

        if grant_public_permissions {
            permission.add_public_permissions();
        }

        permission
    }
}

/// Performs auth.
pub async fn apply_auth<B>(req: Request<B>, next: Next<B>) -> Response {
    let token: Option<Token> = req
        .headers()
        .get("Authorization")
        .and_then(|bytes| bytes.to_str().ok())
        .and_then(parse_authorization_header)
        .and_then(|jwt| {
            let state = req.extensions().get::<State>().unwrap();
            let res_token = Token::from_jwt(&jwt, &state.config.token_hs256_secret.decoding);
            if let Err(e) = &res_token {
                tracing::debug!("Ignoring bad JWT token: {}", e);
            }
            res_token.ok()
        });

    if let Some(token) = token {
        let req_state = req.extensions().get::<RequestState>().unwrap();
        req_state.auth.token.set(token).unwrap();
        tracing::trace!("Added valid token");
    }

    next.run(req).await
}

/// Extracts the JWT from an Authorization header.
fn parse_authorization_header(authorization: &str) -> Option<String> {
    let captures = AUTHORIZATION_REGEX.captures(authorization)?;
    let rest = captures.name("rest").unwrap().as_str();

    if captures.name("bearer").is_some() {
        // Bearer token
        Some(rest.to_string())
    } else {
        // Basic auth
        let bytes = base64::decode(rest).ok()?;

        let user_pass = str::from_utf8(&bytes).ok()?;
        let colon = user_pass.find(':')?;
        let pass = &user_pass[colon + 1..];

        Some(pass.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_authorization_header() {
        assert_eq!(
            "somepass",
            parse_authorization_header("Basic c29tZXVzZXI6c29tZXBhc3M=").unwrap(),
        );

        assert_eq!(
            "somepass",
            parse_authorization_header("baSIC c29tZXVzZXI6c29tZXBhc3M=").unwrap(),
        );

        assert_eq!(
            "some-token",
            parse_authorization_header("bearer some-token").unwrap(),
        );
    }
}
