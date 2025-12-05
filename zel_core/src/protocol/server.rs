use anyhow::Context;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    protocol::ProtocolHandler,
};
use log::{trace, warn};
use std::future::Future;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::protocol::{
    Extensions, Request, RequestContext, ResourceError, ResourceResponse, Response, ServiceMap,
    SubscriptionMsg,
};

// Lowest level will establish a bidi connection and dispatch all requests
// to child services
impl ProtocolHandler for super::RpcServer<'static> {
    fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> impl Future<Output = Result<(), iroh::protocol::AcceptError>> + Send {
        trace!("incoming connection from {}", connection.remote_id());
        let services = self.services.clone();
        let server_extensions = self.server_extensions.clone();
        let connection_hook = self.connection_hook.clone();
        let request_middleware = self.request_middleware.clone();
        let endpoint = self.endpoint.clone();
        let shutdown_signal = self.shutdown_signal.clone();
        let is_shutting_down = self.is_shutting_down.clone();
        let task_tracker = self.task_tracker.clone();

        async move {
            tokio::spawn(async move {
                if let Err(e) = connection_handler(
                    services,
                    server_extensions,
                    connection_hook,
                    request_middleware,
                    endpoint,
                    connection,
                    shutdown_signal,
                    is_shutting_down,
                    task_tracker,
                )
                .await
                {
                    log::error!("{e}");
                }
            });
            Ok(())
        }
    }
}

