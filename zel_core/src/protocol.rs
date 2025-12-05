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

use bytes::Bytes;
use futures::future::BoxFuture;
use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    Endpoint,
};
use serde::{Deserialize, Serialize};
use tokio::time::Duration;
use tokio_util::codec::{FramedWrite, LengthDelimitedCodec};

pub mod client;
pub mod context;
pub mod extensions;
pub mod server;
pub mod shutdown;

// Re-export the procedural macro
pub use zel_macros::zel_service;

// Re-export client types for generated code
pub use client::{ClientError, NotificationSender, RpcClient, SubscriptionStream};

// Re-export new context types
pub use context::RequestContext;
pub use extensions::Extensions;

// Re-export shutdown types
pub use shutdown::TaskTracker;

/// Request body type indicating the endpoint type and optional parameters.
#[derive(Debug, Serialize, Deserialize)]
pub enum Body {
    /// Subscription request (server-to-client streaming)
    Subscribe,
    /// RPC method call with serialized parameters
    Rpc(Bytes),
    /// Raw bidirectional stream request with optional serialized parameters
    Stream(Bytes),
    /// Notification stream request (client-to-server streaming) with optional parameters
    Notify(Bytes),
}

/// RPC request containing service name, resource name, and body.
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    /// The service name to route to
    pub service: String,
    /// The resource (method/subscription/etc) name within the service
    pub resource: String,
    /// The request body indicating endpoint type and parameters
    pub body: Body,
}

/// RPC response containing serialized result data.
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    /// Serialized response data
    pub data: Bytes,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum SubscriptionMsg {
    Established { service: String, resource: String },
    Data(Bytes),
    Stopped,
    ServerShutdown, // Server is shutting down gracefully
}

/// Messages for notification streams (client-to-server streaming)
#[derive(Debug, Serialize, Deserialize)]
pub enum NotificationMsg {
    Established { service: String, resource: String },
    Data(Bytes),    // Client sends data
    Ack,            // Server acknowledges receipt
    Error(String),  // Server reports error
    Completed,      // Client signals completion
    ServerShutdown, // Server is shutting down gracefully
}

// Lowest level
pub struct RpcServer<'a> {
    // APLN for all the child service
    alpn: &'a [u8],
    endpoint: Endpoint,
    services: ServiceMap<'a>,
    server_extensions: Extensions,
    connection_hook: Option<ConnectionHook>,
    request_middleware: Vec<RequestMiddleware>,

    // Shutdown coordination
    shutdown_signal: Arc<tokio::sync::Notify>,
    is_shutting_down: Arc<AtomicBool>,
    task_tracker: TaskTracker,
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
            .finish()
    }
}

type ServiceMap<'a> = Arc<HashMap<&'a str, RpcService<'a>>>;

#[derive(Debug)]
pub struct RpcService<'a> {
    #[allow(dead_code)]
    service: &'a str,
    resources: HashMap<&'a str, ResourceCallback>,
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

#[derive(thiserror::Error, Debug, Serialize, Deserialize)]
pub enum ResourceError {
    #[error("Service '{service}' not found")]
    ServiceNotFound { service: String },

    #[error("Resource '{resource}' not found in service '{service}'")]
    ResourceNotFound { service: String, resource: String },

    #[error("Callback execution failed: {0}")]
    CallbackError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),
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
            FramedWrite<SendStream, LengthDelimitedCodec>,
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
            tokio_util::codec::FramedRead<RecvStream, LengthDelimitedCodec>,
            tokio_util::codec::FramedWrite<SendStream, LengthDelimitedCodec>,
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
        + Fn(RequestContext, Request, SendStream, RecvStream) -> BoxFuture<'static, ResourceResponse>,
>;

/// Hook called when a new connection is established.
///
/// Receives the connection and server extensions, returns connection extensions.
/// Can be used for authentication, session initialization, etc.
///
/// If the hook returns an error, the connection will still be accepted but
/// connection extensions will be empty.
pub type ConnectionHook = Arc<
    dyn Send + Sync + Fn(&Connection, Extensions) -> BoxFuture<'static, Result<Extensions, String>>,
>;

/// Middleware that can modify RequestContext before it reaches handlers.
///
/// Useful for adding trace IDs, timing, logging, etc.
/// Middleware is applied in the order it was added to the builder.
pub type RequestMiddleware =
    Arc<dyn Send + Sync + Fn(RequestContext) -> BoxFuture<'static, RequestContext>>;

// ============================================================================
// Subscription Sink Wrapper
// ============================================================================

