# Extensions System

The Zel RPC framework uses a "three-tier" extension system to share context across different scopes: server-wide, per-connection, and per-request. But, what does that actually mean?

## Three-Tier System

Zel applications operate across 3 separate concern boundaries.

1. Server - The lowest Iroh layer
2. Connection - Per Iroh connection (session/auth state)
3. Request - Which are handled by resources inside your services

### 1. Server Extensions
**Scope**: Shared across ALL connections  
**Lifetime**: For the entire server lifetime  
**Use Cases**: Database pools, configuration, global metrics

```rust
let server_extensions = Extensions::new()
    .with(database_pool)
    .with(config)
    .with(metrics_counter);

RpcServerBuilder::new(alpn, endpoint)
    .with_extensions(server_extensions)
    // ...
```

### 2. Connection Extensions
**Scope**: Per-connection (isolated between connections)  
**Lifetime**: For the duration of a single connection  
**Use Cases**: User sessions, authentication state, per-peer metrics

Connection hooks receive server extensions and return connection-specific extensions:

```rust
.with_connection_hook(|conn, _server_exts| {
    Box::pin(async move {
        let user_id = authenticate(&conn).await?;
        let session = UserSession::new(user_id);
        
        // Server extensions are automatically merged with this
        Ok(Extensions::new().with(session))
    })
})
```

**Internally**, the framework merges these:
```rust
// Pseudocode - actual implementation
let connection_extensions = server_extensions.clone()
    .with(session_from_hook);
```

### 3. Request Extensions  
**Scope**: Per-request (unique to each resource invocation)  
**Lifetime**: For a single request 
**Use Cases**: Trace IDs, request timing, per-call context

Request middleware receives the current context and returns enriched extensions:

```rust
.with_request_middleware(|ctx| {
    Box::pin(async move {
        let trace_id = generate_trace_id();
        let timing = RequestTiming::new();
        
        // Context already has server + connection extensions
        ctx.with_extension(trace_id)
           .with_extension(timing)
    })
})
```

## Data Flow

```
Server Extensions (db_pool, config, counter)
    ↓ clone + merge
Connection Extensions (db_pool, config, counter, session)
    ↓ clone + merge  
Request Extensions (db_pool, config, counter, session, trace_id, timing)
```

**Takeaways:**
- **Isolated**: Changes at request level don't affect connection/server (UNLESS you're using interior mutability)
- **Shared**: Arc-shared resources like Database pools and configs added at lower levels shared upwards (not copied)
- **Layering**: Each tier builds upon the previous tier

## Shared Mutable State (Interior Mutability)

By default, extensions provide **immutable sharing** - values are shared across tiers but changes at one tier don't affect others. However, when you need **mutable shared state**, use interior mutability patterns.

### When to Use Interior Mutability

- **Shared counters/metrics**: Track requests across all tiers
- **Caches**: Share cached data that can be updated
- **Rate limiters**: Coordinate limits across requests
- **Connection pools**: Manage shared connection state

### Example: Shared Request Counter

```rust
use std::sync::Arc;
use tokio::sync::RwLock;

// Shared counter that can be mutated across all tiers
#[derive(Clone)]
struct RequestCounter {
    count: Arc<RwLock<u64>>,
}

impl RequestCounter {
    fn new() -> Self {
        Self {
            count: Arc::new(RwLock::new(0)),
        }
    }
    
    async fn increment(&self) -> u64 {
        let mut count = self.count.write().await;
        *count += 1;
        *count
    }
    
    async fn get(&self) -> u64 {
        *self.count.read().await
    }
}

// Add to server extensions
let counter = RequestCounter::new();
let server_extensions = Extensions::new()
    .with(database_pool)
    .with(counter);

// Access from any tier - all share the same counter
async fn handler(ctx: RequestContext) {
    let counter = ctx.server_extensions()
        .get::<RequestCounter>()
        .expect("Counter not found");
    
    let count = counter.increment().await;
    println!("Total requests: {}", count);
}
```

