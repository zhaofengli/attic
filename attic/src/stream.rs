//! Stream utilities.

use std::collections::VecDeque;
use std::future::Future;
use std::marker::Unpin;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_stream::try_stream;
use bytes::{Bytes, BytesMut};
use digest::{Digest, Output as DigestOutput};
use futures::stream::{BoxStream, Stream, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
use tokio::sync::OnceCell;
use tokio::task::spawn;

/// Stream filter that hashes the bytes that have been read.
///
/// The hash is finalized when EOF is reached.
pub struct StreamHasher<R: AsyncRead + Unpin, D: Digest + Unpin> {
    inner: R,
    digest: Option<D>,
    bytes_read: usize,
    finalized: Arc<OnceCell<(DigestOutput<D>, usize)>>,
}

/// Merge chunks lazily into a continuous stream.
///
/// For each chunk, a function is called to transform it into a
/// `Stream<Item = Result<Bytes>>`. This function does something like
/// opening the local file or sending a request to S3.
///
/// We call this function some time before the start of the chunk
/// is reached to eliminate delays between chunks so the merged
/// stream is smooth. We don't want to start streaming all chunks
/// at once as it's a waste of resources.
///
/// ```text
/// | S3 GET | Chunk | S3 GET | ... | S3 GET | Chunk
/// ```
///
/// ```text
/// | S3 GET | Chunk | Chunk | Chunk | Chunk
/// | S3 GET |-----------^       ^       ^
///              | S3 GET |------|       |
///              | S3 GET |--------------|
///
/// ```
///
/// TODO: Support range requests so we can have seekable NARs.
pub fn merge_chunks<C, F, S, Fut, E>(
    mut chunks: VecDeque<C>,
    streamer: F,
    streamer_arg: S,
    num_prefetch: usize,
) -> Pin<Box<impl Stream<Item = Result<Bytes, E>>>>
where
    F: Fn(C, S) -> Fut,
    S: Clone,
    Fut: Future<Output = Result<BoxStream<'static, Result<Bytes, E>>, E>> + Send + 'static,
    E: Send + 'static,
{
    let s = try_stream! {
        let mut streams = VecDeque::new(); // a queue of JoinHandles

        // otherwise type inference gets confused :/
        if false {
            let chunk = chunks.pop_front().unwrap();
            let stream = spawn(streamer(chunk, streamer_arg.clone()));
            streams.push_back(stream);
        }

        loop {
            if let Some(stream) = streams.pop_front() {
                let mut stream = stream.await.unwrap()?;
                while let Some(item) = stream.next().await {
                    let item = item?;
                    yield item;
                }
            }

            while streams.len() < num_prefetch {
                if let Some(chunk) = chunks.pop_front() {
                    let stream = spawn(streamer(chunk, streamer_arg.clone()));
                    streams.push_back(stream);
                } else {
                    break;
                }
            }

            if chunks.is_empty() && streams.is_empty() {
                // we are done!
                break;
            }
        }
    };
    Box::pin(s)
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

/// Greedily reads from a stream to fill a buffer.
pub async fn read_chunk_async<S: AsyncRead + Unpin + Send>(
    stream: &mut S,
    mut chunk: BytesMut,
) -> std::io::Result<Bytes> {
    while chunk.len() < chunk.capacity() {
        let read = stream.read_buf(&mut chunk).await?;

        if read == 0 {
            break;
        }
    }

    Ok(chunk.freeze())
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_stream::stream;
    use bytes::{BufMut, BytesMut};
    use futures::future;
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

    #[test]
    fn test_merge_chunks() {
        let chunk_a: BoxStream<Result<Bytes, ()>> = {
            let s = stream! {
                yield Ok(Bytes::from_static(b"Hello"));
            };
            Box::pin(s)
        };

        let chunk_b: BoxStream<Result<Bytes, ()>> = {
            let s = stream! {
                yield Ok(Bytes::from_static(b", "));
                yield Ok(Bytes::from_static(b"world"));
            };
            Box::pin(s)
        };

        let chunk_c: BoxStream<Result<Bytes, ()>> = {
            let s = stream! {
                yield Ok(Bytes::from_static(b"!"));
            };
            Box::pin(s)
        };

        let chunks: VecDeque<BoxStream<'static, Result<Bytes, ()>>> =
            [chunk_a, chunk_b, chunk_c].into_iter().collect();

        let streamer = |c, _| future::ok(c);
        let mut merged = merge_chunks(chunks, streamer, (), 2);

        let bytes = block_on(async move {
            let mut bytes = BytesMut::with_capacity(100);
            while let Some(item) = merged.next().await {
                bytes.put(item.unwrap());
            }
            bytes.freeze()
        });

        assert_eq!(&*bytes, b"Hello, world!");
    }
}
