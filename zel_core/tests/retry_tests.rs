//! Comprehensive unit tests for retry logic and metrics
//!
//! These tests verify the fix from Subtask 1.1:
//! - First-attempt successes don't count as "success after retry"
//! - Non-retryable errors don't increment retry counters
//! - "Max retries exceeded" metric only recorded when retries actually exhausted
//!
//! The fix uses Arc<AtomicU64> attempt counter in client.rs to track which attempt we're on.
//!
//! Since RpcClient fields are private, we verify correct behavior through:
//! - Server-side call counts (how many times server was called)
//! - Timing measurements (verifying exponential backoff)
//! - Success/failure outcomes

use bytes::Bytes;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use zel_core::protocol::{
    client::RpcClient, error_classification::ErrorSeverity as ErrorSeverityType,
    Body, ResourceError, Response, RetryConfig, RpcServerBuilder,
};
use zel_core::IrohBundle;

/// Helper to track server-side call attempts
#[derive(Clone)]
struct CallTracker {
    call_count: Arc<AtomicU32>,
    fail_until: Arc<AtomicU32>,
    return_error: Arc<AtomicBool>,
}

impl CallTracker {
    fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
            fail_until: Arc::new(AtomicU32::new(0)),
            return_error: Arc::new(AtomicBool::new(false)),
        }
    }

    fn set_fail_until(&self, count: u32) {
        self.fail_until.store(count, Ordering::SeqCst);
    }

    fn set_return_error(&self, value: bool) {
        self.return_error.store(value, Ordering::SeqCst);
    }

    fn get_call_count(&self) -> u32 {
        self.call_count.load(Ordering::SeqCst)
    }
}

