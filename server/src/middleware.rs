use std::sync::Arc;

use anyhow::anyhow;
use axum::{
    extract::{Extension, Host},
    http::Request,
    middleware::Next,
    response::Response,
};

use super::{AuthState, RequestStateInner, State};
use crate::error::{ServerError, ServerResult};

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
        return Err(ServerError::RequestError(anyhow!("Bad Host")));
    }

    Ok(next.run(req).await)
}
