use std::error::Error as StdError;
use std::fmt;

use anyhow::Result;
use bytes::Bytes;
use const_format::concatcp;
use displaydoc::Display;
use futures::TryStream;
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT},
    Body, Client as HttpClient, Response, StatusCode, Url,
};
use serde::Deserialize;

use crate::config::ServerConfig;
use crate::version::ATTIC_DISTRIBUTOR;
use attic::api::v1::cache_config::{CacheConfig, CreateCacheRequest};
use attic::api::v1::get_missing_paths::{GetMissingPathsRequest, GetMissingPathsResponse};
use attic::api::v1::upload_path::{UploadPathNarInfo, UploadPathResult};
use attic::cache::CacheName;
use attic::nix_store::StorePathHash;

/// The User-Agent string of Attic.
const ATTIC_USER_AGENT: &str =
    concatcp!("Attic/{} ({})", env!("CARGO_PKG_NAME"), ATTIC_DISTRIBUTOR);

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
        let client = build_http_client(config.token.as_deref());

        Ok(Self {
            endpoint: Url::parse(&config.endpoint)?,
            client,
        })
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
    ) -> Result<Option<UploadPathResult>>
    where
        S: TryStream + Send + Sync + 'static,
        S::Error: Into<Box<dyn StdError + Send + Sync>>,
        Bytes: From<S::Ok>,
    {
        let endpoint = self.endpoint.join("_api/v1/upload-path")?;
        let upload_info_json = serde_json::to_string(&nar_info)?;

        let res = self
            .client
            .put(endpoint)
            .header(
                "X-Attic-Nar-Info",
                HeaderValue::from_str(&upload_info_json)?,
            )
            .header(USER_AGENT, HeaderValue::from_str(ATTIC_USER_AGENT)?)
            .body(Body::wrap_stream(stream))
            .send()
            .await?;

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
}

impl StdError for ApiError {}

impl ApiError {
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

    if let Some(token) = token {
        let auth_header = HeaderValue::from_str(&format!("bearer {}", token)).unwrap();
        headers.insert(AUTHORIZATION, auth_header);
    }

    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap()
}
