//! Per-Peer Circuit Breaker Protection
//!
//! This module implements per-peer circuit breakers for protecting servers from cascading
//! failures and misbehaving clients. Each peer gets its own isolated circuit breaker,
//! ensuring failures from one peer don't affect others.
//!
//! **Note on State Observation:** The underlying `circuitbreaker-rs` library implements
//! the full Closed → Open → Half-Open → Closed state machine and exposes it via
//! `CircuitBreaker::current_state()`. This wrapper surfaces those states through
//! [`PeerCircuitBreakers::get_stats()`](zel_core/src/protocol/circuit_breaker.rs:340),
//! which reports whether each peer's circuit is **Closed**, **Open**, or **HalfOpen**.
//!
//! # Circuit Breaker States
//!
//! A circuit breaker can be in one of three states:
//!
//! ## 1. Closed (Normal Operation)
//!
//! - **Behavior**: All requests are allowed through
//! - **Monitoring**: Tracks failure rate and consecutive failures
//! - **Transition**: Opens when failure threshold is exceeded
//!
//! ```text
//! [Closed] --failures exceed threshold--> [Open]
//! ```
//!
//! ## 2. Open (Failing Fast)
//!
//! - **Behavior**: All requests are immediately rejected without attempting operation
//! - **Purpose**: Prevents cascading failures and gives the system time to recover
//! - **Duration**: Remains open for the configured timeout period
//! - **Transition**: After timeout, moves to Half-Open to test recovery
//!
//! ```text
//! [Open] --timeout expires--> [Half-Open]
//! ```
//!
//! ## 3. Half-Open (Testing Recovery)
//!
//! - **Behavior**: Limited number of test requests are allowed through
//! - **Purpose**: Verify if the system has recovered
//! - **Transition**:
//!   - If test requests succeed → Closed (recovered)
//!   - If test requests fail → Open (still failing)
//!
//! ```text
//! [Half-Open] --consecutive successes--> [Closed]
//! [Half-Open] --any failure--> [Open]
//! ```
//!
//! # Complete State Diagram
//!
//! ```text
//!                    ┌─────────┐
//!          ┌────────>│  Closed │<────────┐
//!          │         └─────────┘         │
//!          │              │              │
//!          │     failures │              │ consecutive
//!          │     exceed   │              │ successes
//!          │     threshold│              │
//!          │              v              │
//!     any  │         ┌─────────┐         │
//!     failure        │  Open   │         │
//!          │         └─────────┘         │
//!          │              │              │
//!          │    timeout   │              │
//!          │    expires   │              │
//!          │              v              │
//!          │         ┌──────────┐        │
//!          └─────────┤Half-Open │────────┘
//!                    └──────────┘
//! ```
//!
//! # Configuration Guidelines
//!
//! ## Consecutive Failures (Default: 5)
//!
//! Number of consecutive failures before opening the circuit.
//!
//! - **Too low (1-2)**: Circuit may trip on transient errors, causing false positives
//! - **Recommended (3-5)**: Balances sensitivity with stability; use these as starting
//!   points and adjust based on observed metrics for your workload
//! - **Too high (>10)**: May allow too many failures before protection kicks in
//!
//! ```rust
//! use zel_core::protocol::CircuitBreakerConfig;
//!
//! let config = CircuitBreakerConfig::builder()
//!     .consecutive_failures(5)  // Recommended default
//!     .build();
//! ```
//!
//! ## Success Threshold (Default: 2)
//!
//! Number of consecutive successes needed in half-open state to close the circuit.
//!
//! - **Too low (1)**: May close prematurely on lucky single success
//! - **Recommended (2-3)**: Provides confidence without excessive testing; treat these
//!   as initial values and refine them with production metrics
//! - **Too high (>5)**: Delays recovery unnecessarily
//!
//! ```rust
//! use zel_core::protocol::CircuitBreakerConfig;
//!
//! let config = CircuitBreakerConfig::builder()
//!     .consecutive_successes(2)  // Recommended default
//!     .build();
//! ```
//!
//! ## Timeout Duration (Default: 30 seconds)
//!
//! How long the circuit stays open before attempting recovery.
//!
//! - **Too short (<10s)**: May cause circuit flapping
//! - **Recommended (30-60s)**: Allows time for system recovery; start here and
//!   adjust up or down based on how quickly your dependencies typically recover
//! - **Too long (>5min)**: Delays service restoration unnecessarily
//!
//! ```rust
//! use zel_core::protocol::CircuitBreakerConfig;
//! use std::time::Duration;
//!
//! let config = CircuitBreakerConfig::builder()
//!     .timeout(Duration::from_secs(30))  // Recommended default
//!     .build();
//! ```
//!
//! # Best Practices for Production
//!
//! ## DO: Use Default Configuration Initially
//!
//! The defaults follow common circuit-breaker patterns and align with the
//! example configuration used in `circuitbreaker-rs`'s documentation.
//! They are solid starting points, but you should tune them based on metrics for
//! your specific workload:
//!
//! ```rust
//! use zel_core::protocol::CircuitBreakerConfig;
//!
//! // Start with defaults
//! let config = CircuitBreakerConfig::default();
//! ```
//!
//! ## DO: Classify Errors Correctly
//!
//! Only infrastructure and system failures should trip the circuit:
//!
//! ```rust
//! use anyhow::anyhow;
//! use zel_core::protocol::ErrorClassificationExt;
//!
//! // CORRECT: Mark database failure as system failure
//! let error = anyhow!("Database timeout").as_system_failure();
//!
//! // CORRECT: Application errors don't trip circuit
//! let error = anyhow!("User not found").as_application_error();
//! ```
//!
//! ## DO: Monitor Circuit Breaker State
//!
//! Track circuit state for observability and alerting:
//!
//! ```rust,no_run
//! use zel_core::protocol::{CircuitBreakerConfig, PeerCircuitBreakers};
//!
//! # async fn example() {
//! let breakers = PeerCircuitBreakers::new(CircuitBreakerConfig::default());
//!
//! // Check circuit states periodically
//! let states = breakers.get_stats().await;
//! for (peer_id, state) in states {
//!     println!("Peer {:?}: {:?}", peer_id, state);
//! }
//! # }
//! ```
//!
//! ## DO: Tune Based on Metrics
//!
//! Adjust configuration based on observed behavior:
//!
//! - If circuits trip too often → Increase consecutive_failures
//! - If recovery is too slow → Decrease timeout
//! - If false positives occur → Review error classification
//!
//! ## DON'T: Set Thresholds Too Low
//!
//! ```rust,no_run
//! use zel_core::protocol::CircuitBreakerConfig;
//!
//! // WRONG: Too sensitive, will trip on transient errors
//! let config = CircuitBreakerConfig::builder()
//!     .consecutive_failures(1)  // Too low!
//!     .build();
//! ```
//!
//! ## DON'T: Mark Application Errors as System Failures
//!
//! ```rust,no_run
//! use anyhow::anyhow;
//! use zel_core::protocol::ErrorClassificationExt;
//!
//! // WRONG: This will trip the circuit unnecessarily
//! let error = anyhow!("Validation failed").as_system_failure();
//! ```
//!
//! ## DON'T: Ignore Circuit State in Monitoring
//!
//! Always track and alert on circuit breaker state transitions. Multiple circuits
//! being open simultaneously may indicate systemic problems.
//!
//! ## DON'T: Use Same Configuration for All Services
//!
//! Different services may need different thresholds:
//!
//! - **Critical path services**: Lower thresholds (fail fast)
//! - **Background tasks**: Higher thresholds (more tolerant)
//! - **External APIs**: Longer timeouts (may be temporarily unavailable)