// Primary event loop for a single established connection. Waits for new stream requests and dispatches them upwards.
async fn connection_handler(
    service_map: ServiceMap<'static>,
    server_extensions: Extensions,
    connection_hook: Option<super::ConnectionHook>,
    request_middleware: Vec<super::RequestMiddleware>,
    endpoint: iroh::Endpoint,
    connection: Connection,
    shutdown_signal: std::sync::Arc<tokio::sync::Notify>,
    is_shutting_down: std::sync::Arc<std::sync::atomic::AtomicBool>,
    task_tracker: super::shutdown::TaskTracker,
) -> anyhow::Result<()> {
    use std::sync::atomic::Ordering;
    // Call connection hook to populate connection-scoped extensions
    let connection_extensions = if let Some(hook) = connection_hook {
        match hook(&connection, server_extensions.clone()).await {
            Ok(ext) => {
                log::trace!(
                    "Connection hook populated extensions for {}",
                    connection.remote_id()
                );
                ext
            }
            Err(e) => {
                log::warn!(
                    "Connection hook failed for {}: {}. Using empty extensions.",
                    connection.remote_id(),
                    e
                );
                Extensions::new()
            }
        }
    } else {
        Extensions::new()
    };

    // Loop: accept streams and spawn handlers
    loop {
        tokio::select! {
            // Shutdown signal received
            _ = shutdown_signal.notified() => {
                log::info!(
                    "Shutdown signal received for connection {}, stopping accept loop",
                    connection.remote_id()
                );
                break;
            }

            // Normal stream acceptance
            stream_result = connection.accept_bi() => {
                // Double-check shutdown state before spawning
                if is_shutting_down.load(Ordering::Relaxed) {
                    log::trace!(
                        "Rejecting new stream from {} during shutdown",
                        connection.remote_id()
                    );
                    break;
                }

                match stream_result {
                    Ok((tx, rx)) => {
                        // Increment task counter and get guard
                        let task_guard = task_tracker.increment();

                        // Spawn independent handler for THIS stream
                        let service_map = service_map.clone();
                        let server_ext = server_extensions.clone();
                        let conn_ext = connection_extensions.clone();
                        let middleware = request_middleware.clone();
                        let endpoint_clone = endpoint.clone();
                        let conn = connection.clone();
                        let conn_id = connection.remote_id();

                        let shutdown_sig = shutdown_signal.clone();

                        tokio::spawn(async move {
                            // Keep guard alive for the lifetime of this task
                            let _guard = task_guard;

                            if let Err(e) =
                                handle_stream(tx, rx, service_map, server_ext, conn_ext, middleware, endpoint_clone, conn, shutdown_sig)
                                    .await
                            {
                                log::error!("Stream handler error for {}: {e}", conn_id);
                            }
                            // Guard drops here, decrementing counter
                        });
                    }
                    Err(e) => {
                        log::trace!(
                            "Connection accept_bi failed for {}: {e}. Connection likely closed.",
                            connection.remote_id()
                        );
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

async fn handle_stream(
    tx: SendStream,
    rx: RecvStream,
    service_map: ServiceMap<'static>,
    server_extensions: Extensions,
    connection_extensions: Extensions,
    request_middleware: Vec<super::RequestMiddleware>,
    endpoint: iroh::Endpoint,
    connection: Connection,
    shutdown_signal: std::sync::Arc<tokio::sync::Notify>,
) -> anyhow::Result<()> {
    let mut tx = FramedWrite::new(tx, LengthDelimitedCodec::new());
    let mut rx = FramedRead::new(rx, LengthDelimitedCodec::new());

    // Read ONE request from THIS stream
    let req_bytes = match rx.next().await {
        Some(Ok(bytes)) => bytes,
        Some(Err(e)) => {
            log::error!(
                "Failed to read request from peer {}: {e}",
                connection.remote_id()
            );
            return Err(e.into());
        }
        None => {
            log::warn!(
                "Stream closed by peer {} before sending request",
                connection.remote_id()
            );
            return Ok(());
        }
    };

    let request: Request = match serde_json::from_slice(&req_bytes) {
        Ok(req) => req,
        Err(e) => {
            log::error!(
                "Peer {} sent malformed request: {e}",
                connection.remote_id()
            );
            return Err(e.into());
        }
    };

    // Route to service
    let Some(service) = service_map.get(request.service.as_str()) else {
        let error = ResourceError::ServiceNotFound {
            service: request.service.clone(),
        };
        let response: Result<Response, ResourceError> = Err(error);
        if let Ok(response_bytes) = serde_json::to_vec(&response) {
            let _ = tx.send(response_bytes.into()).await;
        }
        return Ok(());
    };

    // Route to resource
    let Some(resource) = service.resources.get(request.resource.as_str()) else {
        let error = ResourceError::ResourceNotFound {
            service: request.service.clone(),
            resource: request.resource.clone(),
        };
        let response: Result<Response, ResourceError> = Err(error);
        if let Ok(response_bytes) = serde_json::to_vec(&response) {
            let _ = tx.send(response_bytes.into()).await;
        }
        return Ok(());
    };

    // Handle based on resource type
    match resource {
        super::ResourceCallback::Rpc(callback) => {
            // Build RequestContext with all three extension tiers
            let mut ctx = RequestContext::new(
                connection.clone(),
                endpoint.clone(),
                request.service.clone(),
                request.resource.clone(),
                server_extensions.clone(),
                connection_extensions.clone(),
                shutdown_signal.clone(),
            );

            // Apply request middleware chain
            for middleware in &request_middleware {
                ctx = middleware(ctx).await;
            }

            let response: ResourceResponse = (callback)(ctx, request).await;
            let response = match response {
                Ok(r) => Ok(r),
                Err(e) => Err(ResourceError::CallbackError(e.to_string())),
            };

            let response_bytes = match serde_json::to_vec(&response) {
                Ok(bytes) => bytes,
                Err(e) => {
                    let error = ResourceError::SerializationError(e.to_string());
                    serde_json::to_vec(&Err::<Response, _>(error)).context(
                        "failed to serialize error response something really bad is happening",
                    )?
                }
            };

            if let Err(e) = tx.send(response_bytes.into()).await {
                log::error!(
                    "failed to send RPC response to peer {}: {e}",
                    connection.remote_id()
                );
            }

            // Stream will auto-close when this function returns
        }
        super::ResourceCallback::SubscriptionProducer(callback) => {
            // Open uni stream for subscription
            let Ok(sub_tx) = connection.open_uni().await else {
                let error = ResourceError::CallbackError("Failed to establish stream".into());
                let response: Result<Response, ResourceError> = Err(error);
                if let Ok(response_bytes) = serde_json::to_vec(&response) {
                    let _ = tx.send(response_bytes.into()).await;
                }
                return Ok(());
            };

            let ldc = LengthDelimitedCodec::new();
            let mut sub_tx = FramedWrite::new(sub_tx, ldc);

            // Send established ack on uni stream
            let ack = SubscriptionMsg::Established {
                service: request.service.clone(),
                resource: request.resource.clone(),
            };
            let ack = serde_json::to_vec(&ack)
                .context("failed to serialize ack message something really bad is happening")?;
            if let Err(e) = sub_tx.send(ack.into()).await {
                log::error!(
                    "attempted to send subscription ack to peer {} but failed {e}",
                    connection.remote_id()
                );
                let error = ResourceError::CallbackError(format!("Failed to send ack: {e}"));
                let response: Result<Response, ResourceError> = Err(error);
                if let Ok(response_bytes) = serde_json::to_vec(&response) {
                    let _ = tx.send(response_bytes.into()).await;
                }
                return Ok(());
            }

            // Send success response on request stream
            let response: Result<Response, ResourceError> = Ok(Response { data: Bytes::new() });
            let response_bytes = serde_json::to_vec(&response)
                .context("failed to serialize response something really bad is happening")?;
            if let Err(e) = tx.send(response_bytes.into()).await {
                log::error!(
                    "failed to send subscription response to peer {}: {e}",
                    connection.remote_id()
                );
                return Ok(());
            }

            // Spawn publisher task
            let callback = callback.to_owned();
            let conn_clone = connection.clone();
            let endpoint_clone = endpoint.clone();
            let server_ext_clone = server_extensions.clone();
            let conn_ext_clone = connection_extensions.clone();
            let middleware_clone = request_middleware.clone();
            let shutdown_sig_clone = shutdown_signal.clone();

            tokio::spawn(async move {
                let peer = conn_clone.remote_id();

                // Build RequestContext for subscription
                let mut ctx = RequestContext::new(
                    conn_clone,
                    endpoint_clone,
                    request.service.clone(),
                    request.resource.clone(),
                    server_ext_clone,
                    conn_ext_clone,
                    shutdown_sig_clone,
                );

                // Apply request middleware chain
                for middleware in &middleware_clone {
                    ctx = middleware(ctx).await;
                }

                if let Err(e) = (callback)(ctx, request, sub_tx).await {
                    warn!("subscription task for peer {} failed {e}", peer);
                }
            });
        }
        super::ResourceCallback::NotificationConsumer(callback) => {
            // Open bidirectional stream for notification (client sends, server acks)
            let Ok((notif_tx, notif_rx)) = connection.open_bi().await else {
                let error =
                    ResourceError::CallbackError("Failed to establish notification stream".into());
                let response: Result<Response, ResourceError> = Err(error);
                if let Ok(response_bytes) = serde_json::to_vec(&response) {
                    let _ = tx.send(response_bytes.into()).await;
                }
                return Ok(());
            };

            let ldc = LengthDelimitedCodec::new();
            let mut notif_tx = FramedWrite::new(notif_tx, ldc);
            let notif_rx = FramedRead::new(notif_rx, LengthDelimitedCodec::new());

            // Send established ack on notification stream
            let ack = super::NotificationMsg::Established {
                service: request.service.clone(),
                resource: request.resource.clone(),
            };
            let ack =
                serde_json::to_vec(&ack).context("failed to serialize notification ack message")?;
            if let Err(e) = notif_tx.send(ack.into()).await {
                log::error!(
                    "attempted to send notification ack to peer {} but failed {e}",
                    connection.remote_id()
                );
                let error = ResourceError::CallbackError(format!("Failed to send ack: {e}"));
                let response: Result<Response, ResourceError> = Err(error);
                if let Ok(response_bytes) = serde_json::to_vec(&response) {
                    let _ = tx.send(response_bytes.into()).await;
                }
                return Ok(());
            }

            // Send success response on request stream
            let response: Result<Response, ResourceError> = Ok(Response { data: Bytes::new() });
            let response_bytes = serde_json::to_vec(&response)
                .context("failed to serialize notification response")?;
            if let Err(e) = tx.send(response_bytes.into()).await {
                log::error!(
                    "failed to send notification response to peer {}: {e}",
                    connection.remote_id()
                );
                return Ok(());
            }

            // Spawn notification consumer task
            let callback = callback.to_owned();
            let conn_clone = connection.clone();
            let endpoint_clone = endpoint.clone();
            let server_ext_clone = server_extensions.clone();
            let conn_ext_clone = connection_extensions.clone();
            let middleware_clone = request_middleware.clone();
            let shutdown_sig_clone = shutdown_signal.clone();

            tokio::spawn(async move {
                let peer = conn_clone.remote_id();

                // Build RequestContext for notification
                let mut ctx = RequestContext::new(
                    conn_clone,
                    endpoint_clone,
                    request.service.clone(),
                    request.resource.clone(),
                    server_ext_clone,
                    conn_ext_clone,
                    shutdown_sig_clone,
                );

                // Apply request middleware chain
                for middleware in &middleware_clone {
                    ctx = middleware(ctx).await;
                }

                if let Err(e) = (callback)(ctx, request, notif_rx, notif_tx).await {
                    warn!("notification consumer task for peer {} failed {e}", peer);
                }
            });
        }
        super::ResourceCallback::StreamHandler(callback) => {
            // Open new bidirectional stream for raw stream handler
            let Ok((mut stream_tx, stream_rx)) = connection.open_bi().await else {
                let error = ResourceError::CallbackError("Failed to open bidi stream".into());
                let response: Result<Response, ResourceError> = Err(error);
                if let Ok(response_bytes) = serde_json::to_vec(&response) {
                    let _ = tx.send(response_bytes.into()).await;
                }
                return Ok(());
            };

            // CRITICAL: Must write to stream BEFORE client accept_bi() will succeed
            // Send a simple ACK byte to establish the stream
            if let Err(e) = stream_tx.write_all(b"OK").await {
                log::error!(
                    "failed to send stream ACK to peer {}: {e}",
                    connection.remote_id()
                );
                let error = ResourceError::CallbackError(format!("Failed to send stream ACK: {e}"));
                let response: Result<Response, ResourceError> = Err(error);
                if let Ok(response_bytes) = serde_json::to_vec(&response) {
                    let _ = tx.send(response_bytes.into()).await;
                }
                return Ok(());
            };

            // Send success response on request stream
            let response: Result<Response, ResourceError> = Ok(Response { data: Bytes::new() });
            let response_bytes = serde_json::to_vec(&response)
                .context("failed to serialize response something really bad is happening")?;
            if let Err(e) = tx.send(response_bytes.into()).await {
                log::error!(
                    "failed to send stream response to peer {}: {e}",
                    connection.remote_id()
                );
                return Ok(());
            }

            // Spawn stream handler task with RAW streams (no codec wrapping)
            let callback = callback.to_owned();
            let conn_clone = connection.clone();
            let endpoint_clone = endpoint.clone();
            let server_ext_clone = server_extensions.clone();
            let conn_ext_clone = connection_extensions.clone();
            let middleware_clone = request_middleware.clone();
            let shutdown_sig_clone = shutdown_signal.clone();

            tokio::spawn(async move {
                let peer = conn_clone.remote_id();

                // Build RequestContext for stream handler
                let mut ctx = RequestContext::new(
                    conn_clone,
                    endpoint_clone,
                    request.service.clone(),
                    request.resource.clone(),
                    server_ext_clone,
                    conn_ext_clone,
                    shutdown_sig_clone,
                );

                // Apply request middleware chain
                for middleware in &middleware_clone {
                    ctx = middleware(ctx).await;
                }

                // Call handler with raw streams - no framing/codec
                if let Err(e) = (callback)(ctx, request, stream_tx, stream_rx).await {
                    warn!("stream handler task for peer {} failed {e}", peer);
                }
            });
        }
    }

    Ok(())
}
