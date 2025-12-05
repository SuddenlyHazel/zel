use super::Extensions;
use iroh::endpoint::Connection;
use iroh::PublicKey;
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
        service: String,
        resource: String,
        server_extensions: Extensions,
        connection_extensions: Extensions,
        shutdown_signal: Arc<tokio::sync::Notify>,
    ) -> Self {
        Self {
            connection,
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
