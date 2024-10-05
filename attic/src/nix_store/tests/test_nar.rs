//! Utilities for testing the NAR dump functionality.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::task::{Context, Poll};

use tempfile::NamedTempFile;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::Command;

use crate::error::AtticResult;
use crate::nix_store::StorePath;

/// Expected values for `nm1w9sdm6j6icmhd2q3260hl1w9zj6li-attic-test-no-deps`.
pub const NO_DEPS: TestNar = TestNar {
    store_path: "/nix/store/nm1w9sdm6j6icmhd2q3260hl1w9zj6li-attic-test-no-deps",
    _original_file: include_bytes!("nar/nm1w9sdm6j6icmhd2q3260hl1w9zj6li-attic-test-no-deps"),
    nar: include_bytes!("nar/nm1w9sdm6j6icmhd2q3260hl1w9zj6li-attic-test-no-deps.nar"),
    export: include_bytes!("nar/nm1w9sdm6j6icmhd2q3260hl1w9zj6li-attic-test-no-deps.export"),
    closure: &["nm1w9sdm6j6icmhd2q3260hl1w9zj6li-attic-test-no-deps"],
};

/// Expected values for `n7q4i7rlmbk4xz8qdsxpm6jbhrnxraq2-attic-test-with-deps-a`.
///
/// This depends on `544qcchwgcgpz3xi1bbml28f8jj6009p-attic-test-with-deps-b` as well
/// as `3k1wymic8p7h5pfcqfhh0jan8ny2a712-attic-test-with-deps-c-final`.
pub const WITH_DEPS_A: TestNar = TestNar {
    store_path: "/nix/store/n7q4i7rlmbk4xz8qdsxpm6jbhrnxraq2-attic-test-with-deps-a",
    _original_file: include_bytes!("nar/n7q4i7rlmbk4xz8qdsxpm6jbhrnxraq2-attic-test-with-deps-a"),
    nar: include_bytes!("nar/n7q4i7rlmbk4xz8qdsxpm6jbhrnxraq2-attic-test-with-deps-a.nar"),
    export: include_bytes!("nar/n7q4i7rlmbk4xz8qdsxpm6jbhrnxraq2-attic-test-with-deps-a.export"),
    closure: &[
        "n7q4i7rlmbk4xz8qdsxpm6jbhrnxraq2-attic-test-with-deps-a",
        "544qcchwgcgpz3xi1bbml28f8jj6009p-attic-test-with-deps-b",
        "3k1wymic8p7h5pfcqfhh0jan8ny2a712-attic-test-with-deps-c-final",
    ],
};

/// Expected values for `544qcchwgcgpz3xi1bbml28f8jj6009p-attic-test-with-deps-b`.
///
/// This depends on `3k1wymic8p7h5pfcqfhh0jan8ny2a712-attic-test-with-deps-c-final`.
pub const WITH_DEPS_B: TestNar = TestNar {
    store_path: "/nix/store/544qcchwgcgpz3xi1bbml28f8jj6009p-attic-test-with-deps-b",
    _original_file: include_bytes!("nar/544qcchwgcgpz3xi1bbml28f8jj6009p-attic-test-with-deps-b"),
    nar: include_bytes!("nar/544qcchwgcgpz3xi1bbml28f8jj6009p-attic-test-with-deps-b.nar"),
    export: include_bytes!("nar/544qcchwgcgpz3xi1bbml28f8jj6009p-attic-test-with-deps-b.export"),
    closure: &[
        "544qcchwgcgpz3xi1bbml28f8jj6009p-attic-test-with-deps-b",
        "3k1wymic8p7h5pfcqfhh0jan8ny2a712-attic-test-with-deps-c-final",
    ],
};

/// Expected values for `3k1wymic8p7h5pfcqfhh0jan8ny2a712-attic-test-with-deps-c-final`.
pub const WITH_DEPS_C: TestNar = TestNar {
    store_path: "/nix/store/3k1wymic8p7h5pfcqfhh0jan8ny2a712-attic-test-with-deps-c-final",
    _original_file: include_bytes!(
        "nar/3k1wymic8p7h5pfcqfhh0jan8ny2a712-attic-test-with-deps-c-final"
    ),
    nar: include_bytes!("nar/3k1wymic8p7h5pfcqfhh0jan8ny2a712-attic-test-with-deps-c-final.nar"),
    export: include_bytes!(
        "nar/3k1wymic8p7h5pfcqfhh0jan8ny2a712-attic-test-with-deps-c-final.export"
    ),
    closure: &["3k1wymic8p7h5pfcqfhh0jan8ny2a712-attic-test-with-deps-c-final"],
};

