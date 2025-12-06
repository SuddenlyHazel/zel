use crate::protocol::ClientError;
use std::sync::Arc;
use std::time::Duration;

/// Configuration validation error for retry settings.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("max_attempts must be greater than 0, got {0}")]
    InvalidMaxAttempts(u32),

    #[error("initial_backoff ({initial:?}) must be less than max_backoff ({max:?})")]
    InvalidBackoffRange { initial: Duration, max: Duration },

    #[error("backoff_multiplier must be greater than 1.0, got {0}")]
    InvalidMultiplier(f32),
}

/// Default maximum number of retry attempts.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Default initial backoff duration for exponential backoff (in milliseconds).
pub const DEFAULT_INITIAL_BACKOFF_MS: u64 = 100;

/// Default maximum backoff duration for exponential backoff (in seconds).
pub const DEFAULT_MAX_BACKOFF_SECS: u64 = 30;

/// Default backoff multiplier for exponential backoff.
///
/// Formula: `delay_attempt_n = initial_backoff * (multiplier ^ (n - 1))`
///
/// Example with multiplier=2.0, initial=100ms:
/// - Attempt 1: 100ms
/// - Attempt 2: 200ms (100 * 2^1)
/// - Attempt 3: 400ms (100 * 2^2)
pub const DEFAULT_BACKOFF_MULTIPLIER: f32 = 2.0;

