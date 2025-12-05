use iroh::{
    discovery::dns::DnsDiscovery,
    endpoint::BindError,
    protocol::{DynProtocolHandler, Router, RouterBuilder},
    Endpoint, SecretKey,
};
use log::warn;
use std::time::Duration;
use thiserror::Error;
use tokio::{sync::oneshot, task::JoinError, time::timeout};

/// Errors that can occur when building an Iroh endpoint.
#[derive(Error, Debug)]
pub enum BuilderError {
    /// Failed to bind the Iroh endpoint to a network address.
    #[error("Failed to bind iroh endpoint")]
    BindError(#[from] BindError),
}

/// A handle used to signal that a shutdown subscriber has completed its cleanup.
pub struct ShutdownReplier {
    ok: tokio::sync::oneshot::Sender<()>,
}

impl ShutdownReplier {
    /// Signal that the shutdown cleanup is complete.
    ///
    /// This method consumes the replier and sends a completion signal.
    pub async fn complete(self) {
        let _ = self.ok.send(());
    }

    fn new() -> (oneshot::Receiver<()>, Self) {
        let (ok, rx) = oneshot::channel();
        (rx, Self { ok })
    }
}

/// A receiver that listens for shutdown notifications and provides a [`ShutdownReplier`] to signal completion.
pub type ShutdownListener = tokio::sync::oneshot::Receiver<ShutdownReplier>;

/// A collection of shutdown notification senders.
pub type ShutdownSubscribers = Vec<tokio::sync::oneshot::Sender<ShutdownReplier>>;

/// Builder for configuring an [`IrohBundle`] with protocol handlers and shutdown management.
///
/// Created via [`IrohBundle::builder()`]. Use this to register protocol handlers
/// before calling [`finish()`](Builder::finish) to create the bundle.
///
/// # Example
///
/// ```rust,no_run
/// use zel_core::IrohBundle;
/// use zel_core::protocol::RpcServerBuilder;
/// use std::time::Duration;
///
/// # async fn example() -> anyhow::Result<()> {
/// let mut builder = IrohBundle::builder(None).await?;
/// let endpoint = builder.endpoint().clone();
///
/// // Register shutdown subscriber if needed
/// let shutdown_rx = builder.subscribe_shutdown();
///
/// // Build and register server
/// let server = RpcServerBuilder::new(b"myapp/1", endpoint).build();
/// let bundle = builder.accept(b"myapp/1", server).finish().await;
///
/// // Later handle shutdown
/// tokio::spawn(async move {
///     if let Ok(replier) = shutdown_rx.await {
///         // Cleanup...
///         replier.complete().await;
///     }
/// });
/// # Ok(())
/// # }
/// ```
pub struct Builder {
    endpoint: Endpoint,
    router_builder: RouterBuilder,
    shutdown_subscribers: ShutdownSubscribers,
}

impl Builder {
    /// Finalize the builder and create an [`IrohBundle`].
    ///
    /// This spawns the router with all configured protocol handlers.
    pub async fn finish(self) -> IrohBundle {
        let Builder {
            endpoint,
            router_builder,
            shutdown_subscribers,
        } = self;
        let router = router_builder.spawn();

        IrohBundle {
            endpoint,
            router,
            shutdown_subscribers,
        }
    }

    /// Get a mutable reference to the underlying Iroh endpoint.
    pub fn endpoint(&mut self) -> &mut Endpoint {
        &mut self.endpoint
    }

    /// Get a mutable reference to the router builder.
    pub fn router_builder(&mut self) -> &mut RouterBuilder {
        &mut self.router_builder
    }

    /// Register a protocol handler for the given ALPN identifier.
    ///
    /// # Arguments
    /// * `alpn` - The ALPN (Application-Layer Protocol Negotiation) identifier
    /// * `handler` - The protocol handler to accept connections for this ALPN
    pub fn accept(
        mut self,
        alpn: impl AsRef<[u8]>,
        handler: impl Into<Box<dyn DynProtocolHandler>>,
    ) -> Self {
        self.router_builder = self.router_builder.accept(alpn, handler);
        self
    }