use circuitbreaker_rs::{BreakerError, CircuitBreaker, DefaultPolicy};
use iroh::PublicKey;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::protocol::error_classification::{ErrorClassifier, ErrorSeverity as ErrorSeverityType};
use serde::{Deserialize, Serialize};

/// Per-peer circuit breakers for protecting server from misbehaving clients
///
/// This wraps `circuitbreaker-rs` to provide per-peer isolation,
/// so failures from one peer don't affect others.
///
/// # Example
/// ```rust,no_run
/// use zel_core::protocol::{CircuitBreakerConfig, PeerCircuitBreakers};
/// use std::time::Duration;
///
/// let config = CircuitBreakerConfig::builder()
///     .consecutive_failures(5)
///     .timeout(Duration::from_secs(30))
///     .build();
///
/// let breakers = PeerCircuitBreakers::new(config);
/// ```
pub struct PeerCircuitBreakers {
    breakers: Arc<RwLock<HashMap<PublicKey, CircuitBreaker<DefaultPolicy, ServiceError>>>>,
    config: CircuitBreakerConfig,
    /// Error classification strategy used to decide which failures trip the circuit
    classifier: ErrorClassifier,
}

impl PeerCircuitBreakers {
    /// Create a new per-peer circuit breaker manager
    pub fn new(config: CircuitBreakerConfig) -> Self {
        let classifier = config.error_classifier.clone();
        Self {
            breakers: Arc::new(RwLock::new(HashMap::new())),
            config,
            classifier,
        }
    }