/// Configuration for client-side retry behavior
///
/// # Best Practices
///
/// ## Retry Configuration Guidelines
///
/// ### Start with Defaults
///
/// The default configuration is based on industry standards and works well for most scenarios:
///
/// ```rust
/// use zel_core::protocol::RetryConfig;
///
/// // Recommended starting point
/// let config = RetryConfig::default();
/// ```
///
/// ### Maximum Attempts
///
/// - **2-3 attempts**: Good for user-facing operations (balance responsiveness vs reliability)
/// - **3-5 attempts**: Good for background jobs (more tolerance for transient failures)
/// - **Avoid >5 attempts**: Diminishing returns and potential for excessive delays
///
/// ```rust
/// use zel_core::protocol::RetryConfig;
///
/// // User-facing API call
/// let config = RetryConfig::builder()
///     .max_attempts(3)  // Quick failure for better UX
///     .build()
///     .expect("Valid config");
///
/// // Background processing
/// let config = RetryConfig::builder()
///     .max_attempts(5)  // More tolerance for eventual success
///     .build()
///     .expect("Valid config");
/// ```
///
/// ### Backoff Configuration
///
/// - **Initial backoff**: 100-500ms for most cases
/// - **Max backoff**: 30-60s to prevent indefinite waiting
/// - **Multiplier**: 2.0 (standard exponential backoff)
/// - **Jitter**: Always enabled to prevent thundering herd
///
/// ```rust
/// use zel_core::protocol::RetryConfig;
/// use std::time::Duration;
///
/// let config = RetryConfig::builder()
///     .initial_backoff(Duration::from_millis(100))
///     .max_backoff(Duration::from_secs(30))
///     .backoff_multiplier(2.0)
///     .with_jitter()  // Recommended!
///     .build()
///     .expect("Valid config");
/// ```
///
/// ## What to Retry vs Not Retry
///
/// ### DO Retry These Errors
///
/// Transient errors that are likely to resolve:
///
/// - Network connection failures
/// - Temporary timeouts
/// - "Connection reset by peer"
/// - DNS resolution failures
/// - Service temporarily unavailable (503)
///
/// ```rust
/// use zel_core::protocol::RetryConfig;
///
/// // Default: Only retry network errors
/// let config = RetryConfig::builder()
///     .retry_network_errors_only()
///     .build()
///     .expect("Valid config");
/// ```
///
/// ### DON'T Retry These Errors
///
/// Errors that won't resolve with retries:
///
/// - Authentication failures (401)
/// - Permission denied (403)
/// - Not found (404)
/// - Validation errors (400)
/// - Rate limiting (429) - use backoff instead
/// - Application/business logic errors
///
/// ```rust
/// use zel_core::protocol::{RetryConfig, ClientError};
///
/// // Custom predicate to avoid retrying auth errors
/// let config = RetryConfig::builder()
///     .with_custom_predicate(|error| {
///         match error {
///             ClientError::Connection(_) => true,
///             ClientError::Io(_) => true,
///             _ => false,
///         }
///     })
///     .build()
///     .expect("Valid config");
/// ```
///
/// ## Production Recommendations
///
/// ### Combine Retries with Circuit Breakers
///
/// For server-to-server communication:
///
/// - **Client side**: Use retries for transient failures
/// - **Server side**: Use circuit breakers to protect from cascading failures
/// - **Together**: Provides comprehensive resilience
///
/// ### Monitor Retry Metrics
///
/// Track these key metrics:
///
/// - **Total retries**: High count may indicate systemic issues
/// - **Success after retry**: Shows retry effectiveness
/// - **Max retries exceeded**: Indicates persistent failures
///
/// ```rust,no_run
/// use zel_core::protocol::{RpcClient, RetryMetrics};
/// use std::sync::Arc;
///
/// # async fn example() -> anyhow::Result<()> {
/// let metrics = Arc::new(RetryMetrics::new());
///
/// // Use metrics in client
/// // ... later, check metrics
/// println!("Total retries: {}", metrics.get_total_retries());
/// println!("Successful after retry: {}", metrics.get_successful_after_retry());
/// println!("Max retries exceeded: {}", metrics.get_failed_after_max_retries());
/// # Ok(())
/// # }
/// ```
///
/// ### Alert on High Retry Rates
///
/// Set up alerts for:
///
/// - Retry rate >20% of total requests
/// - Max retries exceeded >5% of total requests
/// - Sudden increases in retry counts
///
/// These indicate service degradation and need investigation.
///
/// ### Use Jitter in Production
///
/// Always enable jitter to prevent thundering herd when multiple clients retry simultaneously:
///
/// ```rust
/// use zel_core::protocol::RetryConfig;
///
/// // RECOMMENDED for production
/// let config = RetryConfig::builder()
///     .with_jitter()  // Prevents synchronized retries
///     .build()
///     .expect("Valid config");
/// ```
///
/// ### Consider Context When Configuring
///
/// Different scenarios need different configurations:
///
/// | Scenario | Max Attempts | Initial Backoff | Max Backoff |
/// |----------|--------------|-----------------|-------------|
/// | User-facing API | 2-3 | 100ms | 5s |
/// | Background job | 5 | 500ms | 60s |
/// | Critical operation | 3 | 100ms | 30s |
/// | Batch processing | 5 | 1s | 120s |
///
///
/// # Example
/// ```rust
/// use zel_core::protocol::RetryConfig;
/// use std::time::Duration;
///
/// let config = RetryConfig::builder()
///     .max_attempts(3)
///     .initial_backoff(Duration::from_millis(100))
///     .with_jitter()
///     .build()
///     .expect("Valid retry config");
/// ```
#[derive(Clone, Debug)]
pub struct RetryConfig {
    pub(crate) max_attempts: u32,
    pub(crate) initial_backoff: Duration,
    pub(crate) max_backoff: Duration,
    /// Backoff multiplier for exponential backoff.
    ///
    /// Type: f32 (matches backon library's ExponentialBuilder::with_factor)
    /// Default: 2.0
    ///
    /// Each retry waits approximately `previous_delay * backoff_multiplier`.
    pub(crate) backoff_multiplier: f32,
    pub(crate) jitter: bool,
    pub(crate) retryable_errors: ErrorPredicate,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            initial_backoff: Duration::from_millis(DEFAULT_INITIAL_BACKOFF_MS),
            max_backoff: Duration::from_secs(DEFAULT_MAX_BACKOFF_SECS),
            backoff_multiplier: DEFAULT_BACKOFF_MULTIPLIER,
            jitter: true,
            retryable_errors: ErrorPredicate::NetworkOnly,
        }
    }
}

impl RetryConfig {
    /// Create a new builder for configuring retry behavior
    pub fn builder() -> RetryConfigBuilder {
        RetryConfigBuilder::default()
    }

    /// Check if an error is retryable based on the configured predicate
    pub(crate) fn is_retryable(&self, error: &ClientError) -> bool {
        match &self.retryable_errors {
            ErrorPredicate::NetworkOnly => {
                matches!(error, ClientError::Connection(_) | ClientError::Io(_))
            }
            ErrorPredicate::NetworkAndTimeout => matches!(
                error,
                ClientError::Connection(_) | ClientError::Io(_) | ClientError::Protocol(_)
            ),
            ErrorPredicate::Custom(predicate) => predicate(error),
        }
    }
}

