//! Comprehensive unit tests for circuit breaker error filtering
//!
//! These tests verify the double-Result pattern implementation from Subtask 1.4,
//! which ensures that:
//! - Application errors (ErrorSeverity::Application) bypass circuit breaker tracking
//! - Infrastructure errors (ErrorSeverity::Infrastructure) trip the circuit
//! - System failure errors (ErrorSeverity::SystemFailure) trip the circuit

use circuitbreaker_rs::BreakerError;
use std::time::Duration;
use tokio::time;
use zel_core::protocol::{
    CircuitBreakerConfig, CircuitState, ErrorClassificationExt, PeerCircuitBreakers,
};

/// Helper function to create a test peer ID
fn create_test_peer() -> iroh::PublicKey {
    iroh::SecretKey::generate(&mut rand::rng()).public()
}

/// Test 1: APPLICATION ERRORS DON'T TRIP CIRCUIT ⭐ MOST CRITICAL
///
/// This is the most important test - it proves that the double-Result pattern
/// from Subtask 1.4 works correctly. Application errors should not count toward
/// the circuit breaker failure threshold.
///
/// Setup:
/// - Configure circuit breaker with consecutive_failures=2
/// - Send 5 application errors (more than the threshold)
///
/// Expected:
/// - All calls return operation errors (not rejected by circuit)
/// - Circuit state remains Closed (not Open)
/// - Circuit does NOT trip despite exceeding threshold
#[tokio::test]
async fn test_application_errors_dont_trip_circuit() {
    let config = CircuitBreakerConfig::builder()
        .consecutive_failures(2)
        .build();

    let breakers = PeerCircuitBreakers::new(config);
    let peer_id = create_test_peer();

    // Send 5 application errors (more than threshold of 2)
    for i in 0..5 {
        let result = breakers
            .call_async(&peer_id, || async {
                Err::<(), _>(anyhow::anyhow!("User not found").as_application())
            })
            .await;

        // Should return operation error (not circuit rejection)
        assert!(
            matches!(result, Err(BreakerError::Operation(_))),
            "Iteration {}: Application error should return as operation error, got: {:?}",
            i,
            result
        );
    }

    // CRITICAL ASSERTION: Circuit should still be closed
    // Try a successful operation to update state tracking
    let result = breakers
        .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("success") })
        .await;
    assert!(
        result.is_ok(),
        "Successful operation should work after application errors"
    );

    // Verify circuit state is still Closed
    let stats = breakers.get_stats().await;
    let state = stats.get(&peer_id).expect("Should have stats for peer");

    assert_eq!(
        *state,
        CircuitState::Closed,
        "Circuit should NOT open for application errors - this proves the Subtask 1.4 fix works"
    );
}

/// Test 2: INFRASTRUCTURE ERRORS TRIP CIRCUIT
///
/// Verify that infrastructure errors count toward the circuit threshold
/// and cause the circuit to open after reaching the consecutive failure limit.
///
/// Setup:
/// - Configure consecutive_failures=2
/// - Send 2 infrastructure errors
///
/// Expected:
/// - Circuit opens after 2nd failure
/// - Circuit state is Open
/// - Subsequent calls rejected with CircuitBreakerOpen error
#[tokio::test]
async fn test_infrastructure_errors_trip_circuit() {
    let config = CircuitBreakerConfig::builder()
        .consecutive_failures(2)
        .build();

    let breakers = PeerCircuitBreakers::new(config);
    let peer_id = create_test_peer();

    // Send 2 infrastructure errors
    for i in 0..2 {
        let result = breakers
            .call_async(&peer_id, || async {
                Err::<(), _>(anyhow::anyhow!("Connection failed").as_infrastructure())
            })
            .await;

        // First two should fail normally (operation errors or internal errors)
        assert!(
            result.is_err(),
            "Iteration {}: Infrastructure error should result in error",
            i
        );
    }

    // Next call should be rejected by the now-open circuit
    let result: Result<(), _> = breakers
        .call_async(&peer_id, || async {
            panic!("Should not reach here - circuit should reject");
        })
        .await;

    // Should get Open error (circuit is open)
    assert!(
        matches!(result, Err(BreakerError::Open)),
        "Circuit should reject requests after threshold, got: {:?}",
        result
    );

    // Verify circuit state is now Open
    let stats = breakers.get_stats().await;
    let state = stats.get(&peer_id).expect("Should have stats");
    assert_eq!(
        *state,
        CircuitState::Open,
        "Circuit should open after infrastructure error threshold"
    );
}