    /// Execute an async operation with circuit breaker protection for a specific peer
    ///
    /// Uses a double-Result wrapper pattern to filter errors by severity:
    /// - Application errors (ErrorSeverity::Application) bypass circuit breaker tracking
    /// - Infrastructure/System errors trip the circuit breaker
    ///
    /// Returns `Ok(T)` if operation succeeds, or an error if the circuit is open
    /// or the operation fails.
    pub async fn call_async<F, Fut, T>(
        &self,
        peer_id: &PublicKey,
        operation: F,
    ) -> Result<T, BreakerError<ServiceError>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, anyhow::Error>>,
    {
        let breaker = {
            let mut breakers = self.breakers.write().await;
            breakers
                .entry(*peer_id)
                .or_insert_with(|| self.create_circuit_breaker())
                .clone()
        };

        //let classifier = self.classifier.clone();

        // Double-Result wrapper pattern:
        // - Ok(Ok(T)) = success
        // - Ok(Err(ServiceError)) = application error (bypasses circuit tracking)
        // - Err(ServiceError) = severe error (trips circuit)
        let _classifier = self.classifier.clone();
        let result = breaker
            .call_async(|| async {
                match operation().await {
                    Ok(value) => Ok(Ok(value)),
                    Err(e) => {
                        let severity = _classifier.classify(&e);
                        let message = e.to_string();
                        let service_error = ServiceError::from_severity_and_msg(severity, message);

                        match severity {
                            ErrorSeverityType::Application => {
                                // Application error - bypass circuit breaker tracking
                                Ok(Err(service_error))
                            }
                            ErrorSeverityType::Infrastructure
                            | ErrorSeverityType::SystemFailure => {
                                // Severe error - trip circuit breaker
                                Err(service_error)
                            }
                        }
                    }
                }
            })
            .await;

        // Unwrap the double-Result
        let result = match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(app_error)) => Err(BreakerError::Operation(app_error)),
            Err(severe_error) => Err(severe_error),
        };

        result
    }

    /// Get current circuit states for all peers (for observability)
    ///
    /// Returns the current state of each peer's circuit breaker by querying
    /// the underlying `circuitbreaker-rs` breakers via `current_state()`:
    /// - [`CircuitState::Closed`] when the breaker is allowing calls normally
    /// - [`CircuitState::Open`] when the breaker is rejecting calls
    /// - [`CircuitState::HalfOpen`] when the breaker is probing for recovery
    pub async fn get_stats(&self) -> HashMap<PublicKey, CircuitState> {
        let breakers = self.breakers.read().await;
        breakers
            .iter()
            .map(|(peer_id, breaker)| {
                let state = match breaker.current_state() {
                    circuitbreaker_rs::State::Closed => CircuitState::Closed,
                    circuitbreaker_rs::State::Open => CircuitState::Open,
                    circuitbreaker_rs::State::HalfOpen => CircuitState::HalfOpen,
                };
                (*peer_id, state)
            })
            .collect()
    }

    fn create_circuit_breaker(&self) -> CircuitBreaker<DefaultPolicy, ServiceError> {
        let mut builder = CircuitBreaker::<DefaultPolicy, ServiceError>::builder()
            .failure_threshold(self.config.failure_rate)
            .cooldown(self.config.timeout);

        if let Some(consecutive) = self.config.consecutive_failures {
            builder = builder.consecutive_failures(consecutive as u64);
        }

        if let Some(consecutive) = self.config.consecutive_successes {
            builder = builder.consecutive_successes(consecutive as u64);
        }

        if let Some(probes) = self.config.probe_interval {
            builder = builder.probe_interval(probes);
        }

        builder.build()
    }
}

