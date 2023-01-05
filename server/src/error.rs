//! Error handling.

use std::error::Error as StdError;

use anyhow::Error as AnyError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use displaydoc::Display;
use serde::Serialize;

use attic::error::AtticError;

pub type ServerResult<T> = Result<T, ServerError>;

/// An error.
#[derive(Debug, Display)]
pub enum ServerError {
    // Generic responses
    /// The URL you requested was not found.
    NotFound,

    /// Unauthorized.
    Unauthorized,

    /// The server encountered an internal error or misconfiguration.
    InternalServerError,

    // Specialized responses
    /// The requested cache does not exist.
    NoSuchCache,

    /// The cache already exists.
    CacheAlreadyExists,

    /// The requested object does not exist.
    NoSuchObject,

    /// Invalid compression type "{name}".
    InvalidCompressionType { name: String },

    /// Database error: {0}
    DatabaseError(AnyError),

    /// Storage error: {0}
    StorageError(AnyError),

    /// Manifest serialization error: {0}
    ManifestSerializationError(super::nix_manifest::Error),

    /// Access error: {0}
    AccessError(super::access::Error),

    /// General request error: {0}
    RequestError(AnyError),

    /// Error from the common components.
    AtticError(AtticError),
}

#[derive(Serialize)]
pub struct ErrorResponse {
    code: u16,
    error: String,
    message: String,
}

impl ServerError {
    pub fn database_error(error: impl StdError + Send + Sync + 'static) -> Self {
        Self::DatabaseError(AnyError::new(error))
    }

    pub fn storage_error(error: impl StdError + Send + Sync + 'static) -> Self {
        Self::StorageError(AnyError::new(error))
    }

    pub fn request_error(error: impl StdError + Send + Sync + 'static) -> Self {
        Self::RequestError(AnyError::new(error))
    }

    fn name(&self) -> &'static str {
        match self {
            Self::NotFound => "NotFound",
            Self::Unauthorized => "Unauthorized",
            Self::InternalServerError => "InternalServerError",

            Self::NoSuchObject => "NoSuchObject",
            Self::NoSuchCache => "NoSuchCache",
            Self::CacheAlreadyExists => "CacheAlreadyExists",
            Self::InvalidCompressionType { .. } => "InvalidCompressionType",
            Self::AtticError(e) => e.name(),
            Self::DatabaseError(_) => "DatabaseError",
            Self::StorageError(_) => "StorageError",
            Self::ManifestSerializationError(_) => "ManifestSerializationError",
            Self::AccessError(_) => "AccessError",
            Self::RequestError(_) => "RequestError",
        }
    }

    /// Returns a more restricted version of this error for a client without discovery
    /// permissions.
    pub fn into_no_discovery_permissions(self) -> Self {
        match self {
            Self::NoSuchCache => Self::Unauthorized,
            Self::NoSuchObject => Self::Unauthorized,
            Self::AccessError(_) => Self::Unauthorized,

            _ => self,
        }
    }

    /// Returns a version of this error for clients.
    fn into_clients(self) -> Self {
        match self {
            Self::AccessError(super::access::Error::NoDiscoveryPermission) => Self::Unauthorized,

            Self::DatabaseError(_) => Self::InternalServerError,
            Self::StorageError(_) => Self::InternalServerError,
            Self::ManifestSerializationError(_) => Self::InternalServerError,

            _ => self,
        }
    }

    fn http_status_code(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::InternalServerError => StatusCode::INTERNAL_SERVER_ERROR,

            Self::AccessError(_) => StatusCode::FORBIDDEN,
            Self::NoSuchCache => StatusCode::NOT_FOUND,
            Self::NoSuchObject => StatusCode::NOT_FOUND,
            Self::CacheAlreadyExists => StatusCode::BAD_REQUEST,
            Self::ManifestSerializationError(_) => StatusCode::BAD_REQUEST,
            Self::RequestError(_) => StatusCode::BAD_REQUEST,
            Self::InvalidCompressionType { .. } => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl StdError for ServerError {}

impl From<AtticError> for ServerError {
    fn from(error: AtticError) -> Self {
        Self::AtticError(error)
    }
}

impl From<super::access::Error> for ServerError {
    fn from(error: super::access::Error) -> Self {
        Self::AccessError(error)
    }
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        // TODO: Better logging control
        if matches!(self, Self::DatabaseError(_) | Self::StorageError(_) | Self::ManifestSerializationError(_) | Self::AtticError(_)) {
            tracing::error!("{:?}", self);
        }

        // TODO: don't sanitize in dev mode
        let sanitized = self.into_clients();

        let status_code = sanitized.http_status_code();
        let error_response = ErrorResponse {
            code: status_code.as_u16(),
            message: sanitized.to_string(),
            error: sanitized.name().to_string(),
        };

        (status_code, Json(error_response)).into_response()
    }
}
