use std::sync::Arc;

use digest::Output as DigestOutput;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufRead, AsyncRead, BufReader};
use tokio::sync::OnceCell;

use attic::io::HashReader;

pub type CompressorFn<C> = Box<dyn FnOnce(C) -> Box<dyn AsyncRead + Unpin + Send> + Send>;

/// Applies compression to a stream, computing hashes along the way.
///
/// Our strategy is to stream directly onto a UUID-keyed file on the
/// storage backend, performing compression and computing the hashes
/// along the way. We delete the file if the hashes do not match.
///
/// ```text
///                    ┌───────────────────────────────────►NAR Hash
///                    │
///                    │
///                    ├───────────────────────────────────►NAR Size
///                    │
///              ┌─────┴────┐  ┌──────────┐  ┌───────────┐
/// NAR Stream──►│NAR Hasher├─►│Compressor├─►│File Hasher├─►File Stream
///              └──────────┘  └──────────┘  └─────┬─────┘
///                                                │
///                                                ├───────►File Hash
///                                                │
///                                                │
///                                                └───────►File Size
/// ```
pub struct CompressionStream {
    stream: Box<dyn AsyncRead + Unpin + Send>,
    nar_compute: Arc<OnceCell<(DigestOutput<Sha256>, usize)>>,
    file_compute: Arc<OnceCell<(DigestOutput<Sha256>, usize)>>,
}

impl CompressionStream {
    /// Creates a new compression stream.
    pub fn new<R>(stream: R, compressor: CompressorFn<BufReader<HashReader<R, Sha256>>>) -> Self
    where
        R: AsyncBufRead + Unpin + Send + 'static,
    {
        // compute NAR hash and size
        let (stream, nar_compute) = HashReader::new(stream, Sha256::new());

        // compress NAR
        let stream = compressor(BufReader::new(stream));

        // compute file hash and size
        let (stream, file_compute) = HashReader::new(stream, Sha256::new());

        Self {
            stream: Box::new(stream),
            nar_compute,
            file_compute,
        }
    }

    /// Returns the stream of the compressed object.
    pub fn stream(&mut self) -> &mut (impl AsyncRead + Unpin) {
        &mut self.stream
    }

    /// Returns the NAR hash and size.
    ///
    /// The hash is only finalized when the stream is fully read.
    /// Otherwise, returns `None`.
    pub fn nar_hash_and_size(&self) -> Option<&(DigestOutput<Sha256>, usize)> {
        self.nar_compute.get()
    }

    /// Returns the file hash and size.
    ///
    /// The hash is only finalized when the stream is fully read.
    /// Otherwise, returns `None`.
    pub fn file_hash_and_size(&self) -> Option<&(DigestOutput<Sha256>, usize)> {
        self.file_compute.get()
    }
}