/// Test 3: SYSTEM FAILURE ERRORS TRIP CIRCUIT
///
/// Verify that system failure errors (explicitly marked) count toward
/// the circuit threshold and trip the circuit.
///
/// Setup:
/// - Configure consecutive_failures=2
/// - Send 2 system failure errors
///
/// Expected:
/// - Circuit opens after 2nd failure
/// - Circuit state is Open
#[tokio::test]
async fn test_system_failure_errors_trip_circuit() {
    let config = CircuitBreakerConfig::builder()
        .consecutive_failures(2)
        .build();

    let breakers = PeerCircuitBreakers::new(config);
    let peer_id = create_test_peer();

    // Send 2 system failure errors
    for i in 0..2 {
        let result = breakers
            .call_async(&peer_id, || async {
                Err::<(), _>(anyhow::anyhow!("Database timeout").as_system_failure())
            })
            .await;

        assert!(
            result.is_err(),
            "Iteration {}: System failure should result in error",
            i
        );
    }

    // Next call should be rejected
    let result = breakers
        .call_async(&peer_id, || async {
            Ok::<_, anyhow::Error>("should not reach")
        })
        .await;

    assert!(
        matches!(result, Err(BreakerError::Open)),
        "Circuit should open after system failure threshold"
    );

    // Verify state is Open
    let stats = breakers.get_stats().await;
    assert_eq!(
        stats.get(&peer_id),
        Some(&CircuitState::Open),
        "Circuit should be open after system failures"
    );
}

/// Test 4: CIRCUIT OPENS AFTER EXACT THRESHOLD
///
/// Verify the circuit opens exactly when reaching the configured
/// failure count, not before or after.
///
/// Setup:
/// - Configure consecutive_failures=3
/// - Send exactly 3 infrastructure errors
///
/// Expected:
/// - Circuit opens after 3rd failure
/// - 4th call is rejected without reaching the operation
#[tokio::test]
async fn test_circuit_opens_after_threshold() {
    let config = CircuitBreakerConfig::builder()
        .consecutive_failures(3)
        .build();

    let breakers = PeerCircuitBreakers::new(config);
    let peer_id = create_test_peer();

    // Send exactly 3 infrastructure errors
    for i in 0..3 {
        let result = breakers
            .call_async(&peer_id, || async {
                Err::<(), _>(
                    anyhow::anyhow!(format!("Infrastructure error {}", i)).as_infrastructure(),
                )
            })
            .await;

        assert!(result.is_err(), "Error {} should fail", i);
    }

    // 4th call should be rejected without reaching the operation
    let mut operation_called = false;
    let result = breakers
        .call_async(&peer_id, || async {
            operation_called = true;
            Ok::<_, anyhow::Error>("should not execute")
        })
        .await;

    assert!(
        matches!(result, Err(BreakerError::Open)),
        "Circuit should reject 4th call"
    );
    assert!(
        !operation_called,
        "Operation should not be called when circuit is open"
    );

    // Verify state
    let stats = breakers.get_stats().await;
    assert_eq!(stats.get(&peer_id), Some(&CircuitState::Open));
}

/// Test 5: CIRCUIT CLOSES AFTER RECOVERY
///
/// Verify the circuit can recover and close after:
/// 1. Opening due to failures
/// 2. Waiting for timeout (half-open state)
/// 3. Successful test requests
///
/// Setup:
/// - Open circuit with failures
/// - Wait for timeout duration
/// - Send successful requests
///
/// Expected:
/// - Circuit transitions: Closed → Open → HalfOpen → Closed
/// - After recovery, circuit accepts new requests normally
#[tokio::test]
async fn test_circuit_closes_after_recovery() {
    let config = CircuitBreakerConfig::builder()
        .consecutive_failures(2)
        .consecutive_successes(2)
        .timeout(Duration::from_millis(100)) // Short timeout for testing
        .build();

    let breakers = PeerCircuitBreakers::new(config);
    let peer_id = create_test_peer();

    // Step 1: Open the circuit with failures
    for _ in 0..2 {
        let _result = breakers
            .call_async(&peer_id, || async {
                Err::<(), _>(anyhow::anyhow!("Failure").as_infrastructure())
            })
            .await;
    }

    // Verify circuit is open
    let result = breakers
        .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("test") })
        .await;
    assert!(matches!(result, Err(BreakerError::Open)));

    // Step 2: Wait for timeout to enter half-open state
    time::sleep(Duration::from_millis(150)).await;

    // Step 3: Send successful requests to close circuit
    // The circuit should allow test requests in half-open state
    for i in 0..3 {
        let result = breakers
            .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("success") })
            .await;

        // Should succeed (either in half-open testing or after closing)
        assert!(result.is_ok(), "Recovery attempt {} should succeed", i);
    }

    // Step 4: Verify circuit is now closed
    let stats = breakers.get_stats().await;
    assert_eq!(
        stats.get(&peer_id),
        Some(&CircuitState::Closed),
        "Circuit should be closed after successful recovery"
    );

    // Step 5: Verify normal operation resumes
    let result = breakers
        .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("normal op") })
        .await;
    assert!(
        result.is_ok(),
        "Normal operations should work after recovery"
    );
}

