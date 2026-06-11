//! S3 remote files.

use std::time::Duration;

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_s3::{
    config::Builder as S3ConfigBuilder,
    config::{Credentials, Region},
    operation::get_object::builders::GetObjectFluentBuilder,
    presigning::PresigningConfig,
    types::{CompletedMultipartUpload, CompletedPart},
    Client,
};
use bytes::BytesMut;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncRead;

use super::{Download, RemoteFile, StorageBackend};
use crate::error::{ErrorKind, ServerError, ServerResult};
use attic::io::read_chunk_async;
use attic::util::Finally;

/// The chunk size for each part in a multipart upload.
const CHUNK_SIZE: usize = 8 * 1024 * 1024;

/// The S3 remote file storage backend.
#[derive(Debug)]
pub struct S3Backend {
    client: Client,
    /// Optional client for generating presigned URLs (uses public_endpoint)
    /// If None, falls back to using `client`
    public_client: Option<Client>,
    config: S3StorageConfig,
}

/// S3 remote file storage configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct S3StorageConfig {
    /// The AWS region.
    region: String,

    /// The name of the bucket.
    bucket: String,

    /// Custom S3 endpoint.
    ///
    /// Set this if you are using an S3-compatible object storage (e.g., Minio).
    endpoint: Option<String>,

    /// Public S3 endpoint for client redirects.
    ///
    /// If set, this endpoint will be used in HTTP 307 redirect URLs sent to clients.
    /// If not set, falls back to `endpoint`.
    /// This allows using an internal endpoint for server operations while
    /// redirecting clients to a public endpoint.
    #[serde(rename = "public-endpoint")]
    pub public_endpoint: Option<String>,

    /// Whether to force path-style addressing for the public endpoint.
    ///
    /// If not set, defaults to the same behavior as the internal endpoint:
    /// - If `endpoint` is set, path-style is forced (MinIO/Garage default)
    /// - If `endpoint` is not set, virtual-host style is used (AWS S3 default)
    ///
    /// Set to `false` explicitly for CloudFront/R2 custom domains when using
    /// an internal MinIO/Garage endpoint.
    #[serde(rename = "public-endpoint-path-style")]
    pub public_endpoint_path_style: Option<bool>,

    /// S3 credentials.
    ///
    /// If not specified, it's read from the `AWS_ACCESS_KEY_ID` and
    /// `AWS_SECRET_ACCESS_KEY` environment variables.
    credentials: Option<S3CredentialsConfig>,
}

/// S3 credential configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct S3CredentialsConfig {
    /// Access key ID.
    access_key_id: String,

    /// Secret access key.
    secret_access_key: String,
}

/// Reference to a file in an S3-compatible storage bucket.
///
/// We store the region and bucket to facilitate migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3RemoteFile {
    /// Name of the S3 region.
    pub region: String,

    /// Name of the bucket.
    pub bucket: String,

    /// Key of the file.
    pub key: String,
}

impl S3StorageConfig {
    /// Validates the S3 storage configuration.
    pub fn validate(&self) -> ServerResult<()> {
        // For S3-compatible storage, we don't require an endpoint as it can use AWS defaults
        // But if both endpoint and public_endpoint are None, we'll rely on AWS SDK defaults
        Ok(())
    }
}

impl S3StorageConfig {
    /// Determines the endpoint and path-style setting for public client.
    /// Returns (endpoint, force_path_style)
    fn public_client_config(&self) -> Option<(String, bool)> {
        self.public_endpoint.as_ref().map(|endpoint| {
            let force_path = self.public_endpoint_path_style
                .unwrap_or_else(|| self.endpoint.is_some());
            (endpoint.clone(), force_path)
        })
    }
}

impl S3Backend {
    pub async fn new(config: S3StorageConfig) -> ServerResult<Self> {
        // Validate configuration
        config.validate()?;

        // Build the main client for server-side operations (uses endpoint)
        let s3_config = Self::config_builder(&config, &config.endpoint, true)
            .await?
            .region(Region::new(config.region.to_owned()))
            .build();
        let client = Client::from_conf(s3_config);

        // Build optional public client for presigned URLs (uses public_endpoint)
        let public_client = if let Some((endpoint, force_path)) = config.public_client_config() {
            let endpoint_option = Some(endpoint);
            let public_config = Self::config_builder(&config, &endpoint_option, force_path)
                .await?
                .region(Region::new(config.region.to_owned()))
                .build();
            Some(Client::from_conf(public_config))
        } else {
            None
        };

        Ok(Self {
            client,
            public_client,
            config,
        })
    }