use futures::SinkExt;

/// Wrapper for sending subscription data from server to client.
///
/// This is used in **server-to-client streaming** endpoints (subscriptions).
/// The server sends data over time to the client using this sink.
///
/// # Directionality
///
/// ```text
/// Server ──[SubscriptionSink]──> Client
/// ```
///
/// The server handler receives a `SubscriptionSink` and uses it to push data to the client.
/// Contrast this with [`NotificationSink`] which goes the opposite direction.
///
/// # Example
///
/// ```rust,ignore
/// async fn counter(
///     ctx: RequestContext,
///     mut sink: SubscriptionSink,
///     interval_ms: u64,
/// ) -> Result<(), String> {
///     let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
///
///     for i in 0..10 {
///         interval.tick().await;
///         sink.send(&i).await?; // Server sends to client
///     }
///
///     sink.close().await?;
///     Ok(())
/// }
/// ```
pub struct SubscriptionSink {
    inner: FramedWrite<SendStream, LengthDelimitedCodec>,
}

impl SubscriptionSink {
    /// Create a new SubscriptionSink from a FramedWrite
    pub fn new(inner: FramedWrite<SendStream, LengthDelimitedCodec>) -> Self {
        Self { inner }
    }

    /// Send data to the subscription
    ///
    /// Automatically wraps the data in SubscriptionMsg::Data and serializes it.
    pub async fn send<T: Serialize>(&mut self, data: T) -> Result<(), SubscriptionError> {
        let data = serde_json::to_vec(&data)
            .map_err(|e| SubscriptionError::Serialization(e.to_string()))?;

        let msg = SubscriptionMsg::Data(Bytes::from(data));
        let msg_bytes = serde_json::to_vec(&msg)
            .map_err(|e| SubscriptionError::Serialization(e.to_string()))?;

        self.inner
            .send(msg_bytes.into())
            .await
            .map_err(|e| SubscriptionError::Send(e.to_string()))?;

        Ok(())
    }

    /// Send the stopped message and close the subscription
    pub async fn close(mut self) -> Result<(), SubscriptionError> {
        let msg = SubscriptionMsg::Stopped;
        let msg_bytes = serde_json::to_vec(&msg)
            .map_err(|e| SubscriptionError::Serialization(e.to_string()))?;

        self.inner
            .send(msg_bytes.into())
            .await
            .map_err(|e| SubscriptionError::Send(e.to_string()))?;

        Ok(())
    }

    /// Get a reference to the underlying SendStream
    ///
    /// This allows direct access to all SendStream methods like:
    /// - `set_priority(i32)` - Set stream priority for multiplexing
    /// - `priority()` - Get current priority
    /// - `id()` - Get the stream ID
    /// - `reset(VarInt)` - Abort the stream with an error code
    /// - `stopped()` - Wait for peer acknowledgment
    ///
    /// # Example
    /// ```no_run
    /// # use zel_core::protocol::SubscriptionSink;
    /// # async fn example(mut sink: SubscriptionSink) -> Result<(), Box<dyn std::error::Error>> {
    /// // Set high priority for this subscription
    /// sink.stream().set_priority(10)?;
    ///
    /// // Send data
    /// sink.send(&"important data").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn stream(&self) -> &SendStream {
        self.inner.get_ref()
    }
}

/// Errors that can occur when sending subscription messages
#[derive(thiserror::Error, Debug)]
pub enum SubscriptionError {
    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Send error: {0}")]
    Send(String),
}

// ============================================================================
// Notification Sink Wrapper (Server-side)
// ============================================================================

use tokio_util::codec::FramedRead;

/// Wrapper for sending acknowledgments in client-to-server streaming.
///
/// This is used in **client-to-server streaming** endpoints (notifications).
/// The server receives data from the client and uses this sink to send acknowledgments back.
///
/// # Directionality
///
/// ```text
/// Client ──[NotificationMsg::Data]──> Server
/// Client <──[NotificationMsg::Ack]──── Server (via NotificationSink)
/// ```
///
/// The client pushes data to the server, and the server uses `NotificationSink` to acknowledge
/// receipt. Contrast this with [`SubscriptionSink`] which sends data from server to client.
///
/// # Example
///
/// ```rust,ignore
/// async fn log_receiver(
///     ctx: RequestContext,
///     mut recv: FramedRead<RecvStream, LengthDelimitedCodec>,
///     mut ack_sink: NotificationSink,
/// ) -> Result<(), String> {
///     while let Some(msg_bytes) = recv.next().await {
///         let msg: NotificationMsg = serde_json::from_slice(&msg_bytes?)?;
///
///         match msg {
///             NotificationMsg::Data(data) => {
///                 // Process client data
///                 process_log_entry(&data);
///                 ack_sink.ack().await?; // Acknowledge receipt
///             }
///             NotificationMsg::Completed => break,
///             _ => {}
///         }
///     }
///     Ok(())
/// }
/// ```
pub struct NotificationSink {
    inner: FramedWrite<SendStream, LengthDelimitedCodec>,
}

