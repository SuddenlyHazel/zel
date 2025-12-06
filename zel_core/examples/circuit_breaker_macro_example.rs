//! # Example: Circuit Breaker Macro
//!
//! Demonstrates per-peer circuit breakers with `#[zel_service]`.
//! Features: `with_circuit_breaker`, `ResourceError::app/infra`, `CircuitBreakerOpen`.
//!
//! Run: `cargo run --example circuit_breaker_macro_example`
//!
//! Expected: App errors don't trip, infra errors do → open circuit.
//!
//! This focuses purely on:
//! - Enabling per-peer circuit breakers via `RpcServerBuilder::with_circuit_breaker`
//! - Marking errors with `ErrorClassificationExt` so they DO / DO NOT trip the breaker
//! - Observing `CircuitBreaker` behavior from the client via error types
//!
//! No metrics, stats objects, or extra layers.

use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;
use zel_core::protocol::{
    zel_service, CircuitBreakerConfig, ClientError, RequestContext, RpcServerBuilder,
};
use zel_core::IrohBundle;
use zel_types::ResourceError;

// =============================================================================
// Service definition using macros
// =============================================================================

#[zel_service(name = "demo_cb")]
trait DemoCircuitBreakerService {
    /// Always succeeds – used as a control
    #[method(name = "success")]
    async fn success(&self) -> Result<String, ResourceError>;

    /// Application error – should **NOT** trip the circuit breaker
    #[method(name = "app_error")]
    async fn app_error(&self) -> Result<String, ResourceError>;

    /// Infrastructure error – **DOES** trip the circuit breaker
    #[method(name = "infra_error")]
    async fn infra_error(&self) -> Result<String, ResourceError>;
}

// =============================================================================
// Service implementation
// =============================================================================

#[derive(Clone)]
struct DemoCircuitBreakerImpl;

#[async_trait]
impl DemoCircuitBreakerServiceServer for DemoCircuitBreakerImpl {
    async fn success(&self, _ctx: RequestContext) -> Result<String, ResourceError> {
        Ok("ok".to_string())
    }

    async fn app_error(&self, _ctx: RequestContext) -> Result<String, ResourceError> {
        // Use ResourceError::app() to mark as application error
        // Breaker sees `Application` severity and does NOT count it
        Err(ResourceError::app("User not found"))
    }

    async fn infra_error(&self, _ctx: RequestContext) -> Result<String, ResourceError> {
        // Use ResourceError::infra() to mark as infrastructure error
        // Breaker sees `Infrastructure` severity and DOES count it
        Err(ResourceError::infra("Database timeout"))
    }
}

// =============================================================================
// Main: enable circuit breaker and observe behavior from the client
// =============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    println!("╔══════════════════════════════════════════════╗");
    println!("║   Circuit Breaker + zel_service (Minimal)   ║");
    println!("╚══════════════════════════════════════════════╝\n");

    // ---------------------------------------------------------------------
    // Build server with circuit breaker enabled
    // ---------------------------------------------------------------------

    let mut server_bundle = IrohBundle::builder(None).await?;
    let endpoint = server_bundle.endpoint().clone();

    // Simple configuration: open after 2 severe failures, short timeout
    let cb_config = CircuitBreakerConfig::builder()
        .consecutive_failures(2)
        .timeout(Duration::from_secs(5))
        .build();

    let service_impl = DemoCircuitBreakerImpl;

    let server_builder = RpcServerBuilder::new(b"cb-macro/1", endpoint)
        .with_circuit_breaker(cb_config)
        .service("demo_cb");

    let server_builder = service_impl.into_service_builder(server_builder);
    let server = server_builder.build().build();

    let server_id = server_bundle.endpoint().id();
    let server_bundle = server_bundle.accept(b"cb-macro/1", server).finish().await;

    println!("Server ready:");
    println!("  ALPN: cb-macro/1");
    println!("  Peer ID: {}\n", server_id);

    // ---------------------------------------------------------------------
    // Build a simple client
    // ---------------------------------------------------------------------

    let client_bundle = IrohBundle::builder(None).await?.finish().await;
    // Give Iroh a moment to set up routing
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_id, b"cb-macro/1")
        .await?;

    let rpc_client = zel_core::protocol::client::RpcClient::new(conn).await?;
    let demo_client = DemoCircuitBreakerServiceClient::new(rpc_client);

    // ---------------------------------------------------------------------
    // 1. Success path – shows normal behavior
    // ---------------------------------------------------------------------

    println!("▶ Calling success() ...");
    let value = demo_client.success().await?;
    println!("  ✓ success() returned: {value}\n");

    // ---------------------------------------------------------------------
    // 2. Application errors – do NOT trip the breaker
    // ---------------------------------------------------------------------

    println!("▶ Sending 3 application errors (should NOT trip breaker) ...");

    for i in 1..=3 {
        let res = demo_client.app_error().await;
        match res {
            Ok(v) => println!("  [{i}] Unexpected success: {v}"),
            Err(e) => println!("  [{i}] Application error (expected): {e}"),
        }
    }

    println!("\n▶ Calling success() after application errors ...");
    let value = demo_client.success().await?;
    println!("  ✓ success() still works: {value}\n");

    // ---------------------------------------------------------------------
    // 3. Infrastructure errors – DO trip the breaker
    // ---------------------------------------------------------------------

    println!("▶ Sending 2 infrastructure errors (should OPEN circuit) ...");

    for i in 1..=2 {
        let res = demo_client.infra_error().await;
        match res {
            Ok(v) => println!("  [{i}] Unexpected success: {v}"),
            Err(e) => println!("  [{i}] Infra error (expected severe): {e}"),
        }
    }

    println!("\n▶ Calling infra_error() again – should now be blocked by OPEN circuit ...");
    let res = demo_client.infra_error().await;

    match res {
        Ok(v) => println!("  ❌ Unexpected success after circuit should be open: {v}"),
        Err(e) => {
            // When the circuit is open, client sees ResourceError::CircuitBreakerOpen
            if let Some(ResourceError::CircuitBreakerOpen { peer_id }) = extract_resource_error(&e)
            {
                println!("  ✓ Circuit is OPEN for peer: {peer_id}");
            } else {
                println!("  ❌ Unexpected error type after infra failures: {e}");
            }
        }
    }

    println!("\nDone. This example showed:");
    println!("  • How to enable circuit breakers with with_circuit_breaker(CircuitBreakerConfig)");
    println!("  • How to use ResourceError::app() / ResourceError::infra() for error severity");
    println!("  • That application errors do NOT trip the breaker");
    println!(
        "  • That repeated infrastructure errors DO trip the breaker, yielding CircuitBreakerOpen"
    );

    server_bundle.shutdown(Duration::from_secs(1)).await?;
    Ok(())
}

// Helper: pull a ResourceError out of a ClientError::Resource for nicer matching
fn extract_resource_error(err: &ClientError) -> Option<&ResourceError> {
    if let ClientError::Resource(resource_err) = err {
        Some(resource_err)
    } else {
        None
    }
}
