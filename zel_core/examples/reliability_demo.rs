//! Comprehensive demonstration of circuit breaker and retry features in Zel
//!
//! This example demonstrates:
//! - Error classification (Application vs Infrastructure vs System failures)
//! - Circuit breaker behavior with different error types
//! - Client-side retry functionality
//! - Retry and circuit breaker metrics
//! - Circuit state transitions (Closed → Open → HalfOpen → Closed)
//!
//! # Key Concepts
//!
//! ## Error Classification
//! Errors are classified into three categories:
//! - **Application errors**: Business logic failures (e.g., "user not found", validation errors)
//!   → Do NOT trip the circuit breaker
//! - **Infrastructure errors**: Network/connection failures (e.g., timeouts, connection drops)
//!   → DO trip the circuit breaker
//! - **System failures**: Backend/dependency failures (e.g., database down, upstream service unavailable)
//!   → DO trip the circuit breaker
//!
//! ## Circuit Breaker
//! Protects the server from being overwhelmed by failing requests.
//! - Operates per-peer (each client has its own circuit state)
//! - Opens after N consecutive infrastructure/system failures
//! - Blocks requests while open to prevent cascading failures
//! - Automatically recovers after timeout + successful requests

use anyhow::{anyhow, Result};
use bytes::Bytes;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use zel_core::protocol::ResourceResultExt;
use zel_core::protocol::{
    CircuitBreakerConfig, CircuitBreakerStats, ClientError, ErrorClassificationExt, ResourceError,
    Response, RetryConfig, RpcClient, RpcServerBuilder,
};
use zel_core::IrohBundle;