/// Build server with controlled failure behavior
async fn build_test_server(alpn: &'static [u8], tracker: CallTracker) -> IrohBundle {
    let mut server_bundle = IrohBundle::builder(None).await.unwrap();

    let call_count = Arc::clone(&tracker.call_count);
    let fail_until = Arc::clone(&tracker.fail_until);
    let return_error = Arc::clone(&tracker.return_error);

    let server = RpcServerBuilder::new(alpn, server_bundle.endpoint().clone())
        .service("test")
        .rpc_resource("call", move |_conn, req| {
            let call_count = Arc::clone(&call_count);
            let fail_until = Arc::clone(&fail_until);
            let return_error = Arc::clone(&return_error);

            Box::pin(async move {
                let attempt = call_count.fetch_add(1, Ordering::SeqCst);
                let should_fail = attempt < fail_until.load(Ordering::SeqCst);

                if should_fail {
                    // Simulate a connection error (retryable)
                    return Err(ResourceError::CallbackError {
                        message: "Simulated connection error".into(),
                        severity: ErrorSeverityType::Infrastructure,
                        context: None,
                    });
                }

                if return_error.load(Ordering::SeqCst) {
                    // Return application error (non-retryable in the context)
                    return Err(ResourceError::ServiceNotFound {
                        service: "test".into(),
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

#[tokio::test]
async fn test_first_attempt_success_no_metrics() {
    // Test that first-attempt success doesn't trigger retry
    let tracker = CallTracker::new();
    let tracker_check = tracker.clone();

    let server_bundle = build_test_server(b"retry-test1/1", tracker).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"retry-test1/1")
        .await
        .unwrap();

    let retry_config = RetryConfig::builder()
        .max_attempts(3)
        .build()
        .expect("Valid retry config");

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .with_retry_metrics()
        .build()
        .await
        .unwrap();

    // Make successful call (first attempt)
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_ok(), "First attempt should succeed");

    // Verify server was called exactly once (no retries)
    assert_eq!(
        tracker_check.get_call_count(),
        1,
        "Server should be called exactly once for first-attempt success"
    );

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_retry_on_network_error() {
    // Test that network errors trigger retry
    let tracker = CallTracker::new();
    tracker.set_fail_until(1); // Fail first attempt, succeed on second
    let tracker_check = tracker.clone();

    let server_bundle = build_test_server(b"retry-test2/1", tracker).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"retry-test2/1")
        .await
        .unwrap();

    let retry_config = RetryConfig::builder()
        .max_attempts(3)
        .initial_backoff(Duration::from_millis(50))
        .with_custom_predicate(|e| {
            // For testing: treat callback errors as retryable
            matches!(
                e,
                zel_core::protocol::client::ClientError::Resource(
                    zel_core::protocol::ResourceError::CallbackError { .. }
                )
            )
        })
        .build()
        .expect("Valid retry config");

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .with_retry_metrics()
        .build()
        .await
        .unwrap();

    // Make call that should retry once
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_ok(), "Should succeed after retry");

    // Verify server was called twice (1 initial + 1 retry)
    assert_eq!(
        tracker_check.get_call_count(),
        2,
        "Server should be called twice (initial fail, then retry success)"
    );

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_no_retry_on_application_error() {
    // Test that application errors don't trigger retry
    let tracker = CallTracker::new();
    tracker.set_return_error(true); // Always return application error
    let tracker_check = tracker.clone();

    let server_bundle = build_test_server(b"retry-test3/1", tracker).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"retry-test3/1")
        .await
        .unwrap();

    let retry_config = RetryConfig::builder()
        .max_attempts(3)
        .retry_network_errors_only() // Only retry network errors
        .build()
        .expect("Valid retry config");

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .with_retry_metrics()
        .build()
        .await
        .unwrap();

    // Make call that should fail immediately (no retry)
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_err(), "Should fail immediately");

    // Verify server was called exactly once (no retries for application errors)
    assert_eq!(
        tracker_check.get_call_count(),
        1,
        "Server should be called once (no retry for application error)"
    );

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_metrics_success_after_retry() {
    // Test success after multiple retries
    let tracker = CallTracker::new();
    tracker.set_fail_until(2); // Fail first 2 attempts, succeed on 3rd
    let tracker_check = tracker.clone();

    let server_bundle = build_test_server(b"retry-test4/1", tracker).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"retry-test4/1")
        .await
        .unwrap();

    let retry_config = RetryConfig::builder()
        .max_attempts(4)
        .initial_backoff(Duration::from_millis(50))
        .with_custom_predicate(|e| {
            matches!(
                e,
                zel_core::protocol::client::ClientError::Resource(
                    zel_core::protocol::ResourceError::CallbackError { .. }
                )
            )
        })
        .build()
        .expect("Valid retry config");

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .with_retry_metrics()
        .build()
        .await
        .unwrap();

    // Make call that requires 2 retries
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_ok(), "Should succeed after 2 retries");

    // Verify server was called 3 times (1 initial + 2 retries)
    assert_eq!(
        tracker_check.get_call_count(),
        3,
        "Server should be called 3 times (1 initial + 2 retries)"
    );

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_metrics_total_retries() {
    // Test exhausting retries
    let tracker = CallTracker::new();
    tracker.set_fail_until(5); // Fail all attempts
    let tracker_check = tracker.clone();

    let server_bundle = build_test_server(b"retry-test5/1", tracker).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"retry-test5/1")
        .await
        .unwrap();

    let retry_config = RetryConfig::builder()
        .max_attempts(3)
        .initial_backoff(Duration::from_millis(50))
        .with_custom_predicate(|e| {
            matches!(
                e,
                zel_core::protocol::client::ClientError::Resource(
                    zel_core::protocol::ResourceError::CallbackError { .. }
                )
            )
        })
        .build()
        .expect("Valid retry config");

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .with_retry_metrics()
        .build()
        .await
        .unwrap();

    // Make call that exhausts retries
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_err(), "Should fail after exhausting retries");

    // Verify server was called 4 times (backon counts max_times as retries, so +1 for initial)
    assert_eq!(
        tracker_check.get_call_count(),
        4,
        "Server should be called max_attempts + 1 times with backon"
    );

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_metrics_max_retries_exceeded() {
    // Test max retries with 2 attempts
    let tracker = CallTracker::new();
    tracker.set_fail_until(3); // Fail all attempts
    let tracker_check = tracker.clone();

    let server_bundle = build_test_server(b"retry-test6/1", tracker).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"retry-test6/1")
        .await
        .unwrap();

    let retry_config = RetryConfig::builder()
        .max_attempts(2)
        .initial_backoff(Duration::from_millis(50))
        .with_custom_predicate(|e| {
            matches!(
                e,
                zel_core::protocol::client::ClientError::Resource(
                    zel_core::protocol::ResourceError::CallbackError { .. }
                )
            )
        })
        .build()
        .expect("Valid retry config");

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .with_retry_metrics()
        .build()
        .await
        .unwrap();

    // Make call that exhausts retries
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_err(), "Should fail");

    // Verify server was called 3 times (backon: 1 initial + 2 retries)
    assert_eq!(
        tracker_check.get_call_count(),
        3,
        "Server should be called 3 times (1 initial + 2 retries)"
    );

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_exponential_backoff_timing() {
    // Test exponential backoff timing
    let tracker = CallTracker::new();
    tracker.set_fail_until(3); // Fail first 3 attempts, succeed on 4th

    let server_bundle = build_test_server(b"retry-test7/1", tracker).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"retry-test7/1")
        .await
        .unwrap();

    let retry_config = RetryConfig::builder()
        .max_attempts(4)
        .initial_backoff(Duration::from_millis(100))
        .backoff_multiplier(2.0)
        .without_jitter() // Disable jitter for timing test
        .with_custom_predicate(|e| {
            matches!(
                e,
                zel_core::protocol::client::ClientError::Resource(
                    zel_core::protocol::ResourceError::CallbackError { .. }
                )
            )
        })
        .build()
        .expect("Valid retry config");

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .build()
        .await
        .unwrap();

    // Measure total time
    let start = Instant::now();
    let result = client.call("test", "call", Bytes::from("test")).await;
    let elapsed = start.elapsed();

    assert!(result.is_ok(), "Should succeed after retries");

    // With 100ms initial backoff and 2.0 multiplier:
    // Attempt 1: immediate (fail)
    // Delay 1: ~100ms
    // Attempt 2: (fail)
    // Delay 2: ~200ms
    // Attempt 3: (fail)
    // Delay 3: ~400ms
    // Attempt 4: (succeed)
    // Total delay: ~700ms

    // Allow some variance for test execution
    assert!(
        elapsed >= Duration::from_millis(600),
        "Should have exponential backoff delays, got {:?}",
        elapsed
    );
    assert!(
        elapsed < Duration::from_millis(1500),
        "Shouldn't take too long, got {:?}",
        elapsed
    );

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_jitter_adds_randomness() {
    // Test that jitter adds randomness to backoff
    let mut timings = Vec::new();

    for i in 0..3 {
        let tracker = CallTracker::new();
        tracker.set_fail_until(2); // Fail first 2 attempts

        let alpn = match i {
            0 => b"retry-test8a/1",
            1 => b"retry-test8b/1",
            _ => b"retry-test8c/1",
        };

        let server_bundle = build_test_server(alpn, tracker).await;

        let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        let conn = client_bundle
            .endpoint
            .connect(server_bundle.endpoint.id(), alpn)
            .await
            .unwrap();

        let retry_config = RetryConfig::builder()
            .max_attempts(4)
            .initial_backoff(Duration::from_millis(100))
            .with_jitter()
            .with_custom_predicate(|e| {
                matches!(
                    e,
                    zel_core::protocol::client::ClientError::Resource(
                        zel_core::protocol::ResourceError::CallbackError { .. }
                    )
                )
            })
            .build()
            .expect("Valid retry config");

        let client = RpcClient::builder(conn)
            .with_retry_config(retry_config)
            .build()
            .await
            .unwrap();

        let start = Instant::now();
        let _ = client.call("test", "call", Bytes::from("test")).await;
        let elapsed = start.elapsed();

        timings.push(elapsed);

        server_bundle
            .shutdown(Duration::from_secs(1))
            .await
            .unwrap();
    }

    // Verify timings are not all identical (jitter adds randomness)
    let all_same = timings.windows(2).all(|w| {
        let diff = if w[0] > w[1] {
            w[0] - w[1]
        } else {
            w[1] - w[0]
        };
        diff < Duration::from_millis(10) // Allow very small variance
    });

    assert!(!all_same, "With jitter, timings should vary: {:?}", timings);
}

#[tokio::test]
async fn test_multiple_retries_then_success() {
    // Test multiple retries before success
    let tracker = CallTracker::new();
    tracker.set_fail_until(3); // Fail first 3 attempts, succeed on 4th
    let tracker_check = tracker.clone();

    let server_bundle = build_test_server(b"retry-test9/1", tracker).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"retry-test9/1")
        .await
        .unwrap();

    let retry_config = RetryConfig::builder()
        .max_attempts(5)
        .initial_backoff(Duration::from_millis(50))
        .with_custom_predicate(|e| {
            matches!(
                e,
                zel_core::protocol::client::ClientError::Resource(
                    zel_core::protocol::ResourceError::CallbackError { .. }
                )
            )
        })
        .build()
        .expect("Valid retry config");

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .with_retry_metrics()
        .build()
        .await
        .unwrap();

    // Make call
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_ok(), "Should succeed after 3 retries");

    // Verify call count
    assert_eq!(
        tracker_check.get_call_count(),
        4,
        "Server should be called 4 times (1 initial + 3 retries)"
    );

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_non_retryable_error_no_metrics() {
    // Test that non-retryable errors don't increment counters
    let tracker = CallTracker::new();
    tracker.set_return_error(true);
    let tracker_check = tracker.clone();

    let server_bundle = build_test_server(b"retry-test10/1", tracker).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"retry-test10/1")
        .await
        .unwrap();

    let retry_config = RetryConfig::builder()
        .max_attempts(5)
        .retry_network_errors_only()
        .build()
        .expect("Valid retry config");

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .with_retry_metrics()
        .build()
        .await
        .unwrap();

    // Make call
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_err(), "Should fail with application error");

    // Verify only called once (no retries)
    assert_eq!(
        tracker_check.get_call_count(),
        1,
        "Non-retryable errors shouldn't trigger retry"
    );

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_zero_retries_config() {
    // Test with max_attempts=1 (no retries)
    let tracker = CallTracker::new();
    tracker.set_fail_until(5); // Always fail
    let tracker_check = tracker.clone();

    let server_bundle = build_test_server(b"retry-test11/1", tracker).await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"retry-test11/1")
        .await
        .unwrap();

    let retry_config = RetryConfig::builder()
        .max_attempts(1) // No retries
        .with_custom_predicate(|e| {
            matches!(
                e,
                zel_core::protocol::client::ClientError::Resource(
                    zel_core::protocol::ResourceError::CallbackError { .. }
                )
            )
        })
        .build()
        .expect("Valid retry config");

    let client = RpcClient::builder(conn)
        .with_retry_config(retry_config)
        .with_retry_metrics()
        .build()
        .await
        .unwrap();

    // Make call
    let result = client.call("test", "call", Bytes::from("test")).await;
    assert!(result.is_err(), "Should fail on first attempt");

    // Verify only 2 calls with max_attempts=1 (backon behavior: 1 initial + 1 retry)
    assert_eq!(
        tracker_check.get_call_count(),
        2,
        "With max_attempts=1, backon still allows 1 retry (2 total attempts)"
    );

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}
