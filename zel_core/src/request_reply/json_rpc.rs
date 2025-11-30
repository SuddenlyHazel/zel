//! JSON-RPC 2.0 transport implementation over Iroh
//!
//! This is an **alternative** to the existing request/reply implementation.
//! Use this when you need JSON-RPC 2.0 compatibility, batch requests,
//! subscriptions, or standard JSON-RPC middleware.
//!
//! For simple request/reply, continue using [`crate::request_reply::Client`].
//!
//! # Example
//!
//! ```no_run
//! use zel_core::request_reply::json_rpc::{build_client, ClientT};
//! use jsonrpsee::core::client::ClientT as _;
//!
//! # async fn example(endpoint: iroh::Endpoint, peer: iroh::PublicKey) -> anyhow::Result<()> {
//! let client = build_client(&endpoint, peer, b"jsonrpc/1").await?;
//! // Use rpc_params! macro for parameters
//! let result: String = client.request("say_hello", jsonrpsee::core::client::__reexports::ArrayParams::new()).await?;
//! # Ok(())
//! # }
//! ```

use std::io;
use std::time::Duration;

use iroh::endpoint::{ConnectError, ConnectionError, RecvStream, SendStream};
use iroh::{Endpoint, PublicKey};
use jsonrpsee::core::client::async_client::{Client, ClientBuilder};
use jsonrpsee::core::client::{IdKind, ReceivedMessage, TransportReceiverT, TransportSenderT};
use log::error;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur during Iroh JSON-RPC transport operations
#[derive(Debug, Error)]
pub enum IrohTransportError {
    /// IO error occurred during stream operations
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// COBS encoding failed
    #[error("COBS encoding error: {0}")]
    CobsEncode(String),

    /// COBS decoding failed  
    #[error("COBS decoding error")]
    CobsDecode,

    /// Message exceeds size limit
    #[error("Message too large: {size} bytes exceeds maximum {max} bytes")]
    MessageTooLarge {
        /// Actual message size
        size: usize,
        /// Maximum allowed size
        max: usize,
    },

    /// UTF-8 decoding failed
    #[error("UTF-8 decode error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

// ============================================================================
// Build Error
// ============================================================================

/// Errors that can occur while building the transport
#[derive(Debug, Error)]
pub enum BuildError {
    /// Failed to connect to peer
    #[error("Failed to connect to peer: {0}")]
    Connect(#[from] ConnectError),

    /// Connection error
    #[error("Connection error: {0}")]
    Connection(#[from] ConnectionError),
}

// ============================================================================
// Transport Implementation
// ============================================================================

/// JSON-RPC sender over Iroh streams using COBS framing
///
/// Implements [`TransportSenderT`] to send JSON-RPC messages over Iroh.
/// Uses COBS (Consistent Overhead Byte Stuffing) for message framing.
pub struct IrohSender {
    send: SendStream,
    max_request_size: usize,
    buffer: Vec<u8>,
}

impl IrohSender {
    fn new(send: SendStream, max_request_size: usize) -> Self {
        Self {
            send,
            max_request_size,
            buffer: Vec::with_capacity(max_request_size),
        }
    }
}

impl TransportSenderT for IrohSender {
    type Error = IrohTransportError;

    async fn send(&mut self, msg: String) -> Result<(), Self::Error> {
        let len = msg.len();
        if len > self.max_request_size {
            error!(
                "Length {len} larger than max_request_size {}",
                self.max_request_size
            );
            return Err(IrohTransportError::MessageTooLarge {
                size: len,
                max: self.max_request_size,
            });
        }

        // COBS encode with 0x00 delimiter
        self.buffer.clear();
        self.buffer.resize(msg.len() + (msg.len() / 254) + 2, 0);
        match cobs::try_encode(msg.as_bytes(), &mut self.buffer) {
            Ok(encoded_len) => {
                self.buffer.truncate(encoded_len);
            }
            Err(e) => {
                error!("dest buffer too small {e}");
                return Err(IrohTransportError::CobsEncode(e.to_string()));
            }
        }
        self.buffer.push(0x00); // Frame delimiter

        // Write to stream
        self.send
            .write_all(&self.buffer)
            .await
            .map_err(|e| IrohTransportError::Io(io::Error::new(io::ErrorKind::Other, e)))?;
        self.send
            .flush()
            .await
            .map_err(|e| IrohTransportError::Io(io::Error::new(io::ErrorKind::Other, e)))?;

        Ok(())
    }

    async fn send_ping(&mut self) -> Result<(), Self::Error> {
        // Iroh handles connection keep-alive internally
        Ok(())
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.send.finish().map_err(|e| {
            IrohTransportError::Io(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to finish stream: {}", e),
            ))
        })?;
        Ok(())
    }
}

/// JSON-RPC receiver over Iroh streams using COBS framing
///
/// Implements [`TransportReceiverT`] to receive JSON-RPC messages from Iroh.
/// Uses COBS (Consistent Overhead Byte Stuffing) for message framing.
pub struct IrohReceiver {
    recv: RecvStream,
    max_response_size: usize,
    buffer: Vec<u8>,
}

impl IrohReceiver {
    fn new(recv: RecvStream, max_response_size: usize) -> Self {
        Self {
            recv,
            max_response_size,
            buffer: Vec::with_capacity(max_response_size),
        }
    }
}

impl TransportReceiverT for IrohReceiver {
    type Error = IrohTransportError;

