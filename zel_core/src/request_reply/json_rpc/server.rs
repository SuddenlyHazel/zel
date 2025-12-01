//! Server support for JSON-RPC over Iroh

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use jsonrpsee::core::middleware::RpcServiceT;
use jsonrpsee::core::server::{MethodResponse, Methods};
use log::warn;
use tokio::task::JoinSet;
use tower::Layer;

use crate::request_reply::json_rpc::ConnectionExt;
use crate::request_reply::json_rpc::errors::{BuildError, IrohTransportError};
use crate::request_reply::json_rpc::iroh_service::IrohRpcService;
use crate::request_reply::json_rpc::transport::{
    DEFAULT_MAX_REQUEST_SIZE, DEFAULT_MAX_RESPONSE_SIZE, read_cobs_frame, write_cobs_frame,
};
use crate::request_reply::json_rpc::utils::{BatchRequestConfig, handle_rpc_call};

// Make RpcModule public for server building
pub use jsonrpsee::core::server::RpcModule;

/// Handler for JSON-RPC connections over Iroh
///
/// Implements [`ProtocolHandler`] to integrate with Iroh's protocol routing.
/// Register this handler with your Iroh endpoint using a specific ALPN.
///
/// # Example
///
/// ```no_run
/// use zel_core::request_reply::json_rpc::JsonRpcHandler;
/// use zel_core::IrohBundle;
/// use jsonrpsee::core::server::RpcModule;
///
/// # async fn example() -> anyhow::Result<()> {
/// // Build RPC module with methods
/// let mut module = RpcModule::new(());
/// module.register_method("say_hello", |_, _, _| Ok::<_, jsonrpsee::types::ErrorObjectOwned>("Hello!"))?;
///
/// // Create handler
/// let handler = JsonRpcHandler::new(module.into());
///
/// // Register with Iroh
/// let bundle = IrohBundle::builder(None)
///     .await?
///     .accept(b"jsonrpc/1", handler)
///     .finish()
///     .await;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct JsonRpcHandler<L = tower::layer::util::Identity> {
    methods: Methods,
    max_request_size: usize,
    max_response_size: usize,
    layer: L,
}

impl JsonRpcHandler<tower::layer::util::Identity> {
    /// Create a new JSON-RPC handler with default size limits and no middleware
    ///
    /// # Arguments
    ///
    /// * `methods` - The RPC methods this handler will serve
    pub fn new(methods: Methods) -> Self {
        Self {
            methods,
            max_request_size: DEFAULT_MAX_REQUEST_SIZE,
            max_response_size: DEFAULT_MAX_RESPONSE_SIZE,
            layer: tower::layer::util::Identity::new(),
        }
    }
}

impl<L: Clone> JsonRpcHandler<L> {
    /// Create a handler with custom size limits and middleware layer
    ///
    /// # Arguments
    ///
    /// * `methods` - The RPC methods this handler will serve
    /// * `max_request_size` - Maximum incoming request size in bytes
    /// * `max_response_size` - Maximum outgoing response size in bytes
    /// * `layer` - Tower middleware layer to apply
    pub fn with_limits(
        methods: Methods,
        max_request_size: usize,
        max_response_size: usize,
        layer: L,
    ) -> Self {
        Self {
            methods,
            max_request_size,
            max_response_size,
            layer,
        }
    }
}

impl<L> std::fmt::Debug for JsonRpcHandler<L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonRpcHandler")
            .field("max_request_size", &self.max_request_size)
            .field("max_response_size", &self.max_response_size)
            .field("methods_count", &self.methods.method_names().count())
            .finish()
    }
}