    async fn config_builder(
        config: &S3StorageConfig,
        endpoint: &Option<String>,
        force_path_style: bool,
    ) -> ServerResult<S3ConfigBuilder> {
        let shared_config = aws_config::load_defaults(BehaviorVersion::v2025_01_17()).await;
        let mut builder = S3ConfigBuilder::from(&shared_config);

        if let Some(credentials) = &config.credentials {
            builder = builder.credentials_provider(Credentials::new(
                &credentials.access_key_id,
                &credentials.secret_access_key,
                None,
                None,
                "s3",
            ));
        }

        if let Some(endpoint) = endpoint {
            builder = builder.endpoint_url(endpoint);
            // Only force path-style for custom endpoints (not for public endpoints like CloudFront)
            if force_path_style {
                builder = builder.force_path_style(true);
            }
        }

        Ok(builder)
    }

    async fn get_client_from_db_ref<'a>(
        &self,
        file: &'a RemoteFile,
    ) -> ServerResult<(Client, &'a S3RemoteFile)> {
        let file = if let RemoteFile::S3(file) = file {
            file
        } else {
            return Err(ErrorKind::StorageError(anyhow::anyhow!(
                "Does not understand the remote file reference"
            ))
            .into());
        };

        // FIXME: Ugly
        let client = if self.client.config().region().unwrap().as_ref() == file.region {
            self.client.clone()
        } else {
            // FIXME: Cache the client instance
            let s3_conf = Self::config_builder(&self.config, &self.config.endpoint, true)
                .await?
                .region(Region::new(file.region.to_owned()))
                .build();
            Client::from_conf(s3_conf)
        };

        Ok((client, file))
    }

    async fn get_download(
        &self,
        req: GetObjectFluentBuilder,
        prefer_stream: bool,
    ) -> ServerResult<Download> {
        if prefer_stream {
            let output = req.send().await.map_err(ServerError::storage_error)?;

            Ok(Download::AsyncRead(Box::new(output.body.into_async_read())))
        } else {
            // FIXME: Configurable expiration
            let presign_config = PresigningConfig::expires_in(Duration::from_secs(600))
                .map_err(ServerError::storage_error)?;

            let presigned = req
                .presigned(presign_config)
                .await
                .map_err(ServerError::storage_error)?;

            Ok(Download::Url(presigned.uri().to_string()))
        }
    }
}