### Key Points

- Store `Arc<Mutex<T>>` or `Arc<RwLock<T>>` in Extensions (not just `Arc<T>`)
- The `Arc` in `Arc<Mutex<T>>` is shared across tiers (efficient)
- The `Mutex`/`RwLock` allows mutation of the inner value
- Changes are visible across all tiers that hold a reference
- Use `RwLock` when reads are more frequent than writes

## Important: `get_or_else` Doesn't Break Isolation

The `get_or_else()` method is convenient for providing defaults, but it's important to understand that it **doesn't modify** the Extensions instance:

```rust
// This does NOT add MyValue to the extensions
let value = ctx.extensions().get_or_else(|| MyValue::new());
// value is just Arc<MyValue>, ctx.extensions() is unchanged
```

If you want to actually add a value to extensions with a default, you need to use `with()`:

```rust
// This creates a NEW context with MyValue added
let new_ctx = if !ctx.extensions().contains::<MyValue>() {
    ctx.with_extension(MyValue::new())
} else {
    ctx
};
```

**Why this matters:**
- `get_or_else()` only returns an `Arc<T>` - it doesn't mutate Extensions
- Tier isolation is preserved - calling `get_or_else()` on request tier doesn't affect connection/server
- If you need to actually add the value to extensions, use `with()` which creates a new Extensions instance

## Usage in Handlers

Access extensions via `RequestContext`:

```rust
async fn create_user(&self, ctx: RequestContext, user: UserData) -> Result<UserId, String> {
    // Server extensions (shared across all connections)
    let db_pool = ctx.server_extensions()
        .get::<DatabasePool>()
        .ok_or("Database pool not configured")?;
    
    // Connection extensions (per-connection)
    let session = ctx.connection_extensions()
        .get::<UserSession>()
        .ok_or("Not authenticated")?;
    
    // Request extensions (per-request)
    let trace_id = ctx.extensions()
        .get::<TraceId>()
        .ok_or("No trace ID")?;
    
    // Use the extensions...
}
```

## Custom Extension Tiers

The three-tier system (Server/Connection/Request) is what the **framework** provides, but you can create additional tiers for your application's specific needs.

### Example: Background Tasks

When spawning background tasks, create a new extension scope:

```rust
async fn process_order(ctx: RequestContext, order: Order) -> Result<OrderId, Error> {
    // ctx has: server + connection + request extensions
    
    // Create a subtask tier for background processing
    let subtask_exts = ctx.extensions().clone()
        .with(SubTaskId::generate())
        .with(SubTaskMetrics::new());
    
    // Spawn with its own isolated extension scope
    tokio::spawn(async move {
        send_confirmation_email(subtask_exts, order).await;
    });
    
    // Main request continues with original context
    Ok(order.id)
}
```

### Example: Parallel Processing

Create separate extension contexts for concurrent work:

```rust
async fn batch_process(ctx: RequestContext, items: Vec<Item>) -> Result<(), Error> {
    let mut tasks = Vec::new();
    
    for (i, item) in items.into_iter().enumerate() {
        // Each task gets: server + connection + request + task-specific extensions
        let task_exts = ctx.extensions().clone()
            .with(TaskNumber(i))
            .with(ItemId(item.id));
        
        tasks.push(tokio::spawn(async move {
            process_item(task_exts, item).await
        }));
    }
    
    // All tasks share database pools, but have isolated task-specific data
    for task in tasks {
        let _ = task.await;
    }
    Ok(())
}
```

### Why This Works

- **Arc-Sharing**: Database pools and config from parent tiers are still efficiently shared (not copied)
- **Isolation**: Each custom tier's extensions don't affect siblings or parents
- **Flexibility**: Create as many tiers as your application needs
- **Type-Safety**: Same type-safe API (`get::<T>()`) works at any tier depth

The framework's three tiers are **conventions**, not limitations. The Extensions API scales to any hierarchy your application requires.