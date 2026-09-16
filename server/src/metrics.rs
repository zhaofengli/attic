use std::sync::OnceLock;
use std::time::Duration;

use axum::http::StatusCode;
use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::metrics::{Counter, Histogram, UpDownCounter};

struct HttpMetrics {
    requests: Counter<u64>,
    active_requests: UpDownCounter<i64>,
    request_duration: Histogram<f64>,
}

struct OperationMetrics {
    operations: Counter<u64>,
    duration: Histogram<f64>,
    bytes: Counter<u64>,
}

pub struct ActiveHttpRequest {
    method: String,
    started_at: std::time::Instant,
}

impl ActiveHttpRequest {
    pub fn finish(self, status: StatusCode) {
        let metrics = http_metrics();
        let attributes = [
            KeyValue::new("http.request.method", self.method.clone()),
            KeyValue::new("http.response.status_code", i64::from(status.as_u16())),
            KeyValue::new("http.response.status_class", status_class(status)),
        ];

        metrics.active_requests.add(
            -1,
            &[KeyValue::new("http.request.method", self.method.clone())],
        );
        metrics.requests.add(1, &attributes);
        metrics
            .request_duration
            .record(self.started_at.elapsed().as_secs_f64(), &attributes);
    }
}

pub fn start_http_request(method: &str) -> ActiveHttpRequest {
    http_metrics().active_requests.add(
        1,
        &[KeyValue::new("http.request.method", method.to_owned())],
    );

    ActiveHttpRequest {
        method: method.to_owned(),
        started_at: std::time::Instant::now(),
    }
}

pub fn record_operation(name: &'static str, outcome: &'static str, duration: Duration) {
    let attributes = [
        KeyValue::new("operation.name", name),
        KeyValue::new("operation.outcome", outcome),
    ];
    let metrics = operation_metrics();
    metrics.operations.add(1, &attributes);
    metrics.duration.record(duration.as_secs_f64(), &attributes);
}

pub fn add_bytes(name: &'static str, bytes: u64, attributes: &[KeyValue]) {
    let mut all_attributes = Vec::with_capacity(attributes.len() + 1);
    all_attributes.push(KeyValue::new("operation.name", name));
    all_attributes.extend_from_slice(attributes);
    operation_metrics().bytes.add(bytes, &all_attributes);
}

pub fn add_count(name: &'static str, count: u64, attributes: &[KeyValue]) {
    let mut all_attributes = Vec::with_capacity(attributes.len() + 1);
    all_attributes.push(KeyValue::new("operation.name", name));
    all_attributes.extend_from_slice(attributes);
    operation_metrics().operations.add(count, &all_attributes);
}

fn http_metrics() -> &'static HttpMetrics {
    static METRICS: OnceLock<HttpMetrics> = OnceLock::new();
    METRICS.get_or_init(|| {
        let meter = global::meter("atticd.http");
        HttpMetrics {
            requests: meter
                .u64_counter("http.server.request.count")
                .with_description("HTTP requests received")
                .build(),
            active_requests: meter
                .i64_up_down_counter("http.server.active_requests")
                .with_description("HTTP requests currently being processed")
                .build(),
            request_duration: meter
                .f64_histogram("http.server.request.duration")
                .with_description("HTTP request duration")
                .with_unit("s")
                .build(),
        }
    })
}

fn operation_metrics() -> &'static OperationMetrics {
    static METRICS: OnceLock<OperationMetrics> = OnceLock::new();
    METRICS.get_or_init(|| {
        let meter = global::meter("atticd.operations");
        OperationMetrics {
            operations: meter
                .u64_counter("attic.operation.count")
                .with_description("Attic operations and processed objects")
                .build(),
            duration: meter
                .f64_histogram("attic.operation.duration")
                .with_description("Attic operation duration")
                .with_unit("s")
                .build(),
            bytes: meter
                .u64_counter("attic.operation.bytes")
                .with_description("Bytes processed by Attic operations")
                .with_unit("By")
                .build(),
        }
    })
}

pub(crate) fn status_class(status: StatusCode) -> &'static str {
    match status.as_u16() / 100 {
        1 => "1xx",
        2 => "2xx",
        3 => "3xx",
        4 => "4xx",
        5 => "5xx",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::status_class;

    #[test]
    fn groups_http_status_by_class() {
        assert_eq!(status_class(StatusCode::OK), "2xx");
        assert_eq!(status_class(StatusCode::NOT_FOUND), "4xx");
        assert_eq!(status_class(StatusCode::INTERNAL_SERVER_ERROR), "5xx");
    }
}
