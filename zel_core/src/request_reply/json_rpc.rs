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

pub mod client;
pub mod errors;
pub mod server;
pub mod transport;

pub use client::{IrohClientBuilder, build_client};
pub use errors::{BuildError, IrohTransportError};
pub use server::{JsonRpcHandler, RpcModule, ServerBuilder};
pub use transport::{
    DEFAULT_MAX_REQUEST_SIZE, DEFAULT_MAX_RESPONSE_SIZE, IrohReceiver, IrohSender,
    IrohTransportBuilder, accept_connection,
};

// ============================================================================
// Re-exports for convenience
// ============================================================================

/// Re-export of jsonrpsee's ClientT trait
pub use jsonrpsee::core::client::ClientT;

/// Re-export of jsonrpsee's SubscriptionClientT trait
pub use jsonrpsee::core::client::SubscriptionClientT;

/// Re-export of jsonrpsee's Subscription type
pub use jsonrpsee::core::client::Subscription;

/// Re-export of jsonrpsee's error type for method returns
pub use jsonrpsee::types::ErrorObjectOwned as RpcError;
