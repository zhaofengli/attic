//! Module for implementing streaming decompression across multiple
//! algorithms

use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use anyhow::anyhow;
use async_compression::tokio::bufread::{
    BrotliDecoder, DeflateDecoder, GzipDecoder, XzDecoder, ZstdDecoder,
};
use pin_project::pin_project;
use tokio::io::{AsyncBufRead, AsyncRead, BufReader, ReadBuf};

use crate::error::{ErrorKind, ServerResult};

/// A streaming multi-codec decompressor
#[pin_project(project = SDProj)]
pub enum StreamingDecompressor<S: AsyncBufRead> {
    /// None decompression
    None(#[pin] S),
    /// Brotli decompression
    Brotli(#[pin] BrotliDecoder<S>),
    /// Deflate decompression
    Deflate(#[pin] DeflateDecoder<S>),
    /// Gzip decompression
    Gzip(#[pin] GzipDecoder<S>),
    /// XZ decompression
    Xz(#[pin] XzDecoder<S>),
    /// Zstd decompression
    Zstd(#[pin] ZstdDecoder<S>),
}

impl<S: AsyncBufRead> StreamingDecompressor<S> {
    /// Creates a new streaming decompressor from a buffered stream and compression type.
    ///
    /// An empty string or "identity" corresponds to no decompression.
    ///
    /// # Errors
    /// This function will return an error if the compression type is invalid
    pub fn new(inner: S, kind: &str) -> ServerResult<Self> {
        match kind {
            "" | "identity" => Ok(Self::None(inner)),
            "br" => Ok(Self::Brotli(BrotliDecoder::new(inner))),
            "deflate" => Ok(Self::Deflate(DeflateDecoder::new(inner))),
            "gzip" => Ok(Self::Gzip(GzipDecoder::new(inner))),
            "xz" => Ok(Self::Xz(XzDecoder::new(inner))),
            "zstd" => Ok(Self::Zstd(ZstdDecoder::new(inner))),
            _ => Err(ErrorKind::RequestError(anyhow!(
                "{} is unsupported transport compression",
                kind
            ))
            .into()),
        }
    }
}

impl<U: AsyncRead> StreamingDecompressor<BufReader<U>> {
    /// Creates a new streaming decompressor from an unbuffered stream and compression type.
    ///
    /// # Errors
    /// This function will return an error if the compression type is invalid
    pub fn new_unbuffered(inner: U, kind: &str) -> ServerResult<Self> {
        Self::new(BufReader::new(inner), kind)
    }
}

impl<S: AsyncBufRead> AsyncRead for StreamingDecompressor<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.project() {
            SDProj::None(i) => i.poll_read(cx, buf),
            SDProj::Brotli(i) => i.poll_read(cx, buf),
            SDProj::Deflate(i) => i.poll_read(cx, buf),
            SDProj::Gzip(i) => i.poll_read(cx, buf),
            SDProj::Xz(i) => i.poll_read(cx, buf),
            SDProj::Zstd(i) => i.poll_read(cx, buf),
        }
    }
}
