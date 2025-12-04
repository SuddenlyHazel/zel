//! Comprehensive example demonstrating the three-tier extension system
//!
//! This example shows:
//! - Server extensions: Shared across all connections (database pool, config)
//! - Connection extensions: Per-connection state (authentication, session)
//! - Request extensions: Per-request data (trace IDs, timing)
//! - How to access and use extensions in RPC handlers

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use zel_core::IrohBundle;
use zel_core::protocol::{Extensions, RequestContext, RpcServerBuilder, zel_service};

// ============================================================================
// Server-Level Extensions (Shared Across ALL Connections)
// ============================================================================

/// Simulated database pool - shared across all connections
#[derive(Clone)]
struct DatabasePool {
    connection_string: String,
    max_connections: u32,
}

impl DatabasePool {
    fn new(connection_string: impl Into<String>) -> Self {
        Self {
            connection_string: connection_string.into(),
            max_connections: 10,
        }
    }

    async fn query(&self, sql: &str) -> Result<String, String> {
        println!(
            "📊 [DB] Executing query on {}: {}",
            self.connection_string, sql
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(format!("Result from: {}", sql))
    }
}

/// Server configuration - shared across all connections
#[derive(Clone)]
struct ServerConfig {
    max_request_size: usize,
    enable_logging: bool,
}

/// Global request counter for metrics
struct RequestCounter {
    count: AtomicU64,
}

impl RequestCounter {
    fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
        }
    }

    fn increment(&self) -> u64 {
        self.count.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn get(&self) -> u64 {
        self.count.load(Ordering::SeqCst)
    }
}

// ============================================================================
// Connection-Level Extensions (Per Connection)
// ============================================================================

/// User session data - unique per connection
#[derive(Clone)]
struct UserSession {
    user_id: String,
    authenticated_at: Instant,
    permissions: Vec<String>,
}

impl UserSession {
    fn new(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            authenticated_at: Instant::now(),
            permissions: vec!["read".to_string(), "write".to_string()],
        }
    }

    fn has_permission(&self, permission: &str) -> bool {
        self.permissions.iter().any(|p| p == permission)
    }
}

// ============================================================================
// Request-Level Extensions (Per Request)
// ============================================================================

/// Trace ID for distributed tracing - unique per request
#[derive(Clone, Copy)]
struct TraceId(u64);

impl TraceId {
    fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Request timing for performance monitoring
struct RequestTiming {
    started_at: Instant,
}

impl RequestTiming {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
        }
    }

    fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

// ============================================================================
// Service Definition
// ============================================================================

#[derive(Serialize, Deserialize)]
struct UserData {
    name: String,
    email: String,
}

#[derive(Serialize, Deserialize)]
struct UserId {
    id: String,
}

#[zel_service(name = "user")]
trait UserService {
    /// Create a new user (demonstrates all three extension tiers)
    #[method(name = "create")]
    async fn create_user(&self, user: UserData) -> Result<UserId, String>;

    /// Get user info (demonstrates permission checking)
    #[method(name = "get")]
    async fn get_user(&self, user_id: String) -> Result<UserData, String>;

    /// Get server stats (demonstrates server extensions)
    #[method(name = "stats")]
    async fn get_stats(&self) -> Result<String, String>;
}

// ============================================================================
// Service Implementation
// ============================================================================

#[derive(Clone)]
struct UserServiceImpl;

#[async_trait]
impl UserServiceServer for UserServiceImpl {
    async fn create_user(&self, ctx: RequestContext, user: UserData) -> Result<UserId, String> {
        // Access SERVER extensions (shared across all connections)
        let db_pool = ctx
            .server_extensions()
            .get::<DatabasePool>()
            .ok_or("Database pool not configured")?;

        let config = ctx
            .server_extensions()
            .get::<Arc<ServerConfig>>()
            .ok_or("Server config not found")?;

        let counter = ctx
            .server_extensions()
            .get::<Arc<RequestCounter>>()
            .ok_or("Request counter not found")?;

        // Access CONNECTION extensions (per-connection session)
        // Note: In a real implementation, you'd populate connection extensions in a custom
        // connection handler. For this demo, we'll make it optional to show the pattern.
        let session = ctx.connection_extensions().get::<UserSession>();

        // Check permissions if session exists
        if let Some(ref s) = session {
            if !s.has_permission("write") {
                return Err("Insufficient permissions".to_string());
            }
            println!("  ✓ Authenticated as: {}", s.user_id);
        } else {
            println!("  ⚠️  No session (would require authentication in production)");
        }

        // Access REQUEST extensions (per-request trace ID)
        let trace_id = ctx.extensions().get::<TraceId>();
        let timing = ctx.extensions().get::<RequestTiming>();

        // Log the request with all context
        if config.enable_logging {
            println!("\n🔵 CREATE USER REQUEST");
            println!("  Trace ID: {:?}", trace_id.map(|t| t.0));
            println!("  Remote Peer: {}", ctx.remote_id());
            if let Some(ref s) = session {
                println!("  Session User: {}", s.user_id);
            }
            println!("  User Name: {}", user.name);
            println!("  Request #{}", counter.increment());
        }

        // Simulate database operation
        let query = format!(
            "INSERT INTO users (name, email) VALUES ('{}', '{}')",
            user.name, user.email
        );
        let result = db_pool.query(&query).await?;
        println!("  DB Result: {}", result);

        // Log timing if available
        if let Some(t) = timing {
            println!("  Request Duration: {:?}", t.elapsed());
        }

        Ok(UserId {
            id: format!("user_{}", counter.get()),
        })
    }