impl NotificationSink {
    /// Create a new NotificationSink from a FramedWrite
    pub fn new(inner: FramedWrite<SendStream, LengthDelimitedCodec>) -> Self {
        Self { inner }
    }

    /// Send an acknowledgment for the received notification
    pub async fn ack(&mut self) -> Result<(), NotificationError> {
        let msg = NotificationMsg::Ack;
        let msg_bytes = serde_json::to_vec(&msg)
            .map_err(|e| NotificationError::Serialization(e.to_string()))?;

        self.inner
            .send(msg_bytes.into())
            .await
            .map_err(|e| NotificationError::Send(e.to_string()))?;

        Ok(())
    }

    /// Send an error message to the client
    pub async fn error(&mut self, error_msg: impl Into<String>) -> Result<(), NotificationError> {
        let msg = NotificationMsg::Error(error_msg.into());
        let msg_bytes = serde_json::to_vec(&msg)
            .map_err(|e| NotificationError::Serialization(e.to_string()))?;

        self.inner
            .send(msg_bytes.into())
            .await
            .map_err(|e| NotificationError::Send(e.to_string()))?;

        Ok(())
    }

    /// Get a reference to the underlying SendStream
    ///
    /// This allows direct access to all SendStream methods for the acknowledgment stream.
    pub fn stream(&self) -> &SendStream {
        self.inner.get_ref()
    }
}

/// Errors that can occur when handling notifications
#[derive(thiserror::Error, Debug)]
pub enum NotificationError {
    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Deserialization error: {0}")]
    Deserialization(String),

    #[error("Send error: {0}")]
    Send(String),

    #[error("Receive error: {0}")]
    Receive(String),

    #[error("Protocol error: {0}")]
    Protocol(String),
}

// ============================================================================
// Shutdown Handle
// ============================================================================

/// Handle for triggering graceful shutdown of an RpcServer
///
/// This handle is obtained from [`RpcServerBuilder::build_with_shutdown()`] and allows
/// you to trigger graceful shutdown even after the server has been moved into the router.
///
/// # Example
/// ```no_run
/// use std::time::Duration;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// # use zel_core::protocol::RpcServerBuilder;
/// # let endpoint = iroh::Endpoint::builder().bind().await?;
/// let (server, shutdown_handle) = RpcServerBuilder::new(b"my-protocol", endpoint)
///     .build_with_shutdown();
///
/// // Register server with router (server is moved here)
/// // router.accept(b"my-protocol", server);
///
/// // Later, trigger shutdown using the handle
/// shutdown_handle.shutdown(Duration::from_secs(30)).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct ShutdownHandle {
    shutdown_signal: Arc<tokio::sync::Notify>,
    is_shutting_down: Arc<AtomicBool>,
    task_tracker: TaskTracker,
}

