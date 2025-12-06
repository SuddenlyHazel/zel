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
