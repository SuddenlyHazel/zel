//! Example of three-tier extension system
//!
//! Shows:
//! - Server extensions: Shared across all connections (database pool, config)
//! - Connection extensions: Per-connection state (authentication, session)
//! - Request extensions: Per-request data (trace IDs, timing)
//! - Accessing and using extensions in RPC handlers

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use zel_core::protocol::{zel_service, Extensions, RequestContext, RpcServerBuilder};
use zel_core::IrohBundle;

// Global trace ID counter
static TRACE_COUNTER: AtomicU64 = AtomicU64::new(1);

// ============================================================================
// Server-Level Extensions (Shared Across ALL Connections)
// ============================================================================

/// Real SQLite database pool - shared across all connections
type DatabasePool = SqlitePool;

async fn init_database(pool: &SqlitePool) -> anyhow::Result<()> {
    // Create users table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            email TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    println!("[DB] Database initialized with users table");
    Ok(())
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
        let session = ctx
            .connection_extensions()
            .get::<UserSession>()
            .ok_or("User not authenticated")?;

        // Check permissions
        if !session.has_permission("write") {
            return Err("Insufficient permissions".to_string());
        }

        println!("  Authenticated as: {} (CONNECTION ext)", session.user_id);

        // Access REQUEST extensions (per-request trace ID and timing)
        let trace_id = ctx.extensions().get::<TraceId>().ok_or("No trace ID")?;
        let timing = ctx.extensions().get::<RequestTiming>().ok_or("No timing")?;

        let req_num = counter.increment();

        // Log the request with all context
        if config.enable_logging {
            println!("\nCREATE USER REQUEST");
            println!("  Trace ID: {} (REQUEST ext)", trace_id.0);
            println!("  Remote Peer: {}", ctx.remote_id());
            println!("  Session User: {} (CONNECTION ext)", session.user_id);
            println!("  User Name: {}", user.name);
            println!("  Global Request Count: {} (SERVER ext)", req_num);
        }

        // Insert user into database
        let result = sqlx::query("INSERT INTO users (name, email) VALUES (?, ?)")
            .bind(&user.name)
            .bind(&user.email)
            .execute(&*db_pool)
            .await
            .map_err(|e| e.to_string())?;

        let user_id = result.last_insert_rowid();
        println!("[DB] Inserted user with ID: {}", user_id);

        // Log timing
        println!("  Request Duration: {:?}", timing.elapsed());

        Ok(UserId {
            id: user_id.to_string(),
        })
    }

    async fn get_user(&self, ctx: RequestContext, user_id: String) -> Result<UserData, String> {
        // Check authentication
        let session = ctx
            .connection_extensions()
            .get::<UserSession>()
            .ok_or("Not authenticated")?;

        if !session.has_permission("read") {
            return Err("Insufficient permissions".to_string());
        }

        let db_pool = ctx
            .server_extensions()
            .get::<DatabasePool>()
            .ok_or("Database pool not configured")?;

        let counter = ctx
            .server_extensions()
            .get::<Arc<RequestCounter>>()
            .ok_or("Request counter not found")?;

        let trace_id = ctx.extensions().get::<TraceId>();
        let req_num = counter.increment();

        println!("\nGET USER REQUEST");
        println!(
            "  Trace ID: {} (REQUEST ext)",
            trace_id.map(|t| t.0).unwrap_or(0)
        );
        println!("  User ID: {}", user_id);
        println!("  Session User: {} (CONNECTION ext)", session.user_id);
        println!("  Remote Peer: {}", ctx.remote_id());
        println!("  Global Request Count: {} (SERVER ext)", req_num);

        // Query user from database
        let user_id_i64: i64 = user_id.parse().map_err(|_| "Invalid user ID")?;
        let row = sqlx::query("SELECT name, email FROM users WHERE id = ?")
            .bind(user_id_i64)
            .fetch_one(&*db_pool)
            .await
            .map_err(|e| e.to_string())?;

        let name: String = row.get(0);
        let email: String = row.get(1);

        println!("[DB] Retrieved user: {} ({})", name, email);

        Ok(UserData { name, email })
    }

    async fn get_stats(&self, ctx: RequestContext) -> Result<String, String> {
        let counter = ctx
            .server_extensions()
            .get::<Arc<RequestCounter>>()
            .ok_or("Request counter not found")?;

        let session = ctx.connection_extensions().get::<UserSession>();
        let req_num = counter.increment();

        println!("\nGET STATS REQUEST");
        println!("  Total Requests So Far: {} (SERVER ext)", counter.get());
        println!("  This Request Number: {} (SERVER ext)", req_num);
        if let Some(s) = session {
            println!("  Session User: {} (CONNECTION ext)", s.user_id);
            println!(
                "  Session Age: {:?} (CONNECTION ext)",
                s.authenticated_at.elapsed()
            );
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
    println!("║   Context & Extensions Demo - Three-Tier System        ║");
    println!("╚════════════════════════════════════════════════════════╝\n");

    // ========================================================================
    // Setup Server Extensions (Shared Across ALL Connections)
    // ========================================================================

    // Create an in-memory SQLite database pool
    let db_pool = SqlitePool::connect("sqlite::memory:").await?;

    // Initialize the database schema
    init_database(&db_pool).await?;

    let config = Arc::new(ServerConfig {
        max_request_size: 1024 * 1024,
        enable_logging: true,
    });
    let counter = Arc::new(RequestCounter::new());

    let server_extensions = Extensions::new()
        .with(db_pool.clone())
        .with(config.clone())
        .with(counter.clone());

    // ========================================================================
    // Build Server with Extensions
    // ========================================================================
    let mut server_bundle = IrohBundle::builder(None).await?;

    let user_service = UserServiceImpl;

    // Add connection hook for authentication/session setup
    let server = RpcServerBuilder::new(b"user/1", server_bundle.endpoint().clone())
        .with_extensions(server_extensions)
        .with_connection_hook(|conn, _server_ext| {
            let remote_id = conn.remote_id();
            Box::pin(async move {
                // Simulate authentication - create a session for this connection
                let user_id = format!("user_{}", &remote_id.to_string()[..8]);
                let session = UserSession::new(user_id);
                Ok(Extensions::new().with(session))
            })
        })
        .with_request_middleware(|ctx| {
            Box::pin(async move {
                // Add trace ID and timing to every request
                let trace_id = TraceId::new(TRACE_COUNTER.fetch_add(1, Ordering::SeqCst));
                let timing = RequestTiming::new();
                ctx.with_extension(trace_id).with_extension(timing)
            })
        })
        .service("user");

    let server = user_service.into_service_builder(server).build().build();

    let server_bundle = server_bundle.accept(b"user/1", server).finish().await;

    // ========================================================================
    // Create Multiple Clients (to demonstrate connection isolation)
    // ========================================================================
    let client1_bundle = IrohBundle::builder(None).await?.finish().await;
    let client2_bundle = IrohBundle::builder(None).await?.finish().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn1 = client1_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"user/1")
        .await?;

    let conn2 = client2_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"user/1")
        .await?;

    let client1 = zel_core::protocol::client::RpcClient::new(conn1).await?;
    let user_client1 = UserServiceClient::new(client1);

    let client2 = zel_core::protocol::client::RpcClient::new(conn2).await?;
    let user_client2 = UserServiceClient::new(client2);

    // ========================================================================
    // Demonstrate Extension Usage
    // ========================================================================

    println!("═══════════════════════════════════════════════════════");
    println!("  Testing Three-Tier Extension System");
    println!("═══════════════════════════════════════════════════════\n");

    // ========================================================================
    // Client 1: Create user
    // ========================================================================
    println!("CLIENT 1: Creating user...");
    let user1 = UserData {
        name: "Alice Smith".to_string(),
        email: "alice@example.com".to_string(),
    };

    let user_id1 = user_client1.create_user(user1).await?;
    println!("Client 1 created: {}\n", user_id1.id);

    tokio::time::sleep(Duration::from_millis(100)).await;

    // ========================================================================
    // Client 2: Create different user (shows isolated connection extensions)
    // ========================================================================
    println!("CLIENT 2: Creating user...");
    let user2 = UserData {
        name: "Bob Jones".to_string(),
        email: "bob@example.com".to_string(),
    };

    let user_id2 = user_client2.create_user(user2).await?;
    println!("Client 2 created: {}\n", user_id2.id);

    tokio::time::sleep(Duration::from_millis(100)).await;

    // ========================================================================
    // Client 1: Get user (shows same session, different trace ID)
    // ========================================================================
    println!("CLIENT 1: Getting users...");
    let user_data = user_client1.get_user(user_id1.id.clone()).await?;
    println!(
        "Client 1 retrieved: {} ({})\n",
        user_data.name, user_data.email
    );

    let user_data = user_client1.get_user(user_id2.id.clone()).await?;
    println!(
        "Client 1 retrieved: {} ({})\n",
        user_data.name, user_data.email
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    // ========================================================================
    // Client 2: Get stats (shows different session, shared counter)
    // ========================================================================
    println!("CLIENT 2: Getting stats...");
    let stats = user_client2.get_stats().await?;
    println!("Client 2 stats: {}\n", stats);

    println!("\n═══════════════════════════════════════════════════════");
    println!("  Extension System Summary");
    println!("═══════════════════════════════════════════════════════");
    println!("\n  [SERVER Extensions] - Shared across ALL connections:");
    println!("    • Request counter: {} total requests", counter.get());
    println!("    • SQLite pool: Both clients used same database");
    println!("    • See 'Global Request Count' incrementing 1→2→3→4→5");
    println!("\n  [CONNECTION Extensions] - Isolated per connection:");
    println!(
        "    • Client 1: user_{}",
        &client1_bundle.endpoint.id().to_string()[..8]
    );
    println!(
        "    • Client 2: user_{}",
        &client2_bundle.endpoint.id().to_string()[..8]
    );
    println!("    • See different 'Session User' values above");
    println!("\n  [REQUEST Extensions] - Unique per request:");
    println!("    • Trace IDs: 1 → 2 → 3 → 4 → 5 (one per request)");
    println!("    • See 'Trace ID' incrementing in logs above");
    println!("═══════════════════════════════════════════════════════\n");

    server_bundle.shutdown(Duration::from_secs(1)).await?;
    println!("Server shut down successfully");

    Ok(())
}
