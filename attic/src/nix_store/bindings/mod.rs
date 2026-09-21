//! `libnixstore` Bindings

use std::cell::UnsafeCell;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::stream::{Stream, StreamExt};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::{AtticError, AtticResult};

// The C++ implementation takes care of concurrency
#[repr(transparent)]
pub struct FfiNixStore(UnsafeCell<cxx::UniquePtr<ffi::CNixStore>>);

unsafe impl Send for FfiNixStore {}
unsafe impl Sync for FfiNixStore {}

impl FfiNixStore {
    // The C++ implementation takes care of concurrency
    #[allow(clippy::mut_from_ref)]
    pub fn store(&self) -> Pin<&mut ffi::CNixStore> {
        unsafe {
            let ptr = self.0.get().as_mut().unwrap();
            ptr.pin_mut()
        }
    }
}

/// Obtain a handle to the Nix store.
pub unsafe fn open_nix_store() -> AtticResult<FfiNixStore> {
    match ffi::open_nix_store() {
        Ok(ptr) => {
            let cell = UnsafeCell::new(ptr);
            Ok(FfiNixStore(cell))
        }
        Err(e) => Err(e.into()),
    }
}

// TODO: Benchmark different implementations
// (tokio, crossbeam, flume)
mod mpsc {
    // Tokio
    pub use tokio::sync::mpsc::{Receiver, Sender, channel, error::SendError};
}

/// Maximum number of NAR chunks buffered between the C++ serializer and the uploader.
const NAR_CHANNEL_CAPACITY: usize = 64;

const NAR_CHUNK_SIZE: usize = 64 * 1024;

/// Async write request.
#[derive(Debug)]
enum AsyncWriteMessage {
    Data(Vec<u8>),
    Error(String),
    Eof,
}

/// Async write request sender.
#[derive(Clone)]
pub struct AsyncWriteSender {
    sender: mpsc::Sender<AsyncWriteMessage>,
    buf: Vec<u8>,
}

impl AsyncWriteSender {
    fn send(&mut self, data: &[u8]) -> Result<(), mpsc::SendError<AsyncWriteMessage>> {
        let mut rest = data;

        while !rest.is_empty() {
            if self.buf.capacity() == 0 {
                self.buf.reserve_exact(NAR_CHUNK_SIZE);
            }

            let take = (NAR_CHUNK_SIZE - self.buf.len()).min(rest.len());

            self.buf.extend_from_slice(&rest[..take]);
            rest = &rest[take..];

            if self.buf.len() == NAR_CHUNK_SIZE {
                self.flush()?;
            }
        }

        Ok(())
    }

    fn eof(&mut self) -> Result<(), mpsc::SendError<AsyncWriteMessage>> {
        self.flush()?;
        self.sender.blocking_send(AsyncWriteMessage::Eof)
    }

    pub(crate) fn rust_error(
        &mut self,
        error: impl std::error::Error,
    ) -> Result<(), impl std::error::Error> {
        // No need to preserve buffered data when the consumer is about to abort.
        let message = AsyncWriteMessage::Error(error.to_string());
        self.sender.blocking_send(message)
    }

    fn flush(&mut self) -> Result<(), mpsc::SendError<AsyncWriteMessage>> {
        if self.buf.is_empty() {
            return Ok(());
        }

        // This runs on the spawn_blocking thread driving the C++ serializer,
        // so blocking here does not stall a Tokio runtime worker.
        let chunk = std::mem::take(&mut self.buf);
        self.sender.blocking_send(AsyncWriteMessage::Data(chunk))
    }
}

/// A wrapper of the `AsyncWrite` trait for the synchronous Nix C++ land.
pub struct AsyncWriteAdapter {
    receiver: mpsc::Receiver<AsyncWriteMessage>,
    eof: bool,
}

impl AsyncWriteAdapter {
    pub fn new() -> (Self, Box<AsyncWriteSender>) {
        let (sender, receiver) = mpsc::channel(NAR_CHANNEL_CAPACITY);

        let r = Self {
            receiver,
            eof: false,
        };
        let sender = Box::new(AsyncWriteSender {
            sender,
            buf: Vec::new(),
        });

        (r, sender)
    }

