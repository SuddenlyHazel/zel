//! Type-safe RPC framework built on Iroh with macros for easy service definition.
//!
//! Zel provides four types of RPC endpoints built on top of Iroh's peer-to-peer networking:
//! methods (request/response), subscriptions (server-to-client streaming), notifications
//! (client-to-server streaming), and raw bidirectional streams for custom protocols.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use zel_core::protocol::{zel_service, RequestContext, RpcServerBuilder, RpcClient};
//! use zel_core::IrohBundle;
//! use zel_types::ResourceError;
//! use async_trait::async_trait;
//! use std::time::Duration;
//!
//! // Define a service using the macro
//! #[zel_service(name = "math")]
//! trait Math {
//!     #[method(name = "add")]
//!     async fn add(&self, a: i32, b: i32) -> Result<i32, ResourceError>;
//! }
//!
//! // Implement the service
//! #[derive(Clone)]
//! struct MathImpl;
//!
//! #[async_trait]
//! impl MathServer for MathImpl {
//!     async fn add(&self, ctx: RequestContext, a: i32, b: i32) -> Result<i32, ResourceError> {
//!         Ok(a + b)
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Build and start server
//!     let mut server_bundle = IrohBundle::builder(None).await?;
//!     let endpoint = server_bundle.endpoint().clone();
//!
//!     let service = MathImpl;
//!     let server = RpcServerBuilder::new(b"math/1", endpoint)
//!         .service("math")
//!         .build()
//!         .build();
//!
//!     let server_bundle = server_bundle.accept(b"math/1", server).finish().await;
//!
//!     // Create client and make RPC calls
//!     let client_bundle = IrohBundle::builder(None).await?.finish().await;
//!     let conn = client_bundle.endpoint
//!         .connect(server_bundle.endpoint.id(), b"math/1")
//!         .await?;
//!
//!     let client = RpcClient::new(conn).await?;
//!     let math = MathClient::new(client);
//!     let result = math.add(5, 3).await?; // Returns 8
//!
//!     server_bundle.shutdown(Duration::from_secs(5)).await?;
//!     Ok(())
//! }
//! ```
//!
//! # Core Components
//!
//! - [`IrohBundle`] - Manages Iroh endpoint, router, and graceful shutdown
//! - [`protocol::RpcServerBuilder`] - Builds RPC servers with services and middleware
//! - [`protocol::RpcClient`] - Makes RPC calls and manages subscriptions
//! - [`protocol::zel_service`] - Macro for defining type-safe services
//! - [`protocol::RequestContext`] - Provides handlers access to connection info and extensions
//!
//! # Service Endpoint Types
//!
//! - **Methods** (`#[method]`) - Request/response RPC calls
//! - **Subscriptions** (`#[subscription]`) - Server-to-client streaming
//! - **Notifications** (`#[notification]`) - Client-to-server streaming
//! - **Raw Streams** (`#[stream]`) - Bidirectional custom protocols
//!
//! # Examples
//!
//! The [`examples`](https://github.com/SuddenlyHazel/zel/tree/main/zel_core/examples) directory contains working demonstrations:
//!
//! - [`macro_service_example.rs`](https://github.com/SuddenlyHazel/zel/blob/main/zel_core/examples/macro_service_example.rs) - Complete service with methods and subscriptions
//! - [`context_extensions_demo.rs`](https://github.com/SuddenlyHazel/zel/blob/main/zel_core/examples/context_extensions_demo.rs) - Three-tier extension system
//! - [`notification_example.rs`](https://github.com/SuddenlyHazel/zel/blob/main/zel_core/examples/notification_example.rs) - Client-to-server streaming
//! - [`raw_stream_example.rs`](https://github.com/SuddenlyHazel/zel/blob/main/zel_core/examples/raw_stream_example.rs) - Custom bidirectional protocol

mod iroh_helpers;

pub use iroh_helpers::{
    Builder as BundleBuilder, BuilderError, IrohBundle, ShutdownListener, ShutdownReplier,
};

pub mod protocol;

// Re-export shutdown types for convenience
pub use protocol::{ShutdownError, ShutdownHandle};
