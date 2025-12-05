//! Example demonstrating endpoint metrics export to Prometheus.
//!
//! This example shows how to:
//! 1. Enable and access endpoint metrics
//! 2. Export metrics to Prometheus format
//! 3. Monitor connection statistics in real-time
//!
//! Run with: `cargo run --example metrics_prometheus --features metrics`

use bytes::Bytes;
use futures::StreamExt;
use iroh::Watcher;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use zel_core::protocol::{Body, RequestContext, Response, RpcServerBuilder};
use zel_core::IrohBundle;

#[cfg(feature = "metrics")]
use iroh_metrics::Registry;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    #[cfg(not(feature = "metrics"))]
    {
        eprintln!("ERROR: This example requires the 'metrics' feature.");
        eprintln!("Run with: cargo run --example metrics_prometheus --features metrics");
        std::process::exit(1);
    }

    #[cfg(feature = "metrics")]
    {
        println!("\n=== Zel Metrics & Prometheus Example ===\n");

        // Create metrics registry
        let registry = Arc::new(RwLock::new(Registry::default()));

        // Spawn HTTP server to serve metrics
        let metrics_addr = "127.0.0.1:9090";
        let registry_clone = registry.clone();
        tokio::task::spawn(async move {
            println!("Starting metrics server on http://{}/metrics", metrics_addr);
            if let Err(e) = iroh_metrics::service::start_metrics_server(
                metrics_addr.parse().unwrap(),
                registry_clone,
            )
            .await
            {
                eprintln!("Metrics server error: {}", e);
            }
        });

        // Wait for metrics server to start
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Create Iroh bundle
        let mut bundle_builder = IrohBundle::builder(None).await?;
        let endpoint = bundle_builder.endpoint().clone();

        // Build a simple echo service
        let server = RpcServerBuilder::new(b"metrics-demo/1", endpoint.clone())
            .service("demo")
            .rpc_resource("echo", |ctx: RequestContext, req| {
                Box::pin(async move {
                    // Log connection metrics on each request
                    let rtt = ctx.connection_rtt();
                    let stats = ctx.connection_stats();

                    println!("\nRequest Metrics:");
                    println!("  RTT: {:?}", rtt);
                    println!("  Datagrams sent: {}", stats.udp_tx.datagrams);
                    println!("  Bytes received: {}", stats.udp_rx.bytes);
                    println!("  Path MTU: {:?}", stats.path.current_mtu);

                    // Echo the request body back
                    let response_data = match req.body {
                        Body::Rpc(data) => data,
                        _ => Bytes::from("echo"),
                    };
                    Ok(Response {
                        data: response_data,
                    })
                })
            })
            .build()
            .build();

        let bundle = bundle_builder
            .accept(b"metrics-demo/1", server)
            .finish()
            .await;

        // Register endpoint metrics with Prometheus registry
        registry.write().unwrap().register_all(bundle.metrics());

        println!("\nEndpoint created!");
        println!("   ID: {}", bundle.endpoint.id());
        println!("   Metrics: http://127.0.0.1:9090/metrics");

        // Wait for endpoint to be online
        println!("\nWaiting for endpoint to be online...");
        bundle.wait_online().await;

        if bundle.is_online() {
            println!("Endpoint is online and ready!");
        }

        // Monitor address changes
        let mut addr_watcher = bundle.watch_addr();
        let initial_addr = addr_watcher.get();
        println!("\nEndpoint Address:");
        println!("   Direct addresses: {}", initial_addr.ip_addrs().count());
        if let Some(relay) = initial_addr.relay_urls().next() {
            println!("   Relay URL: {}", relay);
        }

        // Spawn task to monitor address changes
        let addr_watcher_clone = bundle.watch_addr();
        tokio::spawn(async move {
            let mut stream = addr_watcher_clone.stream();
            while let Some(addr) = stream.next().await {
                println!("\nEndpoint address changed!");
                println!("   Direct addresses: {}", addr.ip_addrs().count());
            }
        });

        println!("\nLive Metrics Dashboard:");
        println!("   Visit: http://127.0.0.1:9090/metrics");
        println!("\nUsage:");
        println!("   - Metrics are updated in real-time");
        println!("   - Use Prometheus to scrape: http://127.0.0.1:9090/metrics");
        println!("   - Connect clients to see connection metrics");
        println!("\nKey Metrics to Watch:");
        println!("   - magicsock_recv_datagrams_total - Packets received");
        println!("   - magicsock_num_direct_conns_added - Direct connections");
        println!("   - magicsock_connection_became_direct - Relay → Direct transitions");

        // Display current metrics
        println!("\nCurrent Metrics:");
        let metrics = bundle.metrics();
        println!(
            "   Datagrams received: {}",
            metrics.magicsock.recv_datagrams.get()
        );
        println!(
            "   Direct connections: {}",
            metrics.magicsock.num_direct_conns_added.get()
        );
        println!(
            "   Handshake successes: {}",
            metrics.magicsock.connection_handshake_success.get()
        );

        println!("\nServer running... Press Ctrl+C to shutdown");

        // Wait for Ctrl+C
        tokio::signal::ctrl_c().await?;

        println!("\nShutting down...");
        bundle.shutdown(Duration::from_secs(5)).await?;

        println!("Shutdown complete!");
    }

    Ok(())
}
