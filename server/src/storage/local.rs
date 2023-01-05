//! Local file storage.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::fs::{self, File};
use tokio::io::{self, AsyncRead};

use super::{Download, RemoteFile, StorageBackend};
use crate::error::{ErrorKind, ServerError, ServerResult};

#[derive(Debug)]
pub struct LocalBackend {
    config: LocalStorageConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocalStorageConfig {
    /// The directory to store all files under.
    path: PathBuf,
}

/// Reference to a file in local storage.
///
/// We still call it "remote file" for consistency :)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalRemoteFile {
    /// Name of the file.
    pub name: String,
}

impl LocalBackend {
    pub async fn new(config: LocalStorageConfig) -> ServerResult<Self> {
        fs::create_dir_all(&config.path)
            .await
            .map_err(ServerError::storage_error)?;

        Ok(Self { config })
    }

    fn get_path(&self, p: &str) -> PathBuf {
        self.config.path.join(p)
    }
}

#[async_trait]
impl StorageBackend for LocalBackend {
    async fn upload_file(
        &self,
        name: String,
        mut stream: &mut (dyn AsyncRead + Unpin + Send),
    ) -> ServerResult<RemoteFile> {
        let mut file = File::create(self.get_path(&name))
            .await
            .map_err(ServerError::storage_error)?;

        io::copy(&mut stream, &mut file)
            .await
            .map_err(ServerError::storage_error)?;

        Ok(RemoteFile::Local(LocalRemoteFile { name }))
    }

    async fn delete_file(&self, name: String) -> ServerResult<()> {
        fs::remove_file(self.get_path(&name))
            .await
            .map_err(ServerError::storage_error)?;

        Ok(())
    }

    async fn delete_file_db(&self, file: &RemoteFile) -> ServerResult<()> {
        let file = if let RemoteFile::Local(file) = file {
            file
        } else {
            return Err(ErrorKind::StorageError(anyhow::anyhow!(
                "Does not understand the remote file reference"
            )).into());
        };

        fs::remove_file(self.get_path(&file.name))
            .await
            .map_err(ServerError::storage_error)?;

        Ok(())
    }

    async fn download_file(&self, name: String) -> ServerResult<Download> {
        let file = File::open(self.get_path(&name))
            .await
            .map_err(ServerError::storage_error)?;

        Ok(Download::Stream(Box::new(file)))
    }

    async fn download_file_db(&self, file: &RemoteFile) -> ServerResult<Download> {
        let file = if let RemoteFile::Local(file) = file {
            file
        } else {
            return Err(ErrorKind::StorageError(anyhow::anyhow!(
                "Does not understand the remote file reference"
            )).into());
        };

        let file = File::open(self.get_path(&file.name))
            .await
            .map_err(ServerError::storage_error)?;

        Ok(Download::Stream(Box::new(file)))
    }

    async fn make_db_reference(&self, name: String) -> ServerResult<RemoteFile> {
        Ok(RemoteFile::Local(LocalRemoteFile { name }))
    }
}
