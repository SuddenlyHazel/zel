use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    protocol::ProtocolHandler,
};
use log::trace;
use std::future::Future;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use super::extensions::Extensions;
use super::handlers::*;
use super::{ResourceError, ServiceMap};

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
        let circuit_breakers = self.circuit_breakers.clone();

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
                    circuit_breakers,
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
    circuit_breakers: Option<std::sync::Arc<super::circuit_breaker::PeerCircuitBreakers>>,
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
                        let cb = circuit_breakers.clone();

                        tokio::spawn(async move {
                            // Keep guard alive for the lifetime of this task
                            let _guard = task_guard;

                            if let Err(e) =
                                handle_stream(tx, rx, service_map, server_ext, conn_ext, middleware, endpoint_clone, conn, shutdown_sig, cb)
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
    circuit_breakers: Option<std::sync::Arc<super::circuit_breaker::PeerCircuitBreakers>>,
) -> anyhow::Result<()> {
    let mut tx = FramedWrite::new(tx, LengthDelimitedCodec::new());
    let mut rx = FramedRead::new(rx, LengthDelimitedCodec::new());

    // Read and deserialize request
    let request = match read_and_deserialize_request(&mut rx, &connection).await? {
        Some(req) => req,
        None => return Ok(()),
    };

    // Route to service
    let Some(service) = service_map.get(request.service.as_str()) else {
        let error = ResourceError::ServiceNotFound {
            service: request.service.clone(),
        };
        send_error_response(&mut tx, error).await;
        return Ok(());
    };

    // Route to resource
    let Some(resource) = service.resources.get(request.resource.as_str()) else {
        let error = ResourceError::ResourceNotFound {
            service: request.service.clone(),
            resource: request.resource.clone(),
        };
        send_error_response(&mut tx, error).await;
        return Ok(());
    };

    // Dispatch to appropriate handler based on resource type
    match resource {
        super::ResourceCallback::Rpc(callback) => {
            handle_rpc(
                callback.clone(),
                request,
                &mut tx,
                &connection,
                endpoint,
                server_extensions,
                connection_extensions,
                request_middleware.clone(),
                shutdown_signal.clone(),
                circuit_breakers.clone(),
            )
            .await
        }
        super::ResourceCallback::SubscriptionProducer(callback) => {
            handle_subscription(
                callback.clone(),
                request,
                &mut tx,
                &connection,
                endpoint,
                server_extensions,
                connection_extensions,
                request_middleware.clone(),
                shutdown_signal.clone(),
            )
            .await
        }
        super::ResourceCallback::NotificationConsumer(callback) => {
            handle_notification(
                callback.clone(),
                request,
                &mut tx,
                &connection,
                endpoint,
                server_extensions,
                connection_extensions,
                request_middleware.clone(),
                shutdown_signal.clone(),
            )
            .await
        }
        super::ResourceCallback::StreamHandler(callback) => {
            handle_stream_handler(
                callback.clone(),
                request,
                &mut tx,
                &connection,
                endpoint,
                server_extensions,
                connection_extensions,
                request_middleware.clone(),
                shutdown_signal.clone(),
            )
            .await
        }
    }
}
