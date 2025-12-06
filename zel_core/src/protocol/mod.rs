//! RPC protocol implementation including server, client, and service definition.
//!
//! This module provides the core protocol types for building RPC services over Iroh.
//! Most users will interact with [`RpcServerBuilder`] for servers and [`RpcClient`] for clients.
//!
//! # Key Types
//!
//! - [`RpcServerBuilder`] - Builds RPC servers with services
//! - [`RpcClient`] - Makes RPC calls and manages subscriptions
//! - [`RequestContext`] - Provides handlers access to connection info and extensions
//! - [`zel_service`] - Macro for defining type-safe services
//!
//! # Service Definition
//!
//! Services are typically defined using the [`zel_service`] macro which generates
//! server and client code automatically. See the [crate-level docs](crate) for examples.

use std::sync::atomic::AtomicBool;
use std::{collections::HashMap, fmt::Debug, sync::Arc};

use futures::future::BoxFuture;
use iroh::Endpoint;
use tokio::time::Duration;

// Submodules
pub mod builder;
pub mod circuit_breaker;
pub mod circuit_breaker_stats;
pub mod client;
pub mod context;
pub mod error_classification;
pub mod extensions;
pub mod handlers;
pub mod retry;
pub mod retry_metrics;
pub mod server;
pub mod shutdown;
pub mod streaming;
pub mod types;

// Re-export the procedural macro
pub use zel_macros::zel_service;

// Re-export circuit breaker types
pub use circuit_breaker::{
    CircuitBreakerConfig, CircuitBreakerConfigBuilder, CircuitState, PeerCircuitBreakers,
};
pub use circuit_breaker_stats::CircuitBreakerStats;

// Re-export client types for generated code
pub use client::{ClientError, NotificationSender, RpcClient, SubscriptionStream};

// Re-export new context types
pub use context::RequestContext;

// Re-export error classification types
pub use error_classification::{ErrorClassificationExt, ErrorClassifier, ErrorSeverity};

// Also import ErrorSeverity for use in this module

// Re-export extensions
pub use extensions::Extensions;

// Re-export retry types
pub use retry::{ErrorPredicate, RetryConfig, RetryConfigBuilder};
pub use retry_metrics::RetryMetrics;

// Re-export shutdown types
pub use shutdown::TaskTracker;

// Re-export builder types
pub use builder::{
    ConnectionHook, RequestMiddleware, RpcServerBuilder, ServiceBuilder, ShutdownError,
    ShutdownHandle,
};

// Re-export streaming types
pub use streaming::{NotificationError, NotificationSink, SubscriptionError, SubscriptionSink};

// Re-export core protocol types
pub use types::{Body, NotificationMsg, Request, Response, SubscriptionMsg};

// Shared protocol error types re-exported from `zel_types` so that both
// `zel_core` and `zel_macros` can agree on the same surface.
pub use zel_types::{ErrorSeverity as ResourceErrorSeverity, ResourceError};

pub trait ResourceResultExt<T> {
    fn to_app_error(self) -> Result<T, ResourceError>;
    fn to_infra_error(self) -> Result<T, ResourceError>;
    fn to_system_error(self) -> Result<T, ResourceError>;
}

impl<T> ResourceResultExt<T> for Result<T, anyhow::Error> {
    fn to_app_error(self) -> Result<T, ResourceError> {
        self.map_err(ResourceError::app)
    }

    fn to_infra_error(self) -> Result<T, ResourceError> {
        self.map_err(ResourceError::infra)
    }

    fn to_system_error(self) -> Result<T, ResourceError> {
        self.map_err(ResourceError::system)
    }
}

// ============================================================================
// Server and Service Types
// ============================================================================

pub(crate) type ServiceMap<'a> = Arc<HashMap<&'a str, RpcService<'a>>>;

#[derive(Debug)]
pub(crate) struct RpcService<'a> {
    #[allow(dead_code)]
    pub(crate) service: &'a str,
    pub(crate) resources: HashMap<&'a str, ResourceCallback>,
}

