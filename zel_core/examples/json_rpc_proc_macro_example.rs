//! Demonstrates using jsonrpsee proc-macros with Iroh transport
//!
//! This example shows how to:
//! - Define RPC interfaces using the #[rpc] macro
//! - Implement type-safe server methods
//! - Use auto-generated client methods
//! - Support both regular methods and subscriptions
//!
//! Run with: cargo run --example json_rpc_proc_macro_example

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::StreamExt;
use jsonrpsee::core::server::PendingSubscriptionSink;
use jsonrpsee::core::{RpcResult, SubscriptionResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use zel_core::IrohBundle;
use zel_core::request_reply::json_rpc::{ServerBuilder, build_client};

// ============================================================================
// Step 1: Define RPC Interface with Proc-Macro
// ============================================================================

/// Counter API - demonstrates type-safe RPC with subscriptions
#[rpc(server, client, namespace = "counter")]
pub trait CounterApi {
    /// Get the current counter value
    #[method(name = "get")]
    async fn get_count(&self) -> RpcResult<u64>;

    /// Increment the counter by a value
    #[method(name = "add")]
    async fn add(&self, value: u64) -> RpcResult<u64>;

    /// Reset the counter to zero
    #[method(name = "reset")]
    async fn reset(&self) -> RpcResult<()>;

    /// Subscribe to counter updates (emitted on every change)
    #[subscription(name = "subscribe", item = u64)]
    async fn subscribe_updates(&self) -> SubscriptionResult;

    /// Subscribe to counter with custom interval
    #[subscription(name = "subscribe_interval", item = u64)]
    async fn subscribe_with_interval(&self, interval_ms: u64) -> SubscriptionResult;
}

// ============================================================================
// Step 2: Implement the Server Trait
// ============================================================================

/// Implementation of the Counter API
struct CounterServer {
    count: Arc<AtomicU64>,
    update_tx: tokio::sync::broadcast::Sender<u64>,
}

impl CounterServer {
    fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(100);
        Self {
            count: Arc::new(AtomicU64::new(0)),
            update_tx: tx,
        }
    }
}

#[async_trait]
impl CounterApiServer for CounterServer {
    async fn get_count(&self) -> RpcResult<u64> {
        let value = self.count.load(Ordering::Relaxed);
        log::info!("get_count called, returning {}", value);
        Ok(value)
    }

    async fn add(&self, value: u64) -> RpcResult<u64> {
        let new_value = self.count.fetch_add(value, Ordering::Relaxed) + value;
        log::info!("add({}) called, new value: {}", value, new_value);

        // Notify all subscribers of the update
        let _ = self.update_tx.send(new_value);

        Ok(new_value)
    }

    async fn reset(&self) -> RpcResult<()> {
        self.count.store(0, Ordering::Relaxed);
        log::info!("reset called");

        // Notify subscribers
        let _ = self.update_tx.send(0);

        Ok(())
    }

    async fn subscribe_updates(&self, pending: PendingSubscriptionSink) -> SubscriptionResult {
        log::info!("subscribe_updates called");

        let sink = pending.accept().await?;
        let mut rx = self.update_tx.subscribe();

        // Send current value immediately
        let current = self.count.load(Ordering::Relaxed);
        let msg = serde_json::value::to_raw_value(&current).unwrap();
        sink.send(msg).await?;

        // Forward all updates
        while let Ok(value) = rx.recv().await {
            log::debug!("Sending counter update: {}", value);
            let msg = serde_json::value::to_raw_value(&value).unwrap();
            if sink.send(msg).await.is_err() {
                log::info!("Subscriber disconnected");
                break;
            }
        }

        Ok(())
    }

    async fn subscribe_with_interval(
        &self,
        pending: PendingSubscriptionSink,
        interval_ms: u64,
    ) -> SubscriptionResult {
        log::info!("subscribe_with_interval({}) called", interval_ms);

        let sink = pending.accept().await?;
        let count = Arc::clone(&self.count);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));

            loop {
                interval.tick().await;
                let value = count.load(Ordering::Relaxed);
                let msg = serde_json::value::to_raw_value(&value).unwrap();

                if sink.send(msg).await.is_err() {
                    log::info!("Interval subscriber disconnected");
                    break;
                }
            }
        });

        Ok(())
    }
}

