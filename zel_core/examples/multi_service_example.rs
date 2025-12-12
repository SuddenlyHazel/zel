//! # Example: Multi-Service
//!
//! Demonstrates multiple `#[zel_service]` on one server.
//! Features: ServiceBuilder chaining, typed clients per service.
//!
//! Run: `cargo run --example multi_service_example`
//!
//! Expected: Calculator + Hello services, RPCs + subs on both.
//!
//! Shows:
//! - Defining multiple services using #[zel_service]
//! - Registering multiple services with RpcServerBuilder
//! - Client access to different services
//! - Server → Service → Resource hierarchy

use serde::{Deserialize, Serialize};
use std::time::Duration;
use zel_core::prelude::*;
use zel_core::IrohBundle;

// ============================================================================
// Service 1: Calculator
// ============================================================================

#[derive(Serialize, Deserialize, Debug)]
pub struct CounterMsg {
    pub count: u64,
}

#[zel_service(name = "calculator")]
trait Calculator {
    #[method(name = "add")]
    async fn add(&self, a: i32, b: i32) -> Result<i32, ResourceError>;

    #[method(name = "multiply")]
    async fn multiply(&self, a: i32, b: i32) -> Result<i32, ResourceError>;

    #[subscription(name = "counter", item = "CounterMsg")]
    async fn counter(&self, interval_ms: u64) -> Result<(), ResourceError>;
}

#[derive(Clone)]
struct CalculatorImpl;

#[async_trait]
impl CalculatorServer for CalculatorImpl {
    async fn add(&self, _ctx: RequestContext, a: i32, b: i32) -> Result<i32, ResourceError> {
        Ok(a + b)
    }

    async fn multiply(&self, _ctx: RequestContext, a: i32, b: i32) -> Result<i32, ResourceError> {
        Ok(a * b)
    }

    async fn counter(
        &self,
        _ctx: RequestContext,
        mut sink: CalculatorCounterSink,
        interval_ms: u64,
    ) -> Result<(), ResourceError> {
        let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));

        for count in 1..=3 {
            interval.tick().await;
            if sink.send(CounterMsg { count }).await.is_err() {
                break;
            }
        }

        let _ = sink.close().await;
        Ok(())
    }
}

// ============================================================================
// Service 2: Hello (Greeting Service)
// ============================================================================

#[derive(Serialize, Deserialize, Debug)]
pub struct Greeting {
    pub message: String,
    pub timestamp: String,
}

#[zel_service(name = "hello")]
trait Hello {
    #[method(name = "greet")]
    async fn greet(&self, name: String) -> Result<Greeting, ResourceError>;

    #[method(name = "farewell")]
    async fn farewell(&self, name: String) -> Result<String, ResourceError>;

    #[subscription(name = "notifications", item = "String")]
    async fn notifications(&self, topic: String) -> Result<(), ResourceError>;
}

#[derive(Clone)]
struct HelloImpl;

#[async_trait]
impl HelloServer for HelloImpl {
    async fn greet(&self, _ctx: RequestContext, name: String) -> Result<Greeting, ResourceError> {
        use std::time::SystemTime;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        Ok(Greeting {
            message: format!("Hello, {}! Welcome to Zel RPC!", name),
            timestamp: format!("Timestamp: {}", now),
        })
    }

    async fn farewell(&self, _ctx: RequestContext, name: String) -> Result<String, ResourceError> {
        Ok(format!("Goodbye, {}! Thanks for using Zel!", name))
    }

    async fn notifications(
        &self,
        _ctx: RequestContext,
        mut sink: HelloNotificationsSink,
        topic: String,
    ) -> Result<(), ResourceError> {
        let messages = vec![
            format!("[{}] Service started", topic),
            format!("[{}] New update available", topic),
            format!("[{}] Maintenance scheduled", topic),
        ];

        for msg in messages {
            tokio::time::sleep(Duration::from_millis(300)).await;
            if sink.send(msg).await.is_err() {
                break;
            }
        }

        let _ = sink.close().await;
        Ok(())
    }
}

