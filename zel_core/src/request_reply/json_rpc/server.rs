//! Server support for JSON-RPC over Iroh

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use jsonrpsee::core::client::{ReceivedMessage, TransportReceiverT, TransportSenderT};
use jsonrpsee::core::server::Methods;
use log::warn;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::request_reply::json_rpc::ConnectionExt;
use crate::request_reply::json_rpc::errors::{BuildError, IrohTransportError};
use crate::request_reply::json_rpc::transport::{
    DEFAULT_MAX_REQUEST_SIZE, DEFAULT_MAX_RESPONSE_SIZE, IrohSender, accept_connection,
};

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
pub struct JsonRpcHandler {
    methods: Methods,
    max_request_size: usize,
    max_response_size: usize,
}

impl JsonRpcHandler {
    /// Create a new JSON-RPC handler with default size limits
    ///
    /// # Arguments
    ///
    /// * `methods` - The RPC methods this handler will serve
    pub fn new(methods: Methods) -> Self {
        Self {
            methods,
            max_request_size: DEFAULT_MAX_REQUEST_SIZE,
            max_response_size: DEFAULT_MAX_RESPONSE_SIZE,
        }
    }

    /// Create a handler with custom size limits
    ///
    /// # Arguments
    ///
    /// * `methods` - The RPC methods this handler will serve
    /// * `max_request_size` - Maximum incoming request size in bytes
    /// * `max_response_size` - Maximum outgoing response size in bytes
    pub fn with_limits(
        methods: Methods,
        max_request_size: usize,
        max_response_size: usize,
    ) -> Self {
        Self {
            methods,
            max_request_size,
            max_response_size,
        }
    }
}

impl std::fmt::Debug for JsonRpcHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonRpcHandler")
            .field("max_request_size", &self.max_request_size)
            .field("max_response_size", &self.max_response_size)
            .field("methods_count", &self.methods.method_names().count())
            .finish()
    }
}

impl ProtocolHandler for JsonRpcHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer_id = connection.remote_id();
        let methods = self.methods.clone();
        let max_req = self.max_request_size;
        let max_resp = self.max_response_size;

        log::info!("Accepted JSON-RPC connection from peer {peer_id}");

        tokio::spawn(async move {
            if let Err(e) = handle_jsonrpc_connection(connection, methods, max_req, max_resp).await
            {
                log::error!("JSON-RPC connection error for peer {peer_id}: {e}");
            }
        });

        Ok(())
    }
}

/// Forward subscription notifications from jsonrpsee to the client
///
/// This task listens on the subscription receiver and forwards each
/// notification to the client through the Iroh transport.
async fn forward_subscription_notifications(
    mut rx: mpsc::Receiver<Box<serde_json::value::RawValue>>,
    mut sender: IrohSender,
    sub_id: String,
) -> Result<(), IrohTransportError> {
    log::debug!("Subscription forwarder started for {}", sub_id);

    while let Some(notification) = rx.recv().await {
        // Convert Box<RawValue> to String
        let notification_str = notification.get().to_string();
        log::trace!(
            "Forwarding subscription notification for {}: {}",
            sub_id,
            notification_str
        );

        if let Err(e) = sender.send(notification_str).await {
            log::error!(
                "Failed to send subscription notification for {}: {}",
                sub_id,
                e
            );
            return Err(e);
        }
    }

    log::debug!("Subscription forwarder completed for {}", sub_id);
    Ok(())
}