    async fn receive(&mut self) -> Result<ReceivedMessage, Self::Error> {
        // Read until 0x00 delimiter (COBS frame boundary)
        self.buffer.clear();
        let mut byte = [0u8; 1];

        loop {
            self.recv
                .read_exact(&mut byte)
                .await
                .map_err(|e| IrohTransportError::Io(io::Error::new(io::ErrorKind::Other, e)))?;

            if byte[0] == 0x00 {
                break; // Frame complete
            }

            self.buffer.push(byte[0]);

            if self.buffer.len() > self.max_response_size {
                return Err(IrohTransportError::MessageTooLarge {
                    size: self.buffer.len(),
                    max: self.max_response_size,
                });
            }
        }

        // COBS decode
        let mut decoded = vec![0u8; self.buffer.len()];
        let len =
            cobs::decode(&self.buffer, &mut decoded).map_err(|_| IrohTransportError::CobsDecode)?;
        decoded.truncate(len);

        // UTF-8 decode
        let msg = String::from_utf8(decoded)?;

        Ok(ReceivedMessage::Text(msg))
    }
}

// ============================================================================
// Transport Builder
// ============================================================================

/// Default maximum request size (10 MB)
pub const DEFAULT_MAX_REQUEST_SIZE: usize = 10 * 1024 * 1024;

/// Default maximum response size (10 MB)
pub const DEFAULT_MAX_RESPONSE_SIZE: usize = 10 * 1024 * 1024;

/// Builder for creating Iroh JSON-RPC transport
///
/// # Example
///
/// ```no_run
/// use zel_core::request_reply::json_rpc::IrohTransportBuilder;
///
/// # async fn example(endpoint: iroh::Endpoint, peer: iroh::PublicKey) -> Result<(), Box<dyn std::error::Error>> {
/// let (sender, receiver) = IrohTransportBuilder::new()
///     .max_request_size(5 * 1024 * 1024)  // 5MB
///     .build(&endpoint, peer, b"my-rpc")
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct IrohTransportBuilder {
    max_request_size: usize,
    max_response_size: usize,
}

impl Default for IrohTransportBuilder {
    fn default() -> Self {
        Self {
            max_request_size: DEFAULT_MAX_REQUEST_SIZE,
            max_response_size: DEFAULT_MAX_RESPONSE_SIZE,
        }
    }
}

impl IrohTransportBuilder {
    /// Create a new transport builder with default settings
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