    /// Subscribe to shutdown notifications.
    ///
    /// Returns a [`ShutdownListener`] that will receive a [`ShutdownReplier`] when shutdown begins.
    /// The subscriber should call [`ShutdownReplier::complete()`] when cleanup is finished.
    pub fn subscribe_shutdown(&mut self) -> ShutdownListener {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.shutdown_subscribers.push(tx);
        rx
    }
}

/// A bundle containing an Iroh endpoint, router, and shutdown management.
///
/// `IrohBundle` manages the complete lifecycle of an Iroh networking stack,
/// including endpoint configuration, protocol routing, and coordinated graceful shutdown.
///
/// # Quick Start
///
/// ```rust,no_run
/// use zel_core::IrohBundle;
/// use zel_core::protocol::RpcServerBuilder;
/// use std::time::Duration;
///
/// # async fn example() -> anyhow::Result<()> {
/// // Create and configure bundle
/// let mut builder = IrohBundle::builder(None).await?;
/// let endpoint = builder.endpoint().clone();
///
/// // Build and register server
/// let server = RpcServerBuilder::new(b"myapp/1", endpoint).build();
/// let bundle = builder.accept(b"myapp/1", server).finish().await;
///
/// println!("Server listening at: {}", bundle.endpoint.id());
///
/// // ... run your application ...
///
/// // Graceful shutdown
/// bundle.shutdown(Duration::from_secs(30)).await?;
/// # Ok(())
/// # }
/// ```
///
/// # DNS Discovery
///
/// By default, `IrohBundle::builder()` configures the endpoint with n0 DNS discovery
/// for peer-to-peer connectivity. The endpoint can be further customized through the
/// [`Builder`] before calling [`Builder::finish()`].
///
/// # Shutdown Patterns
///
/// - Use [`shutdown()`](IrohBundle::shutdown) for simple cases - shuts down subscribers then the router
/// - Use [`shutdown_with_handle()`](IrohBundle::shutdown_with_handle) when coordinating RPC server shutdown
///
/// See [`Builder::subscribe_shutdown()`] to receive shutdown notifications in your components.
pub struct IrohBundle {
    /// The Iroh network endpoint.
    pub endpoint: Endpoint,
    /// The protocol router handling incoming connections.
    pub router: Router,
    /// The list of shutdown notification subscribers.
    pub shutdown_subscribers: ShutdownSubscribers,
}

impl IrohBundle {
    /// Create a new builder for configuring an [`IrohBundle`].
    /// By default this configures the endpoint with n0 DNS Discovery.
    ///
    /// # Arguments
    /// * `secret_key` - Optional secret key for the endpoint. If `None`, a new key is generated.
    ///
    /// # Errors
    /// Returns [`BuilderError`] if the endpoint fails to bind to a network address.
    pub async fn builder(secret_key: Option<SecretKey>) -> Result<Builder, BuilderError> {
        let mut endpoint = iroh::Endpoint::builder().discovery(DnsDiscovery::n0_dns());
        if let Some(secret_key) = secret_key {
            endpoint = endpoint.secret_key(secret_key);
        }

        let endpoint = endpoint.bind().await?;
        let router_builder = RouterBuilder::new(endpoint.clone());

        let shutdown_tx = vec![];
        Ok(Builder {
            endpoint,
            router_builder,
            shutdown_subscribers: shutdown_tx,
        })
    }

    /// Initiate graceful shutdown of the Iroh bundle.
    ///
    /// Notifies all subscribers that shutdown has begun and waits for them to complete
    /// their cleanup operations, up to the specified timeout. After the timeout or
    /// when all subscribers signal completion, the router is shut down.
    ///
    /// # Arguments
    /// * `timeout_after` - Maximum duration to wait for shutdown subscribers to complete
    ///
    /// # Errors
    /// Returns [`JoinError`] if the router shutdown task fails.
    ///
    /// # Note
    /// The router shutdown automatically closes the endpoint.
    pub async fn shutdown(self, timeout_after: Duration) -> Result<(), JoinError> {
        let IrohBundle {
            endpoint: _endpoint,
            router,
            shutdown_subscribers,
        } = self;
        let mut shutdown_replies = vec![];
        for sub in shutdown_subscribers {
            let (rx, replier) = ShutdownReplier::new();
            if sub.send(replier).is_ok() {
                shutdown_replies.push(async move {
                    // Should only error if the subscriber droped the receiver
                    // before this was called. Which, is a behavior we want to
                    // expect
                    let _ = rx.await;
                });
            }
        }

        let joined = futures::future::join_all(shutdown_replies);
        if timeout(timeout_after, joined).await.is_err() {
            warn!(
                "shutdown subscribers did not finalize after {}s",
                timeout_after.as_secs()
            )
        }

        // Router::shutdown closes the endpoint
        router.shutdown().await
    }

    /// Shutdown both RPC server and Iroh bundle in coordinated fashion
    ///
    /// This is a convenience method that coordinates shutdown of both the RPC server
    /// (using a shutdown handle) and the Iroh bundle.
    ///
    /// **Note:** Use [`RpcServerBuilder::build_with_shutdown()`](crate::protocol::RpcServerBuilder::build_with_shutdown)
    /// to get a shutdown handle that you can use after registering the server.
    ///
    /// # Arguments
    /// * `shutdown_handle` - The shutdown handle from `build_with_shutdown()`
    /// * `rpc_timeout` - Timeout for RPC server operations
    /// * `bundle_timeout` - Timeout for bundle shutdown subscribers
    ///
    /// # Example
    /// ```no_run
    /// use std::time::Duration;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # use zel_core::protocol::RpcServerBuilder;
    /// let mut bundle_builder = zel_core::IrohBundle::builder(None).await?;
    /// let endpoint = bundle_builder.endpoint().clone();
    ///
    /// // Build server with shutdown handle
    /// let (server, shutdown_handle) = RpcServerBuilder::new(b"test", endpoint)
    ///     .build_with_shutdown();
    ///
    /// // Register and finish
    /// let bundle = bundle_builder.accept(b"test", server).finish().await;
    ///
    /// // Later, coordinate shutdown
    /// bundle.shutdown_with_handle(
    ///     shutdown_handle,
    ///     Duration::from_secs(30),  // RPC timeout
    ///     Duration::from_secs(10),  // Bundle timeout
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn shutdown_with_handle(
        self,
        shutdown_handle: crate::protocol::ShutdownHandle,
        rpc_timeout: Duration,
        bundle_timeout: Duration,
    ) -> Result<(), anyhow::Error> {
        // 1. Shutdown RPC server first (allow requests to complete)
        log::info!("Shutting down RPC server...");
        shutdown_handle.shutdown(rpc_timeout).await?;

        // 2. Then shutdown IrohBundle (allow subscribers to cleanup)
        log::info!("Shutting down Iroh bundle...");
        self.shutdown(bundle_timeout).await?;

        log::info!("Complete shutdown finished");
        Ok(())
    }
}
