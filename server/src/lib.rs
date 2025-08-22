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
pub mod error;
pub mod gc;
mod middleware;
mod narinfo;
pub mod nix_manifest;
pub mod oobe;
mod storage;

use std::collections::HashSet;
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use attic::signing::NixKeypair;
use axum::{
    extract::Extension,
    http::{uri::Scheme, Uri},
    Router,
};
use chrono::Utc;
use sea_orm::{query::Statement, ConnectionTrait, Database, DatabaseConnection};
use sea_orm::{EntityTrait, IntoActiveModel, Set};
use tokio::net::TcpListener;
use tokio::sync::OnceCell;
use tokio::time;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::trace::TraceLayer;

use access::http::{apply_auth, AuthState};
use attic::cache::CacheName;
use config::{Config, StorageConfig};
use database::migration::{Migrator, MigratorTrait};
use error::{ErrorKind, ServerError, ServerResult};
use middleware::{init_request_state, restrict_host, set_visibility_header};
use storage::{LocalBackend, S3Backend, StorageBackend};

use crate::database::entity::cache::{self, Entity as Cache};
use crate::database::entity::Json as DbJson;

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

    /// The canonical substituter endpoint.
    substituter_endpoint: Option<String>,

    /// The potentially-invalid Host header supplied by the client.
    host: String,

    /// Whether the client claims the connection is HTTPS or not.
    client_claims_https: bool,

    /// Whether the cache the client's interacting with is public.
    ///
    /// This is purely informational and used to add the `X-Attic-Cache-Visibility`.
    /// header in responses.
    public_cache: AtomicBool,
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
                let db = Database::connect(&self.config.database.url)
                    .await
                    .map_err(ServerError::database_error);
                if let Ok(DatabaseConnection::SqlxSqlitePoolConnection(ref conn)) = db {
                    // execute some sqlite-specific performance optimizations
                    // see https://phiresky.github.io/blog/2020/sqlite-performance-tuning/ for
                    // more details
                    // intentionally ignore errors from this: this is purely for performance,
                    // not for correctness, so we can live without this
                    _ = conn
                        .execute_unprepared(
                            "
                        pragma journal_mode=WAL;
                        pragma synchronous=normal;
                        pragma temp_store=memory;
                        pragma mmap_size = 30000000000;
                        ",
                        )
                        .await;
                }

                db
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
        if let Some(substituter_endpoint) = &self.substituter_endpoint {
            Ok(format!("{}{}", substituter_endpoint, cache.as_str()))
        } else {
            Ok(format!("{}{}", self.api_endpoint()?, cache.as_str()))
        }
    }

    /// Indicates whether the cache the client is interacting with is public.
    fn set_public_cache(&self, public: bool) {
        self.public_cache.store(public, Ordering::Relaxed);
    }
}

/// The fallback route.
#[axum_macros::debug_handler]
async fn fallback(_: Uri) -> ServerResult<()> {
    Err(ErrorKind::NotFound.into())
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
        .layer(axum::middleware::from_fn(set_visibility_header))
        .layer(axum::middleware::from_fn(init_request_state))
        .layer(axum::middleware::from_fn(restrict_host))
        .layer(Extension(state.clone()))
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new());

    eprintln!("Listening on {:?}...", listen);

    let listener = TcpListener::bind(&listen).await?;

    let (server_ret, _) = tokio::join!(axum::serve(listener, rest).into_future(), async {
        if state.config.database.heartbeat {
            let _ = state.run_db_heartbeat().await;
        }
    },);

    server_ret?;

    Ok(())
}

/// Runs database migrations.
pub async fn run_migrations(config: Config) -> Result<()> {
    eprintln!("Running migrations...");

    let state = StateInner::new(config).await;
    let db = state.database().await?;
    Migrator::up(db, None).await?;

    eprintln!("Aligning caches with config...");

    let db_caches = Cache::find().all(db).await?;
    let mut exists = HashSet::new();
    for cache in db_caches {
        if let Some(configured) = state.config.caches.get(&cache.name) {
            exists.insert(cache.name.clone());
            let mut update = cache.clone().into_active_model();
            let mut modified = false;
            if configured.public != cache.is_public {
                update.is_public = Set(configured.public);
                modified = true;
            }
            let retention = configured
                .retention_period
                .map(|duration| duration.as_secs() as i32);
            if retention != cache.retention_period {
                update.retention_period = Set(retention);
                modified = true;
            }
            if configured.priority != cache.priority {
                update.priority = Set(configured.priority);
                modified = true;
            }
            if configured.upstream_cache_key_names != cache.upstream_cache_key_names.0 {
                update.upstream_cache_key_names =
                    Set(DbJson(configured.upstream_cache_key_names.clone()));
                modified = true;
            }
            if modified {
                eprintln!("Updating cache {:#?}...", cache.name);
                Cache::update(update).exec(db).await?;
            }
        } else if state.config.declarative {
            eprintln!("Removing cache {:#?}...", cache.name);
            Cache::delete_by_id(cache.id).exec(db).await?;
        }
    }

    if exists.len() == state.config.caches.len() {
        return Ok(());
    }

    for (name, config) in state
        .config
        .caches
        .iter()
        .filter(|(name, _)| !exists.contains(name.as_str()))
    {
        eprintln!("Creating cache {:#?}...", name);
        let keypair = NixKeypair::generate(name.as_str())?;
        let retention = config.retention_period.map(|dur| dur.as_secs() as i32);
        Cache::insert(cache::ActiveModel {
            name: Set(name.to_string()),
            keypair: Set(keypair.export_keypair()),
            is_public: Set(config.public),
            store_dir: Set("/nix/store".to_string()),
            priority: Set(config.priority),
            retention_period: Set(retention),
            upstream_cache_key_names: Set(DbJson(config.upstream_cache_key_names.clone())),
            created_at: Set(Utc::now()),
            ..Default::default()
        })
        .exec(db)
        .await?;
    }
    Ok(())
}
