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
    deny(unused_imports, unused_mut, unused_variables,)
)]

pub mod access;
mod api;
pub mod config;
pub mod database;
mod error;
pub mod gc;
mod middleware;
mod narinfo;
pub mod nix_manifest;
pub mod oobe;
mod storage;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::{
    extract::Extension,
    http::{uri::Scheme, Uri},
    Router,
};
use sea_orm::{query::Statement, ConnectionTrait, Database, DatabaseConnection};
use tokio::sync::OnceCell;
use tokio::time;
use tower_http::catch_panic::CatchPanicLayer;

use access::http::{apply_auth, AuthState};
use attic::cache::CacheName;
use config::{Config, StorageConfig};
use database::migration::{Migrator, MigratorTrait};
use error::{ServerError, ServerResult};
use middleware::{init_request_state, restrict_host};
use storage::{LocalBackend, S3Backend, StorageBackend};

type State = Arc<StateInner>;
type RequestState = Arc<RequestStateInner>;

/// Global server state.
#[derive(Debug)]
pub struct StateInner {
    /// The Attic Server configuration.
    config: Config,

    /// Handle to the database.
    database: OnceCell<DatabaseConnection>,

    /// Handle to the storage backend.
    storage: OnceCell<Arc<Box<dyn StorageBackend>>>,
}

/// Request state.
#[derive(Debug)]
struct RequestStateInner {
    /// Auth state.
    auth: AuthState,

    /// The canonical API endpoint.
    api_endpoint: Option<String>,

    /// The potentially-invalid Host header supplied by the client.
    host: String,

    /// Whether the client claims the connection is HTTPS or not.
    client_claims_https: bool,
}

impl StateInner {
    async fn new(config: Config) -> State {
        Arc::new(Self {
            config,
            database: OnceCell::new(),
            storage: OnceCell::new(),
        })
    }

    /// Returns a handle to the database.
    async fn database(&self) -> ServerResult<&DatabaseConnection> {
        self.database
            .get_or_try_init(|| async {
                Database::connect(&self.config.database.url)
                    .await
                    .map_err(ServerError::database_error)
            })
            .await
    }

    /// Returns a handle to the storage backend.
    async fn storage(&self) -> ServerResult<&Arc<Box<dyn StorageBackend>>> {
        self.storage
            .get_or_try_init(|| async {
                match &self.config.storage {
                    StorageConfig::Local(local_config) => {
                        let local = LocalBackend::new(local_config.clone()).await?;
                        let boxed: Box<dyn StorageBackend> = Box::new(local);
                        Ok(Arc::new(boxed))
                    }
                    StorageConfig::S3(s3_config) => {
                        let s3 = S3Backend::new(s3_config.clone()).await?;
                        let boxed: Box<dyn StorageBackend> = Box::new(s3);
                        Ok(Arc::new(boxed))
                    }
                }
            })
            .await
    }

    /// Sends periodic heartbeat queries to the database.
    async fn run_db_heartbeat(&self) -> ServerResult<()> {
        let db = self.database().await?;
        let stmt =
            Statement::from_string(db.get_database_backend(), "SELECT 'heartbeat';".to_string());

        loop {
            let _ = db.execute(stmt.clone()).await;
            time::sleep(Duration::from_secs(60)).await;
        }
    }
}

impl RequestStateInner {
    /// Returns the base API endpoint for clients.
    ///
    /// The APIs encompass both the Attic API and the Nix binary
    /// cache API.
    fn api_endpoint(&self) -> ServerResult<String> {
        if let Some(endpoint) = &self.api_endpoint {
            Ok(endpoint.to_owned())
        } else {
            // Naively synthesize from client's Host header
            // For convenience and shouldn't be used in production!
            let uri = Uri::builder()
                .scheme(if self.client_claims_https {
                    Scheme::HTTPS
                } else {
                    Scheme::HTTP
                })
                .authority(self.host.to_owned())
                .path_and_query("/")
                .build()
                .map_err(ServerError::request_error)?;

            Ok(uri.to_string())
        }
    }

    /// Returns the Nix binary cache endpoint for clients.
    ///
    /// The binary cache endpoint may live on another host than
    /// the canonical API endpoint.
    fn substituter_endpoint(&self, cache: CacheName) -> ServerResult<String> {
        Ok(format!("{}{}", self.api_endpoint()?, cache.as_str()))
    }
}

/// The fallback route.
#[axum_macros::debug_handler]
async fn fallback(_: Uri) -> ServerResult<()> {
    Err(ServerError::NotFound)
}

/// Runs the API server.
pub async fn run_api_server(cli_listen: Option<SocketAddr>, config: Config) -> Result<()> {
    eprintln!("Starting API server...");

    let state = StateInner::new(config).await;

    let listen = if let Some(cli_listen) = cli_listen {
        cli_listen
    } else {
        state.config.listen.to_owned()
    };

    let rest = Router::new()
        .merge(api::get_router())
        .fallback(fallback)
        // middlewares
        .layer(axum::middleware::from_fn(apply_auth))
        .layer(axum::middleware::from_fn(init_request_state))
        .layer(axum::middleware::from_fn(restrict_host))
        .layer(Extension(state.clone()))
        .layer(CatchPanicLayer::new());

    eprintln!("Listening on {:?}...", listen);

    let (server_ret, _) = tokio::join!(
        axum::Server::bind(&listen).serve(rest.into_make_service()),
        async {
            if state.config.database.heartbeat {
                let _ = state.run_db_heartbeat().await;
            }
        },
    );

    server_ret?;

    Ok(())
}

/// Runs database migrations.
pub async fn run_migrations(config: Config) -> Result<()> {
    eprintln!("Running migrations...");

    let state = StateInner::new(config).await;
    let db = state.database().await?;
    Migrator::up(db, None).await?;

    Ok(())
}
