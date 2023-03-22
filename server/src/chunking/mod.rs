//! Chunking.
//!
//! We perform chunking on uncompressed NARs using the FastCDC
//! algorithm.

use async_stream::try_stream;
use bytes::{BufMut, Bytes, BytesMut};
use fastcdc::ronomon::FastCDC;
use futures::stream::Stream;
use tokio::io::AsyncRead;

use attic::stream::read_chunk_async;

/// Splits a streams into content-defined chunks.
///
/// This is a wrapper over fastcdc-rs that takes an `AsyncRead` and
/// returns a `Stream` of chunks as `Bytes`s.
pub fn chunk_stream<R>(
    mut stream: R,
    min_size: usize,
    avg_size: usize,
    max_size: usize,
) -> impl Stream<Item = std::io::Result<Bytes>>
where
    R: AsyncRead + Unpin + Send,
{
    let s = try_stream! {
        let mut buf = BytesMut::with_capacity(max_size);

        loop {
            let read = read_chunk_async(&mut stream, buf).await?;

            let mut eof = false;
            if read.is_empty() {
                // Already EOF
                break;
            } else if read.len() < max_size {
                // Last read
                eof = true;
            }

            let chunks = FastCDC::with_eof(&read, min_size, avg_size, max_size, eof);
            let mut consumed = 0;

            for chunk in chunks {
                consumed += chunk.length;

                let slice = read.slice(chunk.offset..chunk.offset + chunk.length);
                yield slice;
            }

            if eof {
                break;
            }

            buf = BytesMut::with_capacity(max_size);

            if consumed < read.len() {
                // remaining bytes for the next read
                buf.put_slice(&read[consumed..]);
            }
        }
    };

    Box::pin(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Cursor;

    use futures::StreamExt;
    use tokio_test::block_on;

    /// Chunks and reconstructs a file.
    #[test]
    fn test_chunking_basic() {
        fn case(size: usize) {
            block_on(async move {
                let test_file = get_data(size); // 32 MiB
                let mut reconstructed_file = Vec::new();

                let cursor = Cursor::new(&test_file);
                let mut chunks = chunk_stream(cursor, 8 * 1024, 16 * 1024, 32 * 1024);

                while let Some(chunk) = chunks.next().await {
                    let chunk = chunk.unwrap();
                    eprintln!("Got a {}-byte chunk", chunk.len());
                    reconstructed_file.extend(chunk);
                }

                assert_eq!(reconstructed_file, test_file);
            });
        }

        case(32 * 1024 * 1024 - 1);
        case(32 * 1024 * 1024);
        case(32 * 1024 * 1024 + 1);
    }

    /// Returns some fake data.
    fn get_data(len: usize) -> Vec<u8> {
        let mut state = 42u32;
        let mut data = vec![0u8; len];

        for i in 0..data.len() {
            (state, _) = state.overflowing_mul(1664525u32);
            (state, _) = state.overflowing_add(1013904223u32);
            data[i] = ((state >> (i % 24)) & 0xff) as u8;
        }

        data
    }
}
