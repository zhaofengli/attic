//! Remote file storage.

mod local;
mod s3;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncRead;

use crate::error::ServerResult;

pub(crate) use self::local::{LocalBackend, LocalRemoteFile, LocalStorageConfig};
pub(crate) use self::s3::{S3Backend, S3RemoteFile, S3StorageConfig};

/// Reference to a location where a NAR is stored.
///
/// To be compatible with the Nix Binary Cache API, the reference
/// must be able to be converted to a (time-limited) direct link
/// to the file that the client will be redirected to when they
/// request the NAR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteFile {
    /// File in an S3-compatible storage bucket.
    S3(S3RemoteFile),

    /// File in local storage.
    Local(LocalRemoteFile),

    /// A direct HTTP link.
    ///
    /// This is mostly here to facilitate testing.
    Http(HttpRemoteFile),
}

/// Way to download a file.
pub enum Download {
    /// A redirect to a (possibly ephemeral) URL.
    Redirect(String),

    /// A stream.
    Stream(Box<dyn AsyncRead + Unpin + Send>),
}

// TODO: Maybe make RemoteFile the one true reference instead of having two sets of APIs?
/// A storage backend.
#[async_trait::async_trait]
pub trait StorageBackend: Send + Sync + std::fmt::Debug {
    /// Uploads a file.
    async fn upload_file(
        &self,
        name: String,
        stream: &mut (dyn AsyncRead + Unpin + Send),
    ) -> ServerResult<RemoteFile>;

    /// Deletes a file.
    async fn delete_file(&self, name: String) -> ServerResult<()>;

    /// Deletes a file using a database reference.
    async fn delete_file_db(&self, file: &RemoteFile) -> ServerResult<()>;

    /// Downloads a file using the current configuration.
    async fn download_file(&self, name: String) -> ServerResult<Download>;

    /// Downloads a file using a database reference.
    async fn download_file_db(&self, file: &RemoteFile) -> ServerResult<Download>;

    /// Creates a database reference for a file.
    async fn make_db_reference(&self, name: String) -> ServerResult<RemoteFile>;
}

/// Reference to an HTTP link from which the file can be downloaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRemoteFile {
    /// URL of the file.
    pub url: String,
}

impl RemoteFile {
    /// Returns the remote file ID.
    pub fn remote_file_id(&self) -> String {
        match self {
            Self::S3(f) => format!("s3:{}/{}/{}", f.region, f.bucket, f.key),
            Self::Http(f) => format!("http:{}", f.url),
            Self::Local(f) => format!("local:{}", f.name),
        }
    }
}
