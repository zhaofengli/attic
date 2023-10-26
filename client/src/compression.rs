//! Module for implementing streaming decompression across multiple
//! algorithms

use async_compression::futures::bufread::{
    BrotliEncoder, DeflateEncoder, GzipEncoder, XzEncoder, ZstdEncoder,
};
use futures::io::{AsyncBufRead, AsyncRead, BufReader};
use pin_project::pin_project;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use crate::config::CompressionConfig;

/// A streaming multi-codec decompressor
#[pin_project(project = SCProj)]
pub enum StreamingCompressor<S: AsyncBufRead> {
    /// None decompression
    None(#[pin] S),
    /// Brotli decompression
    Brotli(#[pin] BrotliEncoder<S>),
    /// Deflate decompression
    Deflate(#[pin] DeflateEncoder<S>),
    /// Gzip decompression
    Gzip(#[pin] GzipEncoder<S>),
    /// XZ decompression
    Xz(#[pin] XzEncoder<S>),
    /// Zstd decompression
    Zstd(#[pin] ZstdEncoder<S>),
}

impl<S: AsyncBufRead> StreamingCompressor<S> {
    /// Creates a new streaming decompressor from a buffered stream and compression type.
    pub fn new(inner: S, kind: CompressionConfig) -> Self {
        match kind {
            CompressionConfig::None => Self::None(inner),
            CompressionConfig::Brotli => Self::Brotli(BrotliEncoder::new(inner)),
            CompressionConfig::Deflate => Self::Deflate(DeflateEncoder::new(inner)),
            CompressionConfig::Gzip => Self::Gzip(GzipEncoder::new(inner)),
            CompressionConfig::Xz => Self::Xz(XzEncoder::new(inner)),
            CompressionConfig::Zstd => Self::Zstd(ZstdEncoder::new(inner)),
        }
    }
}

impl<U: AsyncRead> StreamingCompressor<BufReader<U>> {
    /// Creates a new streaming decompressor from an unbuffered stream and compression type.
    ///
    /// # Errors
    /// This function will return an error if the compression type is invalid
    pub fn new_unbuffered(inner: U, kind: CompressionConfig) -> Self {
        Self::new(BufReader::new(inner), kind)
    }
}

impl<S: AsyncBufRead> AsyncRead for StreamingCompressor<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.project() {
            SCProj::None(i) => i.poll_read(cx, buf),
            SCProj::Brotli(i) => i.poll_read(cx, buf),
            SCProj::Deflate(i) => i.poll_read(cx, buf),
            SCProj::Gzip(i) => i.poll_read(cx, buf),
            SCProj::Xz(i) => i.poll_read(cx, buf),
            SCProj::Zstd(i) => i.poll_read(cx, buf),
        }
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [io::IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.project() {
            SCProj::None(i) => i.poll_read_vectored(cx, bufs),
            SCProj::Brotli(i) => i.poll_read_vectored(cx, bufs),
            SCProj::Deflate(i) => i.poll_read_vectored(cx, bufs),
            SCProj::Gzip(i) => i.poll_read_vectored(cx, bufs),
            SCProj::Xz(i) => i.poll_read_vectored(cx, bufs),
            SCProj::Zstd(i) => i.poll_read_vectored(cx, bufs),
        }
    }
}
