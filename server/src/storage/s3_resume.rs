//! Resume support for interrupted S3 download streams.
//!
//! Once atticd has started streaming a NAR body to a nix client, the
//! HTTP response status and Content-Length have already been written
//! — there is no way to surface a mid-stream failure as a proper
//! error. The client just sees a truncated body and (silently) caches
//! a broken NAR.
//!
//! [`ResumableS3Read`] wraps the S3 body stream in an [`AsyncRead`]
//! that, on upstream interruption, transparently re-issues the
//! `GetObject` with a `Range` header to continue from where it left
//! off. The downstream connection stays open the whole time.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::get_object::{GetObjectError, GetObjectOutput};
use aws_sdk_s3::Client;
use tokio::io::{AsyncRead, ReadBuf};

use super::s3::StreamResumeConfig;

type IoResult<T> = io::Result<T>;
type BoxedRead = Box<dyn AsyncRead + Unpin + Send>;
type ReconnectFuture = Pin<Box<dyn Future<Output = IoResult<BoxedRead>> + Send>>;

/// Strategy for reopening a download from a given byte offset.
///
/// The production implementation issues an S3 `GetObject` with a
/// `Range` header (and an `If-Match` ETag guard). Tests inject a
/// fake implementation that yields canned byte streams or errors.
type Reconnect = Box<dyn Fn(u64) -> ReconnectFuture + Send>;

pub(super) struct ResumableS3Read {
    reconnect: Reconnect,
    /// Stored only for log/trace labels.
    key: String,
    /// Total size from the first response. `None` means we can't
    /// detect premature EOF (resume still works on hard errors).
    total_size: Option<u64>,

    inner: BoxedRead,
    bytes_read: u64,
    retries_remaining: u8,
    next_backoff: Duration,
    max_backoff: Duration,

    state: State,

    /// Number of reconnects performed for this request. Stays at 0
    /// on the happy path; bumped by `start_reconnect`. Surfaced to
    /// operators via the post-resume completion log.
    resumed_count: u32,
    /// Guards the post-resume completion log against firing on
    /// repeated EOF polls from `read_to_end`.
    completion_logged: bool,
}

enum State {
    Reading,
    Reconnecting(ReconnectFuture),
    Dead { kind: io::ErrorKind, msg: String },
}

impl ResumableS3Read {
    pub(super) fn from_first_response(
        client: Client,
        bucket: String,
        key: String,
        first: GetObjectOutput,
        config: StreamResumeConfig,
    ) -> Self {
        let etag = first.e_tag().map(|s| s.to_string());
        let total_size = first.content_length().and_then(|n| u64::try_from(n).ok());

        if etag.is_none() {
            tracing::warn!(
                bucket = %bucket,
                key = %key,
                "S3 response has no ETag; resumed Range requests will not be guarded against object replacement"
            );
        }

        let inner: BoxedRead = Box::new(first.body.into_async_read());

        let reconnect = make_s3_reconnect(client, bucket, key.clone(), etag);

        Self::from_parts(reconnect, inner, key, total_size, config)
    }

    fn from_parts(
        reconnect: Reconnect,
        inner: BoxedRead,
        key: String,
        total_size: Option<u64>,
        config: StreamResumeConfig,
    ) -> Self {
        Self {
            reconnect,
            key,
            total_size,
            inner,
            bytes_read: 0,
            retries_remaining: config.max_retries,
            next_backoff: Duration::from_millis(config.initial_backoff_ms),
            max_backoff: Duration::from_millis(config.max_backoff_ms),
            state: State::Reading,
            resumed_count: 0,
            completion_logged: false,
        }
    }

    fn start_reconnect(&mut self) {
        self.retries_remaining = self.retries_remaining.saturating_sub(1);
        self.resumed_count = self.resumed_count.saturating_add(1);

        let delay = self.next_backoff;
        self.next_backoff = (self.next_backoff * 2).min(self.max_backoff);

        let key = self.key.clone();
        let bytes_read = self.bytes_read;
        let reconnect_fut = (self.reconnect)(self.bytes_read);

        let fut: ReconnectFuture = Box::pin(async move {
            tokio::time::sleep(delay).await;
            tracing::info!(
                %key,
                bytes_read,
                "Resuming S3 download with Range request"
            );
            reconnect_fut.await
        });

        self.state = State::Reconnecting(fut);
    }

    fn enter_dead(&mut self, e: &io::Error) {
        self.state = State::Dead {
            kind: e.kind(),
            msg: e.to_string(),
        };
    }
}

type FetchFuture<T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send>>;
type Fetcher<T, E> = Box<dyn Fn() -> FetchFuture<T, E> + Send>;

