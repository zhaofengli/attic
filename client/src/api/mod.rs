use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use const_format::formatcp;
use displaydoc::Display;
use futures::{
    future,
    stream::{self, StreamExt, TryStream, TryStreamExt},
};
use reqwest::{
    Body, Client as HttpClient, Response, StatusCode, Url,
    header::{AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT},
};
use serde::Deserialize;

use crate::config::ServerConfig;
use crate::version::ATTIC_DISTRIBUTOR;
use attic::api::v1::cache_config::{CacheConfig, CreateCacheRequest};
use attic::api::v1::get_missing_paths::{GetMissingPathsRequest, GetMissingPathsResponse};
use attic::api::v1::upload_path::{
    ATTIC_NAR_INFO, ATTIC_NAR_INFO_PREAMBLE_SIZE, FinalizeUploadSessionResponse,
    StartUploadPathSessionRequest, StartUploadPathSessionResponse, UploadPathNarInfo,
    UploadPathResult,
};
use attic::cache::CacheName;
use attic::nix_store::StorePathHash;

/// The User-Agent string of Attic.
const ATTIC_USER_AGENT: &str =
    formatcp!("Attic/{} ({})", env!("CARGO_PKG_NAME"), ATTIC_DISTRIBUTOR);

/// The size threshold to send the upload info as part of the PUT body.
const NAR_INFO_PREAMBLE_THRESHOLD: usize = 4 * 1024; // 4 KiB
const UPLOAD_PROGRESS_CHUNK_SIZE: usize = 128 * 1024;

/// The Attic API client.
#[derive(Debug, Clone)]
pub struct ApiClient {
    /// Base endpoint of the server.
    endpoint: Url,

    /// An initialized HTTP client.
    client: HttpClient,
}

/// An API error.
#[derive(Debug, Display)]
pub enum ApiError {
    /// {0}
    Structured(StructuredApiError),

