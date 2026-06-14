//! HTTP API.

mod binary_cache;
mod v1;

use axum::{Router, response::Html, routing::get};

async fn placeholder() -> Html<&'static str> {
    Html(include_str!("placeholder.html"))
}

pub(crate) fn get_router() -> Router {
    Router::new()
        .route("/", get(placeholder))
        .merge(binary_cache::get_router())
        .merge(v1::get_router())
}
