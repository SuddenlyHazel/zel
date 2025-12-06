//! Error Classification for Circuit Breakers
//!
//! This module provides tools for classifying errors to control circuit breaker behavior.
//! Proper error classification is critical - only infrastructure and system failures should
//! trip the circuit breaker, while application/business logic errors should not.
//!
//! # Quick Guide
//!
//! **When to use each error type:**
//!
//! | Error Type | When to Use | Circuit Breaker Impact |
//! |------------|-------------|------------------------|
//! | **Infrastructure** | Network failures, I/O errors, timeouts | TRIPS circuit breaker |
//! | **SystemFailure** | Database down, dependency unavailable | TRIPS circuit breaker |
//! | **Application** | Validation errors, "not found", permissions | Does NOT trip circuit |
//!
//! # Usage Examples
//!
//! ```rust
//! use anyhow::anyhow;
//! use zel_core::protocol::ErrorClassificationExt;
//!
//! // Mark database error as system failure (trips circuit breaker)
//! let db_error = anyhow!("Database connection failed").as_system_failure();
//!
//! // Mark validation error as application error (won't trip circuit breaker)
//! let validation_error = anyhow!("Invalid input").as_application_error();
//!
//! // Infrastructure errors are detected automatically
//! let io_error: anyhow::Error = std::io::Error::new(
//!     std::io::ErrorKind::ConnectionRefused,
//!     "Connection refused"
//! ).into();
//! // This will be classified as Infrastructure automatically
//! ```
//!
//! # Common Pitfalls
//!
//! **DON'T mark validation errors as system failures:**
//! ```rust,no_run
//! use anyhow::anyhow;
//! use zel_core::protocol::ErrorClassificationExt;
//!
//! // WRONG: This will trip the circuit breaker unnecessarily
//! let error = anyhow!("User not found").as_system_failure();
//! ```
//!
//! **DO mark them as application errors:**
//! ```rust
//! use anyhow::anyhow;
//! use zel_core::protocol::ErrorClassificationExt;
//!
//! // CORRECT: Won't affect circuit breaker
//! let error = anyhow!("User not found").as_application_error();
//! ```
//!
//! **DON'T mark "not found" errors as system failures:**
//! ```rust,no_run
//! use anyhow::anyhow;
//! use zel_core::protocol::ErrorClassificationExt;
//!
//! // WRONG: Resource not found is not a system failure
//! let error = anyhow!("Resource not found").as_system_failure();
//! ```
//!
//! **DO use application error for business logic failures:**
//! ```rust
//! use anyhow::anyhow;
//! use zel_core::protocol::ErrorClassificationExt;
//!
//! // CORRECT: Business logic error
//! let error = anyhow!("Insufficient permissions").as_application_error();
//! ```
//!
//! **DO mark database errors as system failures:**
//! ```rust
//! use anyhow::anyhow;
//! use zel_core::protocol::ErrorClassificationExt;
//!
//! // CORRECT: Database unavailable is a system failure
//! let error = anyhow!("Database connection pool exhausted").as_system_failure();
//! ```
//!
//! **DO mark upstream service errors as system failures:**
//! ```rust
//! use anyhow::anyhow;
//! use zel_core::protocol::ErrorClassificationExt;
//!
//! // CORRECT: Upstream dependency failure
//! let error = anyhow!("Payment service timeout").as_system_failure();
//! ```
//!
//! # Circuit Breaker Behavior
//!
//! The classification system integrates with the circuit breaker to provide intelligent
//! failure handling:
//!
//! - **Infrastructure errors**: Automatically detected (I/O, network, timeout errors)
//!   - Trip the circuit breaker immediately
//!   - Indicate systemic problems that won't resolve quickly
//!
//! - **System Failures**: Explicitly marked by developers using `.as_system_failure()`
//!   - Trip the circuit breaker
//!   - Indicate critical dependencies are unavailable
//!   - Examples: database down, cache unavailable, upstream API failing
//!
//! - **Application errors**: Default classification for unmarked errors
//!   - Do NOT trip the circuit breaker
//!   - Represent normal business logic failures
//!   - Examples: validation errors, "not found", permission denied
//!
//! # Default Classification Rules
//!
//! The default classifier (`ErrorClassifier::Default`) uses the following logic:
//!
//! 1. **Check for explicit marking**: If error is marked with `.as_system_failure()`,
//!    classify as SystemFailure
//!
//! 2. **Check for infrastructure error types** (automatically detected):
//!    - `std::io::Error` → Infrastructure
//!    - `tokio::time::error::Elapsed` → Infrastructure
//!    - `iroh::endpoint::ConnectError` → Infrastructure
//!    - `iroh_quinn::ReadError` → Infrastructure
//!    - `iroh_quinn::WriteError` → Infrastructure
//!    - `iroh_quinn::ConnectionError` → Infrastructure
//!
//! 3. **Default to Application**: All other errors are classified as application errors
//!    and will NOT trip the circuit breaker
//!
//! This conservative default ensures the circuit breaker only trips for genuine
//! infrastructure problems, not business logic failures.

use anyhow::Error;
use std::sync::Arc;

/// Classification of error severity for circuit breaker decisions
///
/// This type is shared with `zel_types` so that both `zel_core` and
/// `zel_macros` use a single, canonical definition.
pub use zel_types::ErrorSeverity;

/// Strategy for classifying errors to determine circuit breaker behavior
#[derive(Clone)]
pub enum ErrorClassifier {
    /// Default: Only infrastructure and marked system failures count
    Default,

    /// All errors count as failures (not recommended)
    AllErrors,

    /// Custom classification function
    Custom(Arc<dyn Fn(&Error) -> ErrorSeverity + Send + Sync>),
}

