//! # Example: Raw Streams
//!
//! Demonstrates `#[stream]` for custom bidirectional protocols.
//! Features: Raw SendStream/RecvStream, custom wire format (chunked transfer).
//!
//! Run: `cargo run --example raw_stream_example`
//!
//! Expected: Metadata RPC, chunked file transfer with ACKs.
//!
//! Shows #[stream] attribute usage to create endpoints that receive
//! raw Iroh streams for custom protocols.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use zel_core::protocol::{zel_service, RequestContext, RpcServerBuilder};
use zel_core::IrohBundle;
use zel_types::ResourceError;

// ============================================================================
// Service Definition (using macros)
// ============================================================================

#[derive(Serialize, Deserialize)]
struct FileMetadata {
    filename: String,
    size: u64,
}

#[zel_service(name = "file")]
trait FileService {
    /// Regular RPC method for getting metadata
    #[method(name = "metadata")]
    async fn get_metadata(&self, filename: String) -> Result<FileMetadata, ResourceError>;

    /// Stream method for raw file transfer with custom protocol
    /// Note: send/recv parameters are auto-injected by the macro
    #[stream(name = "transfer")]
    async fn transfer_file(&self, filename: String) -> Result<(), ResourceError>;
}

// ============================================================================
// Service Implementation
// ============================================================================

#[derive(Clone)]
struct FileServiceImpl;

#[async_trait]
impl FileServiceServer for FileServiceImpl {
    async fn get_metadata(
        &self,
        _ctx: RequestContext,
        filename: String,
    ) -> Result<FileMetadata, ResourceError> {
        println!("[Server] Getting metadata for: {}", filename);
        Ok(FileMetadata {
            filename,
            size: 1024,
        })
    }

    async fn transfer_file(
        &self,
        _ctx: RequestContext,
        mut send: iroh::endpoint::SendStream,
        mut recv: iroh::endpoint::RecvStream,
        filename: String,
    ) -> Result<(), ResourceError> {
        println!("[Server] Stream handler started for file: {}", filename);

        // Custom protocol: receive chunks with 4-byte size prefix
        let mut total_bytes = 0u64;

        loop {
            // Read chunk size (4 bytes)
            let mut size_buf = [0u8; 4];
            match recv.read_exact(&mut size_buf).await {
                Ok(_) => {}
                Err(_) => break,
            }

            let size = u32::from_be_bytes(size_buf);

            if size == 0 {
                println!("[Server] Received end-of-stream signal");
                break;
            }

            // Read chunk data
            let mut chunk = vec![0u8; size as usize];
            recv.read_exact(&mut chunk)
                .await
                .map_err(|e| ResourceError::infra(e.to_string()))?;
            total_bytes += size as u64;

            println!(
                "[Server] Received chunk: {} bytes (total: {})",
                size, total_bytes
            );

            // Send ACK
            send.write_all(b"ACK")
                .await
                .map_err(|e| ResourceError::infra(e.to_string()))?;
        }

        println!("[Server] Transfer complete: {} total bytes", total_bytes);

        // Close the send stream to signal completion
        send.finish()
            .map_err(|e| ResourceError::infra(e.to_string()))?;

        Ok(())
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Raw Stream Example (with macros) ===\n");

    // Create server
    let mut server_bundle = IrohBundle::builder(None).await?;
    let server_id = server_bundle.endpoint().id();

    let file_service = FileServiceImpl;
    let server = RpcServerBuilder::new(b"file/1", server_bundle.endpoint().clone()).service("file");

    // Service uses the generated `into_service_builder` method
    let server = file_service.into_service_builder(server).build().build();
    let server_bundle = server_bundle.accept(b"file/1", server).finish().await;

    // Create client
    let client_bundle = IrohBundle::builder(None).await?.finish().await;
    tokio::time::sleep(Duration::from_millis(1000)).await;

    println!("[Client] Connecting to server...");
    let connection = client_bundle.endpoint.connect(server_id, b"file/1").await?;
    let rpc_client = zel_core::protocol::client::RpcClient::new(connection).await?;
    let file_client = FileServiceClient::new(rpc_client);

    // Test regular RPC call first
    println!("\n[Client] Calling get_metadata RPC...");
    let metadata = file_client.get_metadata("test.txt".to_string()).await?;
    println!(
        "[Client] Metadata: {} - {} bytes\n",
        metadata.filename, metadata.size
    );

    // Now test raw stream transfer
    println!("[Client] Opening raw stream for file transfer...");
    let (mut send, mut recv) = file_client.transfer_file("test.txt".to_string()).await?;
    println!("[Client] Stream opened successfully!\n");

    // Send data chunks using our custom protocol
    let data_chunks = vec![
        b"Hello, ".to_vec(),
        b"this is ".to_vec(),
        b"raw stream ".to_vec(),
        b"data!".to_vec(),
    ];

    for chunk in &data_chunks {
        // Send size prefix
        let size = chunk.len() as u32;
        send.write_all(&size.to_be_bytes()).await?;

        // Send data
        send.write_all(chunk).await?;

        // Wait for ACK
        let mut ack = [0u8; 3];
        recv.read_exact(&mut ack).await?;

        println!("[Client] Sent {} bytes, got ACK", size);
    }

    // Send end signal
    send.write_all(&0u32.to_be_bytes()).await?;
    println!("[Client] Sent end-of-stream signal");

    // Close the send stream to signal we're done writing
    send.finish()?;
    println!("[Client] Closed send stream\n");

    tokio::time::sleep(Duration::from_millis(100)).await;

    println!("=== Example Complete ===");
    println!("\nThis demonstrates:");
    println!("  • Using #[stream] attribute in service trait");
    println!("  • Macro-generated server trait with SendStream/RecvStream params");
    println!("  • Macro-generated client returning raw streams");
    println!("  • Custom chunked transfer protocol");
    println!("  • Full control over wire format (no framing/codec)");

    server_bundle.shutdown(Duration::from_secs(1)).await?;
    Ok(())
}
