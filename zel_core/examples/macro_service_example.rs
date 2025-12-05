//! Example of zel_service macro

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use zel_core::protocol::{zel_service, RequestContext, RpcServerBuilder};
use zel_core::IrohBundle;

#[derive(Serialize, Deserialize)]
pub struct CounterMsg {
    pub count: u64,
}

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
    #[subscription(name = "counter", item = "CounterMsg")]
    async fn counter(&self, interval_ms: u64) -> Result<(), String>;
}

// Implement the generated server trait
#[derive(Clone)]
struct CalculatorImpl;

#[async_trait]
impl CalculatorServer for CalculatorImpl {
    async fn add(&self, ctx: RequestContext, a: i32, b: i32) -> Result<i32, String> {
        log::info!("add called from peer: {}", ctx.remote_id());
        Ok(a + b)
    }

    async fn multiply(&self, ctx: RequestContext, a: i32, b: i32) -> Result<i32, String> {
        log::info!("multiply called from peer: {}", ctx.remote_id());
        Ok(a * b)
    }

    async fn counter(
        &self,
        ctx: RequestContext,
        mut sink: CalculatorCounterSink,
        interval_ms: u64,
    ) -> Result<(), String> {
        log::info!("counter subscription from peer: {}", ctx.remote_id());
        let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
        let mut count = 0u64;

        for _ in 0..5 {
            interval.tick().await;
            count += 1;
            let count = CounterMsg { count };
            if sink.send(count).await.is_err() {
                break;
            }
        }

        let _ = sink.close().await;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
    //     .filter(Some("iroh"), log::LevelFilter::Off)
    //     .init();

    println!("╔════════════════════════════════════════╗");
    println!("║       Zel Service Macro Example        ║");
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

    // Create typed client using generated CalculatorClient
    let calculator = CalculatorClient::new(client);

    // Test RPC calls with type-safe interface
    println!("═══ Testing RPC Methods ═══");

    let sum = calculator.add(10, 5).await?;
    println!("✓ add(10, 5) = {}", sum);

    let product = calculator.multiply(10, 5).await?;
    println!("✓ multiply(10, 5) = {}", product);

    println!("\n═══ Testing Subscription ═══");

    // Test subscription with typed stream
    use futures::StreamExt;
    let mut stream = calculator.counter(500).await?;

    println!("Receiving counter updates:");
    let mut updates = 0;
    while let Some(result) = stream.next().await {
        let CounterMsg { count } = result?;
        updates += 1;
        println!("  ├─ Count: {} (update #{})", count, updates);

        if updates >= 5 {
            println!("  └─ Received {} updates", updates);
            break;
        }
    }

    println!("\nShutting down...");
    server_bundle.shutdown(Duration::from_secs(2)).await?;
    println!("✓ Server shut down successfully");

    Ok(())
}