/// Determines which errors should trigger a retry
#[derive(Clone)]
pub enum ErrorPredicate {
    /// Only retry network/connection errors (default)
    ///
    /// Retries on:
    /// - [`ClientError::Connection`]
    /// - [`ClientError::Io`]
    NetworkOnly,

    /// Retry network and timeout/protocol errors
    ///
    /// Retries on:
    /// - [`ClientError::Connection`]
    /// - [`ClientError::Io`]
    /// - [`ClientError::Protocol`]
    NetworkAndTimeout,

    /// Custom predicate function for fine-grained control
    ///
    /// # Example
    /// ```rust
    /// use zel_core::protocol::{RetryConfig, ErrorPredicate, ClientError};
    ///
    /// let config = RetryConfig::builder()
    ///     .with_custom_predicate(|error| {
    ///         matches!(error, ClientError::Connection(_))
    ///     })
    ///     .build();
    /// ```
    Custom(Arc<dyn Fn(&ClientError) -> bool + Send + Sync>),
}

impl std::fmt::Debug for ErrorPredicate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NetworkOnly => write!(f, "NetworkOnly"),
            Self::NetworkAndTimeout => write!(f, "NetworkAndTimeout"),
            Self::Custom(_) => write!(f, "Custom(<fn>)"),
        }
    }
}

/// Builder for constructing [`RetryConfig`]
///
/// # Example
/// ```rust
/// use zel_core::protocol::RetryConfig;
/// use std::time::Duration;
///
/// let config = RetryConfig::builder()
///     .max_attempts(5)
///     .initial_backoff(Duration::from_millis(50))
///     .max_backoff(Duration::from_secs(10))
///     .backoff_multiplier(3.0)
///     .with_jitter()
///     .retry_network_errors_only()
///     .build()
///     .expect("Valid retry config");
/// ```
pub struct RetryConfigBuilder {
    config: RetryConfig,
}

impl Default for RetryConfigBuilder {
    fn default() -> Self {
        Self {
            config: RetryConfig::default(),
        }
    }
}

impl RetryConfigBuilder {
    /// Set maximum number of retry attempts
    ///
    /// Default: 3
    pub fn max_attempts(mut self, attempts: u32) -> Self {
        self.config.max_attempts = attempts;
        self
    }

    /// Set initial backoff duration
    ///
    /// Default: 100ms
    pub fn initial_backoff(mut self, duration: Duration) -> Self {
        self.config.initial_backoff = duration;
        self
    }

    /// Set maximum backoff duration
    ///
    /// Default: 30s
    pub fn max_backoff(mut self, duration: Duration) -> Self {
        self.config.max_backoff = duration;
        self
    }

    /// Set backoff multiplier for exponential backoff
    ///
    /// Default: 2.0 (doubles each retry)
    pub fn backoff_multiplier(mut self, multiplier: f32) -> Self {
        self.config.backoff_multiplier = multiplier;
        self
    }

    /// Enable jitter to randomize backoff delays
    ///
    /// This helps prevent thundering herd problems when multiple
    /// clients retry simultaneously.
    ///
    /// Default: enabled
    pub fn with_jitter(mut self) -> Self {
        self.config.jitter = true;
        self
    }

    /// Disable jitter for deterministic backoff
    pub fn without_jitter(mut self) -> Self {
        self.config.jitter = false;
        self
    }

    /// Only retry network/connection errors (default)
    pub fn retry_network_errors_only(mut self) -> Self {
        self.config.retryable_errors = ErrorPredicate::NetworkOnly;
        self
    }

    /// Retry network and timeout/protocol errors
    pub fn retry_network_and_timeout(mut self) -> Self {
        self.config.retryable_errors = ErrorPredicate::NetworkAndTimeout;
        self
    }

