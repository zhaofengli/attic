# OpenTelemetry

`atticd` can export traces, structured logs, and metrics to an OpenTelemetry collector using OTLP. Export is configured with the standard OpenTelemetry environment variables.

OTLP export is disabled unless a common or signal-specific OTLP endpoint is configured. Set `OTEL_SDK_DISABLED=true` to explicitly disable export even when endpoints are present. Local formatted logs remain enabled in either case.

## Transport

Attic supports both OTLP over HTTP with protobuf and OTLP over gRPC. HTTP/protobuf is the default.

```sh
# Configure an endpoint to enable export. HTTP/protobuf is the default transport.
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318

# Optional: gRPC
export OTEL_EXPORTER_OTLP_PROTOCOL=grpc
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
```

`OTEL_EXPORTER_OTLP_PROTOCOL` accepts `http/protobuf` or `grpc`. Signal-specific protocol variables are also supported:

- `OTEL_EXPORTER_OTLP_TRACES_PROTOCOL`
- `OTEL_EXPORTER_OTLP_LOGS_PROTOCOL`
- `OTEL_EXPORTER_OTLP_METRICS_PROTOCOL`

Signal-specific settings take precedence over `OTEL_EXPORTER_OTLP_PROTOCOL`.

Export is enabled when any of these endpoint variables has a non-empty value:

- `OTEL_EXPORTER_OTLP_ENDPOINT`
- `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`
- `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`
- `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`

When none are configured, Attic does not create exporter workers or attempt connections to the OpenTelemetry SDK's default localhost endpoint.

## Endpoints and authentication

Standard common and signal-specific OTLP settings are honored, including:

- `OTEL_EXPORTER_OTLP_ENDPOINT`
- `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`
- `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`
- `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`
- `OTEL_EXPORTER_OTLP_HEADERS`
- `OTEL_EXPORTER_OTLP_TRACES_HEADERS`
- `OTEL_EXPORTER_OTLP_LOGS_HEADERS`
- `OTEL_EXPORTER_OTLP_METRICS_HEADERS`
- `OTEL_EXPORTER_OTLP_TIMEOUT`

For HTTP, the exporter appends `/v1/traces`, `/v1/logs`, or `/v1/metrics` to the common endpoint. Signal-specific HTTP endpoints should include the complete signal path.

## Resource attributes

The service name defaults to `atticd`. Standard OpenTelemetry resource configuration can add deployment-specific metadata:

```sh
export OTEL_RESOURCE_ATTRIBUTES='service.namespace=cache,deployment.environment.name=production'
```

## Local logging

OTLP export runs alongside the existing formatted log output. `RUST_LOG` controls local output. OTLP logs and traces use an `info` filter and suppress exporter transport targets to avoid telemetry feedback loops.

## Exported metrics

Attic currently emits:

- `http.server.request.count`
- `http.server.active_requests`
- `http.server.request.duration`
- `attic.operation.count`
- `attic.operation.duration`
- `attic.operation.bytes`

Operation attributes distinguish uploads, downloads, database connection and migration work, garbage collection runs, and garbage-collected object types. HTTP metrics use method and status attributes; request paths and cache names are intentionally excluded from metric attributes to prevent unbounded cardinality.

Telemetry providers are flushed during orderly `atticd` shutdown.