impl Default for ErrorClassifier {
    fn default() -> Self {
        Self::Default
    }
}

impl std::fmt::Debug for ErrorClassifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default => write!(f, "Default"),
            Self::AllErrors => write!(f, "AllErrors"),
            Self::Custom(_) => write!(f, "Custom(<fn>)"),
        }
    }
}

impl ErrorClassifier {
    /// Classify an error to determine if it should trip the circuit breaker
    pub fn classify(&self, error: &Error) -> ErrorSeverity {
        match self {
            Self::Default => Self::default_classifier(error),
            Self::AllErrors => ErrorSeverity::SystemFailure,
            Self::Custom(classifier) => classifier(error),
        }
    }

    /// Default classification logic
    fn default_classifier(error: &Error) -> ErrorSeverity {
        // Check if error is marked as system failure
        if error.is::<SystemFailureMarker>() {
            return ErrorSeverity::SystemFailure;
        }

        // Check for known infrastructure errors
        if Self::is_infrastructure_error(error) {
            return ErrorSeverity::Infrastructure;
        }

        // Default to application error (won't trip circuit breaker)
        ErrorSeverity::Application
    }

    /// Check if error is an infrastructure-level failure
    fn is_infrastructure_error(error: &Error) -> bool {
        error.chain().any(|e| {
            // Standard library infrastructure errors
            e.is::<std::io::Error>()
                || e.is::<tokio::time::error::Elapsed>()
                // Iroh errors
                || e.downcast_ref::<iroh::endpoint::ConnectError>().is_some()
                // iroh-quinn QUIC errors
                || e.downcast_ref::<iroh_quinn::ReadError>().is_some()
                || e.downcast_ref::<iroh_quinn::WriteError>().is_some()
                || e.downcast_ref::<iroh_quinn::ConnectionError>().is_some()
        })
    }
}

/// Marker type for errors that should trip circuit breaker
#[derive(Debug)]
pub struct SystemFailureMarker {
    source: Error,
}

impl std::fmt::Display for SystemFailureMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "System failure: {}", self.source)
    }
}

impl std::error::Error for SystemFailureMarker {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// Extension trait for marking errors
///
/// # Example
/// ```rust
/// use zel_core::protocol::ErrorClassificationExt;
/// use anyhow::anyhow;
///
/// // Mark database error as system failure (trips circuit breaker)
/// let error = anyhow!("Database connection failed").as_system_failure();
///
/// // Mark validation error as application error (won't trip circuit breaker)
/// let error = anyhow!("Invalid input").as_application_error();
/// ```
pub trait ErrorClassificationExt: Sized {
    /// Mark this error as a system failure that should trip the circuit breaker
    ///
    /// Use this for errors like database failures, upstream service timeouts, etc.
    fn as_system_failure(self) -> Error;

    /// Mark this error as an application error that should NOT trip the circuit breaker
    ///
    /// Use this for errors like validation failures, "not found" errors, etc.
    /// Note: This is the default behavior, so only needed for clarity.
    fn as_application_error(self) -> Error;

    /// Alias for `as_application_error()` for brevity
    fn as_application(self) -> Error;

    /// Mark this error as an infrastructure error by wrapping in std::io::Error
    ///
    /// Use this for network errors, connection failures, etc.
    fn as_infrastructure(self) -> Error;
}

impl<E: Into<Error>> ErrorClassificationExt for E {
    fn as_system_failure(self) -> Error {
        Error::new(SystemFailureMarker {
            source: self.into(),
        })
    }

    fn as_application_error(self) -> Error {
        // Just convert to anyhow::Error without marking
        self.into()
    }

    fn as_application(self) -> Error {
        // Alias for as_application_error
        self.into()
    }

    fn as_infrastructure(self) -> Error {
        // Wrap in an IO error to trigger infrastructure classification
        let msg = format!("{}", self.into());
        Error::from(std::io::Error::new(std::io::ErrorKind::Other, msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn test_system_failure_marker() {
        let error = anyhow!("Database failed").as_system_failure();

        let classifier = ErrorClassifier::default();
        assert_eq!(classifier.classify(&error), ErrorSeverity::SystemFailure);
    }

    #[test]
    fn test_application_error() {
        let error = anyhow!("User not found").as_application_error();

        let classifier = ErrorClassifier::default();
        assert_eq!(classifier.classify(&error), ErrorSeverity::Application);
    }

    #[test]
    fn test_infrastructure_error_detection() {
        // Test with actual IO error, not just a string
        let io_error =
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "Connection timeout");
        let error: Error = io_error.into();

        let classifier = ErrorClassifier::default();
        assert_eq!(classifier.classify(&error), ErrorSeverity::Infrastructure);
    }

    #[test]
    fn test_io_error_detection() {
        let io_error = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        let error: Error = io_error.into();

        let classifier = ErrorClassifier::default();
        assert_eq!(classifier.classify(&error), ErrorSeverity::Infrastructure);
    }

    #[test]
    fn test_all_errors_classifier() {
        let error = anyhow!("Any error");

        let classifier = ErrorClassifier::AllErrors;
        assert_eq!(classifier.classify(&error), ErrorSeverity::SystemFailure);
    }

    #[test]
    fn test_custom_classifier() {
        let classifier = ErrorClassifier::Custom(Arc::new(|error| {
            if error.to_string().contains("critical") {
                ErrorSeverity::SystemFailure
            } else {
                ErrorSeverity::Application
            }
        }));

        let critical = anyhow!("critical failure");
        let normal = anyhow!("normal error");

        assert_eq!(classifier.classify(&critical), ErrorSeverity::SystemFailure);
        assert_eq!(classifier.classify(&normal), ErrorSeverity::Application);
    }
}