impl ShutdownHandle {
    /// Trigger graceful shutdown of the RPC server
    ///
    /// This will:
    /// 1. Stop accepting new connections
    /// 2. Wait for active operations to complete (or timeout)
    /// 3. Return when all tasks finish or timeout is reached
    pub async fn shutdown(self, timeout: Duration) -> Result<(), ShutdownError> {
        use std::sync::atomic::Ordering;

        log::info!(
            "RpcServer shutdown initiated via handle, timeout: {:?}",
            timeout
        );

        // Signal shutdown
        self.is_shutting_down.store(true, Ordering::Relaxed);
        self.shutdown_signal.notify_waiters();

        log::debug!("Shutdown signal sent to all connection handlers");

        // Wait for tasks to complete
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

// ============================================================================
// Shutdown Error
// ============================================================================

/// Errors that can occur during shutdown
#[derive(thiserror::Error, Debug)]
pub enum ShutdownError {
    #[error("Shutdown timeout exceeded: {remaining_tasks} tasks still active after {timeout:?}")]
    Timeout {
        remaining_tasks: usize,
        timeout: Duration,
    },

    #[error("Task join error: {0}")]
    TaskJoinError(#[from] tokio::task::JoinError),
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let error = ResourceError::CallbackError("test error".to_string());
        let json = serde_json::to_string(&error).unwrap();
        let deserialized: ResourceError = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, ResourceError::CallbackError(msg) if msg == "test error"));

        // Test SerializationError
        let error = ResourceError::SerializationError("test error".to_string());
        let json = serde_json::to_string(&error).unwrap();
        let deserialized: ResourceError = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(deserialized, ResourceError::SerializationError(msg) if msg == "test error")
        );
    }

    #[test]
    fn test_request_serialization() {
        // Test RPC request
        let request = Request {
            service: "test_service".to_string(),
            resource: "test_resource".to_string(),
            body: Body::Rpc(Bytes::from("test data")),
        };
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.service, "test_service");
        assert_eq!(deserialized.resource, "test_resource");
        assert!(matches!(deserialized.body, Body::Rpc(_)));

        // Test Subscribe request
        let request = Request {
            service: "test_service".to_string(),
            resource: "test_resource".to_string(),
            body: Body::Subscribe,
        };
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized.body, Body::Subscribe));
    }

    #[test]
    fn test_response_serialization() {
        let response = Response {
            data: Bytes::from("test response"),
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.data, Bytes::from("test response"));
    }

    #[test]
    fn test_subscription_msg_serialization() {
        // Test Established
        let msg = SubscriptionMsg::Established {
            service: "test_service".to_string(),
            resource: "test_resource".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: SubscriptionMsg = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(deserialized, SubscriptionMsg::Established { service, resource }
            if service == "test_service" && resource == "test_resource")
        );

        // Test Data
        let msg = SubscriptionMsg::Data(Bytes::from("test data"));
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: SubscriptionMsg = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, SubscriptionMsg::Data(_)));

        // Test Stopped
        let msg = SubscriptionMsg::Stopped;
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: SubscriptionMsg = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, SubscriptionMsg::Stopped));
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
        let response: ResourceResponse = Err(ResourceError::CallbackError("test".to_string()));
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: ResourceResponse = serde_json::from_str(&json).unwrap();
        assert!(deserialized.is_err());
    }
}

// ============================================================================
// Builder Pattern
// ============================================================================

/// Builder for constructing an RpcServer with a fluent API.
///
/// `RpcServerBuilder` provides a fluent interface for configuring RPC servers with
/// services, middleware, connection hooks, and server-level extensions.
///
/// # Quick Start
///
/// ```rust,no_run
/// use zel_core::protocol::{RpcServerBuilder, RequestContext};
/// use zel_core::IrohBundle;
///
/// # async fn example() -> anyhow::Result<()> {
/// let mut bundle = IrohBundle::builder(None).await?;
/// let endpoint = bundle.endpoint().clone();
///
/// // Build a server with a service
/// let server = RpcServerBuilder::new(b"myapp/1", endpoint)
///     .service("calculator")
///         .rpc_resource("add", |ctx: RequestContext, req| {
///             Box::pin(async move {
///                 // Handle the request
///                 Ok(zel_core::protocol::Response {
///                     data: bytes::Bytes::from("result")
///                 })
///             })
///         })
///         .build()
///     .build();
///
/// bundle.accept(b"myapp/1", server).finish().await;
/// # Ok(())
/// # }
/// ```
///
/// # Adding Services
///
/// Services are added using [`service()`](RpcServerBuilder::service), which returns a
/// [`ServiceBuilder`] for defining resources (methods, subscriptions, notifications, streams).
///
/// Most users will prefer the [`zel_service`](crate::protocol::zel_service) macro which
/// generates the service builder code automatically.
///
/// # Shutdown Support
///
/// - Use [`build()`](RpcServerBuilder::build) for simple cases
/// - Use [`build_with_shutdown()`](RpcServerBuilder::build_with_shutdown) to get a [`ShutdownHandle`]
///   that allows triggering graceful shutdown after the server has been moved into the router
///
/// # Extensions and Middleware
///
/// - [`with_extensions()`](RpcServerBuilder::with_extensions) - Add server-level extensions (shared across all connections)
/// - [`with_connection_hook()`](RpcServerBuilder::with_connection_hook) - Set up per-connection state
/// - [`with_request_middleware()`](RpcServerBuilder::with_request_middleware) - Add request-level middleware
pub struct RpcServerBuilder<'a> {
    alpn: &'a [u8],
    endpoint: Endpoint,
    services: HashMap<&'a str, RpcService<'a>>,
    server_extensions: Extensions,
    connection_hook: Option<ConnectionHook>,
    request_middleware: Vec<RequestMiddleware>,
}

