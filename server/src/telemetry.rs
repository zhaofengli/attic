use std::env;

use anyhow::{Context, Result, bail};
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_error::ErrorLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

const DEFAULT_PROTOCOL: &str = "http/protobuf";
const OTEL_SDK_DISABLED: &str = "OTEL_SDK_DISABLED";
const OTLP_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const OTLP_PROTOCOL: &str = "OTEL_EXPORTER_OTLP_PROTOCOL";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtlpProtocol {
    HttpProtobuf,
    Grpc,
}

pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    logger_provider: Option<SdkLoggerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl TelemetryGuard {
    pub fn shutdown(self) -> Result<()> {
        let mut errors = Vec::new();

        if let Some(provider) = self.tracer_provider
            && let Err(error) = provider.shutdown()
        {
            errors.push(format!("tracer provider: {error}"));
        }
        if let Some(provider) = self.logger_provider
            && let Err(error) = provider.shutdown()
        {
            errors.push(format!("logger provider: {error}"));
        }
        if let Some(provider) = self.meter_provider
            && let Err(error) = provider.shutdown()
        {
            errors.push(format!("meter provider: {error}"));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            bail!("failed to shut down OpenTelemetry: {}", errors.join("; "))
        }
    }
}

pub fn init(tokio_console: bool) -> Result<TelemetryGuard> {
    let fmt_layer = tracing_subscriber::fmt::layer().with_filter(EnvFilter::from_default_env());
    let error_layer = ErrorLayer::default();
    let console_layer = if tokio_console {
        let (layer, server) = console_subscriber::ConsoleLayer::new();
        tokio::spawn(server.serve());
        Some(layer)
    } else {
        None
    };

    if !export_configured() {
        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(error_layer)
            .with(console_layer)
            .try_init()
            .context("failed to initialize tracing subscriber")?;

        return Ok(TelemetryGuard {
            tracer_provider: None,
            logger_provider: None,
            meter_provider: None,
        });
    }

    let mut resource = Resource::builder().with_attribute(opentelemetry::KeyValue::new(
        "service.version",
        env!("CARGO_PKG_VERSION"),
    ));
    if !service_name_configured() {
        resource = resource.with_service_name("atticd");
    }
    let resource = resource.build();

    let meter_provider = if signal_export_configured("METRICS") {
        let metric_exporter = build_metric_exporter(protocol_for("METRICS")?)?;
        let provider = SdkMeterProvider::builder()
            .with_resource(resource.clone())
            .with_periodic_exporter(metric_exporter)
            .build();
        global::set_meter_provider(provider.clone());
        Some(provider)
    } else {
        None
    };

    let logger_provider = if signal_export_configured("LOGS") {
        Some(
            SdkLoggerProvider::builder()
                .with_resource(resource.clone())
                .with_batch_exporter(build_log_exporter(protocol_for("LOGS")?)?)
                .build(),
        )
    } else {
        None
    };
    let log_layer = logger_provider
        .as_ref()
        .map(|provider| OpenTelemetryTracingBridge::new(provider).with_filter(otel_filter()));

    let tracer_provider = if signal_export_configured("TRACES") {
        let provider = SdkTracerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(build_span_exporter(protocol_for("TRACES")?)?)
            .build();
        global::set_tracer_provider(provider.clone());
        Some(provider)
    } else {
        None
    };
    let trace_layer = tracer_provider.as_ref().map(|provider| {
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("atticd"))
            .with_filter(otel_filter())
    });

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(error_layer)
        .with(console_layer)
        .with(trace_layer)
        .with(log_layer)
        .try_init()
        .context("failed to initialize tracing subscriber")?;

    Ok(TelemetryGuard {
        tracer_provider,
        logger_provider,
        meter_provider,
    })
}

fn export_configured() -> bool {
    export_enabled(
        env::var(OTEL_SDK_DISABLED).is_ok_and(|value| value.eq_ignore_ascii_case("true")),
        non_empty_env(OTLP_ENDPOINT).as_deref(),
        non_empty_env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").as_deref(),
        non_empty_env("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT").as_deref(),
        non_empty_env("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT").as_deref(),
    )
}

fn signal_export_configured(signal: &str) -> bool {
    non_empty_env(OTLP_ENDPOINT).is_some()
        || non_empty_env(&format!("OTEL_EXPORTER_OTLP_{signal}_ENDPOINT")).is_some()
}