impl<L> ProtocolHandler for JsonRpcHandler<L>
where
    L: Layer<IrohRpcService> + Clone + Send + Sync + 'static,
    L::Service: RpcServiceT<
            MethodResponse = MethodResponse,
            BatchResponse = MethodResponse,
            NotificationResponse = MethodResponse,
        > + Clone
        + Send
        + Sync
        + 'static,
{
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer_id = connection.remote_id();
        let methods = self.methods.clone();
        let max_req = self.max_request_size;
        let max_resp = self.max_response_size;
        let layer = self.layer.clone();

        log::info!("Accepted JSON-RPC connection from peer {peer_id}");

        tokio::spawn(async move {
            if let Err(e) =
                handle_jsonrpc_connection(connection, methods, max_req, max_resp, layer).await
            {
                log::error!("JSON-RPC connection error for peer {peer_id}: {e}");
            }
        });

        Ok(())
    }
}

/// Handle a single JSON-RPC connection with subscription support and middleware
///
/// This function processes requests from a client in a loop until the connection
/// closes or an error occurs. Subscriptions are handled by spawning a forwarder
/// task for each subscription. The layer parameter allows tower middleware to be
/// applied to all requests.
async fn handle_jsonrpc_connection<L>(
    connection: Connection,
    mut methods: Methods,
    max_request_size: usize,
    max_response_size: usize,
    layer: L,
) -> Result<(), IrohTransportError>
where
    L: Layer<IrohRpcService>,
    L::Service: RpcServiceT<
            MethodResponse = MethodResponse,
            BatchResponse = MethodResponse,
            NotificationResponse = MethodResponse,
        > + Clone
        + Send
        + Sync
        + 'static,
{
    // Accept bidirectional stream
    let (mut send, mut recv) = connection.accept_bi().await.map_err(|e| {
        IrohTransportError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to accept bi stream: {}", e),
        ))
    })?;

    // Insert connection extension for RPC methods to access
    if let Some(existing) = methods
        .extensions_mut()
        .insert(ConnectionExt::new(connection.clone()))
    {
        let existing = existing.peer();
        warn!(
            "methods extension already contained peer: {existing} for ConnectionExt peer: {}",
            connection.remote_id()
        )
    }

    // Create IrohRpcService and get subscription receiver
    let (iroh_service, mut subscription_rx) = IrohRpcService::new(
        methods,
        max_response_size,
        100, // max subscriptions
        connection.clone(),
        b"jsonrpc/1", // ALPN - TODO: make this configurable
    );

    // Wrap service with middleware layer
    let service = layer.layer(iroh_service);

    // Create extensions with connection context
    let mut extensions = jsonrpsee::Extensions::new();
    extensions.insert(ConnectionExt::new(connection.clone()));

    // Track active subscription forwarder tasks
    let mut subscription_tasks = JoinSet::new();

    // Process requests in a loop
    loop {
        tokio::select! {
            // Handle incoming requests
            frame_result = read_cobs_frame(&mut recv, max_request_size) => {
                let data = match frame_result {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        log::error!("Connection closed or error reading frame: {e}");
                        break;
                    }
                };

                // Detect request type by inspecting first non-whitespace byte
                let first_non_whitespace = data.iter()
                    .enumerate()
                    .take(128)
                    .find(|(_, byte)| !byte.is_ascii_whitespace());

                let (idx, is_single) = match first_non_whitespace {
                    Some((start, b'{')) => (start, true),
                    Some((start, b'[')) => (start, false),
                    _ => {
                        // Parse error - send error response
                        let error_response = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}"#;
                        if let Err(e) = write_cobs_frame(&mut send, error_response.as_bytes()).await {
                            log::error!("Failed to send error response: {e}");
                            break;
                        }
                        continue;
                    }
                };

                // Dispatch through handle_rpc_call (middleware is invoked!)
                let response = handle_rpc_call(
                    &data[idx..],
                    is_single,
                    BatchRequestConfig::Unlimited,
                    &service,
                    extensions.clone(),
                ).await;

                // Convert MethodResponse to JSON string, then bytes
                let response_str: &str = response.as_ref();

                // Send response
                if let Err(e) = write_cobs_frame(&mut send, response_str.as_bytes()).await {
                    log::error!("Failed to send response: {e}");
                    break;
                }
            }

            // Forward subscription notifications
            Some(notification) = subscription_rx.recv() => {
                if let Err(e) = write_cobs_frame(&mut send, notification.get().as_bytes()).await {
                    log::error!("Failed to send subscription notification: {e}");
                    continue;
                }
            }

            // Check for completed subscription tasks
            Some(task_result) = subscription_tasks.join_next(), if !subscription_tasks.is_empty() => {
                match task_result {
                    Ok(Ok(())) => {
                        log::info!("Subscription task completed successfully");
                    }
                    Ok(Err(e)) => {
                        log::error!("Subscription task failed: {e}");
                        // Close entire connection on subscription error
                        return Err(e);
                    }
                    Err(e) => {
                        log::error!("Subscription task panicked: {e}");
                    }
                }
            }
        }
    }

    // Cleanup: abort all remaining subscription tasks
    log::debug!(
        "Connection closing, aborting {} subscription tasks",
        subscription_tasks.len()
    );
    subscription_tasks.abort_all();

    Ok(())
}

