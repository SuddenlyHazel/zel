//! Example of notifications (client-to-server streaming)
//!
//! Shows notification feature where clients push events to the server
//! over time with acknowledgments.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use zel_core::protocol::{zel_service, RequestContext, RpcServerBuilder};
use zel_core::IrohBundle;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum UserEvent {
    Click { button: String, x: i32, y: i32 },
    PageView { url: String },
    Search { query: String },
}

// Define a service with both methods and notifications
#[zel_service(name = "analytics")]
trait Analytics {
    /// Get current statistics
    #[method(name = "get_stats")]
    async fn get_stats(&self) -> Result<Stats, String>;

    /// Client pushes user events to server (client-to-server streaming)
    #[notification(name = "user_events", item = "UserEvent")]
    async fn user_events(&self) -> Result<(), String>;
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Stats {
    pub total_events: u64,
    pub click_events: u64,
    pub page_views: u64,
    pub searches: u64,
}

// Server implementation
#[derive(Clone)]
struct AnalyticsImpl {
    stats: std::sync::Arc<tokio::sync::Mutex<Stats>>,
}

impl AnalyticsImpl {
    fn new() -> Self {
        Self {
            stats: std::sync::Arc::new(tokio::sync::Mutex::new(Stats {
                total_events: 0,
                click_events: 0,
                page_views: 0,
                searches: 0,
            })),
        }
    }
}

#[async_trait]
impl AnalyticsServer for AnalyticsImpl {
    async fn get_stats(&self, ctx: RequestContext) -> Result<Stats, String> {
        log::info!("get_stats called from peer: {}", ctx.remote_id());
        let stats = self.stats.lock().await;
        Ok(stats.clone())
    }

    async fn user_events(
        &self,
        ctx: RequestContext,
        mut receiver: AnalyticsUserEventsReceiver,
    ) -> Result<(), String> {
        log::info!("📥 Receiving events from peer: {}", ctx.remote_id());

        let mut event_count = 0;

        // Process events as they arrive
        while let Some(result) = receiver.next().await {
            let event = result.map_err(|e| e.to_string())?;
            event_count += 1;

            log::info!("  ├─ Event #{}: {:?}", event_count, event);

            // Update statistics based on event type
            let mut stats = self.stats.lock().await;
            stats.total_events += 1;

            match event {
                UserEvent::Click { .. } => stats.click_events += 1,
                UserEvent::PageView { .. } => stats.page_views += 1,
                UserEvent::Search { .. } => stats.searches += 1,
            }

            // Acknowledgment is sent automatically by the receiver
        }

        log::info!("  └─ Completed receiving {} events", event_count);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
    //     .filter(Some("iroh"), log::LevelFilter::Off)
    //     .init();

    println!("╔═════════════════════════════════════════╗");
    println!("║   Zel Notification Example              ║");
    println!("║   (Client-to-Server Streaming)          ║");
    println!("╚═════════════════════════════════════════╝\n");

    // Build server
    let mut server_bundle = IrohBundle::builder(None).await?;

    let analytics = AnalyticsImpl::new();
    let service_builder = RpcServerBuilder::new(b"analytics/1", server_bundle.endpoint().clone())
        .service("analytics");

    let service_builder = analytics.into_service_builder(service_builder);
    let server = service_builder.build().build();

    println!("✓ Server built with analytics service");

    let server_bundle = server_bundle.accept(b"analytics/1", server).finish().await;
    println!("✓ Server listening on ALPN: analytics/1");
    println!("  Server Peer ID: {}\n", server_bundle.endpoint.id());

    // Create client
    let client_bundle = IrohBundle::builder(None).await?.finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"analytics/1")
        .await?;

    let client = zel_core::protocol::client::RpcClient::new(conn).await?;
    println!("✓ Client connected\n");

    // Create typed client
    let analytics_client = AnalyticsClient::new(client);

    println!("═══ Testing Notifications ═══\n");

    // Start notification stream
    let mut sender = analytics_client.user_events().await?;
    println!("✓ Notification stream established");

    // Set high priority for this notification stream
    // This ensures these analytics events get bandwidth preference when multiplexing
    if let Err(e) = sender.send_stream().set_priority(15) {
        log::warn!("Failed to set stream priority: {}", e);
    } else {
        println!("✓ Set notification stream priority to 15 (high priority for analytics)\n");
    }

    println!("Sending user events:");

    // Send multiple events
    sender
        .send(UserEvent::Click {
            button: "login".to_string(),
            x: 150,
            y: 200,
        })
        .await?;
    println!("  ✓ Sent: Click event on login button");
    tokio::time::sleep(Duration::from_millis(100)).await;

    sender
        .send(UserEvent::PageView {
            url: "/dashboard".to_string(),
        })
        .await?;
    println!("  ✓ Sent: PageView event");
    tokio::time::sleep(Duration::from_millis(100)).await;

    sender
        .send(UserEvent::Search {
            query: "rust async".to_string(),
        })
        .await?;
    println!("  ✓ Sent: Search event");
    tokio::time::sleep(Duration::from_millis(100)).await;

    sender
        .send(UserEvent::Click {
            button: "submit".to_string(),
            x: 300,
            y: 400,
        })
        .await?;
    println!("  ✓ Sent: Click event on submit button");
    tokio::time::sleep(Duration::from_millis(100)).await;

    sender
        .send(UserEvent::PageView {
            url: "/profile".to_string(),
        })
        .await?;
    println!("  ✓ Sent: PageView event");
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Complete the stream
    sender.complete().await?;
    println!("\n✓ Notification stream completed\n");

    // Give server time to process
    tokio::time::sleep(Duration::from_millis(200)).await;

    println!("═══ Checking Statistics ═══\n");

    // Get final statistics
    let stats = analytics_client.get_stats().await?;
    println!("Server Statistics:");
    println!("  Total Events:  {}", stats.total_events);
    println!("  Click Events:  {}", stats.click_events);
    println!("  Page Views:    {}", stats.page_views);
    println!("  Searches:      {}", stats.searches);

    println!("\nShutting down...");
    server_bundle.shutdown(Duration::from_secs(2)).await?;
    println!("✓ Server shut down successfully");

    Ok(())
}
