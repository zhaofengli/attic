//! Stream utilities.

use std::marker::Unpin;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use digest::{Digest, Output as DigestOutput};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::OnceCell;

/// Stream filter that hashes the bytes that have been read.
///
/// The hash is finalized when EOF is reached.
pub struct StreamHasher<R: AsyncRead + Unpin, D: Digest + Unpin> {
    inner: R,
    digest: Option<D>,
    bytes_read: usize,
    finalized: Arc<OnceCell<(DigestOutput<D>, usize)>>,
}

impl<R: AsyncRead + Unpin, D: Digest + Unpin> StreamHasher<R, D> {
    pub fn new(inner: R, digest: D) -> (Self, Arc<OnceCell<(DigestOutput<D>, usize)>>) {
        let finalized = Arc::new(OnceCell::new());

        (
            Self {
                inner,
                digest: Some(digest),
                bytes_read: 0,
                finalized: finalized.clone(),
            },
            finalized,
        )
    }
}

impl<R: AsyncRead + Unpin, D: Digest + Unpin> AsyncRead for StreamHasher<R, D> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<tokio::io::Result<()>> {
        let old_filled = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        let read_len = buf.filled().len() - old_filled;

        match r {
            Poll::Ready(Ok(())) => {
                if read_len == 0 {
                    // EOF
                    if let Some(digest) = self.digest.take() {
                        self.finalized
                            .set((digest.finalize(), self.bytes_read))
                            .expect("Hash has already been finalized");
                    }
                } else {
                    // Read something
                    let digest = self.digest.as_mut().expect("Stream has data after EOF");

                    let filled = buf.filled();
                    digest.update(&filled[filled.len() - read_len..]);
                    self.bytes_read += read_len;
                }
            }
            Poll::Ready(Err(_)) => {
                assert!(read_len == 0);
            }
            Poll::Pending => {}
        }

        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::io::AsyncReadExt;
    use tokio_test::block_on;

    #[test]
    fn test_stream_hasher() {
        let expected = b"hello world";
        let expected_sha256 =
            hex::decode("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9")
                .unwrap();

        let (mut read, finalized) = StreamHasher::new(expected.as_slice(), sha2::Sha256::new());
        assert!(finalized.get().is_none());

        // force multiple reads
        let mut buf = vec![0u8; 100];
        let mut bytes_read = 0;
        bytes_read += block_on(read.read(&mut buf[bytes_read..bytes_read + 5])).unwrap();
        bytes_read += block_on(read.read(&mut buf[bytes_read..bytes_read + 5])).unwrap();
        bytes_read += block_on(read.read(&mut buf[bytes_read..bytes_read + 5])).unwrap();
        bytes_read += block_on(read.read(&mut buf[bytes_read..bytes_read + 5])).unwrap();

        assert_eq!(expected.len(), bytes_read);
        assert_eq!(expected, &buf[..bytes_read]);

        let (hash, count) = finalized.get().expect("Hash wasn't finalized");

        assert_eq!(expected_sha256.as_slice(), hash.as_slice());
        assert_eq!(expected.len(), *count);
        eprintln!("finalized = {:x?}", finalized);
    }
}