fn export_enabled(
    sdk_disabled: bool,
    endpoint: Option<&str>,
    traces_endpoint: Option<&str>,
    logs_endpoint: Option<&str>,
    metrics_endpoint: Option<&str>,
) -> bool {
    !sdk_disabled
        && [endpoint, traces_endpoint, logs_endpoint, metrics_endpoint]
            .into_iter()
            .flatten()
            .any(|endpoint| !endpoint.trim().is_empty())
}

fn non_empty_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn service_name_configured() -> bool {
    env::var("OTEL_SERVICE_NAME").is_ok_and(|value| !value.is_empty())
        || env::var("OTEL_RESOURCE_ATTRIBUTES").is_ok_and(|attributes| {
            attributes
                .split(',')
                .filter_map(|attribute| attribute.split_once('='))
                .any(|(key, value)| key.trim() == "service.name" && !value.trim().is_empty())
        })
}

fn protocol_for(signal: &str) -> Result<OtlpProtocol> {
    let signal_protocol = env::var(format!("OTEL_EXPORTER_OTLP_{signal}_PROTOCOL")).ok();
    let protocol = signal_protocol.or_else(|| env::var(OTLP_PROTOCOL).ok());
    select_otlp_protocol(protocol.as_deref())
}

pub fn select_otlp_protocol(protocol: Option<&str>) -> Result<OtlpProtocol> {
    match protocol.unwrap_or(DEFAULT_PROTOCOL) {
        "http/protobuf" => Ok(OtlpProtocol::HttpProtobuf),
        "grpc" => Ok(OtlpProtocol::Grpc),
        protocol => {
            bail!("unsupported OTLP protocol {protocol:?}; expected \"http/protobuf\" or \"grpc\"")
        }
    }
}

fn build_span_exporter(protocol: OtlpProtocol) -> Result<SpanExporter> {
    match protocol {
        OtlpProtocol::HttpProtobuf => SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .build(),
        OtlpProtocol::Grpc => SpanExporter::builder().with_tonic().build(),
    }
    .context("failed to create OTLP trace exporter")
}

fn build_metric_exporter(protocol: OtlpProtocol) -> Result<MetricExporter> {
    match protocol {
        OtlpProtocol::HttpProtobuf => MetricExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .build(),
        OtlpProtocol::Grpc => MetricExporter::builder().with_tonic().build(),
    }
    .context("failed to create OTLP metrics exporter")
}

fn build_log_exporter(protocol: OtlpProtocol) -> Result<LogExporter> {
    match protocol {
        OtlpProtocol::HttpProtobuf => LogExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .build(),
        OtlpProtocol::Grpc => LogExporter::builder().with_tonic().build(),
    }
    .context("failed to create OTLP log exporter")
}

fn otel_filter() -> EnvFilter {
    EnvFilter::new("info")
        .add_directive("hyper=off".parse().unwrap())
        .add_directive("tonic=off".parse().unwrap())
        .add_directive("h2=off".parse().unwrap())
        .add_directive("reqwest=off".parse().unwrap())
}

#[cfg(test)]
mod tests {
    use super::{OtlpProtocol, export_enabled, select_otlp_protocol};

    #[test]
    fn export_is_disabled_without_an_endpoint() {
        assert!(!export_enabled(false, None, None, None, None));
    }

    #[test]
    fn common_endpoint_enables_export() {
        assert!(export_enabled(
            false,
            Some("http://collector:4318"),
            None,
            None,
            None
        ));
    }

    #[test]
    fn signal_endpoint_enables_export() {
        assert!(export_enabled(
            false,
            None,
            Some("http://collector:4318/v1/traces"),
            None,
            None
        ));
    }

    #[test]
    fn sdk_disabled_overrides_endpoints() {
        assert!(!export_enabled(
            true,
            Some("http://collector:4318"),
            None,
            None,
            None
        ));
    }

    #[test]
    fn defaults_to_http_protobuf() {
        assert_eq!(
            select_otlp_protocol(None).unwrap(),
            OtlpProtocol::HttpProtobuf
        );
    }

    #[test]
    fn supports_http_protobuf() {
        assert_eq!(
            select_otlp_protocol(Some("http/protobuf")).unwrap(),
            OtlpProtocol::HttpProtobuf
        );
    }

    #[test]
    fn supports_grpc() {
        assert_eq!(
            select_otlp_protocol(Some("grpc")).unwrap(),
            OtlpProtocol::Grpc
        );
    }

    #[test]
    fn rejects_unsupported_protocols() {
        let error = select_otlp_protocol(Some("http/json")).unwrap_err();

        assert!(error.to_string().contains("http/json"));
    }
}