    /// Build the transport by connecting to a peer
    ///
    /// This method:
    /// 1. Connects to the specified peer using the ALPN
    /// 2. Opens a bidirectional stream
    /// 3. Creates sender and receiver with COBS framing
    ///
    /// # Arguments
    ///
    /// * `endpoint` - The Iroh endpoint to use for connecting
    /// * `peer` - The public key of the peer to connect to
    /// * `alpn` - The ALPN (Application-Layer Protocol Negotiation) identifier
    pub async fn build(
        self,
        endpoint: &Endpoint,
        peer: PublicKey,
        alpn: &[u8],
    ) -> Result<(IrohSender, IrohReceiver), BuildError> {
        // Connect to peer
        let conn = endpoint.connect(peer, alpn).await?;

        // Open bidirectional stream
        let (send, recv) = conn.open_bi().await?;

        let sender = IrohSender::new(send, self.max_request_size);
        let receiver = IrohReceiver::new(recv, self.max_response_size);

        Ok((sender, receiver))
    }
}

// ============================================================================
// Client Builder (Convenience API)
// ============================================================================

/// Create a jsonrpsee client over Iroh with default settings
///
/// This is a convenience function that creates a client with sensible defaults.
/// For more control, use [`IrohClientBuilder`].
///
/// # Example
///
/// ```no_run
/// use zel_core::request_reply::json_rpc::build_client;
/// use jsonrpsee::core::client::ClientT;
///
/// # async fn example(endpoint: iroh::Endpoint, peer: iroh::PublicKey) -> anyhow::Result<()> {
/// let client = build_client(&endpoint, peer, b"my-rpc").await?;
/// # Ok(())
/// # }
/// ```
pub async fn build_client(
    endpoint: &Endpoint,
    peer: PublicKey,
    alpn: &[u8],
) -> Result<Client, BuildError> {
    IrohClientBuilder::default()
        .build(endpoint, peer, alpn)
        .await
}

/// Builder for customized Iroh JSON-RPC client
///
/// Provides fine-grained control over both transport and client settings.
///
/// # Example
///
/// ```no_run
/// use zel_core::request_reply::json_rpc::IrohClientBuilder;
/// use std::time::Duration;
///
/// # async fn example(endpoint: iroh::Endpoint, peer: iroh::PublicKey) -> Result<(), Box<dyn std::error::Error>> {
/// let client = IrohClientBuilder::new()
///     .max_request_size(5 * 1024 * 1024)  // 5MB
///     .request_timeout(Duration::from_secs(10))
///     .max_concurrent_requests(128)
///     .build(&endpoint, peer, b"my-rpc")
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct IrohClientBuilder {
    transport: IrohTransportBuilder,
    request_timeout: Duration,
    max_concurrent_requests: usize,
    max_buffer_capacity_per_subscription: usize,
    id_kind: IdKind,
}

impl Default for IrohClientBuilder {
    fn default() -> Self {
        Self {
            transport: IrohTransportBuilder::default(),
            request_timeout: Duration::from_secs(60),
            max_concurrent_requests: 256,
            max_buffer_capacity_per_subscription: 1024,
            id_kind: IdKind::Number,
        }
    }
}

impl IrohClientBuilder {
    /// Create a new client builder with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum request size in bytes
    ///
    /// Default: 10 MB
    pub fn max_request_size(mut self, size: usize) -> Self {
        self.transport = self.transport.max_request_size(size);
        self
    }

    /// Set the maximum response size in bytes
    ///
    /// Default: 10 MB
    pub fn max_response_size(mut self, size: usize) -> Self {
        self.transport = self.transport.max_response_size(size);
        self
    }

    /// Set the request timeout
    ///
    /// Default: 60 seconds
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Set the maximum number of concurrent requests
    ///
    /// Default: 256
    pub fn max_concurrent_requests(mut self, max: usize) -> Self {
        self.max_concurrent_requests = max;
        self
    }

    /// Set the maximum buffer capacity per subscription
    ///
    /// When the capacity is exceeded, the subscription will be dropped.
    ///
    /// Default: 1024
    pub fn max_buffer_capacity_per_subscription(mut self, max: usize) -> Self {
        self.max_buffer_capacity_per_subscription = max;
        self
    }

