//! Per-Peer Circuit Breaker Protection
//!
//! This module implements per-peer circuit breakers for protecting servers from cascading
//! failures and misbehaving clients. Each peer gets its own isolated circuit breaker,
//! ensuring failures from one peer don't affect others.
//!
//! **Note on State Observation:** While the underlying `circuitbreaker-rs` library implements
//! the full Closed → Open → HalfOpen → Closed state machine internally, our wrapper can only
//! observe Closed and Open states through the library's API. The state diagrams below describe
//! the complete behavior of the underlying circuit breaker.
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
//!     failure       │  Open   │         │
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
//! - **Recommended (3-5)**: Balances sensitivity with stability
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
//! - **Recommended (2-3)**: Provides confidence without excessive testing
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
//! - **Recommended (30-60s)**: Allows time for system recovery
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
//! The defaults are based on industry standards and work well for most scenarios:
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
    /// Track observed circuit states based on operation results
    /// Since circuitbreaker-rs doesn't expose state, we infer it from results
    states: Arc<RwLock<HashMap<PublicKey, CircuitState>>>,
    config: CircuitBreakerConfig,
}

impl PeerCircuitBreakers {
    /// Create a new per-peer circuit breaker manager
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            breakers: Arc::new(RwLock::new(HashMap::new())),
            states: Arc::new(RwLock::new(HashMap::new())),
            config,
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

        // Double-Result wrapper pattern:
        // - Ok(Ok(T)) = success
        // - Ok(Err(ServiceError)) = application error (bypasses circuit tracking)
        // - Err(ServiceError) = severe error (trips circuit)
        let result = breaker
            .call_async(|| async {
                match operation().await {
                    Ok(value) => Ok(Ok(value)),
                    Err(e) => {
                        let service_error = ServiceError::from(e); // Auto-classifies!

                        match service_error.severity() {
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

        // Track state based on result
        let mut states = self.states.write().await;
        match &result {
            Ok(_) => {
                // Success - circuit is either Closed or transitioning from HalfOpen to Closed
                // We'll assume Closed for simplicity (could be HalfOpen but we can't tell)
                states.insert(*peer_id, CircuitState::Closed);
            }
            Err(BreakerError::Open) => {
                // Circuit rejected the request - it's Open
                states.insert(*peer_id, CircuitState::Open);
            }
            Err(BreakerError::Operation(_)) | Err(BreakerError::Internal(_)) => {
                // Operation failed but circuit is still allowing requests
                // We don't update state here - state will be updated when:
                // - Next request gets BreakerError::Open (circuit opened)
                // - Next request succeeds (circuit still closed/half-open)
                // This provides eventual consistency without interfering with the circuit's operation
            }
        }

        result
    }

    /// Get current circuit states for all peers (for observability)
    ///
    /// Returns states inferred from operation results. Since circuitbreaker-rs
    /// doesn't expose internal state, we track it by observing:
    /// - BreakerError::Open -> circuit is Open
    /// - Successful operations -> circuit is Closed
    /// - Operation errors -> state unchanged (could be HalfOpen testing)
    pub async fn get_stats(&self) -> HashMap<PublicKey, CircuitState> {
        self.states.read().await.clone()
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

impl ServiceError {
    pub fn severity(&self) -> ErrorSeverityType {
        match self {
            ServiceError::Application(_) => ErrorSeverityType::Application,
            ServiceError::Infrastructure(_) => ErrorSeverityType::Infrastructure,
            ServiceError::SystemFailure(_) => ErrorSeverityType::SystemFailure,
        }
    }
}

// Automatic conversion from anyhow::Error using error classification
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
///
/// # Rationale
///
/// We chose 5 consecutive failures as our default because it:
/// - Balances sensitivity with stability
/// - Provides enough signal to detect genuine service problems
/// - Avoids false positives from occasional transient errors
///
/// This is a common threshold in circuit breaker implementations, inspired by
/// patterns described in Martin Fowler's "CircuitBreaker" article.
pub const DEFAULT_CONSECUTIVE_FAILURES: u32 = 5;

/// Default number of successful requests needed to close the circuit from half-open state.
///
/// # Rationale
///
/// We chose 2 consecutive successes as our default because it:
/// - Prevents premature closure from a single lucky success
/// - Enables quick restoration once service has genuinely recovered
/// - Balances caution with responsiveness
///
/// This is conservative compared to some implementations that use a single probe,
/// inspired by circuit breaker patterns from "Release It!" by Michael Nygard.
pub const DEFAULT_SUCCESS_THRESHOLD: u32 = 2;

/// Default timeout before circuit transitions from open to half-open (in seconds).
///
/// # Rationale
///
/// We chose 30 seconds as our default because it:
/// - Provides sufficient time for most transient issues to resolve
/// - Balances quick recovery with avoiding circuit flapping
/// - Prevents excessive downtime while still protecting the system
///
/// This timeout is within the common range (10-60s) used by various resilience
/// libraries, though specific defaults vary by implementation.
pub const DEFAULT_CIRCUIT_TIMEOUT_SECS: u64 = 30;

/// Default probe interval for half-open state verification.
///
/// # Rationale
///
/// We chose 3 as our default probe interval because it:
/// - Provides multiple samples to reduce false positives
/// - Limits exposure during recovery testing
/// - Balances confidence in recovery with quick restoration
///
/// This is our engineering choice for the half-open state testing phase.
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