/// Handler function for different RPC endpoint types.
///
/// Each variant wraps a different handler type for the four endpoint patterns:
/// methods (RPC), subscriptions, notifications, and raw streams.
pub enum ResourceCallback {
    /// Handler for request/response methods
    Rpc(Rpc),
    /// Handler for server-to-client subscriptions
    SubscriptionProducer(Subscription),
    /// Handler for client-to-server notifications
    NotificationConsumer(NotificationHandler),
    /// Handler for bidirectional raw streams
    StreamHandler(StreamHandler),
}

impl Debug for ResourceCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rpc(_) => f.debug_tuple("Rpc").finish(),
            Self::SubscriptionProducer(_) => f.debug_tuple("SubscriptionProducer").finish(),
            Self::NotificationConsumer(_) => f.debug_tuple("NotificationConsumer").finish(),
            Self::StreamHandler(_) => f.debug_tuple("StreamHandler").finish(),
        }
    }
}

pub type ResourceResponse = Result<Response, ResourceError>;

pub type Rpc =
    Arc<dyn Send + Sync + Fn(RequestContext, Request) -> BoxFuture<'static, ResourceResponse>>;

pub type Subscription = Arc<
    dyn Send
        + Sync
        + Fn(
            RequestContext,
            Request,
            tokio_util::codec::FramedWrite<
                iroh::endpoint::SendStream,
                tokio_util::codec::LengthDelimitedCodec,
            >,
        ) -> BoxFuture<'static, ResourceResponse>,
>;

/// Handler for notification endpoints (client-to-server streaming).
///
/// Unlike subscriptions (server-to-client), notifications allow clients to push
/// events to the server over time with acknowledgments.
///
/// The handler receives:
/// - RequestContext with full middleware and extension support
/// - Request containing service/resource info and optional parameters
/// - FramedRead for receiving notification messages from client
/// - FramedWrite for sending acknowledgments back to client
pub type NotificationHandler = Arc<
    dyn Send
        + Sync
        + Fn(
            RequestContext,
            Request,
            tokio_util::codec::FramedRead<
                iroh::endpoint::RecvStream,
                tokio_util::codec::LengthDelimitedCodec,
            >,
            tokio_util::codec::FramedWrite<
                iroh::endpoint::SendStream,
                tokio_util::codec::LengthDelimitedCodec,
            >,
        ) -> BoxFuture<'static, ResourceResponse>,
>;

/// Handler for raw bidirectional stream endpoints.
///
/// Unlike RPC and Subscription handlers which work with framed/codec-wrapped streams,
/// StreamHandlers receive raw Iroh SendStream and RecvStream for maximum flexibility.
/// This enables custom protocols for video/audio streaming, file transfers, etc.
///
/// The handler receives:
/// - RequestContext with full middleware and extension support
/// - Request containing service/resource info and optional parameters
/// - Raw SendStream for sending bytes
/// - Raw RecvStream for receiving bytes
pub type StreamHandler = Arc<
    dyn Send
        + Sync
        + Fn(
            RequestContext,
            Request,
            iroh::endpoint::SendStream,
            iroh::endpoint::RecvStream,
        ) -> BoxFuture<'static, ResourceResponse>,
>;

// ============================================================================
// RpcServer
// ============================================================================

/// Lowest level RPC server that handles connections and routing
pub struct RpcServer<'a> {
    // ALPN for all the child services
    pub(crate) alpn: &'a [u8],
    pub(crate) endpoint: Endpoint,
    pub(crate) services: ServiceMap<'a>,
    pub(crate) server_extensions: Extensions,
    pub(crate) connection_hook: Option<ConnectionHook>,
    pub(crate) request_middleware: Vec<RequestMiddleware>,

    // Circuit breaker for per-peer protection
    pub(crate) circuit_breakers: Option<Arc<PeerCircuitBreakers>>,

    // Shutdown coordination
    pub(crate) shutdown_signal: Arc<tokio::sync::Notify>,
    pub(crate) is_shutting_down: Arc<AtomicBool>,
    pub(crate) task_tracker: TaskTracker,
}

impl<'a> std::fmt::Debug for RpcServer<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcServer")
            .field("alpn", &self.alpn)
            .field("endpoint", &self.endpoint)
            .field("services", &self.services)
            .field("server_extensions", &self.server_extensions)
            .field(
                "connection_hook",
                &self
                    .connection_hook
                    .as_ref()
                    .map(|_| "Some(ConnectionHook)"),
            )
            .field("request_middleware_count", &self.request_middleware.len())
            .field("circuit_breakers_enabled", &self.circuit_breakers.is_some())
            .finish()
    }
}

