use std::marker::Unpin;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};

use digest::{Digest, Output as DigestOutput};
use pin_project::pin_project;
use tokio::io::{self, AsyncBufRead, AsyncRead, ReadBuf};
use tokio::sync::OnceCell;

/// AsyncRead filter that hashes the bytes that have been read.
///
/// The hash is finalized when EOF is reached.
#[pin_project(project = HashReaderProj)]
pub struct HashReader<R, D>
where
    R: AsyncRead + Unpin,
    D: Digest + Unpin,
{
    #[pin]
    inner: R,
    state: State<D>,
}

struct State<D>
where
    D: Digest + Unpin,
{
    digest: Option<D>,
    bytes_hashed: usize,
    bytes_consumed: usize,
    finalized: Arc<OnceCell<(DigestOutput<D>, usize)>>,
}

impl<D> State<D>
where
    D: Digest + Unpin,
{
    fn hash_unconsumed(&mut self, unconsumed: &[u8]) {
        let unhashed_offset = self.bytes_hashed - self.bytes_consumed;

        // It's technically possible for the `poll_read`/`poll_fill_buf` implementation
        // to return less data than the unconsumed portion returned by a previous
        // call to `AsyncBufRead::poll_fill_buf`.
        if unhashed_offset < unconsumed.len() {
            let unhashed = &unconsumed[unhashed_offset..];
            self.bytes_hashed += unhashed.len();

            let digest = self.digest.as_mut().expect("Stream has data after EOF");
            digest.update(unhashed);
        }
    }

    fn eof(&mut self) {
        if let Some(digest) = self.digest.take() {
            assert!(self.bytes_hashed == self.bytes_consumed, "bytes_hashed != bytes_consumed but EOF - Unconsumed bytes disappeared from buffer??");
            self.finalized
                .set((digest.finalize(), self.bytes_hashed))
                .expect("Hash has already been finalized");
        }
    }
}

impl<R, D> HashReader<R, D>
where
    R: AsyncRead + Unpin,
    D: Digest + Unpin,
{
    pub fn new(inner: R, digest: D) -> (Self, Arc<OnceCell<(DigestOutput<D>, usize)>>) {
        let finalized = Arc::new(OnceCell::new());

        (
            Self {
                inner,
                state: State {
                    digest: Some(digest),
                    bytes_hashed: 0,
                    bytes_consumed: 0,
                    finalized: finalized.clone(),
                },
            },
            finalized,
        )
    }
}

impl<R, D> AsyncRead for HashReader<R, D>
where
    R: AsyncRead + Unpin,
    D: Digest + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();

        let old_filled = buf.filled().len();
        ready!(this.inner.poll_read(cx, buf))?;

        let filled = buf.filled();
        let unconsumed = &filled[old_filled..];
        if unconsumed.len() == 0 {
            this.state.eof();
        } else {
            this.state.hash_unconsumed(unconsumed);
            this.state.bytes_consumed += unconsumed.len();
        }

        debug_assert!(this.state.bytes_consumed <= this.state.bytes_hashed);
        Poll::Ready(Ok(()))
    }
}

impl<R, D> AsyncBufRead for HashReader<R, D>
where
    R: AsyncBufRead + Unpin,
    D: Digest + Unpin,
{
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.project();
        let unconsumed = ready!(this.inner.poll_fill_buf(cx))?;

        if unconsumed.len() == 0 {
            this.state.eof();
        } else {
            this.state.hash_unconsumed(unconsumed);
        }

        debug_assert!(this.state.bytes_consumed <= this.state.bytes_hashed);
        Poll::Ready(Ok(unconsumed))
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let this = self.project();
        this.inner.consume(amt);
        this.state.bytes_consumed += amt;

        debug_assert!(this.state.bytes_consumed <= this.state.bytes_hashed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::io::{AsyncBufReadExt, AsyncReadExt};

    #[tokio::test]
    async fn test_hash_reader() {
        let expected = b"hello world";
        let expected_sha256 =
            hex::decode("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9")
                .unwrap();

        let (mut read, finalized) = HashReader::new(expected.as_slice(), sha2::Sha256::new());
        assert!(finalized.get().is_none());

        // force multiple reads
        let mut buf = vec![0u8; 100];
        let mut bytes_read = 0;
        bytes_read += read
            .read(&mut buf[bytes_read..bytes_read + 5])
            .await
            .unwrap();
        bytes_read += read
            .read(&mut buf[bytes_read..bytes_read + 5])
            .await
            .unwrap();
        bytes_read += read
            .read(&mut buf[bytes_read..bytes_read + 5])
            .await
            .unwrap();
        bytes_read += read
            .read(&mut buf[bytes_read..bytes_read + 5])
            .await
            .unwrap();

        assert_eq!(expected.len(), bytes_read);
        assert_eq!(expected, &buf[..bytes_read]);

        let (hash, count) = finalized.get().expect("Hash wasn't finalized");

        assert_eq!(expected_sha256.as_slice(), hash.as_slice());
        assert_eq!(expected.len(), *count);
        eprintln!("finalized = {:x?}", finalized);
    }

    #[tokio::test]
    async fn test_hash_reader_buf() {
        let expected = b"hello world";
        let expected_sha256 =
            hex::decode("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9")
                .unwrap();

        let (mut read, finalized) = HashReader::new(expected.as_slice(), sha2::Sha256::new());
        assert!(finalized.get().is_none());

        let mut buf = vec![0u8; 100];
        let mut bytes_read = 0;

        // Mix AsyncRead::read() and AsyncBufRead::fill_buf()

        bytes_read += read
            .read(&mut buf[bytes_read..bytes_read + 1])
            .await
            .unwrap();

        loop {
            // Perform multiple AsyncBufRead::fill_buf()s _without_ consuming
            let _ = read.fill_buf().await.unwrap();
            let _ = read.fill_buf().await.unwrap();
            let read_buf = read.fill_buf().await.unwrap();

            if read_buf.is_empty() {
                break;
            }

            buf[bytes_read] = read_buf[0];
            read.consume(1);
            bytes_read += 1;
        }

        assert_eq!(expected.len(), bytes_read);
        assert_eq!(expected, &buf[..bytes_read]);

        let (hash, count) = finalized.get().expect("Hash wasn't finalized");

        assert_eq!(expected_sha256.as_slice(), hash.as_slice());
        assert_eq!(expected.len(), *count);
        eprintln!("finalized = {:x?}", finalized);
    }
}