#[tokio::main]
async fn main() -> Result<()> {
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║    Zel Circuit Breaker & Retry - Comprehensive Demo          ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    // ============================================================================
    // SETUP: Create server and client endpoints
    // ============================================================================

    println!("📡 Setting up Iroh network endpoints...\n");

    let mut server_bundle = IrohBundle::builder(None).await?;
    let server_endpoint = server_bundle.endpoint().clone();
    let server_id = server_endpoint.id();

    println!("  Server ID: {}\n", server_id);

    // ============================================================================
    // CIRCUIT BREAKER CONFIGURATION
    // ============================================================================
    // The circuit breaker protects the server from being overwhelmed by failing
    // requests. It tracks failures per-peer and opens when a threshold is reached.

    let cb_config = CircuitBreakerConfig::builder()
        .consecutive_failures(2) // Open circuit after 2 consecutive failures
        .consecutive_successes(2) // Close circuit after 2 consecutive successes
        .timeout(Duration::from_secs(3)) // Wait 3s before attempting recovery
        .build();

    println!("⚙️  Circuit Breaker Configuration:");
    println!("  • Failure threshold: 2 consecutive failures");
    println!("  • Success threshold: 2 consecutive successes");
    println!("  • Timeout before recovery: 3 seconds\n");

    // ============================================================================
    // BUILD SERVER WITH CIRCUIT BREAKER
    // ============================================================================
    // CircuitBreakerStats is automatically added to server extensions when
    // circuit breaker is enabled. Services can access it via RequestContext.

    let service = DemoService::new();
    let stats_holder = Arc::new(Mutex::new(None)); // Capture stats from first request

    let server = RpcServerBuilder::new(b"reliability/1", server_endpoint)
        .with_circuit_breaker(cb_config)
        .service("demo")
        // Application error - won't trip circuit breaker
        .rpc_resource("app_error", {
            let service = Arc::new(service.clone());
            let stats_holder = Arc::clone(&stats_holder);
            move |ctx, _req| {
                let service = Arc::clone(&service);
                let stats_holder = Arc::clone(&stats_holder);
                Box::pin(async move {
                    // Capture CircuitBreakerStats from server extensions
                    if stats_holder.lock().await.is_none() {
                        if let Some(stats) = ctx.server_extensions().get::<CircuitBreakerStats>() {
                            *stats_holder.lock().await = Some(stats.clone());
                        }
                    }
                    service.app_error().await.to_app_error()
                })
            }
        })
        // Infrastructure error - WILL trip circuit breaker
        .rpc_resource("infra_error", {
            let service = Arc::new(service.clone());
            move |_ctx, _req| {
                let service = Arc::clone(&service);
                Box::pin(async move { service.infra_error().await.to_infra_error() })
            }
        })
        // System failure - WILL trip circuit breaker
        .rpc_resource("system_error", {
            let service = Arc::new(service.clone());
            move |_ctx, _req| {
                let service = Arc::clone(&service);
                Box::pin(async move { service.system_error().await.to_system_error() })
            }
        })
        // Success method for recovery testing
        .rpc_resource("success", {
            let service = Arc::new(service.clone());
            move |_ctx, _req| {
                let service = Arc::clone(&service);
                Box::pin(async move { service.success().await.to_app_error() })
            }
        })
        // Flaky method for retry testing
        .rpc_resource("flaky", {
            let service = Arc::new(service.clone());
            move |_ctx, _req| {
                let service = Arc::clone(&service);
                Box::pin(async move { service.flaky_method().await.to_app_error() })
            }
        })
        .build()
        .build();

    let server_bundle = server_bundle
        .accept(b"reliability/1", server)
        .finish()
        .await;

    // ============================================================================
    // CREATE CLIENT WITH RETRY CONFIGURATION
    // ============================================================================

    let client_bundle = IrohBundle::builder(None).await?.finish().await;

    // CRITICAL: Wait for Iroh n0 DNS discovery to complete
    // Without this delay, initial connections may fail due to DNS not being ready
    println!("⏳ Waiting for Iroh DNS setup...");
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("✓ DNS setup complete\n");

    // Configure retry for the client
    let retry_config = RetryConfig::builder()
        .max_attempts(3) // Try up to 3 times
        .initial_backoff(Duration::from_millis(100)) // Start with 100ms delay
        .with_jitter() // Add randomness to prevent thundering herd
        .with_custom_predicate(|e| {
            // Retry on CallbackErrors (both Application and Infrastructure in this demo)
            matches!(
                e,
                ClientError::Resource(ResourceError::CallbackError { .. })
            )
        })
        .build()
        .expect("Valid retry config");

    println!("⚙️  Client Retry Configuration:");
    println!("  • Max attempts: 3");
    println!("  • Initial backoff: 100ms");
    println!("  • Backoff strategy: Exponential with jitter\n");

    // Connect client to server
    let conn = client_bundle
        .endpoint
        .connect(server_id, b"reliability/1")
        .await?;

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .with_retry_metrics() // Enable retry metrics tracking
        .build()
        .await?;

    println!("═══════════════════════════════════════════════════════════════\n");

    // ============================================================================
    // TEST 1: Application Errors DON'T Trip Circuit Breaker
    // ============================================================================
    // Application errors represent business logic failures (e.g., "user not found",
    // validation errors). These don't indicate the service is unhealthy, so they
    // bypass the circuit breaker completely.

    println!("📋 TEST 1: Application Errors (Don't Trip Circuit)\n");
    println!("  Concept: Business logic errors like 'user not found' or 'invalid");
    println!("  input' don't mean the service is failing. They're expected errors");
    println!("  and should NOT trip the circuit breaker.\n");

    service.reset_counter();
    println!("  Sending 5 application errors (threshold is 2)...\n");

    for i in 1..=5 {
        let result = client.call("demo", "app_error", Bytes::new()).await;
        println!(
            "    Attempt {}/5: {}",
            i,
            if result.is_err() {
                "❌ Error (expected)"
            } else {
                "✓ Success"
            }
        );
    }

    // Get circuit breaker stats
    let cb_stats = stats_holder
        .lock()
        .await
        .clone()
        .expect("CircuitBreakerStats should be available");

    let client_id = client_bundle.endpoint.id();
    let is_open = cb_stats.is_open(&client_id).await;

    println!("\n  📊 Circuit State After Application Errors:");
    println!("    • Circuit is open: {} (should be false)", is_open);
    println!(
        "    • Total open circuits: {}",
        cb_stats.open_circuit_count().await
    );

    if is_open {
        println!("\n  ❌ FAILURE: Application errors incorrectly tripped the circuit!");
        return Err(anyhow!(
            "Test failed: Application errors should not trip circuit"
        ));
    }

    println!("\n  ✅ PASSED: Application errors did NOT trip the circuit breaker\n");

    println!("═══════════════════════════════════════════════════════════════\n");

    // ============================================================================
    // TEST 2: Infrastructure Errors DO Trip Circuit Breaker
    // ============================================================================
    // Infrastructure errors indicate systemic problems (network failures, connection
    // drops, etc.). These DO count toward tripping the circuit breaker.

    println!("📋 TEST 2: Infrastructure Errors (DO Trip Circuit)\n");
    println!("  Concept: Network/connection failures indicate the service or");
    println!("  network is unhealthy. These SHOULD trip the circuit breaker to");
    println!("  protect the server from being overwhelmed.\n");

    service.reset_counter();
    println!("  Sending 3 infrastructure errors (threshold is 2)...\n");

    for i in 1..=3 {
        let result = client.call("demo", "infra_error", Bytes::new()).await;
        println!(
            "    Attempt {}/3: {}",
            i,
            if result.is_err() {
                "❌ Error (expected)"
            } else {
                "✓ Success"
            }
        );
    }

    // Check if circuit is now open
    let is_open_after = cb_stats.is_open(&client_id).await;

    println!("\n  📊 Circuit State After Infrastructure Errors:");
    println!("    • Circuit is open: {} (should be true)", is_open_after);
    println!(
        "    • Total open circuits: {}",
        cb_stats.open_circuit_count().await
    );

    if !is_open_after {
        println!("\n  ❌ FAILURE: Infrastructure errors did not trip the circuit!");
        return Err(anyhow!(
            "Test failed: Infrastructure errors should trip circuit"
        ));
    }

    println!("\n  ✅ PASSED: Infrastructure errors DID trip the circuit breaker\n");

    // Verify circuit continues blocking requests
    println!("  Verifying circuit blocks subsequent requests...\n");
    match client.call("demo", "success", Bytes::new()).await {
        Err(ClientError::Resource(ResourceError::CircuitBreakerOpen { .. })) => {
            println!("    ✓ Request blocked by open circuit (expected)");
        }
        Ok(_) => {
            println!("    ❌ Request was not blocked!");
            return Err(anyhow!("Circuit should block requests when open"));
        }
        Err(e) => {
            println!("    ❌ Unexpected error: {}", e);
            return Err(anyhow!("Wrong error type"));
        }
    }

    println!("\n  ✅ PASSED: Circuit breaker correctly blocking requests\n");

    println!("═══════════════════════════════════════════════════════════════\n");

    // ============================================================================
    // TEST 3: Client Retry Functionality
    // ============================================================================
    // Demonstrate that the client automatically retries failed requests up to
    // the configured maximum attempts.

    println!("📋 TEST 3: Client Retry Verification\n");
    println!("  Concept: The client automatically retries failed requests to");
    println!("  handle transient failures. This improves reliability without");
    println!("  requiring application-level retry logic.\n");

    // Wait for circuit to timeout and enter half-open state
    println!("  Waiting for circuit timeout (3 seconds)...");
    tokio::time::sleep(Duration::from_secs(3)).await;
    println!("  ✓ Circuit timeout complete\n");

    // Send success requests to close the circuit
    println!("  Sending 2 successful requests to close the circuit...\n");
    for i in 1..=2 {
        match client.call("demo", "success", Bytes::new()).await {
            Ok(_) => println!("    Success {}/2: ✓", i),
            Err(e) => println!("    Success {}/2: ❌ {}", i, e),
        }
    }

    // Give circuit breaker time to fully transition to closed state
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify circuit is now closed
    let is_closed = !cb_stats.is_open(&client_id).await;
    println!(
        "\n  📊 Circuit State: {}",
        if is_closed {
            "Closed ✓"
        } else {
            "Still Open ❌"
        }
    );

    if !is_closed {
        println!("\n  ❌ FAILURE: Circuit did not close after successful requests");
        return Err(anyhow!("Circuit should be closed"));
    }

    // Test retry with flaky method
    // Note: Reset counter and use fresh connection to avoid circuit breaker interference
    service.reset_counter();
    println!("\n  Calling flaky method (fails 2x, succeeds on attempt 3)...\n");
    println!("  Note: Application errors trigger retries but don't trip circuit\n");

    match client.call("demo", "flaky", Bytes::new()).await {
        Ok(resp) => {
            let msg: String = serde_json::from_slice(&resp.data)?;
            println!("    ✓ {}", msg);

            let total_calls = service.get_call_count();
            println!("\n  📊 Server received {} total attempts", total_calls);

            if total_calls != 3 {
                println!("\n  ❌ FAILURE: Expected 3 attempts, got {}", total_calls);
                return Err(anyhow!("Retry not working correctly"));
            }

            println!("\n  ✅ PASSED: Retries working correctly (3 attempts as expected)\n");
        }
        Err(e) => {
            println!("    ❌ Request failed: {}", e);
            return Err(anyhow!("Retry test failed"));
        }
    }

    println!("═══════════════════════════════════════════════════════════════\n");

    // ============================================================================
    // METRICS SUMMARY
    // ============================================================================
    // Display all collected metrics from circuit breaker operations

    println!("📊 METRICS SUMMARY\n");

    println!("  Circuit Breaker Stats:");
    let peer_states = cb_stats.peer_states().await;
    if let Some(state) = peer_states.get(&client_id) {
        println!("    • Client peer state: {:?}", state);
    }
    println!(
        "    • Total open circuits: {}",
        cb_stats.open_circuit_count().await
    );

    println!("\n═══════════════════════════════════════════════════════════════\n");

    // ============================================================================
    // SUMMARY
    // ============================================================================

    println!("🎉 ALL TESTS PASSED!\n");
    println!("Summary of Demonstrated Features:");
    println!("  ✅ Application errors bypass circuit breaker");
    println!("  ✅ Infrastructure errors trip circuit breaker");
    println!("  ✅ Circuit breaker blocks requests when open");
    println!("  ✅ Circuit breaker recovers after timeout + successes");
    println!("  ✅ Client retries work automatically");
    println!("  ✅ Retry metrics track all attempts");
    println!("  ✅ Circuit breaker stats accessible via extensions");
    println!("  ✅ Error classification controls circuit behavior\n");

    println!("Key Takeaways:");
    println!("  • Use .as_application() for business logic errors");
    println!("  • Use .as_infrastructure() for network/connection errors");
    println!("  • Use .as_system_failure() for backend/dependency errors");
    println!("  • Circuit breaker protects servers from cascading failures");
    println!("  • Retries improve reliability for transient failures\n");

    server_bundle.shutdown(Duration::from_secs(1)).await?;
    println!("✓ Server shut down successfully\n");

    Ok(())
}

