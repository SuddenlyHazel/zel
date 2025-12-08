# Zel Core Examples

```bash
cargo run --example <name>
```

## Quick Start Examples (Try These First)

| Example | Purpose | Key Features |
|---------|---------|--------------|
| [macro_service_example](macro_service_example.rs) | Basic RPC service | `#[zel_service]`, methods, subscriptions, typed client/server |
| [notification_example](notification_example.rs) | Client→server streaming | Notifications, typed sinks |
| [context_extensions_demo](context_extensions_demo.rs) | Shared context | 3-tier extensions (server/conn/request), middleware/hooks |

## Advanced Examples

| Example | Purpose | Key Features |
|---------|---------|--------------|
| [raw_stream_example](raw_stream_example.rs) | Custom protocols | Bidirectional streams (file transfer) |
| [multi_service_example](multi_service_example.rs) | Multiple services | ServiceBuilder chaining |
| [circuit_breaker_macro_example](circuit_breaker_macro_example.rs) | Resilience | Per-peer circuit breakers |
| [reliability_demo](reliability_demo.rs) | Retries & errors | RetryConfig, error classification |
| [metrics_prometheus](metrics_prometheus.rs) | Observability | Metrics export, Prometheus |