    async fn get_user(&self, ctx: RequestContext, user_id: String) -> Result<UserData, String> {
        // Check authentication (optional in this demo)
        let session = ctx.connection_extensions().get::<UserSession>();

        if let Some(ref s) = session {
            if !s.has_permission("read") {
                return Err("Insufficient permissions".to_string());
            }
        }

        let db_pool = ctx
            .server_extensions()
            .get::<DatabasePool>()
            .ok_or("Database pool not configured")?;

        println!("\n🟢 GET USER REQUEST");
        println!("  User ID: {}", user_id);
        if let Some(ref s) = session {
            println!("  Session User: {}", s.user_id);
        }
        println!("  Remote Peer: {}", ctx.remote_id());

        let query = format!("SELECT * FROM users WHERE id = '{}'", user_id);
        let result = db_pool.query(&query).await?;
        println!("  DB Result: {}", result);

        Ok(UserData {
            name: "John Doe".to_string(),
            email: "john@example.com".to_string(),
        })
    }

    async fn get_stats(&self, ctx: RequestContext) -> Result<String, String> {
        let counter = ctx
            .server_extensions()
            .get::<Arc<RequestCounter>>()
            .ok_or("Request counter not found")?;

        let session = ctx.connection_extensions().get::<UserSession>();

        println!("\n📈 GET STATS REQUEST");
        println!("  Total Requests: {}", counter.get());
        if let Some(s) = session {
            println!("  Session User: {}", s.user_id);
            println!("  Session Age: {:?}", s.authenticated_at.elapsed());
        }

        Ok(format!("Total requests: {}", counter.get()))
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("╔════════════════════════════════════════════════════════╗");
    println!("║   Context & Extensions Demo - Three-Tier System       ║");
    println!("╚════════════════════════════════════════════════════════╝\n");

    // ========================================================================
    // Setup Server Extensions (Shared Across ALL Connections)
    // ========================================================================
    println!("🔧 Setting up server extensions...");

    let db_pool = DatabasePool::new("postgresql://localhost:5432/mydb");
    let config = Arc::new(ServerConfig {
        max_request_size: 1024 * 1024,
        enable_logging: true,
    });
    let counter = Arc::new(RequestCounter::new());

    let server_extensions = Extensions::new()
        .with(db_pool.clone())
        .with(config.clone())
        .with(counter.clone());

    println!("  ✓ Database pool configured");
    println!("  ✓ Server config loaded");
    println!("  ✓ Request counter initialized\n");

    // ========================================================================
    // Build Server with Extensions
    // ========================================================================
    let mut server_bundle = IrohBundle::builder(None).await?;

    let user_service = UserServiceImpl;
    let server = RpcServerBuilder::new(b"user/1", server_bundle.endpoint().clone())
        .with_extensions(server_extensions)
        .service("user");

    let server = user_service.into_service_builder(server).build().build();

    let server_bundle = server_bundle.accept(b"user/1", server).finish().await;
    println!("🚀 Server listening on ALPN: user/1");
    println!("  Server ID: {}\n", server_bundle.endpoint.id());

    // ========================================================================
    // Create Client
    // ========================================================================
    let client_bundle = IrohBundle::builder(None).await?.finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"user/1")
        .await?;

    let client = zel_core::protocol::client::RpcClient::new(conn).await?;
    let user_client = UserServiceClient::new(client);

    println!("✅ Client connected\n");

    // ========================================================================
    // Demonstrate Extension Usage
    // ========================================================================

    println!("═══════════════════════════════════════════════════════");
    println!("  Testing Three-Tier Extension System");
    println!("═══════════════════════════════════════════════════════\n");

    // Create a user (this will use all three extension tiers)
    let user = UserData {
        name: "Alice Smith".to_string(),
        email: "alice@example.com".to_string(),
    };

    let user_id = user_client.create_user(user).await?;
    println!("\n✅ User created: {}\n", user_id.id);

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Get user info
    let user_data = user_client.get_user(user_id.id.clone()).await?;
    println!(
        "\n✅ User retrieved: {} ({})\n",
        user_data.name, user_data.email
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Get server stats
    let stats = user_client.get_stats().await?;
    println!("\n✅ Server stats: {}\n", stats);

    println!("\n═══════════════════════════════════════════════════════");
    println!("  Extension System Summary");
    println!("═══════════════════════════════════════════════════════");
    println!("  📊 Server Extensions: Shared database pool & config");
    println!("  👤 Connection Extensions: User session (simulated)");
    println!("  🔍 Request Extensions: Trace IDs & timing (optional)");
    println!("  📈 Total Requests Processed: {}", counter.get());
    println!("═══════════════════════════════════════════════════════\n");

    server_bundle.shutdown(Duration::from_secs(1)).await?;
    println!("✅ Server shut down successfully");

    Ok(())
}
