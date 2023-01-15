//! Error handling.

use std::error::Error as StdError;
use std::fmt;

use anyhow::Error as AnyError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use displaydoc::Display;
use serde::Serialize;
use tracing_error::SpanTrace;

use attic::error::AtticError;

pub type ServerResult<T> = Result<T, ServerError>;

/// A server error.
#[derive(Debug)]
pub struct ServerError {
    /// The kind of the error.
    kind: ErrorKind,

    /// Whether the client that caused the error has discovery permissions.
    discovery_permission: bool,

    /// Context of where the error occurred.
    context: SpanTrace,
}

/// The kind of an error.
#[derive(Debug, Display)]
pub enum ErrorKind {
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

    /// The requested NAR has missing chunks and needs to be repaired.
    IncompleteNar,

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
        ErrorKind::DatabaseError(AnyError::new(error)).into()
    }

    pub fn storage_error(error: impl StdError + Send + Sync + 'static) -> Self {
        ErrorKind::StorageError(AnyError::new(error)).into()
    }

    pub fn request_error(error: impl StdError + Send + Sync + 'static) -> Self {
        ErrorKind::RequestError(AnyError::new(error)).into()
    }

    pub fn set_discovery_permission(&mut self, perm: bool) {
        self.discovery_permission = perm;
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.kind)?;
        self.context.fmt(f)?;
        Ok(())
    }
}

impl From<ErrorKind> for ServerError {
    fn from(kind: ErrorKind) -> Self {
        Self {
            kind,
            discovery_permission: true,
            context: SpanTrace::capture(),
        }
    }
}

impl From<AtticError> for ServerError {
    fn from(error: AtticError) -> Self {
        ErrorKind::AtticError(error).into()
    }
}

impl From<super::access::Error> for ServerError {
    fn from(error: super::access::Error) -> Self {
        ErrorKind::AccessError(error).into()
    }
}

impl StdError for ServerError {}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        // TODO: Better logging control
        if matches!(
            self.kind,
            ErrorKind::DatabaseError(_)
                | ErrorKind::StorageError(_)
                | ErrorKind::ManifestSerializationError(_)
                | ErrorKind::AtticError(_)
        ) {
            tracing::error!("{}", self);
        }

        let kind = if self.discovery_permission {
            self.kind
        } else {
            self.kind.into_no_discovery_permissions()
        };

        // TODO: don't sanitize in dev mode
        let sanitized = kind.into_clients();

        let status_code = sanitized.http_status_code();
        let error_response = ErrorResponse {
            code: status_code.as_u16(),
            message: sanitized.to_string(),
            error: sanitized.name().to_string(),
        };

        (status_code, Json(error_response)).into_response()
    }
}

impl ErrorKind {
    fn name(&self) -> &'static str {
        match self {
            Self::NotFound => "NotFound",
            Self::Unauthorized => "Unauthorized",
            Self::InternalServerError => "InternalServerError",

            Self::NoSuchObject => "NoSuchObject",
            Self::NoSuchCache => "NoSuchCache",
            Self::CacheAlreadyExists => "CacheAlreadyExists",
            Self::InvalidCompressionType { .. } => "InvalidCompressionType",
            Self::IncompleteNar => "IncompleteNar",
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
    fn into_no_discovery_permissions(self) -> Self {
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
            Self::IncompleteNar => StatusCode::SERVICE_UNAVAILABLE,
            Self::ManifestSerializationError(_) => StatusCode::BAD_REQUEST,
            Self::RequestError(_) => StatusCode::BAD_REQUEST,
            Self::InvalidCompressionType { .. } => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