/// Builder for creating a JSON-RPC server
///
/// Provides a fluent API for configuring and building JSON-RPC servers over Iroh.
///
/// # Example
///
/// ```no_run
/// use zel_core::request_reply::json_rpc::ServerBuilder;
/// use jsonrpsee::core::server::RpcModule;
///
/// # async fn example() -> anyhow::Result<()> {
/// let mut module = RpcModule::new(());
/// module.register_method("add", |params, _, _| {
///     let params: (u64, u64) = params.parse()?;
///     Ok::<_, jsonrpsee::types::ErrorObjectOwned>(params.0 + params.1)
/// })?;
///
/// let server = ServerBuilder::new()
///     .max_request_size(5 * 1024 * 1024)
///     .build(module)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct ServerBuilder<L = tower::layer::util::Identity> {
    max_request_size: usize,
    max_response_size: usize,
    layer: L,
}

impl Default for ServerBuilder<tower::layer::util::Identity> {
    fn default() -> Self {
        Self {
            max_request_size: DEFAULT_MAX_REQUEST_SIZE,
            max_response_size: DEFAULT_MAX_RESPONSE_SIZE,
            layer: tower::layer::util::Identity::new(),
        }
    }
}

impl ServerBuilder<tower::layer::util::Identity> {
    /// Create a new server builder with default settings and no middleware
    pub fn new() -> Self {
        Self::default()
    }
}

impl<L> ServerBuilder<L> {
    /// Set the maximum request size in bytes
    ///
    /// Default: 10 MB
    pub fn max_request_size(mut self, size: usize) -> Self {
        self.max_request_size = size;
        self
    }

    /// Set the maximum response size in bytes
    ///
    /// Default: 10 MB
    pub fn max_response_size(mut self, size: usize) -> Self {
        self.max_response_size = size;
        self
    }

    /// Apply a tower middleware layer
    ///
    /// This allows you to add middleware for authentication, logging, rate limiting,
    /// and other cross-cutting concerns. Layers are applied in the order they are added.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use tower::ServiceBuilder;
    ///
    /// let server = ServerBuilder::new()
    ///     .layer(
    ///         ServiceBuilder::new()
    ///             .layer(LoggingLayer)
    ///             .layer(AuthLayer)
    ///             .into_inner()
    ///     )
    ///     .build(module)?;
    /// ```
    pub fn layer<NewLayer>(
        self,
        layer: NewLayer,
    ) -> ServerBuilder<tower::layer::util::Stack<NewLayer, L>> {
        ServerBuilder {
            max_request_size: self.max_request_size,
            max_response_size: self.max_response_size,
            layer: tower::layer::util::Stack::new(layer, self.layer),
        }
    }

    /// Build the server handler
    ///
    /// # Arguments
    ///
    /// * `module` - The RPC module containing your methods
    ///
    /// # Returns
    ///
    /// A [`JsonRpcHandler`] ready to be registered with an Iroh endpoint
    pub fn build<T>(self, module: RpcModule<T>) -> Result<JsonRpcHandler<L>, BuildError>
    where
        L: Clone,
    {
        Ok(JsonRpcHandler::with_limits(
            module.into(),
            self.max_request_size,
            self.max_response_size,
            self.layer,
        ))
    }
}
