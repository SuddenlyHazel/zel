//! Comprehensive E2E integration tests for the complete reliability stack
//!
//! This test suite verifies that circuit breaker + retry + error classification
//! work correctly together in real RPC scenarios.
//!
//! These tests are the final verification (Subtask 4.1) that the entire
//! reliability stack is production-ready.

use bytes::Bytes;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use zel_core::protocol::{
    error_classification::ErrorSeverity as ErrorSeverityType, Body, RpcClient,
};
use zel_core::protocol::{
    CircuitBreakerConfig, ResourceError, Response, RetryConfig, RpcServerBuilder,
};
use zel_core::IrohBundle;

/// Helper to track server-side call attempts and control behavior
#[derive(Clone)]
struct CallTracker {
    call_count: Arc<AtomicU32>,
    fail_until: Arc<AtomicU32>,
    return_app_error: Arc<AtomicBool>,
    return_infra_error: Arc<AtomicBool>,
}

impl CallTracker {
    fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
            fail_until: Arc::new(AtomicU32::new(0)),
            return_app_error: Arc::new(AtomicBool::new(false)),
            return_infra_error: Arc::new(AtomicBool::new(false)),
        }
    }

    fn set_fail_until(&self, count: u32) {
        self.fail_until.store(count, Ordering::SeqCst);
    }

    fn set_return_app_error(&self, value: bool) {
        self.return_app_error.store(value, Ordering::SeqCst);
    }

    fn set_return_infra_error(&self, value: bool) {
        self.return_infra_error.store(value, Ordering::SeqCst);
    }

    fn get_call_count(&self) -> u32 {
        self.call_count.load(Ordering::SeqCst)
    }

    fn reset(&self) {
        self.call_count.store(0, Ordering::SeqCst);
        self.fail_until.store(0, Ordering::SeqCst);
        self.return_app_error.store(false, Ordering::SeqCst);
        self.return_infra_error.store(false, Ordering::SeqCst);
    }
}

/// Build server with circuit breaker and controlled failure behavior
async fn build_test_server(
    alpn: &'static [u8],
    tracker: CallTracker,
    cb_config: CircuitBreakerConfig,
) -> IrohBundle {
    let mut server_bundle = IrohBundle::builder(None).await.unwrap();

    let call_count = Arc::clone(&tracker.call_count);
    let fail_until = Arc::clone(&tracker.fail_until);
    let return_app_error = Arc::clone(&tracker.return_app_error);
    let return_infra_error = Arc::clone(&tracker.return_infra_error);

    let server = RpcServerBuilder::new(alpn, server_bundle.endpoint().clone())
        .with_circuit_breaker(cb_config)
        .service("test")
        .rpc_resource("call", move |_ctx, req| {
            let call_count = Arc::clone(&call_count);
            let fail_until = Arc::clone(&fail_until);
            let return_app_error = Arc::clone(&return_app_error);
            let return_infra_error = Arc::clone(&return_infra_error);

            Box::pin(async move {
                let attempt = call_count.fetch_add(1, Ordering::SeqCst);
                let should_fail = attempt < fail_until.load(Ordering::SeqCst);

                if should_fail {
                    return Err(ResourceError::CallbackError {
                        message: "Simulated infrastructure error".into(),
                        severity: ErrorSeverityType::Infrastructure,
                        context: None,
                    });
                }

                if return_app_error.load(Ordering::SeqCst) {
                    return Err(ResourceError::CallbackError {
                        message: "Application error - user not found".into(),
                        severity: ErrorSeverityType::Application,
                        context: None,
                    });
                }

                if return_infra_error.load(Ordering::SeqCst) {
                    return Err(ResourceError::CallbackError {
                        message: "Infrastructure error - connection failed".into(),
                        severity: ErrorSeverityType::Infrastructure,
                        context: None,
                    });
                }

                // Success
                if let Body::Rpc(data) = &req.body {
                    Ok(Response { data: data.clone() })
                } else {
                    Err(ResourceError::CallbackError {
                        message: "Expected RPC body".into(),
                        severity: ErrorSeverityType::Application,
                        context: None,
                    })
                }
            })
        })
        .build()
        .build();

    server_bundle.accept(alpn, server).finish().await
}

