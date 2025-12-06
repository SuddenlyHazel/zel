use iroh::PublicKey;
use std::collections::HashMap;
use std::sync::Arc;

use crate::protocol::circuit_breaker::{CircuitState, PeerCircuitBreakers};

/// Extension providing access to circuit breaker statistics for observability
///
/// This can be added to server extensions to expose circuit breaker metrics
/// for monitoring, dashboards, or health checks.
///
/// # Example
/// ```rust,no_run
/// use zel_core::protocol::{RpcServerBuilder, CircuitBreakerConfig, CircuitBreakerStats, PeerCircuitBreakers, Extensions};
/// use std::sync::Arc;
/// use std::time::Duration;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// # let endpoint = iroh::Endpoint::builder().bind().await?;
/// let cb_config = CircuitBreakerConfig::builder()
///     .consecutive_failures(5)
///     .build();
///
/// let breakers = Arc::new(PeerCircuitBreakers::new(cb_config.clone()));
/// let stats = CircuitBreakerStats::new(Arc::clone(&breakers));
///
/// let server = RpcServerBuilder::new(b"myapp/1", endpoint)
///     .with_circuit_breaker(cb_config)
///     .with_extensions(
///         Extensions::new().with(stats)
///     )
///     .build();
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct CircuitBreakerStats {
    breakers: Arc<PeerCircuitBreakers>,
}

impl CircuitBreakerStats {
    /// Create a new stats accessor
    pub fn new(breakers: Arc<PeerCircuitBreakers>) -> Self {
        Self { breakers }
    }

    /// Get current circuit states for all peers
    ///
    /// Returns a map of peer IDs to their current circuit breaker state
    pub async fn peer_states(&self) -> HashMap<PublicKey, CircuitState> {
        self.breakers.get_stats().await
    }

    /// Check if a specific peer's circuit is open
    ///
    /// Returns `true` if the circuit is open (rejecting requests)
    pub async fn is_open(&self, peer_id: &PublicKey) -> bool {
        let states = self.breakers.get_stats().await;
        matches!(states.get(peer_id), Some(CircuitState::Open))
    }

    /// Check if a specific peer's circuit is half-open
    ///
    /// Returns `true` if the circuit is half-open (testing recovery)
    pub async fn is_half_open(&self, peer_id: &PublicKey) -> bool {
        let states = self.breakers.get_stats().await;
        matches!(states.get(peer_id), Some(CircuitState::HalfOpen))
    }

    /// Get count of peers with open circuits
    pub async fn open_circuit_count(&self) -> usize {
        let states = self.breakers.get_stats().await;
        states
            .values()
            .filter(|state| matches!(state, CircuitState::Open))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{CircuitBreakerConfig, PeerCircuitBreakers};

    #[tokio::test]
    async fn test_circuit_breaker_stats() {
        let config = CircuitBreakerConfig::default();
        let breakers = Arc::new(PeerCircuitBreakers::new(config));
        let stats = CircuitBreakerStats::new(breakers);

        let states = stats.peer_states().await;
        assert_eq!(states.len(), 0); // No peers yet

        let count = stats.open_circuit_count().await;
        assert_eq!(count, 0);
    }
}