// ============================================================================
// Main: Demonstrating Multiple Services
// ============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
    //     .filter(Some("iroh"), log::LevelFilter::Off)
    //     .init();

    println!("╔════════════════════════════════════════╗");
    println!("║   Multi-Service RPC Server Example     ║");
    println!("╚════════════════════════════════════════╝\n");

    // ========================================================================
    // Server Setup: Register MULTIPLE services on ONE server
    // ========================================================================

    let mut server_bundle = IrohBundle::builder(None).await?;

    println!("Building server with MULTIPLE services...\n");

    // Create service implementations
    let calculator = CalculatorImpl;
    let hello = HelloImpl;

    // Build server with BOTH services
    let server_builder = RpcServerBuilder::new(b"multi/1", server_bundle.endpoint().clone());

    // Add Calculator service
    let server_builder = calculator.register_service(server_builder);
    println!("  ✓ Registered 'calculator' service with resources: add, multiply, counter");

    // Add Hello service
    let server_builder = hello.register_service(server_builder);
    println!("  ✓ Registered 'hello' service with resources: greet, farewell, notifications\n");

    let server = server_builder.build();

    let server_bundle = server_bundle.accept(b"multi/1", server).finish().await;
    println!("✓ Server listening on ALPN: multi/1");
    println!("  Server Peer ID: {}", server_bundle.endpoint.id());
    println!("  Services: calculator, hello\n");

    // ========================================================================
    // Client Setup: ONE client, MULTIPLE typed service clients
    // ========================================================================

    let client_bundle = IrohBundle::builder(None).await?.finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"multi/1")
        .await?;

    let rpc_client = zel_core::protocol::client::RpcClient::new(conn).await?;
    println!("✓ RPC Client connected\n");

    // Create typed clients for EACH service
    let calculator = CalculatorClient::new(rpc_client.clone());
    let hello = HelloClient::new(rpc_client.clone());

    println!("✓ Created typed clients for both services\n");

    // ========================================================================
    // Demo: Using Calculator Service
    // ========================================================================

    println!("═══════════════════════════════════════");
    println!("  Service 1: Calculator");
    println!("═══════════════════════════════════════");

    let sum = calculator.add(10, 5).await?;
    println!("  add(10, 5) = {}", sum);

    let product = calculator.multiply(7, 3).await?;
    println!("  multiply(7, 3) = {}", product);

    println!("\n  Subscription: counter(200ms)...");
    use futures::StreamExt;
    let mut counter_stream = calculator.counter(200).await?;
    while let Some(result) = counter_stream.next().await {
        let CounterMsg { count } = result?;
        println!("    ├─ Count: {}", count);
    }
    println!("    └─ Counter finished\n");

    // ========================================================================
    // Demo: Using Hello Service
    // ========================================================================

    println!("═══════════════════════════════════════");
    println!("  Service 2: Hello");
    println!("═══════════════════════════════════════");

    let greeting = hello.greet("Alice".to_string()).await?;
    println!("  {}", greeting.message);
    println!("  Timestamp: {}", greeting.timestamp);

    let farewell = hello.farewell("Bob".to_string()).await?;
    println!("\n  {}", farewell);

    println!("\n  Subscription: notifications(\"system\")...");
    let mut notif_stream = hello.notifications("system".to_string()).await?;
    while let Some(result) = notif_stream.next().await {
        let notification = result?;
        println!("    ├─ {}", notification);
    }
    println!("    └─ Notifications finished\n");

    // ========================================================================
    // Architecture Summary
    // ========================================================================

    println!("═══════════════════════════════════════");
    println!("  Architecture Summary");
    println!("═══════════════════════════════════════");
    println!("  RpcServer (ALPN: multi/1)");
    println!("    ├── Service: 'calculator'");
    println!("    │     ├── Resource: 'add' (RPC)");
    println!("    │     ├── Resource: 'multiply' (RPC)");
    println!("    │     └── Resource: 'counter' (Subscription)");
    println!("    └── Service: 'hello'");
    println!("          ├── Resource: 'greet' (RPC)");
    println!("          ├── Resource: 'farewell' (RPC)");
    println!("          └── Resource: 'notifications' (Subscription)\n");

    println!("Shutting down...");
    server_bundle.shutdown(Duration::from_secs(2)).await?;
    println!("✓ Server shut down successfully");

    Ok(())
}
