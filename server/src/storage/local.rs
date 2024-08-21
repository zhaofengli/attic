//! Local file storage.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
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

async fn read_version(storage_path: &Path) -> ServerResult<u32> {
    let version_path = storage_path.join("VERSION");
    let v = match fs::read_to_string(&version_path).await {
        Ok(version) => version
            .trim()
            .parse()
            .map_err(|_| ErrorKind::StorageError(anyhow::anyhow!("Invalid version file")))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
        Err(e) => {
            return Err(ErrorKind::StorageError(anyhow::anyhow!(
                "Failed to read version file: {}",
                e
            ))
            .into());
        }
    };
    Ok(v)
}

async fn write_version(storage_path: &Path, version: u32) -> ServerResult<()> {
    let version_path = storage_path.join("VERSION");
    fs::write(&version_path, format!("{}", version))
        .await
        .map_err(ServerError::storage_error)?;
    Ok(())
}

async fn upgrade_0_to_1(storage_path: &Path) -> ServerResult<()> {
    let mut files = fs::read_dir(storage_path)
        .await
        .map_err(ServerError::storage_error)?;
    // move all files to subdirectory using the first two characters of the filename
    while let Some(file) = files
        .next_entry()
        .await
        .map_err(ServerError::storage_error)?
    {
        if file
            .file_type()
            .await
            .map_err(ServerError::storage_error)?
            .is_file()
        {
            let name = file.file_name();
            let name_bytes = name.as_os_str().as_bytes();
            let parents = storage_path
                .join(OsStr::from_bytes(&name_bytes[0..1]))
                .join(OsStr::from_bytes(&name_bytes[0..2]));
            let new_path = parents.join(name);
            fs::create_dir_all(&parents).await.map_err(|e| {
                ErrorKind::StorageError(anyhow::anyhow!("Failed to create directory {}", e))
            })?;
            fs::rename(&file.path(), &new_path).await.map_err(|e| {
                ErrorKind::StorageError(anyhow::anyhow!(
                    "Failed to move file {} to {}: {}",
                    file.path().display(),
                    new_path.display(),
                    e
                ))
            })?;
        }
    }

    Ok(())
}

impl LocalBackend {
    pub async fn new(config: LocalStorageConfig) -> ServerResult<Self> {
        fs::create_dir_all(&config.path).await.map_err(|e| {
            ErrorKind::StorageError(anyhow::anyhow!(
                "Failed to create storage directory {}: {}",
                config.path.display(),
                e
            ))
        })?;

        let version = read_version(&config.path).await?;
        if version == 0 {
            upgrade_0_to_1(&config.path).await?;
        }
        write_version(&config.path, 1).await?;

        Ok(Self { config })
    }

    fn get_path(&self, p: &str) -> PathBuf {
        let level1 = &p[0..1];
        let level2 = &p[0..2];
        self.config.path.join(level1).join(level2).join(p)
    }
}

#[async_trait]
impl StorageBackend for LocalBackend {
    async fn upload_file(
        &self,
        name: String,
        mut stream: &mut (dyn AsyncRead + Unpin + Send),
    ) -> ServerResult<RemoteFile> {
        let path = self.get_path(&name);
        fs::create_dir_all(path.parent().unwrap())
            .await
            .map_err(|e| {
                ErrorKind::StorageError(anyhow::anyhow!(
                    "Failed to create directory {}: {}",
                    path.parent().unwrap().display(),
                    e
                ))
            })?;
        let mut file = File::create(self.get_path(&name)).await.map_err(|e| {
            ErrorKind::StorageError(anyhow::anyhow!(
                "Failed to create file {}: {}",
                self.get_path(&name).display(),
                e
            ))
        })?;

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
            ))
            .into());
        };

        fs::remove_file(self.get_path(&file.name))
            .await
            .map_err(ServerError::storage_error)?;

        Ok(())
    }

    async fn download_file(&self, name: String, _prefer_stream: bool) -> ServerResult<Download> {
        let file = File::open(self.get_path(&name))
            .await
            .map_err(ServerError::storage_error)?;

        Ok(Download::AsyncRead(Box::new(file)))
    }

    async fn download_file_db(
        &self,
        file: &RemoteFile,
        _prefer_stream: bool,
    ) -> ServerResult<Download> {
        let file = if let RemoteFile::Local(file) = file {
            file
        } else {
            return Err(ErrorKind::StorageError(anyhow::anyhow!(
                "Does not understand the remote file reference"
            ))
            .into());
        };

        let file = File::open(self.get_path(&file.name))
            .await
            .map_err(ServerError::storage_error)?;

        Ok(Download::AsyncRead(Box::new(file)))
    }

    async fn make_db_reference(&self, name: String) -> ServerResult<RemoteFile> {
        Ok(RemoteFile::Local(LocalRemoteFile { name }))
    }
}
