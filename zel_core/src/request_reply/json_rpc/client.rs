//! Client builder for JSON-RPC over Iroh

use std::time::Duration;

use iroh::{Endpoint, PublicKey};
use jsonrpsee::core::client::IdKind;
use jsonrpsee::core::client::async_client::{Client, ClientBuilder};

use crate::request_reply::json_rpc::errors::BuildError;
use crate::request_reply::json_rpc::transport::IrohTransportBuilder;

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
