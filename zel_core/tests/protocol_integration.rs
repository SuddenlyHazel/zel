use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use zel_core::protocol::{
    client::{ClientError, RpcClient},
    error_classification::ErrorSeverity as ErrorSeverityType,
    Body, ResourceError, Response, RpcServerBuilder, SubscriptionMsg,
};
use zel_core::IrohBundle;

#[tokio::test]
async fn test_basic_rpc_call() {
    // Setup server
    let mut server_bundle = IrohBundle::builder(None).await.unwrap();

    let server = RpcServerBuilder::new(b"test-rpc/1", server_bundle.endpoint().clone())
        .service("math")
        .rpc_resource("add", |_conn, req| {
            Box::pin(async move {
                // Parse the body as two numbers
                if let Body::Rpc(data) = &req.body {
                    let numbers: Vec<i32> =
                        serde_json::from_slice(data).map_err(|e| ResourceError::CallbackError {
                            message: e.to_string(),
                            severity: ErrorSeverityType::Application,
                            context: None,
                        })?;
                    let sum = numbers.iter().sum::<i32>();
                    let result = serde_json::to_vec(&sum)
                        .map_err(|e| ResourceError::SerializationError(e.to_string()))?;
                    Ok(Response {
                        data: Bytes::from(result),
                    })
                } else {
                    Err(ResourceError::CallbackError {
                        message: "Expected RPC body".into(),
                        severity: ErrorSeverityType::Application,
                        context: None,
                    })
                }
            })
        })
        .build()
        .build();

    let server_bundle = server_bundle.accept(b"test-rpc/1", server).finish().await;

    // Setup client
    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"test-rpc/1")
        .await
        .unwrap();

    let client = RpcClient::new(conn).await.unwrap();

    // Test RPC call
    let numbers = vec![1, 2, 3, 4, 5];
    let body = serde_json::to_vec(&numbers).unwrap();
    let response = client.call("math", "add", Bytes::from(body)).await.unwrap();

    let sum: i32 = serde_json::from_slice(&response.data).unwrap();
    assert_eq!(sum, 15);

    // Cleanup
    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_subscription_lifecycle() {
    // Setup server
    let mut server_bundle = IrohBundle::builder(None).await.unwrap();

    let server = RpcServerBuilder::new(b"test-sub/1", server_bundle.endpoint().clone())
        .service("events")
        .subscription_resource("stream", |_conn, _req, mut sink| {
            Box::pin(async move {
                // Send 5 messages
                for i in 1..=5 {
                    let msg = SubscriptionMsg::Data(Bytes::from(format!("Message {}", i)));
                    let msg_bytes = serde_json::to_vec(&msg).unwrap();
                    if sink.send(msg_bytes.into()).await.is_err() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }

                // Send stopped message
                let msg = SubscriptionMsg::Stopped;
                let msg_bytes = serde_json::to_vec(&msg).unwrap();
                let _ = sink.send(msg_bytes.into()).await;

                Ok(Response { data: Bytes::new() })
            })
        })
        .build()
        .build();

    let server_bundle = server_bundle.accept(b"test-sub/1", server).finish().await;

    // Setup client
    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"test-sub/1")
        .await
        .unwrap();

    let client = RpcClient::new(conn).await.unwrap();

    // Subscribe
    let mut stream = client.subscribe("events", "stream", None).await.unwrap();

    // Receive messages
    let mut count = 0;
    while let Some(result) = stream.next().await {
        match result.unwrap() {
            SubscriptionMsg::Data(_) => {
                count += 1;
            }
            SubscriptionMsg::Stopped => {
                break;
            }
            _ => {}
        }
    }

    assert_eq!(count, 5);

    // Cleanup
    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_multiple_concurrent_subscriptions() {
    // Setup server with counter that increments independently per subscription
    let mut server_bundle = IrohBundle::builder(None).await.unwrap();

    let server = RpcServerBuilder::new(b"test-multi/1", server_bundle.endpoint().clone())
        .service("counters")
        .subscription_resource("count", |_conn, _req, mut sink| {
            Box::pin(async move {
                // Each subscription gets its own counter
                for i in 1..=3 {
                    let msg = SubscriptionMsg::Data(Bytes::from(i.to_string()));
                    let msg_bytes = serde_json::to_vec(&msg).unwrap();
                    if sink.send(msg_bytes.into()).await.is_err() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Ok(Response { data: Bytes::new() })
            })
        })
        .build()
        .build();

    let server_bundle = server_bundle.accept(b"test-multi/1", server).finish().await;

    // Setup client
    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"test-multi/1")
        .await
        .unwrap();

    let client = RpcClient::new(conn).await.unwrap();

    // Create two subscriptions
    let mut stream1 = client.subscribe("counters", "count", None).await.unwrap();
    let mut stream2 = client.subscribe("counters", "count", None).await.unwrap();

    // Collect from both streams
    let mut count1 = 0;
    let mut count2 = 0;

    loop {
        tokio::select! {
            Some(result) = stream1.next() => {
                if let Ok(SubscriptionMsg::Data(_)) = result {
                    count1 += 1;
                }
            }
            Some(result) = stream2.next() => {
                if let Ok(SubscriptionMsg::Data(_)) = result {
                    count2 += 1;
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                break;
            }
        }

        if count1 >= 3 && count2 >= 3 {
            break;
        }
    }

    assert_eq!(count1, 3);
    assert_eq!(count2, 3);

    // Cleanup
    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_error_handling_service_not_found() {
    let mut server_bundle = IrohBundle::builder(None).await.unwrap();

    let server = RpcServerBuilder::new(b"test-err/1", server_bundle.endpoint().clone())
        .service("existing")
        .rpc_resource("test", |_conn, _req| {
            Box::pin(async move { Ok(Response { data: Bytes::new() }) })
        })
        .build()
        .build();

    let server_bundle = server_bundle.accept(b"test-err/1", server).finish().await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"test-err/1")
        .await
        .unwrap();

    let client = RpcClient::new(conn).await.unwrap();

    // Try to call non-existent service
    let result = client.call("nonexistent", "test", Bytes::new()).await;

    assert!(result.is_err());
    if let Err(ClientError::Resource(ResourceError::ServiceNotFound { service })) = result {
        assert_eq!(service, "nonexistent");
    } else {
        panic!("Expected ServiceNotFound error");
    }

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_error_handling_resource_not_found() {
    let mut server_bundle = IrohBundle::builder(None).await.unwrap();

    let server = RpcServerBuilder::new(b"test-err2/1", server_bundle.endpoint().clone())
        .service("test_service")
        .rpc_resource("existing_resource", |_conn, _req| {
            Box::pin(async move { Ok(Response { data: Bytes::new() }) })
        })
        .build()
        .build();

    let server_bundle = server_bundle.accept(b"test-err2/1", server).finish().await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"test-err2/1")
        .await
        .unwrap();

    let client = RpcClient::new(conn).await.unwrap();

    // Try to call non-existent resource
    let result = client
        .call("test_service", "nonexistent", Bytes::new())
        .await;

    assert!(result.is_err());
    if let Err(ClientError::Resource(ResourceError::ResourceNotFound { service, resource })) =
        result
    {
        assert_eq!(service, "test_service");
        assert_eq!(resource, "nonexistent");
    } else {
        panic!("Expected ResourceNotFound error");
    }

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_client_drop_unsubscribe() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    let completed = Arc::new(AtomicBool::new(false));
    let completed_clone = completed.clone();

    let mut server_bundle = IrohBundle::builder(None).await.unwrap();

    let server = RpcServerBuilder::new(b"test-drop/1", server_bundle.endpoint().clone())
        .service("infinite")
        .subscription_resource("stream", move |_conn, _req, mut sink| {
            let completed = completed_clone.clone();
            Box::pin(async move {
                // Try to send infinite messages
                for i in 1..=1000 {
                    let msg = SubscriptionMsg::Data(Bytes::from(i.to_string()));
                    let msg_bytes = serde_json::to_vec(&msg).unwrap();
                    if sink.send(msg_bytes.into()).await.is_err() {
                        // Client dropped, we should stop
                        completed.store(true, Ordering::SeqCst);
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Ok(Response { data: Bytes::new() })
            })
        })
        .rpc_resource("ping", |_conn, _req| {
            Box::pin(async move {
                // Simple ping endpoint to test client still works
                Ok(Response {
                    data: Bytes::from("pong"),
                })
            })
        })
        .build()
        .build();

    let server_bundle = server_bundle.accept(b"test-drop/1", server).finish().await;

    let client_bundle = IrohBundle::builder(None).await.unwrap().finish().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let conn = client_bundle
        .endpoint
        .connect(server_bundle.endpoint.id(), b"test-drop/1")
        .await
        .unwrap();

    let client = RpcClient::new(conn).await.unwrap();

    {
        // Subscribe and receive a few messages
        let mut stream = client.subscribe("infinite", "stream", None).await.unwrap();

        // Receive 3 messages then drop
        for _ in 0..3 {
            let _ = stream.next().await;
        }

        // Drop the stream here
    }

    // Wait a bit for server to detect the drop
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Server task should have completed due to client drop
    assert!(completed.load(Ordering::SeqCst));

    // Verify client can still make RPC calls after subscription stream is dropped
    let response = client.call("infinite", "ping", Bytes::new()).await.unwrap();
    assert_eq!(response.data, Bytes::from("pong"));

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_multiple_clients_concurrent_calls() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let mut server_bundle = IrohBundle::builder(None).await.unwrap();

    let server = RpcServerBuilder::new(b"test-multi-client/1", server_bundle.endpoint().clone())
        .service("counter")
        .rpc_resource("increment", move |_conn, _req| {
            let call_count = call_count_clone.clone();
            Box::pin(async move {
                let count = call_count.fetch_add(1, Ordering::SeqCst) + 1;
                let result = serde_json::to_vec(&count).unwrap();
                Ok(Response {
                    data: Bytes::from(result),
                })
            })
        })
        .build()
        .build();

    let server_bundle = server_bundle
        .accept(b"test-multi-client/1", server)
        .finish()
        .await;

    // Create three separate clients
    let client_bundle1 = IrohBundle::builder(None).await.unwrap().finish().await;
    let client_bundle2 = IrohBundle::builder(None).await.unwrap().finish().await;
    let client_bundle3 = IrohBundle::builder(None).await.unwrap().finish().await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn1 = client_bundle1
        .endpoint
        .connect(server_bundle.endpoint.id(), b"test-multi-client/1")
        .await
        .unwrap();
    let conn2 = client_bundle2
        .endpoint
        .connect(server_bundle.endpoint.id(), b"test-multi-client/1")
        .await
        .unwrap();
    let conn3 = client_bundle3
        .endpoint
        .connect(server_bundle.endpoint.id(), b"test-multi-client/1")
        .await
        .unwrap();

    let client1 = RpcClient::new(conn1).await.unwrap();
    let client2 = RpcClient::new(conn2).await.unwrap();
    let client3 = RpcClient::new(conn3).await.unwrap();

    // Make concurrent calls from all three clients
    let (r1, r2, r3) = tokio::join!(
        client1.call("counter", "increment", Bytes::new()),
        client2.call("counter", "increment", Bytes::new()),
        client3.call("counter", "increment", Bytes::new()),
    );

    // All should succeed
    assert!(r1.is_ok());
    assert!(r2.is_ok());
    assert!(r3.is_ok());

    // Make more calls to ensure continued connectivity
    let (r1, r2, r3) = tokio::join!(
        client1.call("counter", "increment", Bytes::new()),
        client2.call("counter", "increment", Bytes::new()),
        client3.call("counter", "increment", Bytes::new()),
    );

    assert!(r1.is_ok());
    assert!(r2.is_ok());
    assert!(r3.is_ok());

    // Total of 6 calls should have been made
    assert_eq!(call_count.load(Ordering::SeqCst), 6);

    server_bundle
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

/// This test validates the critical claim in Extensions::with():
///
/// Extensions internally stores data as: Arc<HashMap<TypeId, Arc<dyn Any>>>
///                                       ^outer Arc    ^HashMap  ^inner Arc for each value
///
/// When with() is called, it must:
/// 1. Clone the HashMap itself (create a new HashMap) - NOT just clone the outer Arc
/// 2. Clone the inner Arc pointers (share values efficiently) - NOT deep-copy values
///
/// This is achieved via: (*self.map).clone()
///                       ^dereference outer Arc, then clone the HashMap
///
/// Some definitions, because this is a bit confusion:
/// - Server Extensions: Shared across all connections (database pools, config)
/// - Connection Extensions: Server extensions + per-connection state (sessions)
/// - Request Extensions: Connection extensions + per-request data (trace IDs)
#[test]
fn test_extensions_with_clones_hashmap_not_arc() {
    use std::sync::Arc;
    use zel_core::protocol::Extensions;

    // Simulate a database pool that would be in server extensions
    #[derive(Clone)]
    struct DbPool {
        name: String,
    }

    // ========================================================================
    // TEST 1: Server → Connection extension flow (separate HashMaps)
    // ========================================================================
    // Simulates: Server extensions are cloned, then connection-specific data is added

    // Server extensions: shared database pool
    let server_extensions = Extensions::new().with(DbPool {
        name: "postgres".to_string(),
    });

    // Connection extensions: clone server extensions + add session
    // This is what happens in connection hooks
    let connection_extensions = server_extensions
        .clone()
        .with(String::from("user_session_abc"));

    // Server extensions should NOT have the session
    // (So, separate HashMaps for server vs connection tiers)
    assert!(
        !server_extensions.contains::<String>(),
        "Server extensions should not have session - proves connection isolation"
    );

    // Connection extensions should have BOTH database pool and session
    assert!(connection_extensions.contains::<DbPool>());
    assert!(connection_extensions.contains::<String>());

    // ========================================================================
    // TEST 2: Shared values across tiers (Arc pointers cloned, not data)
    // ========================================================================
    // So, database pool is shared (not copied) from server → connection → request

    // Server extensions with database pool
    let server_exts = Extensions::new().with(DbPool {
        name: "shared_pool".to_string(),
    });

    // Connection extensions: clone server + add connection data
    let conn_exts = server_exts.clone().with(42u32); // connection ID

    // Final stage (but at this point we can be sure it'll work the deeper we go)
    // Request extensions: clone connection + add request data
    let request_exts = conn_exts.clone().with(999u64); // trace ID

    // Retrieve the database pool from all three tiers
    let pool_from_server = server_exts.get::<DbPool>().unwrap();
    let pool_from_conn = conn_exts.get::<DbPool>().unwrap();
    let pool_from_request = request_exts.get::<DbPool>().unwrap();

    // All three should point to the SAME Arc
    // Yes, this is how an Arc works but I dont want to deal with headaches later..
    assert!(
        Arc::ptr_eq(&pool_from_server, &pool_from_conn),
        "Server and connection should share same Arc"
    );
    assert!(
        Arc::ptr_eq(&pool_from_conn, &pool_from_request),
        "Connection and request should share same Arc"
    );

    // Trace ID should NOT propagate up to parent tiers
    // So, we have tier isolation. Changes at request level don't affect connection/server
    assert!(
        !server_exts.contains::<u64>(),
        "Server extensions should not have trace ID - proves upward isolation"
    );
    assert!(
        !conn_exts.contains::<u64>(),
        "Connection extensions should not have trace ID - proves upward isolation"
    );
    assert!(
        request_exts.contains::<u64>(),
        "Request extensions should have trace ID"
    );
}
