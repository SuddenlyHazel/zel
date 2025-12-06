use std::sync::atomic::{AtomicU64, Ordering};

/// Metrics for tracking retry behavior
///
/// This structure tracks statistics about retry attempts for observability.
/// All operations are atomic and thread-safe.
///
/// # Example
/// ```rust
/// use zel_core::protocol::RetryMetrics;
///
/// let metrics = RetryMetrics::new();
/// assert_eq!(metrics.get_total_retries(), 0);
/// ```
#[derive(Debug)]
pub struct RetryMetrics {
    total_retries: AtomicU64,
    successful_after_retry: AtomicU64,
    failed_after_max_retries: AtomicU64,
}

impl RetryMetrics {
    /// Create a new metrics instance with all counters at zero
    pub fn new() -> Self {
        Self {
            total_retries: AtomicU64::new(0),
            successful_after_retry: AtomicU64::new(0),
            failed_after_max_retries: AtomicU64::new(0),
        }
    }

    /// Record that a retry attempt was made
    pub(crate) fn increment_total_retries(&self) {
        self.total_retries.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that a request succeeded after retrying
    pub(crate) fn record_success_after_retry(&self) {
        self.successful_after_retry.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that a request failed after exhausting max retries
    pub(crate) fn record_max_retries_exceeded(&self) {
        self.failed_after_max_retries
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Get total number of retry attempts made
    pub fn get_total_retries(&self) -> u64 {
        self.total_retries.load(Ordering::Relaxed)
    }

    /// Get number of requests that succeeded after retrying
    pub fn get_successful_after_retry(&self) -> u64 {
        self.successful_after_retry.load(Ordering::Relaxed)
    }

    /// Get number of requests that failed after max retries
    pub fn get_failed_after_max_retries(&self) -> u64 {
        self.failed_after_max_retries.load(Ordering::Relaxed)
    }

    /// Reset all counters to zero
    ///
    /// Useful for testing or periodic metric snapshots
    pub fn reset(&self) {
        self.total_retries.store(0, Ordering::Relaxed);
        self.successful_after_retry.store(0, Ordering::Relaxed);
        self.failed_after_max_retries.store(0, Ordering::Relaxed);
    }
}

impl Default for RetryMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_metrics() {
        let metrics = RetryMetrics::new();
        assert_eq!(metrics.get_total_retries(), 0);
        assert_eq!(metrics.get_successful_after_retry(), 0);
        assert_eq!(metrics.get_failed_after_max_retries(), 0);
    }

    #[test]
    fn test_increment_total_retries() {
        let metrics = RetryMetrics::new();
        metrics.increment_total_retries();
        metrics.increment_total_retries();
        assert_eq!(metrics.get_total_retries(), 2);
    }

    #[test]
    fn test_record_success() {
        let metrics = RetryMetrics::new();
        metrics.record_success_after_retry();
        assert_eq!(metrics.get_successful_after_retry(), 1);
    }

    #[test]
    fn test_record_max_retries_exceeded() {
        let metrics = RetryMetrics::new();
        metrics.record_max_retries_exceeded();
        assert_eq!(metrics.get_failed_after_max_retries(), 1);
    }

    #[test]
    fn test_reset() {
        let metrics = RetryMetrics::new();
        metrics.increment_total_retries();
        metrics.record_success_after_retry();
        metrics.record_max_retries_exceeded();

        metrics.reset();

        assert_eq!(metrics.get_total_retries(), 0);
        assert_eq!(metrics.get_successful_after_retry(), 0);
        assert_eq!(metrics.get_failed_after_max_retries(), 0);
    }
}
