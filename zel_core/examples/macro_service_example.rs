//! Example demonstrating the zel_service macro

use async_trait::async_trait;
use bytes::Bytes;
use std::time::Duration;
use zel_core::IrohBundle;
use zel_core::protocol::{RpcServerBuilder, SubscriptionSink, zel_service};

// Define a service using the macro
#[zel_service(name = "calculator")]
trait Calculator {
    /// Add two numbers
    #[method(name = "add")]
    async fn add(&self, a: i32, b: i32) -> Result<i32, String>;

    /// Multiply two numbers
    #[method(name = "multiply")]
    async fn multiply(&self, a: i32, b: i32) -> Result<i32, String>;

    /// Subscribe to a counter
    #[subscription(name = "counter")]
    async fn counter(&self, interval_ms: u64) -> Result<(), String>;
}

// Implement the generated server trait
#[derive(Clone)]
struct CalculatorImpl;

#[async_trait]
impl CalculatorServer for CalculatorImpl {
    async fn add(&self, a: i32, b: i32) -> Result<i32, String> {
        Ok(a + b)
    }

    async fn multiply(&self, a: i32, b: i32) -> Result<i32, String> {
        Ok(a * b)
    }

    async fn counter(&self, mut sink: SubscriptionSink, interval_ms: u64) -> Result<(), String> {
        let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
        let mut count = 0u64;

        for _ in 0..5 {
            interval.tick().await;
            count += 1;

            if sink.send(&count).await.is_err() {
                break;
            }
        }

        let _ = sink.close().await;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter(Some("iroh"), log::LevelFilter::Off)
        .init();

    println!("╔════════════════════════════════════════╗");
    println!("║  Zel Service Macro Example            ║");
    println!("╚════════════════════════════════════════╝\n");

    // Build server
    let mut server_bundle = IrohBundle::builder(None).await?;

    let calculator = CalculatorImpl;
    let service_builder =
        RpcServerBuilder::new(b"calc/1", server_bundle.endpoint().clone()).service("calculator");

    // Use the generated into_service_builder method
    let service_builder = calculator.into_service_builder(service_builder);

    let server = service_builder.build().build();

    println!("✓ Server built with calculator service");

    let server_bundle = server_bundle.accept(b"calc/1", server).finish().await;
    println!("✓ Server listening on ALPN: calc/1");
    println!("  Server Peer ID: {}\n", server_bundle.endpoint.id());

    // Create client
    let client_bundle = IrohBundle::builder(None).await?.finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"calc/1")
        .await?;

    let client = zel_core::protocol::client::RpcClient::new(conn).await?;
    println!("✓ Client connected\n");

    // Test RPC calls
    println!("═══ Testing RPC Methods ═══");

    let numbers = (10i32, 5i32);
    let body = serde_json::to_vec(&numbers)?;
    let response = client.call("calculator", "add", Bytes::from(body)).await?;
    let sum: i32 = serde_json::from_slice(&response.data)?;
    println!("✓ add(10, 5) = {}", sum);

    let numbers = (10i32, 5i32);
    let body = serde_json::to_vec(&numbers)?;
    let response = client
        .call("calculator", "multiply", Bytes::from(body))
        .await?;
    let product: i32 = serde_json::from_slice(&response.data)?;
    println!("✓ multiply(10, 5) = {}", product);

    println!("\n═══ Testing Subscription ═══");

    // Test subscription
    use futures::StreamExt;
    let interval_ms = 500u64;
    let body = serde_json::to_vec(&interval_ms)?;
    let mut stream = client
        .subscribe("calculator", "counter", Some(Bytes::from(body)))
        .await?;

    println!("Receiving counter updates:");
    while let Some(result) = stream.next().await {
        match result? {
            zel_core::protocol::SubscriptionMsg::Data(data) => {
                let count: u64 = serde_json::from_slice(&data)?;
                println!("  ├─ Count: {}", count);
            }
            zel_core::protocol::SubscriptionMsg::Stopped => {
                println!("  └─ Subscription stopped");
                break;
            }
            _ => {}
        }
    }

    println!("\nShutting down...");
    server_bundle.shutdown(Duration::from_secs(2)).await?;
    println!("✓ Server shut down successfully");

    Ok(())
}