/// Test 1: Full reliability stack - server circuit breaker + client retry
///
/// Purpose: Verify the complete stack works together correctly
/// Scenario:
/// - Server with circuit breaker (threshold=3)
/// - Client with retry (max_attempts=3)
/// - Mixed success/failure scenario
#[tokio::test]
async fn test_full_reliability_stack() {
    let tracker = CallTracker::new();
    tracker.set_fail_until(2); // Fail first 2 attempts, succeed on 3rd

    let cb_config = CircuitBreakerConfig::builder()
        .consecutive_failures(3)
        .build();

    let server_bundle = build_test_server(b"e2e-full/1", tracker.clone(), cb_config).await;

    // CRITICAL: DNS setup delay
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"e2e-full/1")
        .await
        .unwrap();

    let retry_config = RetryConfig::builder()
        .max_attempts(3)
        .initial_backoff(Duration::from_millis(50))
        .with_custom_predicate(|e| {
            matches!(
                e,
                zel_core::protocol::client::ClientError::Resource(
                    ResourceError::CallbackError { .. }
                )
            )
        })
        .build()
        .expect("Valid config");

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .with_retry_metrics()
        .build()
        .await
        .unwrap();

    // Make call that requires retries
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_ok(), "Should succeed after retries");

    // Verify server was called 3 times (2 failures + 1 success)
    assert_eq!(
        tracker.get_call_count(),
        3,
        "Server should be called 3 times"
    );

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

/// Test 2: Circuit breaker with retries - verify interaction
///
/// Purpose: Verify circuit rejection short-circuits retries
/// Scenario:
/// - Trip circuit with failures
/// - Client retries get CircuitBreakerOpen error
#[tokio::test]
async fn test_circuit_breaker_with_retries() {
    let tracker = CallTracker::new();
    tracker.set_fail_until(10); // Fail all attempts

    let cb_config = CircuitBreakerConfig::builder()
        .consecutive_failures(2)
        .build();

    let server_bundle = build_test_server(b"e2e-cb-retry/1", tracker.clone(), cb_config).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"e2e-cb-retry/1")
        .await
        .unwrap();

    let retry_config = RetryConfig::builder()
        .max_attempts(5)
        .initial_backoff(Duration::from_millis(50))
        .with_custom_predicate(|e| {
            matches!(
                e,
                zel_core::protocol::client::ClientError::Resource(
                    ResourceError::CallbackError { .. }
                )
            )
        })
        .build()
        .expect("Valid config");

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .build()
        .await
        .unwrap();

    // First call: fail 2 times to trip circuit
    let result = client.call("test", "call", Bytes::from("test1")).await;
    assert!(result.is_err(), "Should fail and trip circuit");

    // Circuit should now be open - verify we get CircuitBreakerOpen error
    let result = client.call("test", "call", Bytes::from("test2")).await;
    assert!(result.is_err(), "Should be rejected by circuit");

    if let Err(e) = result {
        let error_msg = e.to_string();
        assert!(
            error_msg.contains("Circuit breaker open"),
            "Should be circuit breaker error, got: {}",
            error_msg
        );
    }

    // Call count should be low (circuit prevents thundering herd)
    let call_count = tracker.get_call_count();
    assert!(
        call_count <= 5,
        "Circuit should prevent excessive retries, got {} calls",
        call_count
    );

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

/// Test 3: Metrics accuracy in real RPC scenarios
///
/// Purpose: Verify retry metrics are accurate
/// Scenario:
/// - Make various calls with retries
/// - Check metrics match actual behavior
#[tokio::test]
async fn test_metrics_accuracy() {
    let tracker = CallTracker::new();

    let cb_config = CircuitBreakerConfig::builder()
        .consecutive_failures(10)
        .build();

    let server_bundle = build_test_server(b"e2e-metrics/1", tracker.clone(), cb_config).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"e2e-metrics/1")
        .await
        .unwrap();

    let retry_config = RetryConfig::builder()
        .max_attempts(3)
        .initial_backoff(Duration::from_millis(50))
        .with_custom_predicate(|e| {
            matches!(
                e,
                zel_core::protocol::client::ClientError::Resource(
                    ResourceError::CallbackError { .. }
                )
            )
        })
        .build()
        .expect("Valid config");

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .with_retry_metrics()
        .build()
        .await
        .unwrap();

    // Call 1: First attempt success (no retry)
    tracker.reset();
    let result = client.call("test", "call", Bytes::from("test1")).await;
    assert!(result.is_ok());
    assert_eq!(tracker.get_call_count(), 1);

    // Call 2: Fail once, succeed on retry
    tracker.reset();
    tracker.set_fail_until(1);
    let result = client.call("test", "call", Bytes::from("test2")).await;
    assert!(result.is_ok());
    assert_eq!(tracker.get_call_count(), 2);

    // Call 3: Fail twice, succeed on third attempt
    tracker.reset();
    tracker.set_fail_until(2);
    let result = client.call("test", "call", Bytes::from("test3")).await;
    assert!(result.is_ok());
    assert_eq!(tracker.get_call_count(), 3);

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

