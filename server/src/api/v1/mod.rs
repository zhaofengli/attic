mod cache_config;
mod get_missing_paths;
mod pin;
mod upload_path;

use axum::{
    Router,
    routing::{delete, get, patch, post, put},
};

pub(crate) fn get_router() -> Router {
    Router::new()
        .route(
            "/_api/v1/get-missing-paths",
            post(get_missing_paths::get_missing_paths),
        )
        .route("/_api/v1/upload-path", put(upload_path::upload_path))
        .route(
            "/{cache}/attic-cache-info",
            get(cache_config::get_cache_config),
        )
        .route(
            "/_api/v1/cache-config/{cache}",
            get(cache_config::get_cache_config),
        )
        .route(
            "/_api/v1/cache-config/{cache}",
            post(cache_config::create_cache),
        )
        .route(
            "/_api/v1/cache-config/{cache}",
            patch(cache_config::configure_cache),
        )
        .route(
            "/_api/v1/cache-config/{cache}",
            delete(cache_config::destroy_cache),
        )
        .route("/_api/v1/pins/{cache}", get(pin::list_pins))
        .route("/_api/v1/pins/{cache}/{name}", put(pin::create_pin))
        .route("/_api/v1/pins/{cache}/{name}", delete(pin::delete_pin))
}
