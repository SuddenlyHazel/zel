//! JSON-RPC Subscriptions Example over Iroh
//!
//! This example demonstrates:
//! - Registering subscription methods on the server
//! - Client subscribing and receiving real-time notifications
//! - Multiple concurrent subscriptions
//! - Graceful unsubscribe and cleanup
//!
//! Run with: cargo run --example json_rpc_subscriptions_example

use futures::StreamExt;
use jsonrpsee::core::client::SubscriptionClientT;
use jsonrpsee::rpc_params;
use std::time::Duration;
use zel_core::IrohBundle;
use zel_core::request_reply::json_rpc::{ConnectionExt, RpcModule, ServerBuilder, build_client};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter(Some("iroh"), log::LevelFilter::Off)
        .filter(Some("tracing"), log::LevelFilter::Off)
        .init();

    println!("╔════════════════════════════════════════╗");
    println!("║  JSON-RPC Subscriptions Example       ║");
    println!("╚════════════════════════════════════════╝\n");

    // Build RPC module with subscription methods
    let rpc_module = build_subscription_module()?;
    println!(
        "✓ RPC module built with {} methods",
        rpc_module.method_names().count()
    );

    // Create server handler
    let handler = ServerBuilder::new().build(rpc_module)?;
    println!("✓ Server handler created\n");

    // Start server
    let server_bundle = IrohBundle::builder(None)
        .await?
        .accept(b"jsonrpc-sub/1", handler)
        .finish()
        .await;

    println!("✓ Server started and listening on ALPN: jsonrpc-sub/1");
    println!("  Server Peer ID: {}\n", server_bundle.endpoint.id());

    // Give server time to initialize
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create client
    let client_bundle = IrohBundle::builder(None).await?.finish().await;
    let client = build_client(
        &client_bundle.endpoint,
        server_bundle.endpoint.id(),
        b"jsonrpc-sub/1",
    )
    .await?;

    let client_bundle_two = IrohBundle::builder(None).await?.finish().await;
    let client_two = build_client(
        &client_bundle_two.endpoint,
        server_bundle.endpoint.id(),
        b"jsonrpc-sub/1",
    )
    .await?;

    println!("✓ Client connected to server\n");
    println!("════════════════════════════════════════");
    println!("  Demonstrating Subscriptions");
    println!("════════════════════════════════════════\n");

    // Demo 1: Counter Subscription
    println!("📊 Demo 1: Counter Subscription");
    println!("   Subscribing to counter with 500ms interval...\n");

    let mut counter_sub: jsonrpsee::core::client::Subscription<u64> = client
        .subscribe("subscribe_counter", rpc_params![500], "unsubscribe_counter")
        .await?;

    println!("   Receiving counter updates:");
    for i in 0..5 {
        if let Some(Ok(count)) = counter_sub.next().await {
            println!("   ├─ Counter: {} (update #{})", count, i + 1);
        }
    }
    let _ = counter_sub.unsubscribe().await;
    println!("   └─ Unsubscribed from counter\n");

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Demo 2: Two Clients, Same Subscription
    println!("👥 Demo 2: Two Clients Subscribe to Counter");
    println!("   Both clients subscribe to counter simultaneously...\n");
    println!("   Note: Each gets their OWN independent counter!\n");

    let mut client1_sub: jsonrpsee::core::client::Subscription<u64> = client
        .subscribe("subscribe_counter", rpc_params![400], "unsubscribe_counter")
        .await?;

    let mut client2_sub: jsonrpsee::core::client::Subscription<u64> = client_two
        .subscribe("subscribe_counter", rpc_params![400], "unsubscribe_counter")
        .await?;

    println!("   Receiving from both clients:");
    for _ in 0..8 {
        tokio::select! {
            Some(result) = client1_sub.next() => {
                if let Ok(count) = result {
                    println!("   ├─ [CLIENT 1] Counter: {}", count);
                }
            }

            Some(result) = client2_sub.next() => {
                if let Ok(count) = result {
                    println!("   ├─ [CLIENT 2] Counter: {}", count);
                }
            }

            _ = tokio::time::sleep(Duration::from_secs(4)) => {
                println!("   └─ Timeout reached");
                break;
            }
        }
    }

    drop(client1_sub);
    drop(client2_sub);
    println!("   └─ Both clients unsubscribed\n");

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Demo 3: Ticker Subscription
    println!("📈 Demo 3: Stock Ticker Subscription");
    println!("   Subscribing to AAPL ticker...\n");

    let mut ticker_sub: jsonrpsee::core::client::Subscription<serde_json::Value> = client
        .subscribe(
            "subscribe_ticker",
            rpc_params!["AAPL"],
            "unsubscribe_ticker",
        )
        .await?;

    println!("   Receiving ticker updates:");
    for i in 0..5 {
        if let Some(Ok(update)) = ticker_sub.next().await {
            println!(
                "   ├─ Symbol: {} | Price: ${:.2} | Time: {} (update #{})",
                update["symbol"].as_str().unwrap_or("?"),
                update["price"].as_f64().unwrap_or(0.0),
                update["timestamp"].as_str().unwrap_or("?"),
                i + 1
            );
        }
    }
    drop(ticker_sub);
    println!("   └─ Unsubscribed from ticker\n");

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Demo 4: Multiple Concurrent Subscriptions
    println!("🔀 Demo 4: Multiple Concurrent Subscriptions (Same Client)");
    println!("   Subscribing to both counter and events...\n");

    let mut counter_sub: jsonrpsee::core::client::Subscription<u64> = client
        .subscribe("subscribe_counter", rpc_params![300], "unsubscribe_counter")
        .await?;

    let mut events_sub: jsonrpsee::core::client::Subscription<String> = client
        .subscribe(
            "subscribe_events",
            rpc_params!["system"],
            "unsubscribe_events",
        )
        .await?;

    println!("   Receiving from multiple subscriptions:");
    for _ in 0..10 {
        tokio::select! {
            Some(result) = counter_sub.next() => {
                if let Ok(count) = result {
                    println!("   ├─ [COUNTER] Value: {}", count);
                }
            }

            Some(result) = events_sub.next() => {
                if let Ok(event) = result {
                    println!("   ├─ [EVENT] {}", event);
                }
            }

            _ = tokio::time::sleep(Duration::from_secs(3)) => {
                println!("   └─ Timeout reached");
                break;
            }
        }
    }

    drop(counter_sub);
    drop(events_sub);
    println!("   └─ Multiple subscriptions closed\n");

    println!("════════════════════════════════════════");
    println!("  Subscription Demo Complete");
    println!("════════════════════════════════════════\n");

    // Cleanup
    println!("Shutting down...");
    server_bundle.shutdown(Duration::from_secs(2)).await?;
    println!("✓ Server shut down successfully");

    Ok(())
}