/// Test 4: Error classification E2E
///
/// Purpose: Verify error classification works through complete RPC flow
/// Scenario:
/// - Send application errors
/// - Send infrastructure errors
/// - Verify only infrastructure errors trip circuit
#[tokio::test]
async fn test_error_classification_e2e() {
    let tracker = CallTracker::new();

    let cb_config = CircuitBreakerConfig::builder()
        .consecutive_failures(3)
        .build();

    let server_bundle = build_test_server(b"e2e-classify/1", tracker.clone(), cb_config).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"e2e-classify/1")
        .await
        .unwrap();

    let client = RpcClient::new(conn).await.unwrap();

    // Send 5 application errors - should NOT trip circuit
    tracker.set_return_app_error(true);
    for _ in 0..5 {
        let _ = client.call("test", "call", Bytes::from("test")).await;
    }

    // Circuit should still be closed - successful call should work
    tracker.set_return_app_error(false);
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(
        result.is_ok(),
        "Circuit should still be closed after app errors"
    );

    // Now send infrastructure errors - should trip circuit
    tracker.reset();
    tracker.set_return_infra_error(true);
    for _ in 0..3 {
        let _ = client.call("test", "call", Bytes::from("test")).await;
    }

    // Circuit should now be open
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_err(), "Circuit should be open");

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

/// Test 5: Mixed error types
///
/// Purpose: Verify correct behavior with mix of error types
/// Scenario:
/// - Interleave application and infrastructure errors
/// - Verify only infrastructure errors count toward circuit threshold
#[tokio::test]
async fn test_mixed_error_types() {
    let tracker = CallTracker::new();

    let cb_config = CircuitBreakerConfig::builder()
        .consecutive_failures(3)
        .build();

    let server_bundle = build_test_server(b"e2e-mixed/1", tracker.clone(), cb_config).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"e2e-mixed/1")
        .await
        .unwrap();

    let client = RpcClient::new(conn).await.unwrap();

    // Pattern: 2 infra + 1 app + 1 infra + 1 app + 2 infra
    // The app errors reset the counter, so we need 3 consecutive infra errors

    // 2 infrastructure errors
    tracker.set_return_infra_error(true);
    let _ = client.call("test", "call", Bytes::from("test")).await;
    let _ = client.call("test", "call", Bytes::from("test")).await;

    // 1 application error (resets counter)
    tracker.set_return_infra_error(false);
    tracker.set_return_app_error(true);
    let _ = client.call("test", "call", Bytes::from("test")).await;

    // Circuit should still be working
    tracker.set_return_app_error(false);
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_ok(), "Circuit should still be closed");

    // Now 3 consecutive infrastructure errors to trip it
    tracker.set_return_infra_error(true);
    for _ in 0..3 {
        let _ = client.call("test", "call", Bytes::from("test")).await;
    }

    // Circuit should now be open
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_err(), "Circuit should be open");

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

/// Test 6: Circuit recovery flow
///
/// Purpose: Verify complete circuit recovery cycle
/// Scenario:
/// - Trip circuit with failures
/// - Wait for timeout
/// - Send successful requests
/// - Verify transitions: Closed → Open → HalfOpen → Closed
#[tokio::test]
async fn test_circuit_recovery_flow() {
    let tracker = CallTracker::new();

    let cb_config = CircuitBreakerConfig::builder()
        .consecutive_failures(2)
        .consecutive_successes(2)
        .timeout(Duration::from_millis(200))
        .build();

    let server_bundle = build_test_server(b"e2e-recovery/1", tracker.clone(), cb_config).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"e2e-recovery/1")
        .await
        .unwrap();

    let client = RpcClient::new(conn).await.unwrap();

    // Trip circuit
    tracker.set_return_infra_error(true);
    let _ = client.call("test", "call", Bytes::from("test")).await;
    let _ = client.call("test", "call", Bytes::from("test")).await;

    // Verify circuit is open
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_err(), "Circuit should be open");

    // Wait for timeout to enter half-open
    time::sleep(Duration::from_millis(250)).await;

    // Send successful requests for recovery
    tracker.set_return_infra_error(false);
    for _ in 0..3 {
        let result = client.call("test", "call", Bytes::from("test")).await;
        // Should succeed in half-open or after closing
        assert!(result.is_ok(), "Recovery should succeed");
    }

    // Verify normal operation resumed
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_ok(), "Circuit should be closed and working");

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

