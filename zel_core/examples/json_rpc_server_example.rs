//! Complete JSON-RPC server example over Iroh
//!
//! This example demonstrates:
//! - Building an RPC module with methods
//! - Creating a JSON-RPC handler
//! - Registering with Iroh endpoint
//! - Running alongside existing protocols

use futures::lock;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::rpc_params;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use zel_core::IrohBundle;
use zel_core::request_reply::json_rpc::RpcError;
use zel_core::request_reply::json_rpc::{RpcModule, ServerBuilder, build_client};

type SomeCtx = Arc<Mutex<BTreeMap<String, String>>>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter(Some("iroh"), log::LevelFilter::Off)
        .filter(Some("tracing"), log::LevelFilter::Off)
        .init();

    println!("JSON-RPC Server Example");
    println!("======================\n");

    let ctx = Arc::new(Mutex::new(BTreeMap::new()));
    // Step 1: Build RPC module with methods
    let rpc_module = build_rpc_module(ctx.clone())?;
    println!(
        "✓ RPC module built with {} methods",
        rpc_module.method_names().count()
    );

    // Step 2: Create JSON-RPC handler
    let handler = ServerBuilder::new()
        .max_request_size(5 * 1024 * 1024) // 5MB requests
        .max_response_size(10 * 1024 * 1024) // 10MB responses
        .build(rpc_module)?;

    println!("✓ JSON-RPC handler created");

    // Step 3: Set up Iroh endpoint with handler
    let bundle = IrohBundle::builder(None)
        .await?
        .accept(b"jsonrpc/1", handler)
        .finish()
        .await;

    tokio::time::sleep(Duration::from_secs(3)).await;
    let client_bundle = IrohBundle::builder(None).await?.finish().await;

    let client = build_client(&client_bundle.endpoint, bundle.endpoint.id(), b"jsonrpc/1").await?;

    let result: String = client.request("say_hello", rpc_params![]).await?;
    println!("server said {result}");

    let result: u64 = client.request("add", rpc_params![1, 1]).await?;
    println!("server said 1+1={result}");

    let result: String = client
        .request("echo", rpc_params!["Hello, JsonRPC!"])
        .await?;
    println!("server echod {result}");

    let result: String = client.request("async_operation", rpc_params![1000]).await?;
    println!("server should have waited atleast 1000ms.. Did it? time_ms {result}");
    {
        let locked = ctx.lock().await;
        println!("key sleep should be 1000ms {:?}", locked.get("sleep"));
    }
    let result = client
        .request::<f64, _>("divide", rpc_params![100.0, 0.0])
        .await;
    println!("Server should give us an error because we cant divide by zero.. yet.. {result:?}");

    // Step 4: Run until Ctrl+C
    tokio::signal::ctrl_c().await?;

    println!("\nShutting down gracefully...");
    bundle.shutdown(Duration::from_secs(5)).await?;

    println!("✓ Server shut down successfully");
    Ok(())
}

/// Build an RPC module with example methods
fn build_rpc_module(ctx: SomeCtx) -> anyhow::Result<RpcModule<SomeCtx>> {
    let mut module = RpcModule::new(ctx);

    // Simple method: say_hello
    module.register_method("say_hello", |_, _, _| {
        log::info!("say_hello called");
        Ok::<_, RpcError>("Hello from JSON-RPC server!".to_string())
    })?;

    // Method with parameters: add two numbers
    module.register_method("add", |params, _, _| {
        let params: (u64, u64) = params.parse()?;
        log::info!("add called with {} + {}", params.0, params.1);
        Ok::<_, RpcError>(params.0 + params.1)
    })?;

    // Method with complex parameters: echo
    module.register_method("echo", |params, _, _| {
        let message: String = params.one()?;
        log::info!("echo called with: {}", message);
        Ok::<_, RpcError>(message)
    })?;

    // Async method example
    module.register_async_method("async_operation", |params, ctx, _| async move {
        let delay_ms: u64 = params.one().unwrap_or(100);
        log::info!("async_operation called with {}ms delay", delay_ms);
        let mut locked = ctx.lock().await;
        locked.insert("sleep".into(), format!("{delay_ms}"));
        // Simulate async work
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

        Ok::<_, RpcError>(format!("Completed after {}ms", delay_ms))
    })?;

    // Method that can return an error
    module.register_method("divide", |params, _, _| {
        let params: (f64, f64) = params.parse()?;
        log::info!("divide called with {} / {}", params.0, params.1);

        if params.1 == 0.0 {
            return Err(RpcError::owned(-32000, "Division by zero", None::<String>));
        }

        Ok::<_, RpcError>(params.0 / params.1)
    })?;

    Ok(module)
}