/// A test NAR.
#[derive(Debug, Clone)]
pub struct TestNar {
    /// Full path in the Nix Store when imported.
    store_path: &'static str,

    /// The original file.
    _original_file: &'static [u8],

    /// A NAR dump without path metadata.
    nar: &'static [u8],

    /// An importable NAR dump produced by `nix-store --export`.
    export: &'static [u8],

    /// The expected closure.
    closure: &'static [&'static str],
}

/// A target that can receive and verify a NAR dump.
pub struct NarDump {
    /// The produced NAR dump.
    actual: NamedTempFile,

    /// The expected values.
    expected: TestNar,
}

pub struct NarDumpWriter {
    file: File,
    _lifetime: Arc<NarDump>,
}

impl TestNar {
    /// Attempts to import the NAR into the local Nix Store.
    ///
    /// This requires the current user to be trusted by the nix-daemon.
    pub async fn import(&self) -> io::Result<()> {
        let mut child = Command::new("nix-store")
            .arg("--import")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(self.export).await?;
        drop(stdin);

        let output = child.wait_with_output().await?;
        if !output.status.success() {
            let e = format!("Nix exit with code {:?}", output.status.code());
            return Err(io::Error::new(io::ErrorKind::Other, e));
        }

        // ensure that we imported the correct thing
        let store_path = String::from_utf8_lossy(&output.stdout);
        let store_path = store_path.trim_end();
        if store_path != self.store_path {
            let e = format!(
                "Import resulted in \"{}\", but we want \"{}\"",
                store_path, self.store_path
            );
            return Err(io::Error::new(io::ErrorKind::Other, e));
        }

        Ok(())
    }

    /// Returns the full store path that will be present when imported.
    pub fn path(&self) -> &Path {
        Path::new(self.store_path)
    }

    /// Returns the closure of the store path.
    pub fn closure(&self) -> HashSet<StorePath> {
        self.closure
            .iter()
            .map(|bp| {
                let bp = PathBuf::from(bp);
                StorePath::from_base_name(bp)
            })
            .collect::<AtticResult<HashSet<StorePath>>>()
            .unwrap()
    }

    /// Returns the raw expected NAR.
    pub fn nar(&self) -> &[u8] {
        self.nar
    }

    /// Creates a new test target.
    pub fn get_target(&self) -> io::Result<Arc<NarDump>> {
        let target = NarDump::new(self.clone())?;
        Ok(Arc::new(target))
    }
}

impl NarDump {
    /// Creates a new dump target.
    fn new(expected: TestNar) -> io::Result<Self> {
        Ok(Self {
            actual: NamedTempFile::new()?,
            expected,
        })
    }

    /// Returns a handle to write to the buffer.
    pub async fn get_writer(self: &Arc<Self>) -> io::Result<Box<NarDumpWriter>> {
        let file = OpenOptions::new()
            .read(false)
            .write(true)
            .open(self.actual.path())
            .await?;

        Ok(Box::new(NarDumpWriter {
            file,
            _lifetime: self.clone(),
        }))
    }

    /// Validates the resulting dump against expected values.
    pub async fn validate(&self) -> io::Result<()> {
        let mut file = File::open(self.actual.path()).await?;

        let metadata = file.metadata().await?;
        if metadata.len() != self.expected.nar.len() as u64 {
            let e = format!(
                "Length mismatch - Got {}, should be {}",
                metadata.len(),
                self.expected.nar.len()
            );
            return Err(io::Error::new(io::ErrorKind::InvalidData, e));
        }

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).await?;
        if bytes != self.expected.nar {
            assert_eq!(bytes.len(), self.expected.nar.len());

            for i in 0..bytes.len() {
                if bytes[i] != self.expected.nar[i] {
                    eprintln!(
                        "Byte {} mismatch - We got {}, should be {}",
                        i, bytes[i], self.expected.nar[i]
                    );
                }
            }

            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Content mismatch",
            ));
        }

        Ok(())
    }
}

impl AsyncWrite for NarDumpWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.file).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.file).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.file).poll_shutdown(cx)
    }
}