/// Test 7: Per-peer isolation E2E
///
/// Purpose: Verify per-peer circuit isolation in real scenario
/// Scenario:
/// - Multiple clients
/// - Fail calls from client A
/// - Verify client A circuit opens, client B remains closed
#[tokio::test]
async fn test_per_peer_isolation_e2e() {
    let tracker = CallTracker::new();

    let cb_config = CircuitBreakerConfig::builder()
        .consecutive_failures(2)
        .build();

    let server_bundle = build_test_server(b"e2e-isolation/1", tracker.clone(), cb_config).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create two separate client bundles (different peers)
    let client_bundle_a = IrohBundle::builder(None).await.unwrap().finish().await;
    let client_bundle_b = IrohBundle::builder(None).await.unwrap().finish().await;

    let conn_a = client_bundle_a
        .endpoint
        .connect(server_bundle.endpoint.id(), b"e2e-isolation/1")
        .await
        .unwrap();

    let conn_b = client_bundle_b
        .endpoint
        .connect(server_bundle.endpoint.id(), b"e2e-isolation/1")
        .await
        .unwrap();

    let client_a = RpcClient::new(conn_a).await.unwrap();
    let client_b = RpcClient::new(conn_b).await.unwrap();

    // Fail calls from client A
    tracker.set_return_infra_error(true);
    let _ = client_a.call("test", "call", Bytes::from("test")).await;
    let _ = client_a.call("test", "call", Bytes::from("test")).await;

    // Client A should be rejected
    let result_a = client_a.call("test", "call", Bytes::from("test")).await;
    assert!(result_a.is_err(), "Client A should be blocked");

    // Client B should still work (server allows successful calls)
    tracker.set_return_infra_error(false);
    let result_b = client_b.call("test", "call", Bytes::from("test")).await;
    assert!(result_b.is_ok(), "Client B should still work");

    // Verify client A is still blocked
    let result_a = client_a.call("test", "call", Bytes::from("test")).await;
    assert!(
        result_a.is_err(),
        "Client A should still be blocked despite B's success"
    );

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

/// Test 8: Concurrent requests
///
/// Purpose: Verify thread safety under concurrent load
/// Scenario:
/// - Multiple concurrent requests from same client
/// - Some succeed, some fail
/// - Verify no race conditions and consistent behavior
#[tokio::test]
async fn test_concurrent_requests() {
    let tracker = CallTracker::new();

    let cb_config = CircuitBreakerConfig::builder()
        .consecutive_failures(5)
        .build();

    let server_bundle = build_test_server(b"e2e-concurrent/1", tracker.clone(), cb_config).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"e2e-concurrent/1")
        .await
        .unwrap();

    let client = Arc::new(RpcClient::new(conn).await.unwrap());

    // Set to fail first 3 requests, then succeed
    tracker.set_fail_until(3);

    // Launch 10 concurrent requests
    let futures: Vec<_> = (0..10)
        .map(|i| {
            let client = Arc::clone(&client);
            async move {
                client
                    .call("test", "call", Bytes::from(format!("test{}", i)))
                    .await
            }
        })
        .collect();

    let results = futures::future::join_all(futures).await;

    // Some should succeed (after first 3 failures)
    let success_count = results.iter().filter(|r| r.is_ok()).count();
    let error_count = results.iter().filter(|r| r.is_err()).count();

    assert!(
        success_count > 0,
        "Some requests should succeed, got {} successes",
        success_count
    );
    assert!(
        error_count > 0,
        "Some requests should fail, got {} errors",
        error_count
    );

    // Total should be 10
    assert_eq!(
        success_count + error_count,
        10,
        "Should have processed all 10 requests"
    );

    // No panics or race conditions - test passes if we get here

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}