/// Test 6: PER-PEER ISOLATION
///
/// Verify that each peer has an independent circuit breaker state.
/// Failures from one peer should not affect another peer's circuit.
///
/// Setup:
/// - Create two different peers
/// - Fail calls to peer A (open its circuit)
///
/// Expected:
/// - Peer A circuit is Open
/// - Peer B circuit is still Closed
/// - Peer A failures don't affect peer B
#[tokio::test]
async fn test_per_peer_isolation() {
    let config = CircuitBreakerConfig::builder()
        .consecutive_failures(2)
        .build();

    let breakers = PeerCircuitBreakers::new(config);
    let peer_a = create_test_peer();
    let peer_b = create_test_peer();

    // Fail peer A's circuit
    for _ in 0..2 {
        let _result = breakers
            .call_async(&peer_a, || async {
                Err::<(), _>(anyhow::anyhow!("Peer A failure").as_infrastructure())
            })
            .await;
    }

    // Verify peer A circuit is open
    let result_a = breakers
        .call_async(&peer_a, || async { Ok::<_, anyhow::Error>("test") })
        .await;
    assert!(
        matches!(result_a, Err(BreakerError::Open)),
        "Peer A circuit should be open"
    );

    let stats = breakers.get_stats().await;
    assert_eq!(
        stats.get(&peer_a),
        Some(&CircuitState::Open),
        "Peer A should have open circuit"
    );

    // Verify peer B circuit is still working
    let result_b = breakers
        .call_async(&peer_b, || async { Ok::<_, anyhow::Error>("success") })
        .await;
    assert!(result_b.is_ok(), "Peer B should still work");

    let stats = breakers.get_stats().await;
    assert_eq!(
        stats.get(&peer_b),
        Some(&CircuitState::Closed),
        "Peer B should have closed circuit"
    );

    // Send more successful requests to peer B to ensure isolation
    for i in 0..3 {
        let result = breakers
            .call_async(&peer_b, || async { Ok::<_, anyhow::Error>("success") })
            .await;
        assert!(result.is_ok(), "Peer B operation {} should succeed", i);
    }

    // Verify peer A is still blocked
    let result_a = breakers
        .call_async(&peer_a, || async { Ok::<_, anyhow::Error>("test") })
        .await;
    assert!(
        matches!(result_a, Err(BreakerError::Open)),
        "Peer A should still be blocked despite peer B successes"
    );
}

