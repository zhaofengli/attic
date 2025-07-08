//! Stream utilities.

mod hash_reader;

use std::collections::VecDeque;
use std::future::Future;
use std::marker::Unpin;
use std::pin::Pin;

use async_stream::try_stream;
use bytes::{Bytes, BytesMut};
use futures::stream::{BoxStream, Stream, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::task::spawn;

pub use hash_reader::HashReader;

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

    #[tokio::test]
    async fn test_merge_chunks() {
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

        let mut bytes = BytesMut::with_capacity(100);
        while let Some(item) = merged.next().await {
            bytes.put(item.unwrap());
        }
        let bytes = bytes.freeze();

        assert_eq!(&*bytes, b"Hello, world!");
    }
}