/// Service errors with built-in severity classification
#[derive(thiserror::Error, Debug, Clone, Serialize, Deserialize)]
pub enum ServiceError {
    /// Application-level error (validation, business logic)
    #[error("Application error: {0}")]
    Application(String),

    /// Infrastructure error (network, timeout, I/O)
    #[error("Infrastructure error: {0}")]
    Infrastructure(String),

    /// System failure (database, critical service)
    #[error("System failure: {0}")]
    SystemFailure(String),
}

/// ServiceError helpers
impl ServiceError {
    pub fn severity(&self) -> ErrorSeverityType {
        match self {
            ServiceError::Application(_) => ErrorSeverityType::Application,
            ServiceError::Infrastructure(_) => ErrorSeverityType::Infrastructure,
            ServiceError::SystemFailure(_) => ErrorSeverityType::SystemFailure,
        }
    }

    /// Construct a ServiceError from a severity classification and message
    pub fn from_severity_and_msg(severity: ErrorSeverityType, message: String) -> Self {
        match severity {
            ErrorSeverityType::Application => ServiceError::Application(message),
            ErrorSeverityType::Infrastructure => ServiceError::Infrastructure(message),
            ErrorSeverityType::SystemFailure => ServiceError::SystemFailure(message),
        }
    }
}

/// Convenience conversion from `anyhow::Error` using the *default* [`ErrorClassifier`].
///
/// This helper always classifies with `ErrorClassifier::Default`, independent of any
/// [`CircuitBreakerConfig::error_classifier`](zel_core/src/protocol/circuit_breaker.rs:468)
/// or [`PeerCircuitBreakers`](zel_core/src/protocol/circuit_breaker.rs:240) instance.
///
/// Circuit breaker behavior (which errors contribute to tripping) is controlled by
/// `PeerCircuitBreakers::call_async`, which uses its configured classifier and
/// constructs a [`ServiceError`] via
/// [`ServiceError::from_severity_and_msg`](zel_core/src/protocol/circuit_breaker.rs:392).
/// Use this impl only when the default classification policy is sufficient.
impl From<anyhow::Error> for ServiceError {
    fn from(err: anyhow::Error) -> Self {
        let classifier = ErrorClassifier::default();
        let severity = classifier.classify(&err);
        let message = err.to_string();

        match severity {
            ErrorSeverityType::Application => ServiceError::Application(message),
            ErrorSeverityType::Infrastructure => ServiceError::Infrastructure(message),
            ErrorSeverityType::SystemFailure => ServiceError::SystemFailure(message),
        }
    }
}

/// Default number of consecutive failures before opening the circuit.
pub const DEFAULT_CONSECUTIVE_FAILURES: u32 = 5;

/// Default number of successful requests needed to close the circuit from half-open state.
pub const DEFAULT_SUCCESS_THRESHOLD: u32 = 2;

/// Default timeout before circuit transitions from open to half-open (in seconds).
pub const DEFAULT_CIRCUIT_TIMEOUT_SECS: u64 = 30;

/// Default probe interval for half-open state verification.
pub const DEFAULT_PROBE_INTERVAL: u32 = 3;

/// Configuration for circuit breaker behavior
#[derive(Clone, Debug)]
pub struct CircuitBreakerConfig {
    /// Failure rate threshold (0.0 to 1.0) before opening circuit
    pub(crate) failure_rate: f64,

    /// Optional: Number of consecutive failures before opening (overrides rate)
    pub(crate) consecutive_failures: Option<u32>,

    /// Number of consecutive successes in half-open state before closing
    pub(crate) consecutive_successes: Option<u32>,

    /// Duration to wait in open state before attempting half-open
    pub(crate) timeout: Duration,

    /// Number of test requests allowed in half-open state
    pub(crate) probe_interval: Option<u32>,

    /// Strategy for classifying which errors should trip the circuit
    pub(crate) error_classifier: ErrorClassifier,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_rate: 0.5, // 50% error rate
            consecutive_failures: Some(DEFAULT_CONSECUTIVE_FAILURES),
            consecutive_successes: Some(DEFAULT_SUCCESS_THRESHOLD),
            timeout: Duration::from_secs(DEFAULT_CIRCUIT_TIMEOUT_SECS),
            probe_interval: Some(DEFAULT_PROBE_INTERVAL),
            error_classifier: ErrorClassifier::default(),
        }
    }
}