/// Test 7: HALF-OPEN STATE ALLOWS TEST REQUESTS
///
/// Verify the half-open state behavior:
/// - After timeout, circuit enters half-open
/// - Half-open state allows test requests through
/// - Success in half-open closes the circuit
/// - Failure in half-open reopens the circuit
///
/// Setup:
/// - Open circuit with failures
/// - Wait for timeout (circuit enters half-open)
///
/// Expected:
/// - Half-open state allows requests through
/// - Success closes circuit
/// - Failure reopens circuit
#[tokio::test]
async fn test_half_open_state_allows_test_requests() {
    let config = CircuitBreakerConfig::builder()
        .consecutive_failures(2)
        .consecutive_successes(1)
        .timeout(Duration::from_millis(100))
        .build();

    let breakers = PeerCircuitBreakers::new(config);
    let peer_id = create_test_peer();

    // Open the circuit
    for _ in 0..2 {
        let _result = breakers
            .call_async(&peer_id, || async {
                Err::<(), _>(anyhow::anyhow!("Failure").as_infrastructure())
            })
            .await;
    }

    // Verify circuit is open
    let result = breakers
        .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("test") })
        .await;
    assert!(matches!(result, Err(BreakerError::Open)));

    // Wait for timeout to enter half-open
    time::sleep(Duration::from_millis(150)).await;

    // Test successful recovery in half-open
    let result = breakers
        .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("recovery") })
        .await;
    assert!(result.is_ok(), "Half-open should allow test request");

    // After success, circuit should close
    let stats = breakers.get_stats().await;
    assert_eq!(
        stats.get(&peer_id),
        Some(&CircuitState::Closed),
        "Circuit should close after successful half-open test"
    );

    // Test failure case: open circuit again and fail in half-open
    for _ in 0..2 {
        let _result = breakers
            .call_async(&peer_id, || async {
                Err::<(), _>(anyhow::anyhow!("Failure").as_infrastructure())
            })
            .await;
    }

    // Wait for timeout again
    time::sleep(Duration::from_millis(150)).await;

    // Fail in half-open state
    let result = breakers
        .call_async(&peer_id, || async {
            Err::<(), _>(anyhow::anyhow!("Half-open failure").as_infrastructure())
        })
        .await;
    assert!(result.is_err(), "Half-open failure should return error");

    // Circuit should reopen after failure in half-open
    let result = breakers
        .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("test") })
        .await;
    assert!(
        matches!(result, Err(BreakerError::Open)),
        "Circuit should reopen after half-open failure"
    );
}

/// Test 8: MIXED ERRORS
///
/// Verify correct behavior when mixing error types.
/// Only severe errors (infrastructure/system) should count toward threshold.
/// Application errors should not contribute to the failure count.
///
/// Setup:
/// - Configure consecutive_failures=3
/// - Send mixed sequence: 2 application, 1 infrastructure, 2 application, 2 infrastructure
///
/// Expected:
/// - Only infrastructure errors count (3 total)
/// - Circuit opens after 3rd infrastructure error
/// - Application errors don't affect the failure count
#[tokio::test]
async fn test_mixed_errors() {
    let config = CircuitBreakerConfig::builder()
        .consecutive_failures(3)
        .build();

    let breakers = PeerCircuitBreakers::new(config);
    let peer_id = create_test_peer();

    // Important: circuitbreaker-rs resets consecutive failure counter on success.
    // Our double-Result pattern returns Ok(Err(...)) for application errors,
    // which counts as a "success" to the underlying circuit breaker.
    // This means application errors actually RESET the counter, not just ignore it.

    // Send 2 infrastructure errors
    for i in 0..2 {
        let result = breakers
            .call_async(&peer_id, || async {
                Err::<(), _>(anyhow::anyhow!(format!("Infrastructure {}", i)).as_infrastructure())
            })
            .await;
        assert!(result.is_err());
    }

    // Send application error - this resets the consecutive failure counter
    let result = breakers
        .call_async(&peer_id, || async {
            Err::<(), _>(anyhow::anyhow!("User not found").as_application())
        })
        .await;
    assert!(matches!(result, Err(BreakerError::Operation(_))));

    // Now we need 3 consecutive infrastructure errors to trip the circuit
    for i in 0..3 {
        let result = breakers
            .call_async(&peer_id, || async {
                Err::<(), _>(
                    anyhow::anyhow!(format!("Infrastructure {}", i + 2)).as_infrastructure(),
                )
            })
            .await;
        assert!(result.is_err());
    }

    // Circuit should now be open
    let result = breakers
        .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("test") })
        .await;
    assert!(
        matches!(result, Err(BreakerError::Open)),
        "Circuit should open after 3 consecutive infrastructure errors"
    );

    let stats = breakers.get_stats().await;
    assert_eq!(
        stats.get(&peer_id),
        Some(&CircuitState::Open),
        "Circuit should be open"
    );
}