#[async_trait]
impl StorageBackend for S3Backend {
    async fn upload_file(
        &self,
        name: String,
        mut stream: &mut (dyn AsyncRead + Unpin + Send),
    ) -> ServerResult<RemoteFile> {
        let buf = BytesMut::with_capacity(CHUNK_SIZE);
        let first_chunk = read_chunk_async(&mut stream, buf)
            .await
            .map_err(ServerError::storage_error)?;

        if first_chunk.len() < CHUNK_SIZE {
            // do a normal PutObject
            let put_object = self
                .client
                .put_object()
                .bucket(&self.config.bucket)
                .key(&name)
                .body(first_chunk.into())
                .send()
                .await
                .map_err(ServerError::storage_error)?;

            tracing::debug!("put_object -> {:#?}", put_object);

            return Ok(RemoteFile::S3(S3RemoteFile {
                region: self.config.region.clone(),
                bucket: self.config.bucket.clone(),
                key: name,
            }));
        }

        let multipart = self
            .client
            .create_multipart_upload()
            .bucket(&self.config.bucket)
            .key(&name)
            .send()
            .await
            .map_err(ServerError::storage_error)?;

        let upload_id = multipart.upload_id().unwrap();

        let cleanup = Finally::new({
            let bucket = self.config.bucket.clone();
            let client = self.client.clone();
            let upload_id = upload_id.to_owned();
            let name = name.clone();

            async move {
                tracing::warn!("Upload was interrupted - Aborting multipart upload");

                let r = client
                    .abort_multipart_upload()
                    .bucket(bucket)
                    .key(name)
                    .upload_id(upload_id)
                    .send()
                    .await;

                if let Err(e) = r {
                    tracing::warn!("Failed to abort multipart upload: {}", e);
                }
            }
        });

        let mut part_number = 1;
        let mut parts = Vec::new();
        let mut first_chunk = Some(first_chunk);

        loop {
            let chunk = if part_number == 1 {
                first_chunk.take().unwrap()
            } else {
                let buf = BytesMut::with_capacity(CHUNK_SIZE);
                read_chunk_async(&mut stream, buf)
                    .await
                    .map_err(ServerError::storage_error)?
            };

            if chunk.is_empty() {
                break;
            }

            let client = self.client.clone();
            let fut = tokio::task::spawn({
                client
                    .upload_part()
                    .bucket(&self.config.bucket)
                    .key(&name)
                    .upload_id(upload_id)
                    .part_number(part_number)
                    .body(chunk.clone().into())
                    .send()
            });

            parts.push(fut);
            part_number += 1;
        }

        let completed_parts = join_all(parts)
            .await
            .into_iter()
            .map(|join_result| join_result.unwrap())
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(ServerError::storage_error)?
            .into_iter()
            .enumerate()
            .map(|(idx, part)| {
                let part_number = idx + 1;
                CompletedPart::builder()
                    .set_e_tag(part.e_tag().map(str::to_string))
                    .set_part_number(Some(part_number as i32))
                    .build()
            })
            .collect::<Vec<_>>();

        let completed_multipart_upload = CompletedMultipartUpload::builder()
            .set_parts(Some(completed_parts))
            .build();

        let completion = self
            .client
            .complete_multipart_upload()
            .bucket(&self.config.bucket)
            .key(&name)
            .upload_id(upload_id)
            .multipart_upload(completed_multipart_upload)
            .send()
            .await
            .map_err(ServerError::storage_error)?;

        tracing::debug!("complete_multipart_upload -> {:#?}", completion);

        cleanup.cancel();

        Ok(RemoteFile::S3(S3RemoteFile {
            region: self.config.region.clone(),
            bucket: self.config.bucket.clone(),
            key: name,
        }))
    }

    async fn delete_file(&self, name: String) -> ServerResult<()> {
        let deletion = self
            .client
            .delete_object()
            .bucket(&self.config.bucket)
            .key(&name)
            .send()
            .await
            .map_err(ServerError::storage_error)?;

        tracing::debug!("delete_file -> {:#?}", deletion);

        Ok(())
    }

    async fn delete_file_db(&self, file: &RemoteFile) -> ServerResult<()> {
        let (client, file) = self.get_client_from_db_ref(file).await?;

        let deletion = client
            .delete_object()
            .bucket(&file.bucket)
            .key(&file.key)
            .send()
            .await
            .map_err(ServerError::storage_error)?;

        tracing::debug!("delete_file -> {:#?}", deletion);

        Ok(())
    }

    async fn download_file(&self, name: String, prefer_stream: bool) -> ServerResult<Download> {
        // Use public_client for presigned URLs if available, otherwise use main client
        let client = if prefer_stream {
            &self.client
        } else {
            self.public_client.as_ref().unwrap_or(&self.client)
        };

        let req = client
            .get_object()
            .bucket(&self.config.bucket)
            .key(&name);

        self.get_download(req, prefer_stream).await
    }