// Shutdown method implementation
impl<'a> RpcServer<'a> {
    /// Initiate graceful shutdown of the RPC server
    ///
    /// This method:
    /// 1. Stops accepting new connections immediately
    /// 2. Waits for inflight operations to complete (or timeout)
    /// 3. Returns when all tasks have completed or timeout is reached
    ///
    /// # Arguments
    /// * `timeout` - Maximum duration to wait for inflight operations
    ///
    /// # Returns
    /// * `Ok(())` if shutdown completed successfully
    /// * `Err(ShutdownError::Timeout)` if timeout was reached with active tasks
    ///
    /// # Example
    /// ```no_run
    /// use std::time::Duration;
    /// # use zel_core::protocol::RpcServerBuilder;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let endpoint = iroh::Endpoint::builder().bind().await?;
    /// let server = RpcServerBuilder::new(b"my-protocol", endpoint).build();
    /// server.shutdown(Duration::from_secs(30)).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn shutdown(self, timeout: Duration) -> Result<(), ShutdownError> {
        use std::sync::atomic::Ordering;

        log::info!("RpcServer shutdown initiated, timeout: {:?}", timeout);

        // 1. Signal all connection handlers to stop accepting new connections
        self.is_shutting_down.store(true, Ordering::Relaxed);
        self.shutdown_signal.notify_waiters();

        log::debug!("Shutdown signal sent to all connection handlers");

        // 2. Wait for all active tasks to complete (or timeout)
        let task_count_before = self.task_tracker.count();
        log::debug!("Waiting for {} active tasks to complete", task_count_before);

        match self.task_tracker.wait_for_completion(timeout).await {
            Ok(()) => {
                log::info!(
                    "RpcServer shutdown complete - all {} tasks finished gracefully",
                    task_count_before
                );
                Ok(())
            }
            Err(_) => {
                let remaining = self.task_tracker.count();
                log::warn!(
                    "RpcServer shutdown timeout - {} of {} tasks still active after {:?}",
                    remaining,
                    task_count_before,
                    timeout
                );
                Err(ShutdownError::Timeout {
                    remaining_tasks: remaining,
                    timeout,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn test_error_serialization() {
        // Test ServiceNotFound
        let error = ResourceError::ServiceNotFound {
            service: "test_service".to_string(),
        };
        let json = serde_json::to_string(&error).unwrap();
        let deserialized: ResourceError = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(deserialized, ResourceError::ServiceNotFound { service } if service == "test_service")
        );

        // Test ResourceNotFound
        let error = ResourceError::ResourceNotFound {
            service: "test_service".to_string(),
            resource: "test_resource".to_string(),
        };
        let json = serde_json::to_string(&error).unwrap();
        let deserialized: ResourceError = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(deserialized, ResourceError::ResourceNotFound { service, resource }
            if service == "test_service" && resource == "test_resource")
        );

        // Test CallbackError
        let error = ResourceError::CallbackError {
            message: "test error".to_string(),
            severity: ResourceErrorSeverity::Application,
            context: None,
        };
        let json = serde_json::to_string(&error).unwrap();
        let deserialized: ResourceError = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(deserialized, ResourceError::CallbackError { message, .. } if message == "test error")
        );

        // Test SerializationError
        let error = ResourceError::SerializationError("test error".to_string());
        let json = serde_json::to_string(&error).unwrap();
        let deserialized: ResourceError = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(deserialized, ResourceError::SerializationError(msg) if msg == "test error")
        );
    }

    #[test]
    fn test_resource_response_serialization() {
        // Test success response
        let response: ResourceResponse = Ok(Response {
            data: Bytes::from("success"),
        });
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: ResourceResponse = serde_json::from_str(&json).unwrap();
        assert!(deserialized.is_ok());

        // Test error response
        let response: ResourceResponse = Err(ResourceError::CallbackError {
            message: "test".to_string(),
            severity: ResourceErrorSeverity::Application,
            context: None,
        });
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: ResourceResponse = serde_json::from_str(&json).unwrap();
        assert!(deserialized.is_err());
    }
}
