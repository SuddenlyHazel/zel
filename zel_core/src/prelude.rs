//! Prelude module containing commonly used types for Zel RPC development.
//!
//! This module re-exports the most frequently used types from across the crate,
//! allowing users to import everything they need with a single glob import:
//!
//! ```rust
//! use zel_core::prelude::*;
//! ```
//!
//! # What's Included
//!
//! ## Core Protocol Types
//! - Error types: [`ResourceError`], [`ErrorSeverity`]
//! - Protocol messages: [`Request`], [`Response`], [`Body`]
//! - Streaming messages: [`SubscriptionMsg`], [`NotificationMsg`]
//!
//! ## Server Types
//! - [`RpcServerBuilder`] - Build RPC servers
//! - [`RpcServer`] - The actual server instance
//! - [`RequestContext`] - Handler context with extensions
//! - [`zel_service`] - Service definition macro
//!
//! ## Client Types
//! - [`RpcClient`] - Make RPC calls
//! - [`SubscriptionStream`] - Server-to-client streams
//! - [`NotificationSender`] - Client-to-server streams
//!
//! ## Reliability & Resilience
//! - [`RetryConfig`] - Configure retry behavior
//! - [`CircuitBreakerConfig`] - Configure circuit breakers
//! - [`ErrorClassifier`] - Classify errors for retry/circuit breaker
//!
//! ## Streaming Helpers
//! - [`SubscriptionSink`] - Send subscription data
//! - [`NotificationSink`] - Send notification acknowledgments
//!
//! ## Extensions
//! - [`Extensions`] - Type-safe extension storage
//!
//! ## Shutdown
//! - [`ShutdownHandle`] - Control server shutdown
//! - [`ShutdownError`] - Shutdown-related errors

// Re-export protocol error types (from zel_types via protocol module)
pub use crate::protocol::ResourceError;
pub use crate::protocol::ResourceErrorSeverity as ErrorSeverity;
pub use crate::protocol::ResourceResultExt;

// Re-export wire protocol types (from zel_types via protocol module)
pub use crate::protocol::{Body, NotificationMsg, Request, Response, SubscriptionMsg};

// Re-export macro
pub use crate::protocol::zel_service;

// Re-export server types
pub use crate::protocol::{ConnectionHook, RequestMiddleware};
pub use crate::protocol::{RpcServer, RpcServerBuilder, ServiceBuilder};

// Re-export client types
pub use crate::protocol::{ClientError, NotificationSender, RpcClient, SubscriptionStream};

// Re-export context and extensions
pub use crate::protocol::{Extensions, RequestContext};

// Re-export retry configuration
pub use crate::protocol::{ErrorPredicate, RetryConfig, RetryConfigBuilder, RetryMetrics};

// Re-export circuit breaker types
pub use crate::protocol::{
    CircuitBreakerConfig, CircuitBreakerConfigBuilder, CircuitBreakerStats, CircuitState,
    PeerCircuitBreakers,
};

// Re-export error classification
pub use crate::protocol::{ErrorClassificationExt, ErrorClassifier};

// Re-export streaming helpers
pub use crate::protocol::{
    NotificationError, NotificationSink, SubscriptionError, SubscriptionSink,
};

// Re-export shutdown types
pub use crate::protocol::{ShutdownError, ShutdownHandle, TaskTracker};

// Re-export Iroh helper types
pub use crate::{BuilderError, BundleBuilder, IrohBundle, ShutdownListener, ShutdownReplier};

// Re-export commonly used external types for convenience
pub use async_trait::async_trait;
pub use bytes::Bytes;