impl<'a> RpcServerBuilder<'a> {
    /// Create a new RPC server builder
    ///
    /// # Arguments
    /// * `alpn` - The ALPN protocol identifier
    /// * `endpoint` - The Iroh endpoint
    /// * `router` - The Iroh protocol router
    pub fn new(alpn: &'a [u8], endpoint: Endpoint) -> Self {
        Self {
            alpn,
            endpoint,
            services: HashMap::new(),
            server_extensions: Extensions::new(),
            connection_hook: None,
            request_middleware: Vec::new(),
        }
    }

    /// Set server-level extensions (shared across all connections)
    pub fn with_extensions(mut self, extensions: Extensions) -> Self {
        self.server_extensions = extensions;
        self
    }

    /// Set a connection hook for populating connection-level extensions
    ///
    /// The hook is called once per connection and can be used for authentication,
    /// session initialization, or any per-connection setup.
    pub fn with_connection_hook<F>(mut self, hook: F) -> Self
    where
        F: Fn(&Connection, Extensions) -> BoxFuture<'static, Result<Extensions, String>>
            + Send
            + Sync
            + 'static,
    {
        self.connection_hook = Some(Arc::new(hook));
        self
    }

    /// Add request middleware for enriching RequestContext before handlers
    ///
    /// Middleware is applied in the order it was added. Each middleware receives
    /// the context and can add request-level extensions.
    pub fn with_request_middleware<F>(mut self, middleware: F) -> Self
    where
        F: Fn(RequestContext) -> BoxFuture<'static, RequestContext> + Send + Sync + 'static,
    {
        self.request_middleware.push(Arc::new(middleware));
        self
    }

    /// Begin defining a service
    ///
    /// # Arguments
    /// * `name` - The service name
    ///
    /// # Returns
    /// A ServiceBuilder for adding resources to this service
    pub fn service(self, name: &'a str) -> ServiceBuilder<'a> {
        ServiceBuilder::new(name, self)
    }

    /// Build the RpcServer
    pub fn build(self) -> RpcServer<'a> {
        RpcServer {
            alpn: self.alpn,
            endpoint: self.endpoint,
            services: Arc::new(self.services),
            server_extensions: self.server_extensions,
            connection_hook: self.connection_hook,
            request_middleware: self.request_middleware,

            // Initialize shutdown infrastructure
            shutdown_signal: Arc::new(tokio::sync::Notify::new()),
            is_shutting_down: Arc::new(AtomicBool::new(false)),
            task_tracker: TaskTracker::new(),
        }
    }

    /// Build the RpcServer and return a shutdown handle
    ///
    /// This is the preferred way to build a server if you need graceful shutdown support.
    /// The shutdown handle can be used to trigger shutdown even after the server has been
    /// moved into the router.
    ///
    /// # Example
    /// ```no_run
    /// use std::time::Duration;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # use zel_core::protocol::RpcServerBuilder;
    /// # let endpoint = iroh::Endpoint::builder().bind().await?;
    /// let (server, shutdown_handle) = RpcServerBuilder::new(b"my-protocol", endpoint)
    ///     .build_with_shutdown();
    ///
    /// // Register server with router
    /// // router.accept(b"my-protocol", server);
    ///
    /// // Later...
    /// shutdown_handle.shutdown(Duration::from_secs(30)).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn build_with_shutdown(self) -> (RpcServer<'a>, ShutdownHandle) {
        let shutdown_signal = Arc::new(tokio::sync::Notify::new());
        let is_shutting_down = Arc::new(AtomicBool::new(false));
        let task_tracker = TaskTracker::new();

        let handle = ShutdownHandle {
            shutdown_signal: Arc::clone(&shutdown_signal),
            is_shutting_down: Arc::clone(&is_shutting_down),
            task_tracker: task_tracker.clone(),
        };

        let server = RpcServer {
            alpn: self.alpn,
            endpoint: self.endpoint,
            services: Arc::new(self.services),
            server_extensions: self.server_extensions,
            connection_hook: self.connection_hook,
            request_middleware: self.request_middleware,
            shutdown_signal,
            is_shutting_down,
            task_tracker,
        };

        (server, handle)
    }
}

// ============================================================================
// Shutdown Method
// ============================================================================

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