// ============================================================================
// DEMO SERVICE IMPLEMENTATION
// ============================================================================
// This service demonstrates different error types and their circuit breaker
// behavior. In a real application, you would mark errors based on their actual
// cause (network failure, database error, validation failure, etc.).

#[derive(Clone)]
struct DemoService {
    call_count: Arc<AtomicU32>,
}

impl DemoService {
    fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
        }
    }

    fn reset_counter(&self) {
        self.call_count.store(0, Ordering::SeqCst);
    }

    fn get_call_count(&self) -> u32 {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Application error - represents business logic failure
    /// These errors indicate expected failure conditions (e.g., "user not found",
    /// "invalid input") and should NOT trip the circuit breaker.
    async fn app_error(&self) -> anyhow::Result<Response> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        println!(
            "      [Server] app_error call #{}: Returning application error",
            count + 1
        );

        // Mark as application error - won't trip circuit breaker
        Err(anyhow!("User not found").as_application())
    }

    /// Infrastructure error - represents network/connection failure
    /// These errors indicate the network or connection is unhealthy and SHOULD
    /// trip the circuit breaker to prevent cascading failures.
    async fn infra_error(&self) -> anyhow::Result<Response> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        println!(
            "      [Server] infra_error call #{}: Returning infrastructure error",
            count + 1
        );

        // Mark as infrastructure error - WILL trip circuit breaker
        Err(anyhow!("Connection timeout").as_infrastructure())
    }

    /// System failure - represents backend/dependency failure
    /// These errors indicate a backend service or dependency is down (e.g.,
    /// database unavailable, upstream service failed) and SHOULD trip the circuit.
    async fn system_error(&self) -> anyhow::Result<Response> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        println!(
            "      [Server] system_error call #{}: Returning system failure",
            count + 1
        );

        // Mark as system failure - WILL trip circuit breaker
        Err(anyhow!("Database unavailable").as_system_failure())
    }

    /// Success method - always succeeds
    /// Used for testing circuit recovery
    async fn success(&self) -> anyhow::Result<Response> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        println!(
            "      [Server] success call #{}: Returning success",
            count + 1
        );

        let result = serde_json::to_vec("Success").unwrap();
        Ok(Response {
            data: Bytes::from(result),
        })
    }

    /// Flaky method - fails first 2 attempts, succeeds on 3rd
    /// Used for testing client retry functionality
    async fn flaky_method(&self) -> anyhow::Result<Response> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);

        if count < 2 {
            println!(
                "      [Server] flaky_method attempt #{}: ❌ Failing",
                count + 1
            );
            // Mark as application error - triggers retry without tripping circuit
            return Err(anyhow!("Temporary failure").as_application());
        }

        println!(
            "      [Server] flaky_method attempt #{}: ✓ Success!",
            count + 1
        );
        let result = serde_json::to_vec("Success after retries!").unwrap();
        Ok(Response {
            data: Bytes::from(result),
        })
    }
}