impl CircuitBreakerConfig {
    /// Create a new builder for circuit breaker configuration
    pub fn builder() -> CircuitBreakerConfigBuilder {
        CircuitBreakerConfigBuilder::default()
    }
}

/// Builder for constructing [`CircuitBreakerConfig`]
pub struct CircuitBreakerConfigBuilder {
    config: CircuitBreakerConfig,
}

impl Default for CircuitBreakerConfigBuilder {
    fn default() -> Self {
        Self {
            config: CircuitBreakerConfig::default(),
        }
    }
}

impl CircuitBreakerConfigBuilder {
    /// Set the failure rate threshold (0.0 to 1.0)
    ///
    /// Circuit opens when error rate exceeds this threshold.
    /// Default: 0.5 (50%)
    pub fn failure_threshold(mut self, rate: f64) -> Self {
        self.config.failure_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// Set consecutive failures before opening (overrides rate threshold)
    ///
    /// Default: Some(5)
    pub fn consecutive_failures(mut self, count: u32) -> Self {
        self.config.consecutive_failures = Some(count);
        self
    }

    /// Set consecutive successes needed in half-open to close
    ///
    /// Default: Some(2)
    pub fn consecutive_successes(mut self, count: u32) -> Self {
        self.config.consecutive_successes = Some(count);
        self
    }

    /// Set the timeout duration before attempting half-open state
    ///
    /// Default: 30 seconds
    pub fn timeout(mut self, duration: Duration) -> Self {
        self.config.timeout = duration;
        self
    }

    /// Set number of test requests allowed in half-open state
    ///
    /// Default: Some(3)
    pub fn probe_interval(mut self, probes: u32) -> Self {
        self.config.probe_interval = Some(probes);
        self
    }

    /// Set a custom error classifier
    pub fn with_error_classifier(mut self, classifier: ErrorClassifier) -> Self {
        self.config.error_classifier = classifier;
        self
    }

    /// Build the circuit breaker configuration
    pub fn build(self) -> CircuitBreakerConfig {
        self.config
    }
}

/// Current state of a circuit breaker
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CircuitState {
    /// Circuit is closed, allowing all requests
    Closed,

    /// Circuit is open, rejecting all requests
    Open,

    /// Circuit is half-open, testing with limited requests
    HalfOpen,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ErrorClassificationExt;
    use anyhow::anyhow;

    #[tokio::test]
    async fn test_circuit_breaker_with_operation() {
        let config = CircuitBreakerConfig::builder()
            .consecutive_failures(3)
            .build();

        let breakers = PeerCircuitBreakers::new(config);
        let peer_id = iroh::SecretKey::generate(&mut rand::rng()).public();

        // Successful operation
        let result = breakers
            .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("success") })
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_circuit_opens_after_failures() {
        let config = CircuitBreakerConfig::builder()
            .consecutive_failures(2)
            .build();

        let breakers = PeerCircuitBreakers::new(config);
        let peer_id = iroh::SecretKey::generate(&mut rand::rng()).public();

        // Cause failures - these will execute but cause circuit to open
        for _ in 0..2 {
            let _: Result<(), _> = breakers
                .call_async(&peer_id, || async {
                    Err::<(), _>(anyhow!("System failure").as_system_failure())
                })
                .await;
        }

        // Next call should be rejected due to open circuit
        let result = breakers
            .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("should fail") })
            .await;
        assert!(matches!(result, Err(BreakerError::Open)));

        // State is updated on the rejection - verify it now
        let stats = breakers.get_stats().await;
        assert_eq!(stats.get(&peer_id), Some(&CircuitState::Open));
    }

    #[tokio::test]
    async fn test_per_peer_isolation() {
        let config = CircuitBreakerConfig::builder()
            .consecutive_failures(2)
            .build();

        let breakers = PeerCircuitBreakers::new(config);
        let peer1 = iroh::SecretKey::generate(&mut rand::rng()).public();
        let peer2 = iroh::SecretKey::generate(&mut rand::rng()).public();

        // Fail peer1 - these will execute but cause circuit to open
        for _ in 0..2 {
            let _: Result<(), _> = breakers
                .call_async(&peer1, || async {
                    Err::<(), _>(anyhow!("Failure").as_system_failure())
                })
                .await;
        }

        // Peer1 should be blocked on next call
        let result1 = breakers
            .call_async(&peer1, || async { Ok::<_, anyhow::Error>("test") })
            .await;
        assert!(matches!(result1, Err(BreakerError::Open)));

        // State is updated on the rejection - verify it now
        let stats = breakers.get_stats().await;
        assert_eq!(stats.get(&peer1), Some(&CircuitState::Open));

        // Peer2 should still work (not affected by peer1's failures)
        let result2 = breakers
            .call_async(&peer2, || async { Ok::<_, anyhow::Error>("success") })
            .await;
        assert!(result2.is_ok());
    }

    #[tokio::test]
    async fn test_application_errors_dont_trip_circuit() {
        let config = CircuitBreakerConfig::builder()
            .consecutive_failures(2)
            .build();

        let breakers = PeerCircuitBreakers::new(config);
        let peer_id = iroh::SecretKey::generate(&mut rand::rng()).public();

        // Send 5 application errors (more than threshold of 2)
        for _ in 0..5 {
            let result = breakers
                .call_async(&peer_id, || async {
                    Err::<(), _>(anyhow!("User not found").as_application())
                })
                .await;

            // Should get error back, but circuit should stay closed
            assert!(matches!(result, Err(BreakerError::Operation(_))));
        }

        // Verify circuit is still CLOSED (not tripped)
        // Try a successful operation - should work
        let result = breakers
            .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("success") })
            .await;
        assert!(result.is_ok());

        // Verify state is still Closed
        let stats = breakers.get_stats().await;
        assert_eq!(stats.get(&peer_id), Some(&CircuitState::Closed));
    }

    #[tokio::test]
    async fn test_infrastructure_errors_do_trip_circuit() {
        let config = CircuitBreakerConfig::builder()
            .consecutive_failures(2)
            .build();

        let breakers = PeerCircuitBreakers::new(config);
        let peer_id = iroh::SecretKey::generate(&mut rand::rng()).public();

        // Send 2 infrastructure errors (exactly the threshold)
        for _ in 0..2 {
            let _: Result<(), _> = breakers
                .call_async(&peer_id, || async {
                    Err::<(), _>(anyhow!("Network timeout").as_infrastructure())
                })
                .await;
        }

        // Next call should be rejected due to open circuit
        let result = breakers
            .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("should fail") })
            .await;
        assert!(matches!(result, Err(BreakerError::Open)));

        // Verify state is Open
        let stats = breakers.get_stats().await;
        assert_eq!(stats.get(&peer_id), Some(&CircuitState::Open));
    }

    #[tokio::test]
    async fn test_mixed_errors_only_severe_count() {
        let config = CircuitBreakerConfig::builder()
            .consecutive_failures(3)
            .build();

        let breakers = PeerCircuitBreakers::new(config);
        let peer_id = iroh::SecretKey::generate(&mut rand::rng()).public();

        // Important: circuitbreaker-rs resets consecutive failure counter on ANY success
        // (including Ok(Err(...)) from our double-Result pattern).
        // So we need to test that consecutive severe errors trip the circuit,
        // and that application errors don't contribute to that count.

        // Send 2 infrastructure errors consecutively
        for _ in 0..2 {
            let _: Result<(), _> = breakers
                .call_async(&peer_id, || async {
                    Err::<(), _>(anyhow!("Network error").as_infrastructure())
                })
                .await;
        }

        // Now send application error - this should NOT add to failure count
        // and also should NOT reset it (because it returns Ok(Err(...)))
        let result = breakers
            .call_async(&peer_id, || async {
                Err::<(), _>(anyhow!("User not found").as_application())
            })
            .await;
        // Should get operation error back
        assert!(matches!(result, Err(BreakerError::Operation(_))));

        // Circuit should still be closed after 2 severe errors + 1 app error
        // (we need 3 consecutive severe errors to trip)
        let result = breakers
            .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("test") })
            .await;
        assert!(result.is_ok());

        // Now verify that 3 consecutive severe errors DO trip the circuit
        for _ in 0..3 {
            let _: Result<(), _> = breakers
                .call_async(&peer_id, || async {
                    Err::<(), _>(anyhow!("System failure").as_system_failure())
                })
                .await;
        }

        // Next call should be rejected
        let result = breakers
            .call_async(&peer_id, || async { Ok::<_, anyhow::Error>("should fail") })
            .await;
        assert!(matches!(result, Err(BreakerError::Open)));
    }
}
