//! Request context providing access to connection metadata and extensions.
//!
//! [`RequestContext`] is passed to all service handlers and provides access to:
//! - The underlying Iroh connection
//! - Connection type monitoring (direct vs relay)
//! - Three-tier extension system (server/connection/request)
//! - Remote peer information
//! - Graceful shutdown notifications
//!
//! See [`RequestContext`] for usage examples.

use super::Extensions;
use iroh::endpoint::Connection;
use iroh::{Endpoint, PublicKey, Watcher};
use std::sync::Arc;

/// Context provided to each RPC and subscription handler.
///
/// RequestContext provides access to connection metadata and three tiers of extensions:
/// - Server extensions: Shared across all connections
/// - Connection extensions: Scoped to a single connection
/// - Request extensions: Unique per request
///
/// Additionally, handlers can monitor for graceful shutdown notifications to
/// cleanly exit loops, flush pending data, and release resources.
///
/// This allows handlers to access shared resources (like database pools),
/// connection-specific state (like authentication sessions), and request-specific
/// data (like trace IDs).
#[derive(Clone)]
pub struct RequestContext {
    connection: Connection,
    endpoint: Endpoint,
    service: String,
    resource: String,
    server_extensions: Extensions,
    connection_extensions: Extensions,
    request_extensions: Extensions,
    shutdown_signal: Arc<tokio::sync::Notify>,
}

impl RequestContext {
    /// Create a new RequestContext.
    ///
    /// This is typically called by the connection handler for each incoming request.
    pub fn new(
        connection: Connection,
        endpoint: Endpoint,
        service: String,
        resource: String,
        server_extensions: Extensions,
        connection_extensions: Extensions,
        shutdown_signal: Arc<tokio::sync::Notify>,
    ) -> Self {
        Self {
            connection,
            endpoint,
            service,
            resource,
            server_extensions,
            connection_extensions,
            request_extensions: Extensions::new(),
            shutdown_signal,
        }
    }

    /// Get a reference to the underlying Iroh connection.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Get a watcher for the connection type (direct vs relay).
    ///
    /// The returned watcher provides real-time monitoring of how this connection is routed:
    /// - `Direct` - Direct UDP connection to peer
    /// - `Relay` - Routed through a relay server
    /// - `Mixed` - Both paths available
    /// - `None` - No path available
    ///
    /// Use this for adaptive behavior, monitoring, and debugging connectivity issues.
    ///
    /// # Example
    /// ```rust,ignore
    /// use iroh::Watcher;
    ///
    /// // Get current connection type
    /// if let Some(mut watcher) = ctx.connection_type() {
    ///     let conn_type = watcher.get();
    ///     log::info!("Connection type: {:?}", conn_type);
    /// }
    ///
    /// // Monitor for changes
    /// if let Some(watcher) = ctx.connection_type() {
    ///     use futures::StreamExt;
    ///     let mut stream = watcher.stream();
    ///     while let Some(conn_type) = stream.next().await {
    ///         log::info!("Connection type changed: {:?}", conn_type);
    ///     }
    /// }
    /// ```
    #[cfg(not(wasm_browser))]
    pub fn connection_type(&self) -> Option<impl Watcher + use<>> {
        self.endpoint.conn_type(self.connection.remote_id())
    }

    /// Get the remote peer's PublicKey.
    pub fn remote_id(&self) -> PublicKey {
        self.connection.remote_id()
    }

    /// Get the service name for this request.
    pub fn service(&self) -> &str {
        &self.service
    }

    /// Get the resource name for this request.
    pub fn resource(&self) -> &str {
        &self.resource
    }

    /// Get server-level extensions (shared across all connections).
    ///
    /// Use this to access resources like database pools, configuration, etc.
    pub fn server_extensions(&self) -> &Extensions {
        &self.server_extensions
    }

    /// Get connection-level extensions (scoped to this connection).
    ///
    /// Use this to access connection-specific state like authentication sessions.
    pub fn connection_extensions(&self) -> &Extensions {
        &self.connection_extensions
    }

    /// Get request-level extensions (unique to this request).
    ///
    /// Use this to access request-specific data like trace IDs, request timing, etc.
    pub fn extensions(&self) -> &Extensions {
        &self.request_extensions
    }

    /// Add a request-level extension, returning a new RequestContext.
    ///
    /// This is useful for adding request-specific data like trace IDs.
    pub fn with_extension<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        self.request_extensions = self.request_extensions.with(value);
        self
    }

    /// Wait for shutdown notification (async).
    ///
    /// Use this in `tokio::select!` to react to graceful shutdown:
    ///
    /// ```rust,ignore
    /// loop {
    ///     tokio::select! {
    ///         _ = interval.tick() => {
    ///             // Normal operation
    ///         }
    ///         _ = ctx.shutdown_notified() => {
    ///             log::info!("Shutdown detected, exiting cleanly");
    ///             break;
    ///         }
    ///     }
    /// }
    /// ```
    pub async fn shutdown_notified(&self) {
        self.shutdown_signal.notified().await
    }

    /// Get the current round-trip time (RTT) estimate for this connection.
    ///
    /// This provides the current best estimate of the connection's latency.
    /// Useful for adaptive behavior, monitoring, and debugging.
    ///
    /// # Example
    /// ```rust,ignore
    /// let rtt = ctx.connection_rtt();
    /// if rtt > Duration::from_millis(100) {
    ///     log::warn!("High latency detected: {:?}", rtt);
    /// }
    /// ```
    pub fn connection_rtt(&self) -> std::time::Duration {
        self.connection.rtt()
    }

    /// Get detailed statistics for this connection.
    ///
    /// Returns comprehensive metrics including:
    /// - UDP statistics (packets sent/received, bytes transferred)
    /// - Path statistics (congestion window, RTT variance)
    /// - Frame statistics (sent/received frame counts by type)
    ///
    /// Useful for monitoring, debugging, and performance analysis.
    ///
    /// # Example
    /// ```rust,ignore
    /// let stats = ctx.connection_stats();
    /// log::info!(
    ///     "Connection stats - RTT: {:?}, sent: {} bytes, lost: {} packets",
    ///     stats.path.rtt,
    ///     stats.udp.datagrams_sent,
    ///     stats.path.lost_packets
    /// );
    /// ```
    pub fn connection_stats(&self) -> iroh::endpoint::ConnectionStats {
        self.connection.stats()
    }
}

impl std::fmt::Debug for RequestContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestContext")
            .field("service", &self.service)
            .field("resource", &self.resource)
            .field("remote_id", &self.remote_id())
            .finish()
    }
}
