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
mod compression;
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
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use axum::{
    extract::Extension,
    http::{uri::Scheme, Uri},
    routing::get,
    Router,
};
use axum_prometheus::metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
use axum_prometheus::PrometheusMetricLayerBuilder;
use moka::future::Cache as MetadataCache;
use sea_orm::{query::Statement, ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use tokio::net::TcpListener;
use tokio::sync::OnceCell;
use tokio::time;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::trace::TraceLayer;

use access::http::{apply_auth, AuthState};
use attic::cache::CacheName;
use attic::nix_store::StorePathHash;
use config::{Config, StorageConfig};
use database::migration::{Migrator, MigratorTrait};
use database::{AtticDatabase, ObjectAndChunks};
use error::{ErrorKind, ServerError, ServerResult};
use middleware::{init_request_state, restrict_host, set_visibility_header};
use storage::{LocalBackend, S3Backend, StorageBackend};

type State = Arc<StateInner>;
type RequestState = Arc<RequestStateInner>;

/// How often queued last-accessed bumps are flushed to the database.
const BUMP_FLUSH_INTERVAL: Duration = Duration::from_secs(30);

/// Key for the metadata cache: `(cache name, store path hash, include_chunks)`.
type MetadataCacheKey = (String, String, bool);

/// Global server state.
pub struct StateInner {
    /// The Attic Server configuration.
    config: Config,

    /// Handle to the database.
    database: OnceCell<DatabaseConnection>,

    /// Handle to the storage backend.
    storage: OnceCell<Arc<Box<dyn StorageBackend>>>,

    /// Object IDs whose last-accessed bump awaits the next batched flush.
    ///
    /// One UPDATE per NAR download was a third of all DB queries during a
    /// 100-worker wave; the IDs are deduplicated here and flushed by
    /// `run_bump_flush_loop` in a single batched UPDATE instead.
    bump_queue: Mutex<HashSet<i64>>,

    /// In-memory TTL cache of `find_object_and_chunks` results.
    ///
    /// During a wave ~100 workers pull the same closure, so without this each
    /// identical `.narinfo`/`.nar` lookup re-runs the quintuple JOIN on
    /// Postgres. `None` when `metadata-cache-ttl-seconds` is 0.
    find_cache: Option<MetadataCache<MetadataCacheKey, ObjectAndChunks>>,
}

impl std::fmt::Debug for StateInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Hand-written (not derived) so the struct doesn't depend on `Debug`
        // for the connection handle, storage backend, or metadata cache.
        f.debug_struct("StateInner")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
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
        let find_cache = if config.metadata_cache_ttl_seconds > 0 {
            Some(
                MetadataCache::builder()
                    .time_to_live(Duration::from_secs(config.metadata_cache_ttl_seconds))
                    .max_capacity(config.metadata_cache_capacity)
                    // Weight by chunk count (~200 bytes each) so capacity bounds
                    // memory rather than entry count; narinfo entries (no chunks)
                    // weigh 1.
                    .weigher(|_key, value: &ObjectAndChunks| {
                        value.chunks.len().clamp(1, u32::MAX as usize) as u32
                    })
                    .build(),
            )
        } else {
            None
        };

        Arc::new(Self {
            config,
            database: OnceCell::new(),
            storage: OnceCell::new(),
            bump_queue: Mutex::new(HashSet::new()),
            find_cache,
        })
    }

    /// Like [`AtticDatabase::find_object_and_chunks_by_store_path_hash`], but
    /// served from the in-memory TTL cache when possible.
    ///
    /// Only successful lookups are cached. Permission checks still run
    /// per-request against the cached `cache.is_public`, so caching never
    /// grants access.
    async fn find_object_and_chunks_cached(
        &self,
        cache_name: &CacheName,
        store_path_hash: &StorePathHash,
        include_chunks: bool,
    ) -> ServerResult<ObjectAndChunks> {
        let key: MetadataCacheKey = (
            cache_name.as_str().to_owned(),
            store_path_hash.as_str().to_owned(),
            include_chunks,
        );

        if let Some(find_cache) = &self.find_cache {
            if let Some(hit) = find_cache.get(&key).await {
                metrics::counter!("atticd_metadata_cache_total", "result" => "hit").increment(1);
                return Ok(hit);
            }
        }

        let db = self.database().await?;
        let (object, cache, nar, chunks) = db
            .find_object_and_chunks_by_store_path_hash(cache_name, store_path_hash, include_chunks)
            .await?;
        let value = ObjectAndChunks {
            object,
            cache,
            nar,
            chunks,
        };

        if let Some(find_cache) = &self.find_cache {
            metrics::counter!("atticd_metadata_cache_total", "result" => "miss").increment(1);
            find_cache.insert(key, value.clone()).await;
        }

        Ok(value)
    }

    /// Drops all cached object/chunk metadata.
    ///
    /// Called after cache-config mutations (visibility, keypair, retention,
    /// deletion) so they take effect immediately instead of after the TTL —
    /// otherwise a public→private flip would keep serving cached entries to
    /// anonymous clients for up to `metadata-cache-ttl-seconds`.
    fn invalidate_metadata_cache(&self) {
        if let Some(find_cache) = &self.find_cache {
            find_cache.invalidate_all();
        }
    }

    /// Returns a handle to the database.
    async fn database(&self) -> ServerResult<&DatabaseConnection> {
        self.database
            .get_or_try_init(|| async {
                let mut opt = ConnectOptions::new(&self.config.database.url);
                opt.min_connections(self.config.database.min_connections)
                    .max_connections(self.config.database.max_connections);

                let db = Database::connect(opt)
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

    /// Queues an object's last-accessed bump for the next batched flush.
    fn queue_bump_object_last_accessed(&self, object_id: i64) {
        self.bump_queue.lock().unwrap().insert(object_id);
    }

    /// Periodically flushes queued last-accessed bumps as batched UPDATEs.
    async fn run_bump_flush_loop(&self) {
        loop {
            time::sleep(BUMP_FLUSH_INTERVAL).await;

            let ids: Vec<i64> = {
                let mut queue = self.bump_queue.lock().unwrap();
                queue.drain().collect()
            };
            if ids.is_empty() {
                continue;
            }

            let flush = async {
                let db = self.database().await?;
                db.bump_objects_last_accessed(&ids).await
            };
            if let Err(e) = flush.await {
                tracing::warn!("Failed to flush {} last-accessed bumps: {}", ids.len(), e);
                // Re-merge so a transient DB error doesn't lose them; the set is
                // bounded by the number of distinct objects ever queued.
                self.bump_queue.lock().unwrap().extend(ids);
            }
        }
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

    let (prometheus_layer, metric_handle) = PrometheusMetricLayerBuilder::new()
        .with_prefix("atticd")
        .with_metrics_from_fn(|| {
            PrometheusBuilder::new()
                .set_buckets_for_metric(
                    Matcher::Full("atticd_http_requests_duration_seconds".to_string()),
                    &[
                        0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                    ],
                )
                .unwrap()
                .set_buckets_for_metric(
                    Matcher::Full("atticd_oss_request_duration_seconds".to_string()),
                    &[
                        0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                    ],
                )
                .unwrap()
                .set_buckets_for_metric(
                    Matcher::Full("atticd_db_query_duration_seconds".to_string()),
                    &[
                        0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                    ],
                )
                .unwrap()
                .install_recorder()
                .unwrap()
        })
        .build_pair();

    let metrics_route =
        Router::new().route("/metrics", get(|| async move { metric_handle.render() }));

    let api_routes = Router::new()
        .merge(api::get_router())
        // middlewares — apply only to API routes, not /metrics
        .layer(axum::middleware::from_fn(apply_auth))
        .layer(axum::middleware::from_fn(set_visibility_header))
        .layer(axum::middleware::from_fn(init_request_state))
        .layer(axum::middleware::from_fn(restrict_host));

    let rest = Router::new()
        .merge(metrics_route)
        .merge(api_routes)
        .fallback(fallback)
        .layer(prometheus_layer)
        .layer(Extension(state.clone()))
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new());

    eprintln!("Listening on {:?}...", listen);

    let listener = TcpListener::bind(&listen).await?;

    let (server_ret, _, _) = tokio::join!(
        axum::serve(listener, rest).into_future(),
        async {
            if state.config.database.heartbeat {
                let _ = state.run_db_heartbeat().await;
            }
        },
        state.run_bump_flush_loop(),
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
