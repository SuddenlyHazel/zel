//! Builder types for constructing RPC servers and services.
//!
//! This module provides the fluent builder API for configuring RPC servers with
//! services, middleware, extensions, and circuit breakers.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use futures::future::BoxFuture;
use iroh::{endpoint::Connection, Endpoint};
use tokio::time::Duration;

use super::circuit_breaker::{CircuitBreakerConfig, PeerCircuitBreakers};
use super::circuit_breaker_stats::CircuitBreakerStats;
use super::context::RequestContext;
use super::extensions::Extensions;
use super::shutdown::TaskTracker;
use super::{Request, ResourceCallback, ResourceResponse, RpcService};

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
    pub(super) shutdown_signal: Arc<tokio::sync::Notify>,
    pub(super) is_shutting_down: Arc<AtomicBool>,
    pub(super) task_tracker: TaskTracker,
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
    pub(super) alpn: &'a [u8],
    pub(super) endpoint: Endpoint,
    pub(super) services: HashMap<&'a str, RpcService<'a>>,
    pub(super) server_extensions: Extensions,
    pub(super) connection_hook: Option<ConnectionHook>,
    pub(super) request_middleware: Vec<RequestMiddleware>,
    pub(super) circuit_breaker_config: Option<CircuitBreakerConfig>,
}

impl<'a> RpcServerBuilder<'a> {
    /// Create a new RPC server builder
    ///
    /// # Arguments
    /// * `alpn` - The ALPN protocol identifier
    /// * `endpoint` - The Iroh endpoint
    pub fn new(alpn: &'a [u8], endpoint: Endpoint) -> Self {
        Self {
            alpn,
            endpoint,
            services: HashMap::new(),
            server_extensions: Extensions::new(),
            connection_hook: None,
            request_middleware: Vec::new(),
            circuit_breaker_config: None,
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

    /// Enable circuit breaker protection with the given configuration
    ///
    /// This adds per-peer circuit breakers to protect the server from
    /// misbehaving clients.
    ///
    /// # Example
    /// ```rust,no_run
    /// use zel_core::protocol::{RpcServerBuilder, CircuitBreakerConfig};
    /// use std::time::Duration;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let endpoint = iroh::Endpoint::builder().bind().await?;
    /// let cb_config = CircuitBreakerConfig::builder()
    ///     .failure_threshold(0.5)
    ///     .consecutive_failures(5)
    ///     .timeout(Duration::from_secs(30))
    ///     .build();
    ///
    /// let server = RpcServerBuilder::new(b"myapp/1", endpoint)
    ///     .with_circuit_breaker(cb_config)
    ///     .build();
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_circuit_breaker(mut self, config: CircuitBreakerConfig) -> Self {
        self.circuit_breaker_config = Some(config);
        self
    }

    /// Initialize circuit breakers if configured.
    ///
    /// Returns a tuple of (optional circuit breaker, server extensions).
    /// If circuit breaker is configured, stats are added to extensions.
    pub(super) fn initialize_circuit_breakers(
        &mut self,
    ) -> (Option<Arc<PeerCircuitBreakers>>, Extensions) {
        if let Some(config) = self.circuit_breaker_config.take() {
            let breakers = Arc::new(PeerCircuitBreakers::new(config));
            let stats = CircuitBreakerStats::new(Arc::clone(&breakers));
            let exts = self.server_extensions.clone().with(stats);
            (Some(breakers), exts)
        } else {
            (None, self.server_extensions.clone())
        }
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
    pub fn build(mut self) -> super::RpcServer<'a> {
        let (circuit_breakers, server_extensions) = self.initialize_circuit_breakers();

        super::RpcServer {
            alpn: self.alpn,
            endpoint: self.endpoint,
            services: Arc::new(self.services),
            server_extensions,
            connection_hook: self.connection_hook,
            request_middleware: self.request_middleware,
            circuit_breakers,

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
    pub fn build_with_shutdown(mut self) -> (super::RpcServer<'a>, ShutdownHandle) {
        let shutdown_signal = Arc::new(tokio::sync::Notify::new());
        let is_shutting_down = Arc::new(AtomicBool::new(false));
        let task_tracker = TaskTracker::new();

        let (circuit_breakers, server_extensions) = self.initialize_circuit_breakers();

        let handle = ShutdownHandle {
            shutdown_signal: Arc::clone(&shutdown_signal),
            is_shutting_down: Arc::clone(&is_shutting_down),
            task_tracker: task_tracker.clone(),
        };

        let server = super::RpcServer {
            alpn: self.alpn,
            endpoint: self.endpoint,
            services: Arc::new(self.services),
            server_extensions,
            connection_hook: self.connection_hook,
            request_middleware: self.request_middleware,
            circuit_breakers,
            shutdown_signal,
            is_shutting_down,
            task_tracker,
        };

        (server, handle)
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
    pub(super) fn new(name: &'a str, server_builder: RpcServerBuilder<'a>) -> Self {
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
                tokio_util::codec::FramedWrite<
                    iroh::endpoint::SendStream,
                    tokio_util::codec::LengthDelimitedCodec,
                >,
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
                tokio_util::codec::FramedRead<
                    iroh::endpoint::RecvStream,
                    tokio_util::codec::LengthDelimitedCodec,
                >,
                tokio_util::codec::FramedWrite<
                    iroh::endpoint::SendStream,
                    tokio_util::codec::LengthDelimitedCodec,
                >,
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
                iroh::endpoint::SendStream,
                iroh::endpoint::RecvStream,
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