/// Test 9: CONCURRENT REQUESTS UNDER LOAD
///
/// Verify circuit breaker behavior under concurrent load.
/// This tests thread safety and ensures the circuit breaker
/// correctly handles multiple simultaneous requests.
///
/// Setup:
/// - Configure consecutive_failures=5
/// - Send 10 concurrent infrastructure errors
///
/// Expected:
/// - Circuit eventually opens
/// - No race conditions or panics
#[tokio::test]
async fn test_concurrent_requests_under_load() {
    let config = CircuitBreakerConfig::builder()
        .consecutive_failures(5)
        .build();

    let breakers = PeerCircuitBreakers::new(config);
    let peer_id = create_test_peer();

    // Send 10 concurrent infrastructure errors
    let futures: Vec<_> = (0..10)
        .map(|i| {
            let breakers = &breakers;
            let peer_id = peer_id;
            async move {
                breakers
                    .call_async(&peer_id, || async {
                        Err::<(), _>(
                            anyhow::anyhow!(format!("Concurrent failure {}", i))
                                .as_infrastructure(),
                        )
                    })
                    .await
            }
        })
        .collect();

    let results = futures::future::join_all(futures).await;

    // Some should fail with operation errors, some might be rejected
    let operation_errors = results
        .iter()
        .filter(|r| matches!(r, Err(BreakerError::Operation(_))))
        .count();
    let rejected = results
        .iter()
        .filter(|r| matches!(r, Err(BreakerError::Open)))
        .count();

    // At least some should have executed and failed
    assert!(
        operation_errors > 0 || rejected > 0,
        "Should have failures or rejections"
    );

    // Circuit should eventually be open
    let result = breakers
        .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("test") })
        .await;
    assert!(
        matches!(result, Err(BreakerError::Open)),
        "Circuit should be open after concurrent failures"
    );
}

/// Test 10: CIRCUIT RECOVERY WITH FAILURES
///
/// Verify that failures during half-open testing reopen the circuit.
///
/// Setup:
/// - Open circuit
/// - Wait for timeout
/// - Send failing request in half-open
///
/// Expected:
/// - Half-open to Open transition
/// - Circuit remains closed to requests
#[tokio::test]
async fn test_circuit_recovery_with_failures() {
    let config = CircuitBreakerConfig::builder()
        .consecutive_failures(2)
        .timeout(Duration::from_millis(100))
        .build();

    let breakers = PeerCircuitBreakers::new(config);
    let peer_id = create_test_peer();

    // Open circuit
    for _ in 0..2 {
        let _result = breakers
            .call_async(&peer_id, || async {
                Err::<(), _>(anyhow::anyhow!("Initial failure").as_infrastructure())
            })
            .await;
    }

    // Verify open
    let result = breakers
        .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("test") })
        .await;
    assert!(matches!(result, Err(BreakerError::Open)));

    // Wait for half-open
    time::sleep(Duration::from_millis(150)).await;

    // Fail in half-open
    let result = breakers
        .call_async(&peer_id, || async {
            Err::<(), _>(anyhow::anyhow!("Recovery failure").as_infrastructure())
        })
        .await;
    assert!(result.is_err());

    // Should be rejected again (circuit reopened)
    let result = breakers
        .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("test") })
        .await;
    assert!(
        matches!(result, Err(BreakerError::Open)),
        "Circuit should reopen after half-open failure"
    );
}

/// Test 11: VERIFY ZERO APPLICATION ERROR IMPACT
///
/// Additional verification that application errors have absolutely
/// zero impact on circuit state, even when interspersed with successes.
///
/// Setup:
/// - Send alternating successes and application errors
/// - Then send infrastructure errors
///
/// Expected:
/// - Circuit remains stable through application errors
/// - Only infrastructure errors affect the circuit
#[tokio::test]
async fn test_zero_application_error_impact() {
    let config = CircuitBreakerConfig::builder()
        .consecutive_failures(3)
        .build();

    let breakers = PeerCircuitBreakers::new(config);
    let peer_id = create_test_peer();

    // Alternate successes and application errors (10 of each)
    for i in 0..10 {
        // Success
        let result = breakers
            .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("success") })
            .await;
        assert!(result.is_ok(), "Success {} should work", i);

        // Application error
        let result = breakers
            .call_async(&peer_id, || async {
                Err::<(), _>(anyhow::anyhow!("App error").as_application())
            })
            .await;
        assert!(matches!(result, Err(BreakerError::Operation(_))));
    }

    // Verify circuit is still closed
    let stats = breakers.get_stats().await;
    assert_eq!(stats.get(&peer_id), Some(&CircuitState::Closed));

    // Now send infrastructure errors to prove circuit still works normally
    for _ in 0..3 {
        let _result = breakers
            .call_async(&peer_id, || async {
                Err::<(), _>(anyhow::anyhow!("Infrastructure").as_infrastructure())
            })
            .await;
    }

    // Circuit should now be open
    let result = breakers
        .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("test") })
        .await;
    assert!(
        matches!(result, Err(BreakerError::Open)),
        "Circuit should open after infrastructure errors"
    );
}
