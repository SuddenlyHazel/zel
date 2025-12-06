# Resilience Guide

## Circuit Breakers (`PeerCircuitBreakers`)

**Purpose**: Isolate peers who might be misbehaving. Each peer has independent state (Closed → Open → Half-Open → Closed).

**Key Features**:
- **Per-Peer**: No global impact from bad peers.
- **Error Classification**: Infra errors trip; app errors don't.
- **Auto-Recovery**: Timeout + probes.
- **Stats**: `CircuitBreakerStats` in server extensions.

### Enable
```rust
use zel_core::protocol::CircuitBreakerConfig;

let config = CircuitBreakerConfig::builder()
    .failure_threshold(0.5) // 50% failure rate
    .consecutive_failures(5)
    .timeout(Duration::from_secs(30))
    .build();

let server = RpcServerBuilder::new(b"app/1", endpoint)
    .with_circuit_breaker(config)
    .build();
```

### Error Classification
```rust
// App error - NO trip
Err(ResourceError::app("User not found"));

// Infra error - TRIPS breaker
Err(ResourceError::infra("Timeout"));

// Custom classifier
CircuitBreakerConfig::builder()
    .with_error_classifier(|err| {
        if err.is::<db::PoolError>() { ErrorSeverity::Infrastructure }
        else { ErrorSeverity::Application }
    });
```

### Stats Access
```rust
async fn handler(ctx: RequestContext) {
    let stats = ctx.server_extensions()
        .get::<CircuitBreakerStats>()
        .unwrap();
    let is_open = stats.is_open(&ctx.remote_id()).await;
}
```

**See**: [`circuit_breaker_macro_example.rs`](../zel_core/examples/circuit_breaker_macro_example.rs)

## Client Retries (`RetryConfig`)

**Purpose**: Handle transient failures (NAT flaps, packet loss).

```rust
let config = RetryConfig::builder()
    .max_attempts(5)
    .initial_backoff(Duration::from_millis(100))
    .with_jitter()
    .retry_network_errors_only() // or custom predicate
    .build();

let client = RpcClient::builder(conn)
    .with_retry_config(config)
    .build()
    .await?;
```

**Metrics**: `RetryMetrics` tracks attempts/successes.

**See**: [`reliability_demo.rs`](../zel_core/examples/reliability_demo.rs)