    /// Use a custom predicate to determine which errors are retryable
    ///
    /// # Example
    /// ```rust
    /// use zel_core::protocol::{RetryConfig, ClientError};
    ///
    /// let config = RetryConfig::builder()
    ///     .with_custom_predicate(|error| {
    ///         // Only retry connection errors, not I/O errors
    ///         matches!(error, ClientError::Connection(_))
    ///     })
    ///     .build();
    /// ```
    pub fn with_custom_predicate<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&ClientError) -> bool + Send + Sync + 'static,
    {
        self.config.retryable_errors = ErrorPredicate::Custom(Arc::new(predicate));
        self
    }

    /// Build the retry configuration with validation.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError` if the configuration is invalid:
    /// - `max_attempts` must be > 0
    /// - `initial_backoff` must be < `max_backoff`
    /// - `backoff_multiplier` must be > 1.0
    pub fn build(self) -> Result<RetryConfig, ConfigError> {
        // Validate max_attempts
        if self.config.max_attempts == 0 {
            return Err(ConfigError::InvalidMaxAttempts(0));
        }

        // Validate backoff range
        if self.config.initial_backoff >= self.config.max_backoff {
            return Err(ConfigError::InvalidBackoffRange {
                initial: self.config.initial_backoff,
                max: self.config.max_backoff,
            });
        }

        // Validate multiplier
        if self.config.backoff_multiplier <= 1.0 {
            return Err(ConfigError::InvalidMultiplier(
                self.config.backoff_multiplier,
            ));
        }

        Ok(self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RetryConfig::default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.initial_backoff, Duration::from_millis(100));
        assert_eq!(config.max_backoff, Duration::from_secs(30));
        assert_eq!(config.backoff_multiplier, 2.0);
        assert!(config.jitter);
    }

    #[test]
    fn test_builder() {
        let config = RetryConfig::builder()
            .max_attempts(5)
            .initial_backoff(Duration::from_millis(50))
            .max_backoff(Duration::from_secs(10))
            .backoff_multiplier(3.0)
            .without_jitter()
            .build()
            .expect("Valid retry config");

        assert_eq!(config.max_attempts, 5);
        assert_eq!(config.initial_backoff, Duration::from_millis(50));
        assert_eq!(config.max_backoff, Duration::from_secs(10));
        assert_eq!(config.backoff_multiplier, 3.0);
        assert!(!config.jitter);
    }

    #[test]
    fn test_error_predicate_network_only() {
        let config = RetryConfig::builder()
            .retry_network_errors_only()
            .build()
            .expect("Valid retry config");

        assert!(config.is_retryable(&ClientError::Connection("test".into())));
        assert!(config.is_retryable(&ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "test"
        ))));
        assert!(!config.is_retryable(&ClientError::Protocol("test".into())));
    }

    #[test]
    fn test_error_predicate_network_and_timeout() {
        let config = RetryConfig::builder()
            .retry_network_and_timeout()
            .build()
            .expect("Valid retry config");

        assert!(config.is_retryable(&ClientError::Connection("test".into())));
        assert!(config.is_retryable(&ClientError::Protocol("test".into())));
    }

    #[test]
    fn test_custom_predicate() {
        let config = RetryConfig::builder()
            .with_custom_predicate(|error| matches!(error, ClientError::Connection(_)))
            .build()
            .expect("Valid retry config");

        assert!(config.is_retryable(&ClientError::Connection("test".into())));
        assert!(!config.is_retryable(&ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "test"
        ))));
    }

    #[test]
    fn test_zero_max_attempts_fails() {
        let result = RetryConfig::builder().max_attempts(0).build();

        assert!(matches!(result, Err(ConfigError::InvalidMaxAttempts(0))));
    }

    #[test]
    fn test_invalid_backoff_range_fails() {
        let result = RetryConfig::builder()
            .initial_backoff(Duration::from_secs(10))
            .max_backoff(Duration::from_secs(5)) // max < initial
            .build();

        assert!(matches!(
            result,
            Err(ConfigError::InvalidBackoffRange { .. })
        ));
    }

    #[test]
    fn test_invalid_multiplier_fails() {
        let result = RetryConfig::builder()
            .backoff_multiplier(0.5) // ≤ 1.0
            .build();

        assert!(matches!(result, Err(ConfigError::InvalidMultiplier(_))));
    }

    #[test]
    fn test_valid_config_succeeds() {
        let result = RetryConfig::builder()
            .max_attempts(3)
            .initial_backoff(Duration::from_millis(100))
            .max_backoff(Duration::from_secs(30))
            .backoff_multiplier(2.0)
            .build();

        assert!(result.is_ok());
    }
}