// ============================================================================
// Step 3: Run Server and Client
// ============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter(Some("iroh"), log::LevelFilter::Off)
        .filter(Some("tracing"), log::LevelFilter::Off)
        .init();

    println!("╔═══════════════════════════════════════╗");
    println!("║  Proc-Macro RPC Example over Iroh     ║");
    println!("╚═══════════════════════════════════════╝\n");

    // Create server implementation
    let server_impl = CounterServer::new();

    // Generate RpcModule using the macro-generated into_rpc()
    let module = server_impl.into_rpc();
    println!("✓ RPC module created via proc-macro");
    println!(
        "  Methods: {}",
        module.method_names().collect::<Vec<_>>().join(", ")
    );

    // Build handler with our ServerBuilder
    let handler = ServerBuilder::new().build(module)?;
    println!("✓ Server handler created\n");

    // Start server
    let server_bundle = IrohBundle::builder(None)
        .await?
        .accept(b"counter/1", handler)
        .finish()
        .await;

    println!("✓ Server started (Peer: {})", server_bundle.endpoint.id());

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create client with our build_client
    let client_bundle = IrohBundle::builder(None).await?.finish().await;
    let client = build_client(
        &client_bundle.endpoint,
        server_bundle.endpoint.id(),
        b"counter/1",
    )
    .await?;

    println!("✓ Client connected\n");

    println!("════════════════════════════════════════");
    println!("  Testing Type-Safe RPC Methods");
    println!("════════════════════════════════════════\n");

    // Use the macro-generated client methods (type-safe!)
    println!("📊 Calling counter.get...");
    let count = client.get_count().await?;
    println!("   Current count: {}\n", count);

    println!("➕ Calling counter.add(5)...");
    let new_count = client.add(5).await?;
    println!("   New count: {}\n", new_count);

    println!("➕ Calling counter.add(3)...");
    let new_count = client.add(3).await?;
    println!("   New count: {}\n", new_count);

    println!("📊 Calling counter.get...");
    let count = client.get_count().await?;
    println!("   Current count: {}\n", count);

    println!("════════════════════════════════════════");
    println!("  Testing Subscriptions");
    println!("════════════════════════════════════════\n");

    println!("📡 Subscribing to updates (broadcast on changes)...");
    let mut update_sub = client.subscribe_updates().await?;
    println!("   Subscribed! Will receive notifications on counter changes.\n");

    // Spawn task to modify counter
    let second_client = build_client(
        &client_bundle.endpoint,
        server_bundle.endpoint.id(),
        b"counter/1",
    )
    .await?;
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = second_client.add(10).await;

        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = second_client.add(5).await;

        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = second_client.reset().await;
    });

    // Receive updates
    println!("   Receiving updates:");
    for _ in 0..4 {
        if let Some(Ok(value)) = update_sub.next().await {
            println!("   ├─ Counter changed to: {}", value);
        }
    }
    drop(update_sub);
    println!("   └─ Unsubscribed\n");

    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("⏱️  Subscribing with interval (200ms)...");
    println!("   Counter will be modified by a background task while we watch!");
    let mut interval_sub = client.subscribe_with_interval(200).await?;

    // Spawn background task to modify counter while subscription is active
    let bg_client = build_client(
        &client_bundle.endpoint,
        server_bundle.endpoint.id(),
        b"counter/1",
    )
    .await?;
    tokio::spawn(async move {
        for i in 1..=10 {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let _ = bg_client.add(i).await;
        }
    });

    println!("   Watching counter change in real-time:");
    for i in 0..12 {
        if let Some(Ok(value)) = interval_sub.next().await {
            println!("   ├─ Poll #{}: counter = {}", i + 1, value);
        }
    }
    drop(interval_sub);
    println!("   └─ Subscription ended\n");

    println!("════════════════════════════════════════");
    println!("  Demo Complete");
    println!("════════════════════════════════════════\n");

    println!("Shutting down...");
    server_bundle.shutdown(Duration::from_secs(1)).await?;
    println!("✓ Server shut down successfully");

    Ok(())
}
