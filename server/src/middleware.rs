use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::anyhow;
use axum::{
    extract::{Extension, Host},
    http::{HeaderValue, Request},
    middleware::Next,
    response::Response,
};

use super::{AuthState, RequestState, RequestStateInner, State};
use crate::error::{ErrorKind, ServerResult};
use attic::api::binary_cache::ATTIC_CACHE_VISIBILITY;

/// Initializes per-request state.
pub async fn init_request_state<B>(
    Extension(state): Extension<State>,
    Host(host): Host,
    mut req: Request<B>,
    next: Next<B>,
) -> Response {
    // X-Forwarded-Proto is an untrusted header
    let client_claims_https =
        if let Some(x_forwarded_proto) = req.headers().get("x-forwarded-proto") {
            x_forwarded_proto.as_bytes() == b"https"
        } else {
            false
        };

    let req_state = Arc::new(RequestStateInner {
        auth: AuthState::new(),
        api_endpoint: state.config.api_endpoint.to_owned(),
        host,
        client_claims_https,
        public_cache: AtomicBool::new(false),
    });

    req.extensions_mut().insert(req_state);
    next.run(req).await
}

/// Restricts valid Host headers.
///
/// We also require that all request have a Host header in
/// the first place.
pub async fn restrict_host<B>(
    Extension(state): Extension<State>,
    Host(host): Host,
    req: Request<B>,
    next: Next<B>,
) -> ServerResult<Response> {
    let allowed_hosts = &state.config.allowed_hosts;

    if !allowed_hosts.is_empty() && !allowed_hosts.iter().any(|h| h.as_str() == host) {
        return Err(ErrorKind::RequestError(anyhow!("Bad Host")).into());
    }

    Ok(next.run(req).await)
}

/// Sets the `X-Attic-Cache-Visibility` header in responses.
pub(crate) async fn set_visibility_header<B>(
    Extension(req_state): Extension<RequestState>,
    req: Request<B>,
    next: Next<B>,
) -> ServerResult<Response> {
    let mut response = next.run(req).await;

    if req_state.public_cache.load(Ordering::Relaxed) {
        response
            .headers_mut()
            .append(ATTIC_CACHE_VISIBILITY, HeaderValue::from_static("public"));
    }

    Ok(response)
}
