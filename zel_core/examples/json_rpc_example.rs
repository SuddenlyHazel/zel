//! Example demonstrating JSON-RPC over Iroh transport
//!
//! This example shows how to use the jsonrpsee client with Iroh transport.

use zel_core::IrohBundle;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    println!("JSON-RPC over Iroh Example");
    println!("==========================\n");

    // Create Iroh endpoint
    let bundle = IrohBundle::builder(None).await?.finish().await;
    let _endpoint = &bundle.endpoint;

    println!("Iroh endpoint created successfully");

    // Note: In a real application, you would:
    // 1. Get the peer's PublicKey from somewhere (discovery, config, etc.)
    // 2. Define the ALPN for your JSON-RPC protocol
    // 3. Have a running server accepting connections

    println!("\n--- Example 1: Simple client creation ---");
    println!("Creating client with default settings...");

    // Example of how you would create a client:
    // let peer: PublicKey = /* get from somewhere */;
    // let client = build_client(&endpoint, peer, b"jsonrpc/1").await?;
    // let result: String = client.request("say_hello", jsonrpsee::core::rpc_params![]).await?;

    println!("Client configuration:");
    println!("  - Max request size: 10 MB (default)");
    println!("  - Max response size: 10 MB (default)");
    println!("  - Request timeout: 60 seconds (default)");
    println!("  - Max concurrent requests: 256 (default)");

    println!("\n--- Example 2: Customized client ---");
    println!("Creating client with custom settings...");

    // Example of customized client:
    // let client = IrohClientBuilder::new()
    //     .max_request_size(5 * 1024 * 1024)  // 5 MB
    //     .request_timeout(Duration::from_secs(10))
    //     .max_concurrent_requests(128)
    //     .build(&endpoint, peer, b"jsonrpc/1")
    //     .await?;

    println!("Custom client configuration:");
    println!("  - Max request size: 5 MB");
    println!("  - Max response size: 10 MB");
    println!("  - Request timeout: 10 seconds");
    println!("  - Max concurrent requests: 128");

    println!("\n--- Example 3: Using the client ---");
    println!("Example client usage:");
    println!(
        r#"
    // Make a simple request
    let result: String = client.request("say_hello", jsonrpsee::core::rpc_params![]).await?;
    
    // Make a request with parameters
    let sum: u64 = client.request("add", jsonrpsee::core::rpc_params![2, 3]).await?;
    
    // Batch requests
    let batch = jsonrpsee::core::params::BatchRequestBuilder::new()
        .insert("method1", jsonrpsee::core::rpc_params![])
        .insert("method2", jsonrpsee::core::rpc_params![42]);
    let results = client.batch_request(batch).await?;
    "#
    );

    println!("\n--- Key Features ---");
    println!("✓ Standard JSON-RPC 2.0 protocol");
    println!("✓ COBS framing for reliable message delivery");
    println!("✓ Configurable size limits and timeouts");
    println!("✓ Support for batch requests");
    println!("✓ Support for subscriptions");
    println!("✓ Tower-based middleware support");
    println!("✓ Coexists with existing custom protocol");

    println!("\nExample completed successfully!");
    println!("\nNote: This example shows the API usage.");
    println!("To run a real client/server, you need:");
    println!("  1. A running server accepting JSON-RPC connections");
    println!("  2. The server's PublicKey for connection");
    println!("  3. Matching ALPN on both client and server");

    Ok(())
}