/// Extract subscription ID from response for logging
fn extract_subscription_id(response: &serde_json::value::RawValue) -> String {
    serde_json::from_str::<serde_json::Value>(response.get())
        .ok()
        .and_then(|v| {
            v.get("result")
                .and_then(|r| r.as_str().map(|s| s.to_string()))
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Handle a single JSON-RPC connection with subscription support
///
/// This function processes requests from a client in a loop until the connection
/// closes or an error occurs. Subscriptions are handled by spawning a forwarder
/// task for each subscription.
async fn handle_jsonrpc_connection(
    connection: Connection,
    mut methods: Methods,
    max_request_size: usize,
    max_response_size: usize,
) -> Result<(), IrohTransportError> {
    // Accept connection and create transport
    let (mut sender, mut receiver) =
        accept_connection(&connection, max_request_size, max_response_size).await?;

    // Track active subscription forwarder tasks
    let mut subscription_tasks = JoinSet::new();
    const MAX_SUBSCRIPTIONS_PER_CONNECTION: usize = 100;

    if let Some(existing) = methods
        .extensions_mut()
        .insert(ConnectionExt::new(connection.remote_id()))
    {
        let existing = existing.peer();
        warn!(
            "methods extention already contained peer: {existing} for ConnectionExt peer: {}",
            connection.remote_id()
        )
    }
    // Process requests in a loop
    loop {
        tokio::select! {
            // Handle incoming requests
            request_result = receiver.receive() => {
                let request = match request_result {
                    Ok(ReceivedMessage::Text(msg)) => msg,
                    Ok(ReceivedMessage::Bytes(_)) => {
                        log::warn!("Received unexpected binary message, ignoring");
                        continue;
                    }
                    Ok(ReceivedMessage::Pong) => {
                        log::debug!("Received pong");
                        continue;
                    }
                    Err(e) => {
                        log::debug!("Connection closed or error receiving message: {e}");
                        break;
                    }
                };

                // Process request with jsonrpsee
                let response = match methods.raw_json_request(&request, 1024).await {
                    Ok((response, rx)) => {
                        // Check subscription limit first
                        if subscription_tasks.len() >= MAX_SUBSCRIPTIONS_PER_CONNECTION {
                            log::error!(
                                "Subscription limit reached ({} active subscriptions)",
                                MAX_SUBSCRIPTIONS_PER_CONNECTION
                            );
                            // Send error response
                            format!(
                                r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32000,"message":"Too many active subscriptions"}}}}"#
                            )
                        } else {
                            // Extract subscription ID for logging
                            let sub_id = extract_subscription_id(&response);
                            log::info!("New subscription created: {}", sub_id);

                            // Clone sender for the forwarder task
                            let sender_clone = sender.clone();
                            let sub_id_clone = sub_id.clone();

                            // ALWAYS spawn forwarder task when we have an rx channel
                            // The presence of rx indicates this is a subscription
                            subscription_tasks.spawn(async move {
                                forward_subscription_notifications(rx, sender_clone, sub_id_clone).await
                            });

                            // Send initial response (subscription ID)
                            response.get().to_string()
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to process request: {e}");
                        // Return error response
                        format!(
                            r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"Internal error"}}}}"#
                        )
                    }
                };

                // Send response
                if let Err(e) = sender.send(response).await {
                    log::error!("Failed to send response: {e}");
                    break;
                }
            }

            // Check for completed subscription tasks
            Some(task_result) = subscription_tasks.join_next(), if !subscription_tasks.is_empty() => {
                match task_result {
                    Ok(Ok(())) => {
                        log::debug!("Subscription task completed successfully");
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
pub struct ServerBuilder {
    max_request_size: usize,
    max_response_size: usize,
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self {
            max_request_size: DEFAULT_MAX_REQUEST_SIZE,
            max_response_size: DEFAULT_MAX_RESPONSE_SIZE,
        }
    }
}

impl ServerBuilder {
    /// Create a new server builder with default settings
    pub fn new() -> Self {
        Self::default()
    }

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

    /// Build the server handler
    ///
    /// # Arguments
    ///
    /// * `module` - The RPC module containing your methods
    ///
    /// # Returns
    ///
    /// A [`JsonRpcHandler`] ready to be registered with an Iroh endpoint
    pub fn build<T>(self, module: RpcModule<T>) -> Result<JsonRpcHandler, BuildError> {
        Ok(JsonRpcHandler::with_limits(
            module.into(),
            self.max_request_size,
            self.max_response_size,
        ))
    }
}