/// Builder for defining resources within a service.
///
/// Created by [`RpcServerBuilder::service()`]. Add resources (methods, subscriptions,
/// notifications, streams) then call [`build()`](ServiceBuilder::build) to return to
/// the server builder.
///
/// # Example
///
/// ```rust,no_run
/// use zel_core::protocol::{RpcServerBuilder, RequestContext, Response};
/// use bytes::Bytes;
///
/// # async fn example() -> anyhow::Result<()> {
/// # let endpoint = iroh::Endpoint::builder().bind().await?;
/// let server = RpcServerBuilder::new(b"myapp/1", endpoint)
///     .service("calculator")
///         .rpc_resource("add", |ctx: RequestContext, req| {
///             Box::pin(async move {
///                 // Handle request
///                 Ok(Response { data: Bytes::from("result") })
///             })
///         })
///         .rpc_resource("multiply", |ctx, req| {
///             Box::pin(async move {
///                 Ok(Response { data: Bytes::from("result") })
///             })
///         })
///         .build() // Returns to RpcServerBuilder
///     .build(); // Builds the server
/// # Ok(())
/// # }
/// ```
///
/// Typically you'll use the [`zel_service`](crate::protocol::zel_service) macro instead
/// of manually adding resources. The macro generates all this builder code for you.
pub struct ServiceBuilder<'a> {
    name: &'a str,
    resources: HashMap<&'a str, ResourceCallback>,
    server_builder: RpcServerBuilder<'a>,
}

impl<'a> ServiceBuilder<'a> {
    fn new(name: &'a str, server_builder: RpcServerBuilder<'a>) -> Self {
        Self {
            name,
            resources: HashMap::new(),
            server_builder,
        }
    }

    /// Add an RPC resource to this service
    ///
    /// # Arguments
    /// * `name` - The resource name
    /// * `callback` - The RPC callback function
    pub fn rpc_resource<F>(mut self, name: &'a str, callback: F) -> Self
    where
        F: Fn(RequestContext, Request) -> BoxFuture<'static, ResourceResponse>
            + Send
            + Sync
            + 'static,
    {
        self.resources
            .insert(name, ResourceCallback::Rpc(Arc::new(callback)));
        self
    }

    /// Add a subscription resource to this service
    ///
    /// # Arguments
    /// * `name` - The resource name
    /// * `callback` - The subscription callback function
    pub fn subscription_resource<F>(mut self, name: &'a str, callback: F) -> Self
    where
        F: Fn(
                RequestContext,
                Request,
                FramedWrite<SendStream, LengthDelimitedCodec>,
            ) -> BoxFuture<'static, ResourceResponse>
            + Send
            + Sync
            + 'static,
    {
        self.resources.insert(
            name,
            ResourceCallback::SubscriptionProducer(Arc::new(callback)),
        );
        self
    }

    /// Add a notification resource to this service (client-to-server streaming)
    ///
    /// Notification resources allow clients to push events to the server over time
    /// with acknowledgment support.
    ///
    /// # Arguments
    /// * `name` - The resource name
    /// * `callback` - The notification callback function
    pub fn notification_resource<F>(mut self, name: &'a str, callback: F) -> Self
    where
        F: Fn(
                RequestContext,
                Request,
                FramedRead<RecvStream, LengthDelimitedCodec>,
                FramedWrite<SendStream, LengthDelimitedCodec>,
            ) -> BoxFuture<'static, ResourceResponse>
            + Send
            + Sync
            + 'static,
    {
        self.resources.insert(
            name,
            ResourceCallback::NotificationConsumer(Arc::new(callback)),
        );
        self
    }

    /// Add a raw stream resource to this service
    ///
    /// Stream resources receive raw Iroh SendStream and RecvStream for implementing
    /// custom protocols (video/audio streaming, file transfers, etc.)
    ///
    /// # Arguments
    /// * `name` - The resource name
    /// * `callback` - The stream callback function receiving raw streams
    pub fn stream_resource<F>(mut self, name: &'a str, callback: F) -> Self
    where
        F: Fn(
                RequestContext,
                Request,
                SendStream,
                RecvStream,
            ) -> BoxFuture<'static, ResourceResponse>
            + Send
            + Sync
            + 'static,
    {
        self.resources
            .insert(name, ResourceCallback::StreamHandler(Arc::new(callback)));
        self
    }

    /// Finish building this service and return to the server builder
    pub fn build(mut self) -> RpcServerBuilder<'a> {
        let service = RpcService {
            service: self.name,
            resources: self.resources,
        };

        self.server_builder.services.insert(self.name, service);
        self.server_builder
    }
}