    /// Set the ID format for JSON-RPC requests
    ///
    /// Default: [`IdKind::Number`]
    pub fn id_format(mut self, id_kind: IdKind) -> Self {
        self.id_kind = id_kind;
        self
    }

    /// Build the client by connecting to a peer
    ///
    /// This method:
    /// 1. Creates the Iroh transport
    /// 2. Configures the jsonrpsee client
    /// 3. Returns a ready-to-use JSON-RPC client
    ///
    /// # Arguments
    ///
    /// * `endpoint` - The Iroh endpoint to use for connecting
    /// * `peer` - The public key of the peer to connect to
    /// * `alpn` - The ALPN identifier for the JSON-RPC protocol
    pub async fn build(
        self,
        endpoint: &Endpoint,
        peer: PublicKey,
        alpn: &[u8],
    ) -> Result<Client, BuildError> {
        let (sender, receiver) = self.transport.build(endpoint, peer, alpn).await?;

        let client = ClientBuilder::default()
            .request_timeout(self.request_timeout)
            .max_concurrent_requests(self.max_concurrent_requests)
            .max_buffer_capacity_per_subscription(self.max_buffer_capacity_per_subscription)
            .id_format(self.id_kind)
            .build_with_tokio(sender, receiver);

        Ok(client)
    }
}

// ============================================================================
// Re-exports for convenience
// ============================================================================

// ============================================================================
// Server Support
// ============================================================================

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use jsonrpsee::core::server::Methods;

// Make RpcModule public for server building
pub use jsonrpsee::core::server::RpcModule;

/// Accept an incoming connection and create transports for server-side handling
///
/// This is the server-side equivalent of [`IrohTransportBuilder::build`].
/// It accepts a bidirectional stream from the client and creates sender/receiver.
///
/// # Arguments
///
/// * `connection` - The incoming Iroh connection
/// * `max_request_size` - Maximum size for incoming requests
/// * `max_response_size` - Maximum size for outgoing responses
pub async fn accept_connection(
    connection: &Connection,
    max_request_size: usize,
    max_response_size: usize,
) -> Result<(IrohSender, IrohReceiver), IrohTransportError> {
    // Server accepts the bidirectional stream that client opened
    let (send, recv) = connection.accept_bi().await.map_err(|e| {
        IrohTransportError::Io(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to accept bi stream: {}", e),
        ))
    })?;

    // Create sender and receiver (note: server sends responses, receives requests)
    let sender = IrohSender::new(send, max_response_size);
    let receiver = IrohReceiver::new(recv, max_request_size);

    Ok((sender, receiver))
}

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

/// Handle a single JSON-RPC connection
///
/// This function processes requests from a client in a loop until the connection
/// closes or an error occurs.
async fn handle_jsonrpc_connection(
    connection: Connection,
    methods: Methods,
    max_request_size: usize,
    max_response_size: usize,
) -> Result<(), IrohTransportError> {
    // Accept connection and create transport
    let (mut sender, mut receiver) =
        accept_connection(&connection, max_request_size, max_response_size).await?;

    // Process requests in a loop
    loop {
        // Receive request
        let request = match receiver.receive().await {
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
            Ok((response, _rx)) => {
                // Convert Box<RawValue> to String
                response.get().to_string()
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
    pub fn build(self, module: RpcModule<()>) -> Result<JsonRpcHandler, BuildError> {
        Ok(JsonRpcHandler::with_limits(
            module.into(),
            self.max_request_size,
            self.max_response_size,
        ))
    }
}

// Re-export server and client types for convenience

/// Re-export of jsonrpsee's ClientT trait
pub use jsonrpsee::core::client::ClientT;

/// Re-export of jsonrpsee's SubscriptionClientT trait
pub use jsonrpsee::core::client::SubscriptionClientT;

/// Re-export of jsonrpsee's Subscription type
pub use jsonrpsee::core::client::Subscription;

/// Re-export of jsonrpsee's error type for method returns
pub use jsonrpsee::types::ErrorObjectOwned as RpcError;
