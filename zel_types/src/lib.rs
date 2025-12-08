//! Shared types for the Zel RPC framework.
//!
//! This crate provides core types used across the Zel ecosystem, including wire
//! protocol definitions and error types. It exists as a separate crate to allow
//! both `zel_core` and `zel_macros` to depend on the same types without circular
//! dependencies.
//!
//! # Main Types
//!
//! ## Protocol Types
//!
//! - [`protocol::Request`] - RPC request structure
//! - [`protocol::Response`] - RPC response structure
//! - [`protocol::Body`] - Request body enum
//! - [`protocol::SubscriptionMsg`] - Subscription stream messages
//! - [`protocol::NotificationMsg`] - Notification stream messages
//!
//! ## Error Types
//!
//! - [`ResourceError`] - Error type returned by RPC resource handlers
//! - [`ErrorSeverity`] - Classification for circuit breaker decisions
//!
//! # Error Severity
//!
//! Errors are classified into three categories:
//!
//! - **Application** - Business logic errors (validation, "not found", etc.) - don't trip circuit breakers
//! - **Infrastructure** - Network/protocol failures (connection drops, panics) - trip circuit breakers
//! - **SystemFailure** - Backend failures (database timeout, service down) - trip circuit breakers
//!
//! # Example
//!
//! ```rust
//! use zel_types::{ResourceError, ErrorSeverity};
//!
//! // Application error - won't trip circuit breaker
//! let app_err = ResourceError::app("User not found");
//! assert_eq!(app_err.severity(), ErrorSeverity::Application);
//!
//! // Infrastructure error - will trip circuit breaker
//! let infra_err = ResourceError::infra("Connection timeout");
//! assert_eq!(infra_err.severity(), ErrorSeverity::Infrastructure);
//!
//! // System failure - will trip circuit breaker
//! let sys_err = ResourceError::system("Database unavailable");
//! assert_eq!(sys_err.severity(), ErrorSeverity::SystemFailure);
//! ```
//!
//! # Protocol Types
//!
//! ```rust
//! use zel_types::protocol::{Request, Response, Body};
//! use bytes::Bytes;
//!
//! // Create an RPC request
//! let request = Request {
//!     service: "my_service".to_string(),
//!     resource: "my_method".to_string(),
//!     body: Body::Rpc(Bytes::from("parameters")),
//! };
//!
//! // Create a response
//! let response = Response {
//!     data: Bytes::from("result"),
//! };
//! ```

pub mod protocol;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Classification of error severity for circuit breaker and protocol decisions.
///
/// This is a minimal shared copy so that both `zel_core` and `zel_macros` can
/// agree on the same severity vocabulary without depending on each other.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ErrorSeverity {
    /// Infrastructure errors - ALWAYS trip circuit breaker
    ///
    /// Examples: panics, connection drops, protocol failures
    Infrastructure,

    /// System failures - trip circuit breaker
    ///
    /// Examples: database timeout, upstream service down
    SystemFailure,

    /// Application/business logic errors - DON'T trip circuit breaker
    ///
    /// Examples: validation errors, "user not found", permission denied
    Application,
}

/// Resource-level error type shared between core protocol and macros.
///
/// This is the error type returned by RPC resources and surfaced to clients.
/// It carries a severity for callback failures so the reliability layer can
/// distinguish application vs. infrastructure/system issues.
#[derive(Error, Debug, Serialize, Deserialize, Clone)]
pub enum ResourceError {
    #[error("Service '{service}' not found")]
    ServiceNotFound { service: String },

    #[error("Resource '{resource}' not found in service '{service}'")]
    ResourceNotFound { service: String, resource: String },

    // Structured callback error with severity + optional JSON context
    #[error("Callback execution failed: {message}")]
    CallbackError {
        message: String,
        severity: ErrorSeverity,
        /// Optional structured context - users can provide custom error details
        /// This is serialized as JSON, allowing any `Serialize` type
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<serde_json::Value>,
    },

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Circuit breaker open for peer {peer_id}")]
    CircuitBreakerOpen { peer_id: String },
}

impl ResourceError {
    /// Extract severity from error for circuit breaker classification.
    pub fn severity(&self) -> ErrorSeverity {
        match self {
            ResourceError::CallbackError { severity, .. } => *severity,
            // Circuit breaker errors are always infrastructure failures
            ResourceError::CircuitBreakerOpen { .. } => ErrorSeverity::Infrastructure,
            // Other error types default to application errors (safe default)
            ResourceError::ServiceNotFound { .. }
            | ResourceError::ResourceNotFound { .. }
            | ResourceError::SerializationError(_) => ErrorSeverity::Application,
        }
    }

    pub fn app<E: ToString>(e: E) -> Self {
        ResourceError::CallbackError {
            message: e.to_string(),
            severity: ErrorSeverity::Application,
            context: None,
        }
    }

    pub fn infra<E: ToString>(e: E) -> Self {
        ResourceError::CallbackError {
            message: e.to_string(),
            severity: ErrorSeverity::Infrastructure,
            context: None,
        }
    }

    pub fn system<E: ToString>(e: E) -> Self {
        ResourceError::CallbackError {
            message: e.to_string(),
            severity: ErrorSeverity::SystemFailure,
            context: None,
        }
    }
}

impl From<String> for ResourceError {
    fn from(message: String) -> Self {
        ResourceError::CallbackError {
            message,
            severity: ErrorSeverity::Application,
            context: None,
        }
    }
}

impl From<&str> for ResourceError {
    fn from(message: &str) -> Self {
        ResourceError::CallbackError {
            message: message.to_string(),
            severity: ErrorSeverity::Application,
            context: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_helper() {
        let app = ResourceError::app("oops");
        assert_eq!(app.severity(), ErrorSeverity::Application);

        let infra = ResourceError::infra("net down");
        assert_eq!(infra.severity(), ErrorSeverity::Infrastructure);

        let sys = ResourceError::system("db down");
        assert_eq!(sys.severity(), ErrorSeverity::SystemFailure);

        let cb = ResourceError::CircuitBreakerOpen {
            peer_id: "peer1".into(),
        };
        assert_eq!(cb.severity(), ErrorSeverity::Infrastructure);
    }
}