    /// Write everything the sender sends to us.
    pub async fn write_all(mut self, mut writer: Box<dyn AsyncWrite + Unpin>) -> AtticResult<()> {
        let writer = writer.as_mut();

        while let Some(data) = self.next().await {
            match data {
                Ok(v) => {
                    writer.write_all(&v).await?;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        if !self.eof {
            Err(io::Error::from(io::ErrorKind::BrokenPipe).into())
        } else {
            Ok(())
        }
    }
}

impl Stream for AsyncWriteAdapter {
    type Item = AtticResult<Vec<u8>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.receiver.poll_recv(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(message)) => {
                use AsyncWriteMessage::*;
                match message {
                    Data(v) => Poll::Ready(Some(Ok(v))),
                    Error(exception) => {
                        let error = AtticError::CxxError { exception };
                        Poll::Ready(Some(Err(error)))
                    }
                    Eof => {
                        self.eof = true;
                        Poll::Ready(None)
                    }
                }
            }
            Poll::Ready(None) => {
                if !self.eof {
                    Poll::Ready(Some(Err(io::Error::from(io::ErrorKind::BrokenPipe).into())))
                } else {
                    Poll::Ready(None)
                }
            }
        }
    }
}

#[cxx::bridge]
/// Generated by `cxx.rs`.
///
/// Mid-level wrapper of `libnixstore` implemented in C++.
mod ffi {
    extern "Rust" {
        type AsyncWriteSender;
        fn send(self: &mut AsyncWriteSender, data: &[u8]) -> Result<()>;
        fn eof(self: &mut AsyncWriteSender) -> Result<()>;
    }

    unsafe extern "C++" {
        include!("attic/src/nix_store/bindings/nix.hpp");

        // =========
        // CNixStore
        // =========

        /// Mid-level wrapper for the Unix Domain Socket Nix Store.
        type CNixStore;

        /// Returns the path of the Nix store itself.
        fn store_dir(self: Pin<&mut CNixStore>) -> String;

        /*
        /// Verifies that a path is indeed in the Nix store, then return the base store path.
        ///
        /// Use parse_store_path instead.
        fn to_store_path(self: Pin<&mut CNixStore>, path: &str) -> Result<String>;
        */

        /// Queries information about a valid path.
        fn query_path_info(
            self: Pin<&mut CNixStore>,
            store_path: &[u8],
        ) -> Result<UniquePtr<CPathInfo>>;

        /// Computes the closure of a valid path.
        ///
        /// If `flip_directions` is true, the set of paths that can reach `store_path` is
        /// returned.
        fn compute_fs_closure(
            self: Pin<&mut CNixStore>,
            store_path: &[u8],
            flip_direction: bool,
            include_outputs: bool,
            include_derivers: bool,
        ) -> Result<UniquePtr<CxxVector<CxxString>>>;

        /// Computes the closure of a list of valid paths.
        ///
        /// This is the multi-path variant of `compute_fs_closure`.
        /// If `flip_directions` is true, the set of paths that can reach `store_path` is
        /// returned.
        ///
        /// It's easier and more efficient to just pass a vector of slices
        /// instead of wrangling with concrete "extern rust" / "extern C++"
        /// types.
        fn compute_fs_closure_multi(
            self: Pin<&mut CNixStore>,
            base_names: &[&[u8]],
            flip_direction: bool,
            include_outputs: bool,
            include_derivers: bool,
        ) -> Result<UniquePtr<CxxVector<CxxString>>>;

        /// Creates a NAR dump from a path.
        fn nar_from_path(
            self: Pin<&mut CNixStore>,
            base_name: Vec<u8>,
            sender: Box<AsyncWriteSender>,
        ) -> Result<()>;

        /// Obtains a handle to the Nix store.
        fn open_nix_store() -> Result<UniquePtr<CNixStore>>;

        // =========
        // CPathInfo
        // =========

        /// Mid-level wrapper for the `nix::ValidPathInfo` struct.
        type CPathInfo;

        /// Returns the SHA-256 hash of the store path.
        fn nar_sha256_hash(self: Pin<&mut CPathInfo>) -> &[u8];

        /// Returns the size of the NAR.
        fn nar_size(self: Pin<&mut CPathInfo>) -> u64;

        /// Returns the references of the store path.
        fn references(self: Pin<&mut CPathInfo>) -> UniquePtr<CxxVector<CxxString>>;

        /// Returns the possibly invalid signatures attached to the store path.
        fn sigs(self: Pin<&mut CPathInfo>) -> UniquePtr<CxxVector<CxxString>>;

        /// Returns the CA field of the store path.
        fn ca(self: Pin<&mut CPathInfo>) -> String;
    }
}