    /// HTTP {0}: {1}
    Unstructured(StatusCode, String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct StructuredApiError {
    #[allow(dead_code)]
    code: u16,
    error: String,
    message: String,
}

impl ApiClient {
    pub fn from_server_config(config: ServerConfig) -> Result<Self> {
        let client = build_http_client(config.token()?.as_deref());

        Ok(Self {
            endpoint: Url::parse(&config.endpoint)?,
            client,
        })
    }

    /// Sets the API endpoint of this client.
    pub fn set_endpoint(&mut self, endpoint: &str) -> Result<()> {
        self.endpoint = Url::parse(endpoint)?;
        Ok(())
    }

    /// Returns the configuration of a cache.
    pub async fn get_cache_config(&self, cache: &CacheName) -> Result<CacheConfig> {
        let endpoint = self
            .endpoint
            .join("_api/v1/cache-config/")?
            .join(cache.as_str())?;

        let res = self.client.get(endpoint).send().await?;

        if res.status().is_success() {
            let cache_config = res.json().await?;
            Ok(cache_config)
        } else {
            let api_error = ApiError::try_from_response(res).await?;
            Err(api_error.into())
        }
    }

    /// Creates a cache.
    pub async fn create_cache(&self, cache: &CacheName, request: CreateCacheRequest) -> Result<()> {
        let endpoint = self
            .endpoint
            .join("_api/v1/cache-config/")?
            .join(cache.as_str())?;

        let res = self.client.post(endpoint).json(&request).send().await?;

        if res.status().is_success() {
            Ok(())
        } else {
            let api_error = ApiError::try_from_response(res).await?;
            Err(api_error.into())
        }
    }

    /// Configures a cache.
    pub async fn configure_cache(&self, cache: &CacheName, config: &CacheConfig) -> Result<()> {
        let endpoint = self
            .endpoint
            .join("_api/v1/cache-config/")?
            .join(cache.as_str())?;

        let res = self.client.patch(endpoint).json(&config).send().await?;

        if res.status().is_success() {
            Ok(())
        } else {
            let api_error = ApiError::try_from_response(res).await?;
            Err(api_error.into())
        }
    }

    /// Destroys a cache.
    pub async fn destroy_cache(&self, cache: &CacheName) -> Result<()> {
        let endpoint = self
            .endpoint
            .join("_api/v1/cache-config/")?
            .join(cache.as_str())?;

        let res = self.client.delete(endpoint).send().await?;

        if res.status().is_success() {
            Ok(())
        } else {
            let api_error = ApiError::try_from_response(res).await?;
            Err(api_error.into())
        }
    }

    /// Returns paths missing from a cache.
    pub async fn get_missing_paths(
        &self,
        cache: &CacheName,
        store_path_hashes: Vec<StorePathHash>,
    ) -> Result<GetMissingPathsResponse> {
        let endpoint = self.endpoint.join("_api/v1/get-missing-paths")?;
        let payload = GetMissingPathsRequest {
            cache: cache.to_owned(),
            store_path_hashes,
        };

        let res = self.client.post(endpoint).json(&payload).send().await?;

        if res.status().is_success() {
            let cache_config = res.json().await?;
            Ok(cache_config)
        } else {
            let api_error = ApiError::try_from_response(res).await?;
            Err(api_error.into())
        }
    }

    /// Uploads a path.
    pub async fn upload_path<S>(
        &self,
        nar_info: UploadPathNarInfo,
        stream: S,
        force_preamble: bool,
    ) -> Result<Option<UploadPathResult>>
    where
        S: TryStream<Ok = Bytes> + Send + Sync + 'static,
        S::Error: Into<Box<dyn StdError + Send + Sync>> + Send + Sync,
    {
        let endpoint = self.endpoint.join("_api/v1/upload-path")?;
        let upload_info_json = serde_json::to_string(&nar_info)?;

        let mut req = self.client.put(endpoint);

        if force_preamble || upload_info_json.len() >= NAR_INFO_PREAMBLE_THRESHOLD {
            let preamble = Bytes::from(upload_info_json);
            let preamble_len = preamble.len();
            let preamble_stream = stream::once(future::ok(preamble));

            let chained = preamble_stream.chain(stream.into_stream());
            req = req
                .header(ATTIC_NAR_INFO_PREAMBLE_SIZE, preamble_len)
                .body(Body::wrap_stream(chained));
        } else {
            req = req
                .header(ATTIC_NAR_INFO, HeaderValue::from_str(&upload_info_json)?)
                .body(Body::wrap_stream(stream));
        }

        let res = req.send().await?;

        if res.status().is_success() {
            match res.json().await {
                Ok(r) => Ok(Some(r)),
                Err(_) => Ok(None),
            }
        } else {
            let api_error = ApiError::try_from_response(res).await?;
            Err(api_error.into())
        }
    }

    /// Starts a chunked transport upload session.
    pub async fn start_upload_session(
        &self,
        nar_info: UploadPathNarInfo,
        chunk_size: Option<usize>,
    ) -> Result<StartUploadPathSessionResponse> {
        let endpoint = self.endpoint.join("_api/v1/upload-path/sessions")?;
        let payload = StartUploadPathSessionRequest {
            nar_info,
            chunk_size,
        };

        let res = self.client.post(endpoint).json(&payload).send().await?;
        if res.status().is_success() {
            Ok(res.json().await?)
        } else {
            let api_error = ApiError::try_from_response(res).await?;
            Err(api_error.into())
        }
    }

    /// Uploads a chunked transport upload session part, reporting body bytes as
    /// they are sent to the HTTP client.
    pub async fn upload_session_part_with_progress<F>(
        &self,
        session_id: uuid::Uuid,
        seq: u32,
        bytes: Bytes,
        on_progress: F,
    ) -> Result<()>
    where
        F: Fn(u64) + Send + Sync + 'static,
    {
        let endpoint = self.endpoint.join(&format!(
            "_api/v1/upload-path/sessions/{}/parts/{}",
            session_id, seq
        ))?;

        let body_len = bytes.len();
        let on_progress = Arc::new(on_progress);
        let body_stream = stream::unfold((bytes, 0), move |(bytes, offset)| {
            let on_progress = on_progress.clone();
            async move {
                if offset >= body_len {
                    return None;
                }

                let end = (offset + UPLOAD_PROGRESS_CHUNK_SIZE).min(body_len);
                let chunk = bytes.slice(offset..end);
                on_progress(chunk.len() as u64);
                Some((Ok::<_, io::Error>(chunk), (bytes, end)))
            }
        });

        let res = self
            .client
            .put(endpoint)
            .body(Body::wrap_stream(body_stream))
            .send()
            .await?;
        if res.status().is_success() {
            Ok(())
        } else {
            let api_error = ApiError::try_from_response(res).await?;
            Err(api_error.into())
        }
    }

    /// Finalizes a chunked transport upload session.
    pub async fn finalize_upload_session(
        &self,
        session_id: uuid::Uuid,
    ) -> Result<FinalizeUploadSessionResponse> {
        let endpoint = self.endpoint.join(&format!(
            "_api/v1/upload-path/sessions/{}/finalize",
            session_id
        ))?;

        let res = self.client.post(endpoint).send().await?;
        if res.status().is_success() {
            Ok(res.json().await?)
        } else {
            let api_error = ApiError::try_from_response(res).await?;
            Err(api_error.into())
        }
    }

    /// Aborts a chunked transport upload session.
    pub async fn abort_upload_session(&self, session_id: uuid::Uuid) -> Result<()> {
        let endpoint = self
            .endpoint
            .join(&format!("_api/v1/upload-path/sessions/{}", session_id))?;

        let res = self.client.delete(endpoint).send().await?;
        if res.status().is_success() {
            Ok(())
        } else {
            let api_error = ApiError::try_from_response(res).await?;
            Err(api_error.into())
        }
    }
}

impl StdError for ApiError {}

impl ApiError {
    pub fn status_code(&self) -> Option<StatusCode> {
        match self {
            Self::Structured(error) => StatusCode::from_u16(error.code).ok(),
            Self::Unstructured(status, _) => Some(*status),
        }
    }

    pub fn is_retryable(&self) -> bool {
        self.status_code().is_some_and(|status| {
            status == StatusCode::REQUEST_TIMEOUT
                || status == StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error()
        })
    }

    async fn try_from_response(response: Response) -> Result<Self> {
        let status = response.status();
        let text = response.text().await?;
        match serde_json::from_str(&text) {
            Ok(s) => Ok(Self::Structured(s)),
            Err(_) => Ok(Self::Unstructured(status, text)),
        }
    }
}

impl fmt::Display for StructuredApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.error, self.message)
    }
}

fn build_http_client(token: Option<&str>) -> HttpClient {
    let mut headers = HeaderMap::new();

    headers.insert(USER_AGENT, HeaderValue::from_str(ATTIC_USER_AGENT).unwrap());

    if let Some(token) = token {
        let auth_header = HeaderValue::from_str(&format!("bearer {}", token)).unwrap();
        headers.insert(AUTHORIZATION, auth_header);
    }

    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap()
}