/// Issue a `GetObject` against S3 with bounded retries on transient
/// failures.
///
/// `ResumableS3Read` only kicks in *after* the first response body
/// has started arriving — a `DispatchFailure` on the very first call
/// (e.g. the S3 endpoint briefly refusing connections) never reaches
/// it. When the server is reassembling a multi-chunk NAR, the
/// response headers have already been written to the nix client by
/// the time we fetch the second-onward chunk, so a failure here
/// truncates the body just like a mid-stream drop. Retrying buys us
/// resilience against the same class of transient errors that the
/// stream-resume path already handles.
pub(super) async fn get_object_with_retry(
    client: &Client,
    bucket: &str,
    key: &str,
    config: &StreamResumeConfig,
) -> Result<GetObjectOutput, SdkError<GetObjectError>> {
    let fetcher = make_s3_get_object_fetcher(client.clone(), bucket.to_string(), key.to_string());
    let log_label_bucket = bucket.to_string();
    let log_label_key = key.to_string();
    retry_with_backoff(
        fetcher,
        is_sdk_err_retryable,
        move |err| {
            tracing::warn!(
                bucket = %log_label_bucket,
                key = %log_label_key,
                error = %err,
                "S3 GetObject failed; retrying"
            );
        },
        config,
    )
    .await
}

fn make_s3_get_object_fetcher(
    client: Client,
    bucket: String,
    key: String,
) -> Fetcher<GetObjectOutput, SdkError<GetObjectError>> {
    Box::new(move || {
        let client = client.clone();
        let bucket = bucket.clone();
        let key = key.clone();
        Box::pin(async move { client.get_object().bucket(&bucket).key(&key).send().await })
    })
}

