use std::error::Error as StdError;
use std::fmt;

use anyhow::Result;
use bytes::Bytes;
use const_format::concatcp;
use displaydoc::Display;
use futures::{
    future,
    stream::{self, StreamExt, TryStream, TryStreamExt},
    AsyncReadExt,
};
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_ENCODING, USER_AGENT},
    Body, Client as HttpClient, Response, StatusCode, Url,
};
use serde::Deserialize;

use crate::version::ATTIC_DISTRIBUTOR;
use crate::{
    compression::StreamingCompressor,
    config::{CompressionConfig, ServerConfig},
};
use attic::api::v1::cache_config::{CacheConfig, CreateCacheRequest};
use attic::api::v1::get_missing_paths::{GetMissingPathsRequest, GetMissingPathsResponse};
use attic::api::v1::upload_path::{
    UploadPathNarInfo, UploadPathResult, ATTIC_NAR_INFO, ATTIC_NAR_INFO_PREAMBLE_SIZE,
};
use attic::cache::CacheName;
use attic::nix_store::StorePathHash;

/// The User-Agent string of Attic.
const ATTIC_USER_AGENT: &str =
    concatcp!("Attic/{} ({})", env!("CARGO_PKG_NAME"), ATTIC_DISTRIBUTOR);

/// The size threshold to send the upload info as part of the PUT body.
const NAR_INFO_PREAMBLE_THRESHOLD: usize = 4 * 1024; // 4 KiB

/// The Attic API client.
#[derive(Debug, Clone)]
pub struct ApiClient {
    /// Base endpoint of the server.
    endpoint: Url,

    /// An initialized HTTP client.
    client: HttpClient,

    /// The compression algorithm to use.
    compression: CompressionConfig,
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
            compression: config.compression,
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
        S: TryStream<Ok = Bytes> + Send + Sync + std::marker::Unpin + 'static,
        S::Error: Into<Box<dyn StdError + Send + Sync>> + Send + Sync,
    {
        let endpoint = self.endpoint.join("_api/v1/upload-path")?;
        let upload_info_json = serde_json::to_string(&nar_info)?;

        let mut req = self
            .client
            .put(endpoint)
            .header(USER_AGENT, HeaderValue::from_str(ATTIC_USER_AGENT)?)
            .header(
                CONTENT_ENCODING,
                HeaderValue::from_static(self.compression.http_value()),
            );

        if force_preamble || upload_info_json.len() >= NAR_INFO_PREAMBLE_THRESHOLD {
            let preamble = Bytes::from(upload_info_json);
            let preamble_len = preamble.len();
            let preamble_stream = stream::once(future::ok(preamble));

            let chained = preamble_stream.chain(stream.into_stream());
            let chained = chained.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
            let mut compressed =
                StreamingCompressor::new_unbuffered(chained.into_async_read(), self.compression);
            let final_stream = async_stream::stream! {
              loop {
                let mut buf = vec![0; 4096];
                match compressed.read(&mut buf).await {
                  Ok(0) => {break;}
                  Ok(n) => {
                    buf.truncate(n);
                    yield Ok(buf);
                  }
                  Err(e) => {yield Err(e);}
                }
              }
            };
            req = req
                .header(ATTIC_NAR_INFO_PREAMBLE_SIZE, preamble_len)
                .body(Body::wrap_stream(final_stream));
        } else {
            let stream = stream.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
            let mut compressed =
                StreamingCompressor::new_unbuffered(stream.into_async_read(), self.compression);
            let final_stream = async_stream::stream! {
              loop {
                let mut buf = vec![0; 4096];
                match compressed.read(&mut buf).await {
                  Ok(0) => {break;}
                  Ok(n) => {
                    buf.truncate(n);
                    yield Ok(buf);
                  }
                  Err(e) => {yield Err(e);}
                }
              }
            };
            req = req
                .header(ATTIC_NAR_INFO, HeaderValue::from_str(&upload_info_json)?)
                .body(Body::wrap_stream(final_stream));
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