/// Build an RPC module with subscription methods
fn build_subscription_module() -> anyhow::Result<RpcModule<()>> {
    let mut module = RpcModule::new(());

    // Subscription 1: Counter
    // Emits incrementing numbers at a specified interval
    module.register_subscription(
        "subscribe_counter",
        "counter",
        "unsubscribe_counter",
        |params, pending, _, ext| async move {
            let interval_ms: u64 = params.one().unwrap_or(1000);
            log::info!("New counter subscription ({}ms interval)", interval_ms);
            let Some(connection_info) = ext.get::<ConnectionExt>() else {
                panic!("Eeeek")
            };
            let sink = match pending.accept().await {
                Ok(s) => {
                    log::info!(
                        "Counter subscription accepted successfully for peer {}",
                        connection_info.peer()
                    );
                    s
                }
                Err(e) => {
                    log::error!("Failed to accept counter subscription: {}", e);
                    return Err(e.into());
                }
            };

            let mut counter = 0u64;
            let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));

            loop {
                interval.tick().await;
                counter += 1;

                log::info!(
                    "Attempting to send counter update: {} to peer {}",
                    counter,
                    connection_info.peer()
                );
                let msg = serde_json::value::to_raw_value(&counter).unwrap();
                match sink.send(msg).await {
                    Ok(_) => log::info!(
                        "Successfully sent counter: {} to peer {}",
                        counter,
                        connection_info.peer()
                    ),
                    Err(e) => {
                        log::error!("Failed to send counter {}: {}", counter, e);
                        break;
                    }
                }
            }

            log::info!("Counter subscription loop ended");
            Ok(())
        },
    )?;

    // Subscription 2: Stock Ticker
    // Emits simulated stock price updates
    module.register_subscription(
        "subscribe_ticker",
        "ticker",
        "unsubscribe_ticker",
        |params, pending, _, _| async move {
            let symbol: String = params.one()?;
            log::info!("New ticker subscription for {}", symbol);

            let sink = pending.accept().await?;

            let mut interval = tokio::time::interval(Duration::from_secs(1));
            let mut price = 150.0_f64;
            let mut tick_count = 0u64;

            loop {
                interval.tick().await;
                tick_count += 1;

                // Simulate price changes using tick count as pseudo-random
                let change = ((tick_count * 17) % 20) as f64 - 10.0;
                price = (price + change).max(100.0).min(200.0);

                let timestamp = format!("T+{}s", tick_count);
                let update = serde_json::json!({
                    "symbol": symbol,
                    "price": (price * 100.0).round() / 100.0,
                    "timestamp": timestamp,
                });

                log::debug!("Sending ticker update for {}: ${:.2}", symbol, price);
                let msg = serde_json::value::to_raw_value(&update).unwrap();
                if sink.send(msg).await.is_err() {
                    log::info!("Ticker subscription for {} ended", symbol);
                    break;
                }
            }

            Ok(())
        },
    )?;

    // Subscription 3: Event Stream
    // Emits system events at varying intervals
    module.register_subscription(
        "subscribe_events",
        "event",
        "unsubscribe_events",
        |params, pending, _, _| async move {
            let event_type: String = params.one()?;
            log::info!("New event subscription for type: {}", event_type);

            let sink = pending.accept().await?;

            let events = vec![
                "system.startup",
                "system.health_check",
                "system.metrics_updated",
                "system.cache_cleared",
            ];

            let mut event_count = 0usize;

            loop {
                // Varying delay between events (300-1000ms)
                let delay = Duration::from_millis(300 + ((event_count * 173) % 700) as u64);
                tokio::time::sleep(delay).await;

                let event = events[event_count % events.len()];
                let message = format!("[Event #{}] {}: {}", event_count + 1, event_type, event);

                log::debug!("Sending event: {}", message);
                let msg = serde_json::value::to_raw_value(&message).unwrap();
                if sink.send(msg).await.is_err() {
                    log::info!("Event subscription ended");
                    break;
                }

                event_count += 1;
            }

            Ok(())
        },
    )?;

    Ok(module)
}