async fn retry_with_backoff<T, E>(
    fetcher: Fetcher<T, E>,
    is_retryable: impl Fn(&E) -> bool,
    mut on_retry: impl FnMut(&E),
    config: &StreamResumeConfig,
) -> Result<T, E> {
    let mut attempts_remaining = config.max_retries;
    let mut backoff = Duration::from_millis(config.initial_backoff_ms);
    let max_backoff = Duration::from_millis(config.max_backoff_ms);

    loop {
        match fetcher().await {
            Ok(output) => return Ok(output),
            Err(err) if attempts_remaining > 0 && is_retryable(&err) => {
                on_retry(&err);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                attempts_remaining -= 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Mirror of [`is_retryable`] for `SdkError` before we have a body.
///
/// 4xx (other than 408/429) means the request is wrong — retrying
/// won't help. Everything else (network/timeout/parse/5xx) is treated
/// as transient.
fn is_sdk_err_retryable(err: &SdkError<GetObjectError>) -> bool {
    match err {
        SdkError::DispatchFailure(_) | SdkError::TimeoutError(_) | SdkError::ResponseError(_) => {
            true
        }
        SdkError::ServiceError(svc) => {
            let status = svc.raw().status().as_u16();
            status >= 500 || status == 408 || status == 429
        }
        _ => false,
    }
}

fn make_s3_reconnect(
    client: Client,
    bucket: String,
    key: String,
    etag: Option<String>,
) -> Reconnect {
    Box::new(move |range_start: u64| {
        let client = client.clone();
        let bucket = bucket.clone();
        let key = key.clone();
        let etag = etag.clone();
        Box::pin(async move {
            let range = format!("bytes={range_start}-");
            let mut req = client.get_object().bucket(&bucket).key(&key).range(&range);
            if let Some(t) = &etag {
                req = req.if_match(t);
            }
            let output = req.send().await.map_err(sdk_err_to_io)?;
            let new_inner: BoxedRead = Box::new(output.body.into_async_read());
            Ok(new_inner)
        })
    })
}

impl AsyncRead for ResumableS3Read {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        loop {
            // Drive an in-flight reconnect to completion before reading.
            if let State::Reconnecting(fut) = &mut self.state {
                match fut.as_mut().poll(cx) {
                    Poll::Ready(Ok(new_inner)) => {
                        self.inner = new_inner;
                        self.state = State::Reading;
                    }
                    Poll::Ready(Err(e)) => {
                        if self.retries_remaining > 0 && is_retryable(&e) {
                            tracing::warn!(
                                key = %self.key,
                                bytes_read = self.bytes_read,
                                error = %e,
                                "S3 resume attempt failed; retrying"
                            );
                            self.start_reconnect();
                            continue;
                        }
                        self.enter_dead(&e);
                        return Poll::Ready(Err(e));
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }

            match &self.state {
                State::Dead { kind, msg } => {
                    return Poll::Ready(Err(io::Error::new(*kind, msg.clone())));
                }
                State::Reading => {}
                State::Reconnecting(_) => unreachable!("handled above"),
            }

            let before = buf.filled().len();
            match Pin::new(&mut self.inner).poll_read(cx, buf) {
                Poll::Ready(Ok(())) => {
                    let read = buf.filled().len() - before;
                    self.bytes_read += read as u64;

                    let premature_eof =
                        read == 0 && self.total_size.is_some_and(|total| self.bytes_read < total);

                    if premature_eof && self.retries_remaining > 0 {
                        tracing::warn!(
                            key = %self.key,
                            bytes_read = self.bytes_read,
                            total = ?self.total_size,
                            "S3 stream ended prematurely; resuming"
                        );
                        self.start_reconnect();
                        continue;
                    }

                    // Real EOF (not a truncation we gave up on) and
                    // we got here by reconnecting at least once: emit
                    // one info-level line so an operator can grep
                    // `S3 download completed after N resume(s)` to
                    // attest from logs that the resume path actually
                    // delivered a body, not just attempted to.
                    if read == 0
                        && !premature_eof
                        && self.resumed_count > 0
                        && !self.completion_logged
                    {
                        self.completion_logged = true;
                        tracing::info!(
                            key = %self.key,
                            resumes = self.resumed_count,
                            bytes_read = self.bytes_read,
                            "S3 download completed after {} resume(s)",
                            self.resumed_count
                        );
                    }

                    return Poll::Ready(Ok(()));
                }
                Poll::Ready(Err(e)) => {
                    if self.retries_remaining > 0 && is_retryable(&e) {
                        tracing::warn!(
                            key = %self.key,
                            bytes_read = self.bytes_read,
                            error = %e,
                            "S3 stream interrupted; resuming with Range"
                        );
                        self.start_reconnect();
                        continue;
                    }
                    self.enter_dead(&e);
                    return Poll::Ready(Err(e));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

fn is_retryable(e: &io::Error) -> bool {
    use io::ErrorKind::*;
    match e.kind() {
        ConnectionReset | ConnectionAborted | BrokenPipe | TimedOut | UnexpectedEof
        | Interrupted => true,

        // FIXME: This retries ALL Other-kind io::Errors, including cases
        // we probably shouldn't retry (e.g. malformed body, decoder
        // errors). The AWS Rust SDK flattens body-stream errors into
        // io::Error::new(Other, ...) without exposing typed variants,
        // so precise classification would require walking the source
        // chain and string-matching hyper/h2 messages — brittle across
        // SDK upgrades. Revisit when the SDK exposes typed body errors
        // or telemetry shows what we're actually retrying.
        Other => true,

        _ => false,
    }
}

/// Convert an SDK error from the reconnect `GetObject` into an
/// [`io::Error`] whose kind drives the retry decision via
/// [`is_retryable`].
///
/// 412 PreconditionFailed (our `If-Match` ETag guard) becomes
/// `InvalidData` — non-retryable, since the object has been replaced
/// in S3 and resuming would splice bytes from two different objects.
/// Other 4xx (except 408/429) become `PermissionDenied` — also
/// non-retryable. Everything else becomes `Other` and is retried.
fn sdk_err_to_io(err: SdkError<GetObjectError>) -> io::Error {
    use io::ErrorKind;

    if let SdkError::ServiceError(svc) = &err {
        let status = svc.raw().status().as_u16();
        if status == 412 {
            return io::Error::new(
                ErrorKind::InvalidData,
                format!("S3 If-Match precondition failed (object changed): {err}"),
            );
        }
        if (400..500).contains(&status) && status != 408 && status != 429 {
            return io::Error::new(
                ErrorKind::PermissionDenied,
                format!("S3 fatal error during resume: {err}"),
            );
        }
    }

    io::Error::other(format!("S3 transient error during resume: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use tokio::io::AsyncReadExt;

    /// Per-poll behavior for the fake inner stream.
    enum MockChunk {
        Data(Vec<u8>),
        Error(io::ErrorKind),
        // Absence of further chunks = clean EOF.
    }

    struct MockReader {
        chunks: VecDeque<MockChunk>,
    }

    impl AsyncRead for MockReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            match self.chunks.pop_front() {
                Some(MockChunk::Data(d)) => {
                    let n = d.len().min(buf.remaining());
                    buf.put_slice(&d[..n]);
                    // If the caller's buffer was smaller, push the
                    // unread tail back so the next poll picks it up.
                    if n < d.len() {
                        self.chunks.push_front(MockChunk::Data(d[n..].to_vec()));
                    }
                    Poll::Ready(Ok(()))
                }
                Some(MockChunk::Error(kind)) => {
                    Poll::Ready(Err(io::Error::new(kind, "mock stream error")))
                }
                None => Poll::Ready(Ok(())), // clean EOF
            }
        }
    }

    /// Per-reconnect-call response: either a fresh canned stream or
    /// a failure of the `GetObject` itself.
    enum ReconnectResponse {
        Success(Vec<MockChunk>),
        Failure(io::ErrorKind),
    }

    fn fake_reconnect(responses: Vec<ReconnectResponse>) -> (Reconnect, Arc<Mutex<Vec<u64>>>) {
        let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
        let calls = Arc::new(Mutex::new(Vec::new()));

        let queue_for_closure = queue.clone();
        let calls_for_closure = calls.clone();

        let reconnect: Reconnect = Box::new(move |range_start: u64| {
            calls_for_closure.lock().unwrap().push(range_start);
            let next = queue_for_closure.lock().unwrap().pop_front();
            Box::pin(async move {
                match next {
                    Some(ReconnectResponse::Success(chunks)) => {
                        let reader = MockReader {
                            chunks: chunks.into_iter().collect(),
                        };
                        Ok(Box::new(reader) as BoxedRead)
                    }
                    Some(ReconnectResponse::Failure(kind)) => {
                        Err(io::Error::new(kind, "mock reconnect failure"))
                    }
                    None => Err(io::Error::other("no more mock responses queued")),
                }
            })
        });

        (reconnect, calls)
    }

    fn cfg(max_retries: u8) -> StreamResumeConfig {
        StreamResumeConfig {
            max_retries,
            initial_backoff_ms: 1,
            max_backoff_ms: 1,
        }
    }

    fn boxed_mock(chunks: Vec<MockChunk>) -> BoxedRead {
        Box::new(MockReader {
            chunks: chunks.into_iter().collect(),
        })
    }

    #[tokio::test]
    async fn happy_path_no_resume() {
        let (reconnect, calls) = fake_reconnect(vec![]);
        let inner = boxed_mock(vec![
            MockChunk::Data(b"hello ".to_vec()),
            MockChunk::Data(b"world".to_vec()),
        ]);
        let mut wrapper =
            ResumableS3Read::from_parts(reconnect, inner, "k".into(), Some(11), cfg(3));

        let mut out = Vec::new();
        wrapper.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"hello world");
        assert!(calls.lock().unwrap().is_empty(), "no reconnects expected");
        assert_eq!(wrapper.resumed_count, 0);
        assert!(!wrapper.completion_logged);
    }

    #[tokio::test]
    async fn mid_stream_error_triggers_resume() {
        let (reconnect, calls) =
            fake_reconnect(vec![ReconnectResponse::Success(vec![MockChunk::Data(
                b"world".to_vec(),
            )])]);
        let inner = boxed_mock(vec![
            MockChunk::Data(b"hello ".to_vec()),
            MockChunk::Error(io::ErrorKind::ConnectionReset),
        ]);
        let mut wrapper =
            ResumableS3Read::from_parts(reconnect, inner, "k".into(), Some(11), cfg(3));

        let mut out = Vec::new();
        wrapper.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"hello world");
        assert_eq!(
            *calls.lock().unwrap(),
            vec![6],
            "reconnect should resume from byte 6"
        );
        assert_eq!(wrapper.resumed_count, 1);
        assert!(
            wrapper.completion_logged,
            "completion log must fire after a successful resume"
        );
    }

    #[tokio::test]
    async fn premature_eof_triggers_resume() {
        // Inner returns 6 bytes then clean EOF; total_size says 11.
        let (reconnect, calls) =
            fake_reconnect(vec![ReconnectResponse::Success(vec![MockChunk::Data(
                b"world".to_vec(),
            )])]);
        let inner = boxed_mock(vec![MockChunk::Data(b"hello ".to_vec())]);
        let mut wrapper =
            ResumableS3Read::from_parts(reconnect, inner, "k".into(), Some(11), cfg(3));

        let mut out = Vec::new();
        wrapper.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"hello world");
        assert_eq!(*calls.lock().unwrap(), vec![6]);
        assert_eq!(wrapper.resumed_count, 1);
        assert!(wrapper.completion_logged);
    }

    #[tokio::test]
    async fn other_kind_error_is_retried() {
        // Whatever the SDK shoves into `Other` should be retried.
        let (reconnect, calls) =
            fake_reconnect(vec![ReconnectResponse::Success(vec![MockChunk::Data(
                b"world".to_vec(),
            )])]);
        let inner = boxed_mock(vec![
            MockChunk::Data(b"hello ".to_vec()),
            MockChunk::Error(io::ErrorKind::Other),
        ]);
        let mut wrapper =
            ResumableS3Read::from_parts(reconnect, inner, "k".into(), Some(11), cfg(3));

        let mut out = Vec::new();
        wrapper.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"hello world");
        assert_eq!(*calls.lock().unwrap(), vec![6]);
    }

    #[tokio::test]
    async fn max_retries_zero_disables_resume() {
        let (reconnect, calls) = fake_reconnect(vec![]);
        let inner = boxed_mock(vec![
            MockChunk::Data(b"hello ".to_vec()),
            MockChunk::Error(io::ErrorKind::ConnectionReset),
        ]);
        let mut wrapper =
            ResumableS3Read::from_parts(reconnect, inner, "k".into(), Some(11), cfg(0));

        let mut out = Vec::new();
        let err = wrapper.read_to_end(&mut out).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(out, b"hello ");
        assert!(
            calls.lock().unwrap().is_empty(),
            "no reconnects when disabled"
        );
    }

    #[tokio::test]
    async fn non_retryable_error_propagates() {
        let (reconnect, calls) = fake_reconnect(vec![]);
        let inner = boxed_mock(vec![
            MockChunk::Data(b"hello ".to_vec()),
            MockChunk::Error(io::ErrorKind::PermissionDenied),
        ]);
        let mut wrapper =
            ResumableS3Read::from_parts(reconnect, inner, "k".into(), Some(11), cfg(3));

        let mut out = Vec::new();
        let err = wrapper.read_to_end(&mut out).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            calls.lock().unwrap().is_empty(),
            "PermissionDenied should not trigger reconnect"
        );
    }

    #[tokio::test]
    async fn retry_budget_exhausted_propagates_last_error() {
        // Inner errors immediately. Two reconnect attempts both yield
        // failing inner streams. After max_retries=2, the third error
        // surfaces.
        let (reconnect, calls) = fake_reconnect(vec![
            ReconnectResponse::Success(vec![MockChunk::Error(io::ErrorKind::ConnectionReset)]),
            ReconnectResponse::Success(vec![MockChunk::Error(io::ErrorKind::ConnectionReset)]),
        ]);
        let inner = boxed_mock(vec![MockChunk::Error(io::ErrorKind::ConnectionReset)]);
        let mut wrapper =
            ResumableS3Read::from_parts(reconnect, inner, "k".into(), Some(100), cfg(2));

        let mut buf = [0u8; 16];
        let err = wrapper.read(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(
            calls.lock().unwrap().len(),
            2,
            "should exhaust both retries"
        );
    }

    #[tokio::test]
    async fn reconnect_failure_then_success() {
        // First reconnect call fails with retryable error; second
        // succeeds. Wrapper should keep going.
        let (reconnect, calls) = fake_reconnect(vec![
            ReconnectResponse::Failure(io::ErrorKind::TimedOut),
            ReconnectResponse::Success(vec![MockChunk::Data(b"world".to_vec())]),
        ]);
        let inner = boxed_mock(vec![
            MockChunk::Data(b"hello ".to_vec()),
            MockChunk::Error(io::ErrorKind::ConnectionReset),
        ]);
        let mut wrapper =
            ResumableS3Read::from_parts(reconnect, inner, "k".into(), Some(11), cfg(3));

        let mut out = Vec::new();
        wrapper.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"hello world");
        // Both calls resume from byte 6.
        assert_eq!(*calls.lock().unwrap(), vec![6, 6]);
    }

    #[tokio::test]
    async fn reconnect_non_retryable_failure_propagates() {
        // Mimics a 412 If-Match failure that sdk_err_to_io would map
        // to InvalidData — wrapper must NOT keep retrying.
        let (reconnect, calls) =
            fake_reconnect(vec![ReconnectResponse::Failure(io::ErrorKind::InvalidData)]);
        let inner = boxed_mock(vec![
            MockChunk::Data(b"hello ".to_vec()),
            MockChunk::Error(io::ErrorKind::ConnectionReset),
        ]);
        let mut wrapper =
            ResumableS3Read::from_parts(reconnect, inner, "k".into(), Some(11), cfg(3));

        let mut out = Vec::new();
        let err = wrapper.read_to_end(&mut out).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "should not retry past InvalidData"
        );
    }

    #[tokio::test]
    async fn unknown_total_size_skips_premature_eof_detection() {
        // No total_size: a clean short read is treated as real EOF,
        // not as a premature truncation to resume from.
        let (reconnect, calls) = fake_reconnect(vec![]);
        let inner = boxed_mock(vec![MockChunk::Data(b"hello".to_vec())]);
        let mut wrapper = ResumableS3Read::from_parts(reconnect, inner, "k".into(), None, cfg(3));

        let mut out = Vec::new();
        wrapper.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"hello");
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn is_retryable_classification() {
        use io::ErrorKind::*;
        for k in [
            ConnectionReset,
            ConnectionAborted,
            BrokenPipe,
            TimedOut,
            UnexpectedEof,
            Interrupted,
            Other,
        ] {
            assert!(is_retryable(&io::Error::new(k, "x")), "{k:?} should retry");
        }
        for k in [NotFound, PermissionDenied, InvalidData, InvalidInput] {
            assert!(
                !is_retryable(&io::Error::new(k, "x")),
                "{k:?} should not retry"
            );
        }
    }

    // --- Tests for the initial-GetObject retry path ----------------

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Build a fetcher that returns the queued responses in order,
    /// and a counter of how many times it's been called.
    fn fake_fetcher<T, E>(responses: Vec<Result<T, E>>) -> (Fetcher<T, E>, Arc<AtomicUsize>)
    where
        T: Send + 'static,
        E: Send + 'static,
    {
        let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
        let calls = Arc::new(AtomicUsize::new(0));

        let queue_for_closure = queue.clone();
        let calls_for_closure = calls.clone();

        let fetcher: Fetcher<T, E> = Box::new(move || {
            calls_for_closure.fetch_add(1, Ordering::SeqCst);
            let next = queue_for_closure.lock().unwrap().pop_front();
            Box::pin(async move { next.expect("no more mock responses queued") })
        });

        (fetcher, calls)
    }

    #[tokio::test]
    async fn retry_happy_path_no_retry() {
        let (fetcher, calls) = fake_fetcher::<&str, &str>(vec![Ok("ok")]);
        let result = retry_with_backoff(
            fetcher,
            |_| true,
            |_| panic!("on_retry should not fire"),
            &cfg(3),
        )
        .await;
        assert_eq!(result, Ok("ok"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_transient_then_success() {
        let (fetcher, calls) = fake_fetcher::<&str, &str>(vec![Err("transient"), Ok("ok")]);
        let retries = Arc::new(AtomicUsize::new(0));
        let retries_clone = retries.clone();
        let result = retry_with_backoff(
            fetcher,
            |_| true,
            move |_| {
                retries_clone.fetch_add(1, Ordering::SeqCst);
            },
            &cfg(3),
        )
        .await;
        assert_eq!(result, Ok("ok"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(retries.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_non_retryable_propagates_immediately() {
        let (fetcher, calls) = fake_fetcher::<&str, &str>(vec![Err("fatal")]);
        let result = retry_with_backoff(
            fetcher,
            |_| false,
            |_| panic!("on_retry should not fire"),
            &cfg(3),
        )
        .await;
        assert_eq!(result, Err("fatal"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_budget_exhausted_returns_last_error() {
        let (fetcher, calls) = fake_fetcher::<&str, &str>(vec![
            Err("e1"),
            Err("e2"),
            Err("e3"), // returned when budget hits zero
        ]);
        let result = retry_with_backoff(fetcher, |_| true, |_| {}, &cfg(2)).await;
        assert_eq!(result, Err("e3"));
        // 1 initial + 2 retries = 3 calls
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_max_retries_zero_disables_retry() {
        let (fetcher, calls) = fake_fetcher::<&str, &str>(vec![Err("transient")]);
        let result = retry_with_backoff(
            fetcher,
            |_| true,
            |_| panic!("on_retry should not fire"),
            &cfg(0),
        )
        .await;
        assert_eq!(result, Err("transient"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // --- Tests for SdkError classification -------------------------

    fn dispatch_io_err() -> SdkError<GetObjectError> {
        use aws_sdk_s3::error::ConnectorError;
        SdkError::dispatch_failure(ConnectorError::io(Box::new(io::Error::other("refused"))))
    }

    fn dispatch_timeout_err() -> SdkError<GetObjectError> {
        use aws_sdk_s3::error::ConnectorError;
        SdkError::dispatch_failure(ConnectorError::timeout(Box::new(io::Error::other("slow"))))
    }

    fn timeout_err() -> SdkError<GetObjectError> {
        SdkError::timeout_error(Box::<dyn std::error::Error + Send + Sync>::from("timeout"))
    }

    fn construction_failure_err() -> SdkError<GetObjectError> {
        SdkError::construction_failure(Box::<dyn std::error::Error + Send + Sync>::from("bug"))
    }

    #[test]
    fn sdk_err_classification() {
        assert!(
            is_sdk_err_retryable(&dispatch_io_err()),
            "dispatch IO failure (Connection refused class) must retry"
        );
        assert!(
            is_sdk_err_retryable(&dispatch_timeout_err()),
            "dispatch timeout must retry"
        );
        assert!(
            is_sdk_err_retryable(&timeout_err()),
            "timeout error must retry"
        );
        assert!(
            !is_sdk_err_retryable(&construction_failure_err()),
            "construction failure is a programming error and must not retry"
        );
    }

    #[tokio::test]
    async fn retry_with_sdk_predicate_retries_dispatch_failure() {
        // End-to-end check: when the queue yields a real dispatch
        // failure followed by Ok, the retry loop drives it through
        // using the production predicate.
        let (fetcher, calls) =
            fake_fetcher::<u32, SdkError<GetObjectError>>(vec![Err(dispatch_io_err()), Ok(42)]);
        let result = retry_with_backoff(fetcher, is_sdk_err_retryable, |_| {}, &cfg(3)).await;
        assert_eq!(result.ok(), Some(42));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_with_sdk_predicate_does_not_retry_construction_failure() {
        let (fetcher, calls) =
            fake_fetcher::<u32, SdkError<GetObjectError>>(vec![Err(construction_failure_err())]);
        let result = retry_with_backoff(fetcher, is_sdk_err_retryable, |_| {}, &cfg(3)).await;
        assert!(matches!(result, Err(SdkError::ConstructionFailure(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // --- End-to-end resume against an in-process HTTP server -------
    //
    // The mock-based tests above exercise the wrapper's state machine
    // but bypass the AWS SDK and the real body-stream plumbing. This
    // test stands up a minimal HTTP/1.1 server on a loopback port,
    // points a real aws_sdk_s3::Client at it, and verifies that:
    //   1. A response truncated below its Content-Length is detected
    //      by hyper and surfaces as a retryable io::Error (or is
    //      caught by premature-EOF detection).
    //   2. The reconnect closure issues a real `Range: bytes=N-`
    //      request that the server can serve, and the wrapper
    //      seamlessly continues the body.

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn end_to_end_resume_against_fake_s3() {
        use tokio::io::{AsyncReadExt as TokAsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // 4 KiB body so the SDK has something to actually stream.
        let body: Vec<u8> = (0u32..4096).map(|i| (i & 0xff) as u8).collect();
        let total = body.len();
        let truncate_at = 256usize;
        let etag = "fake-etag-123";

        let body_for_server = body.clone();
        let server = tokio::spawn(async move {
            async fn read_until_headers_end(sock: &mut tokio::net::TcpStream) -> String {
                let mut buf = Vec::with_capacity(8192);
                let mut tmp = [0u8; 1024];
                loop {
                    let n = sock.read(&mut tmp).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                String::from_utf8_lossy(&buf).into_owned()
            }

            // First request: announce full Content-Length, then write
            // only `truncate_at` bytes and close the socket cleanly.
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = read_until_headers_end(&mut sock).await;
            let headers = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Length: {total}\r\n\
                 ETag: \"{etag}\"\r\n\
                 Content-Type: application/octet-stream\r\n\
                 Connection: close\r\n\
                 \r\n",
            );
            sock.write_all(headers.as_bytes()).await.unwrap();
            sock.write_all(&body_for_server[..truncate_at])
                .await
                .unwrap();
            sock.shutdown().await.unwrap();
            drop(sock);

            // Second request: parse Range, serve the remainder.
            let (mut sock, _) = listener.accept().await.unwrap();
            let req = read_until_headers_end(&mut sock).await;
            let start: usize = req
                .lines()
                .find_map(|line| {
                    let lower = line.to_ascii_lowercase();
                    lower
                        .strip_prefix("range: bytes=")
                        .map(|s| s.trim_end_matches('-').trim().parse().unwrap())
                })
                .expect("resume request must carry Range: bytes=N-");
            let remainder = &body_for_server[start..];
            let headers = format!(
                "HTTP/1.1 206 Partial Content\r\n\
                 Content-Length: {}\r\n\
                 Content-Range: bytes {}-{}/{}\r\n\
                 ETag: \"{etag}\"\r\n\
                 Content-Type: application/octet-stream\r\n\
                 Connection: close\r\n\
                 \r\n",
                remainder.len(),
                start,
                total - 1,
                total,
            );
            sock.write_all(headers.as_bytes()).await.unwrap();
            sock.write_all(remainder).await.unwrap();
            sock.shutdown().await.unwrap();

            start
        });

        // Real SDK client pointed at the fake.
        use aws_sdk_s3::config::{Credentials, Region};
        let conf = aws_sdk_s3::Config::builder()
            .behavior_version(aws_config::BehaviorVersion::v2025_01_17())
            .endpoint_url(format!("http://{addr}"))
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("ak", "sk", None, None, "test"))
            .force_path_style(true)
            .build();
        let client = Client::from_conf(conf);

        let output = client
            .get_object()
            .bucket("b")
            .key("k")
            .send()
            .await
            .expect("initial GetObject must succeed");

        // max_retries: 1 — the scenario needs exactly one resume; if
        // the wrapper bug-spent more budget than it should, this test
        // would fail with "no more mock responses queued" on the server
        // side (only two accept()s are queued). Tiny backoff just keeps
        // wall-clock low; production defaults are 100ms/5000ms.
        let mut resumable = ResumableS3Read::from_first_response(
            client,
            "b".to_string(),
            "k".to_string(),
            output,
            StreamResumeConfig {
                max_retries: 1,
                initial_backoff_ms: 1,
                max_backoff_ms: 1,
            },
        );

        let mut got = Vec::new();
        TokAsyncReadExt::read_to_end(&mut resumable, &mut got)
            .await
            .expect("resume must reconstitute the truncated body");

        let resume_start = server.await.unwrap();
        assert_eq!(
            resume_start, truncate_at,
            "reconnect must Range from where the first response ended"
        );
        assert_eq!(got, body, "final bytes must match the original body");
        assert_eq!(
            resumable.resumed_count, 1,
            "scenario should consume exactly one resume"
        );
        assert!(
            resumable.completion_logged,
            "completion attestation log must have fired"
        );
    }

    /// Companion to the end-to-end resume test: same fake-S3
    /// scenario, but with `max_retries: 0` so the wrapper is
    /// effectively the bare SDK body-stream. The truncation that
    /// the resume test recovers from must, in this configuration,
    /// surface to the caller (either as an error or as a body
    /// shorter than the original). This proves the recovery in the
    /// sibling test is the resume code doing its job, not some
    /// happy accident in the SDK or the fake server.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn end_to_end_disabled_resume_surfaces_truncation() {
        use tokio::io::{AsyncReadExt as TokAsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let body: Vec<u8> = (0u32..4096).map(|i| (i & 0xff) as u8).collect();
        let total = body.len();
        let truncate_at = 256usize;
        let etag = "fake-etag-123";

        let body_for_server = body.clone();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // Drain the request headers.
            let mut tmp = [0u8; 1024];
            let mut buf = Vec::new();
            loop {
                let n = sock.read(&mut tmp).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let headers = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Length: {total}\r\n\
                 ETag: \"{etag}\"\r\n\
                 Content-Type: application/octet-stream\r\n\
                 Connection: close\r\n\
                 \r\n",
            );
            sock.write_all(headers.as_bytes()).await.unwrap();
            sock.write_all(&body_for_server[..truncate_at])
                .await
                .unwrap();
            sock.shutdown().await.unwrap();
            // No second accept: if the wrapper tries to resume, it
            // will hit a connection refused on the closed listener
            // — also a valid signal that resume was attempted.
            drop(listener);
        });

        use aws_sdk_s3::config::{Credentials, Region};
        let conf = aws_sdk_s3::Config::builder()
            .behavior_version(aws_config::BehaviorVersion::v2025_01_17())
            .endpoint_url(format!("http://{addr}"))
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("ak", "sk", None, None, "test"))
            .force_path_style(true)
            .build();
        let client = Client::from_conf(conf);

        let output = client
            .get_object()
            .bucket("b")
            .key("k")
            .send()
            .await
            .expect("initial GetObject must succeed");

        let mut resumable = ResumableS3Read::from_first_response(
            client,
            "b".to_string(),
            "k".to_string(),
            output,
            StreamResumeConfig {
                max_retries: 0,
                initial_backoff_ms: 1,
                max_backoff_ms: 1,
            },
        );

        let mut got = Vec::new();
        let read_result = TokAsyncReadExt::read_to_end(&mut resumable, &mut got).await;
        server.await.unwrap();

        // With resume disabled, hyper's Content-Length mismatch
        // surfaces as one of:
        //   - read_to_end Err (hyper raised on close before CL bytes)
        //   - read_to_end Ok with got.len() < body.len() (clean EOF
        //     before CL bytes, premature-EOF detection inactive)
        // Either is fine for our claim. The opposite — got == body —
        // would mean we did not actually need the resume code.
        match read_result {
            Err(_) => {
                assert!(
                    got.len() < body.len(),
                    "errored read must still leave a short body"
                );
            }
            Ok(_) => {
                assert_ne!(
                    got, body,
                    "without resume, body must not equal the original"
                );
                assert!(
                    got.len() < body.len(),
                    "without resume, short read must produce fewer bytes"
                );
            }
        }
        assert_eq!(
            resumable.resumed_count, 0,
            "no resume should have been attempted"
        );
        assert!(
            !resumable.completion_logged,
            "completion log must not fire when resume is disabled"
        );
    }
}