    async fn download_file_db(
        &self,
        file: &RemoteFile,
        prefer_stream: bool,
    ) -> ServerResult<Download> {
        // For streaming, use get_client_from_db_ref (handles multi-region)
        // For presigned URLs, use public_client if available
        if prefer_stream {
            let (client, file) = self.get_client_from_db_ref(file).await?;
            let req = client.get_object().bucket(&file.bucket).key(&file.key);
            self.get_download(req, prefer_stream).await
        } else {
            let file = if let RemoteFile::S3(file) = file {
                file
            } else {
                return Err(ErrorKind::StorageError(anyhow::anyhow!(
                    "Does not understand the remote file reference"
                ))
                .into());
            };

            // Determine which client to use based on region and public_client availability
            let client = if self.client.config().region().unwrap().as_ref() == file.region {
                // Same region: use public_client if available, otherwise main client
                self.public_client.as_ref().unwrap_or(&self.client)
            } else {
                // Different region: need to build a new client for that region
                let (endpoint, force_path) = self.config.public_client_config()
                    .unwrap_or_else(|| (
                        self.config.endpoint.clone().unwrap_or_default(),
                        self.config.endpoint.is_some()
                    ));
                let endpoint_option = if !endpoint.is_empty() { Some(endpoint) } else { None };

                let s3_config = Self::config_builder(&self.config, &endpoint_option, force_path)
                    .await?
                    .region(Region::new(file.region.clone()))
                    .build();
                let client = Client::from_conf(s3_config);
                let req = client.get_object().bucket(&file.bucket).key(&file.key);
                return self.get_download(req, prefer_stream).await;
            };

            let req = client.get_object().bucket(&file.bucket).key(&file.key);
            self.get_download(req, prefer_stream).await
        }
    }

    async fn make_db_reference(&self, name: String) -> ServerResult<RemoteFile> {
        Ok(RemoteFile::S3(S3RemoteFile {
            region: self.config.region.clone(),
            bucket: self.config.bucket.clone(),
            key: name,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> S3StorageConfig {
        S3StorageConfig {
            region: "us-east-1".to_string(),
            bucket: "test-bucket".to_string(),
            endpoint: None,
            public_endpoint: None,
            public_endpoint_path_style: None,
            credentials: Some(S3CredentialsConfig {
                access_key_id: "test-key".to_string(),
                secret_access_key: "test-secret".to_string(),
            }),
        }
    }

    #[test]
    fn test_public_client_config_helper() {
        // Test 1: MinIO setup - path-style defaults to true
        let mut config = create_test_config();
        config.endpoint = Some("http://minio:9000".to_string());
        config.public_endpoint = Some("https://public.example.com".to_string());

        let (endpoint, force_path) = config.public_client_config().unwrap();
        assert_eq!(endpoint, "https://public.example.com");
        assert_eq!(force_path, true, "Should force path-style when endpoint is set");

        // Test 2: MinIO + CloudFront - explicit override
        let mut config = create_test_config();
        config.endpoint = Some("http://minio:9000".to_string());
        config.public_endpoint = Some("https://d111111.cloudfront.net".to_string());
        config.public_endpoint_path_style = Some(false);

        let (endpoint, force_path) = config.public_client_config().unwrap();
        assert_eq!(endpoint, "https://d111111.cloudfront.net");
        assert_eq!(force_path, false, "Should respect explicit path-style override");

        // Test 3: AWS S3 (no endpoint) - path-style defaults to false
        let mut config = create_test_config();
        config.public_endpoint = Some("https://s3.amazonaws.com".to_string());

        let (endpoint, force_path) = config.public_client_config().unwrap();
        assert_eq!(endpoint, "https://s3.amazonaws.com");
        assert_eq!(force_path, false, "Should not force path-style when endpoint is not set");

        // Test 4: No public endpoint
        let config = create_test_config();
        assert!(config.public_client_config().is_none());
    }

    #[tokio::test]
    async fn test_client_caching() {
        // Both endpoints specified - should create separate public_client
        let mut config = create_test_config();
        config.endpoint = Some("http://internal:9000".to_string());
        config.public_endpoint = Some("https://public.example.com".to_string());

        let backend = S3Backend::new(config.clone()).await.unwrap();
        assert!(backend.public_client.is_some());

        // Only endpoint specified - public_client should be None (uses main client)
        let mut config = create_test_config();
        config.endpoint = Some("http://internal:9000".to_string());

        let backend = S3Backend::new(config.clone()).await.unwrap();
        assert!(backend.public_client.is_none());
    }

    #[test]
    fn test_backward_compatible_deserialization() {
        // Old config without public-endpoint still works
        let toml_str = r#"
            region = "us-west-2"
            bucket = "my-bucket"
            endpoint = "https://s3.example.com"

            [credentials]
            access_key_id = "key"
            secret_access_key = "secret"
        "#;

        let config: S3StorageConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.endpoint, Some("https://s3.example.com".to_string()));
        assert_eq!(config.public_endpoint, None);
        assert_eq!(config.public_endpoint_path_style, None);
    }
}

