use anyhow::Context;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use iroh::{endpoint::Connection, protocol::ProtocolHandler};
use log::{trace, warn};
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
        async move {
            tokio::spawn(async move {
                if let Err(e) = connection_handler(services, server_extensions, connection).await {
                    log::error!("{e}");
                }
            });
            Ok(())
        }
    }
}

async fn connection_handler<'a>(
    service_map: ServiceMap<'a>,
    server_extensions: Extensions,
    connection: Connection,
) -> anyhow::Result<()> {
    // Create connection-scoped extensions
    let connection_extensions = Extensions::new();

    let (tx, rx) = connection.accept_bi().await?;

    let mut tx = FramedWrite::new(tx, LengthDelimitedCodec::new());
    let mut rx = FramedRead::new(rx, LengthDelimitedCodec::new());

    while let Some(req) = rx.next().await {
        let req_maybe = match req {
            Ok(req) => serde_json::from_slice::<Request>(&req),
            Err(err) => {
                log::error!(
                    "Failed to read frame from peer {}: aborting connection. error: {err}",
                    connection.remote_id()
                );
                break;
            }
        };

        let Ok(request) = req_maybe else {
            log::error!(
                "Peer {} sent a malformed request aborting connection",
                connection.remote_id()
            );
            break;
        };

        let Some(service) = service_map.get(request.service.as_str()) else {
            let error = ResourceError::ServiceNotFound {
                service: request.service.clone(),
            };
            let response: Result<Response, ResourceError> = Err(error);
            if let Ok(response_bytes) = serde_json::to_vec(&response) {
                let _ = tx.send(response_bytes.into()).await;
            }
            continue;
        };

        let Some(resource) = service.resources.get(request.resource.as_str()) else {
            let error = ResourceError::ResourceNotFound {
                service: request.service.clone(),
                resource: request.resource.clone(),
            };
            let response: Result<Response, ResourceError> = Err(error);
            if let Ok(response_bytes) = serde_json::to_vec(&response) {
                let _ = tx.send(response_bytes.into()).await;
            }
            continue;
        };

        match resource {
            super::ResourceCallback::Rpc(callback) => {
                // Build RequestContext with all three extension tiers
                let ctx = RequestContext::new(
                    connection.clone(),
                    request.service.clone(),
                    request.resource.clone(),
                    server_extensions.clone(),
                    connection_extensions.clone(),
                );

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
                        "failed to send resource response to peer {}: {e}",
                        connection.remote_id()
                    );
                    break;
                }
            }
            super::ResourceCallback::SubscriptionProducer(callback) => {
                // Open uni stream for subscription
                let Ok(sub_tx) = connection.open_uni().await else {
                    let error = ResourceError::CallbackError("Failed to establish stream".into());
                    let response: Result<Response, ResourceError> = Err(error);
                    if let Ok(response_bytes) = serde_json::to_vec(&response) {
                        let _ = tx.send(response_bytes.into()).await;
                    }
                    continue;
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
                    continue;
                }

                // Send success response on bidi stream
                let response: Result<Response, ResourceError> = Ok(Response { data: Bytes::new() });
                let response_bytes = serde_json::to_vec(&response)
                    .context("failed to serialize response something really bad is happening")?;
                if let Err(e) = tx.send(response_bytes.into()).await {
                    log::error!(
                        "failed to send subscription response to peer {}: {e}",
                        connection.remote_id()
                    );
                    break;
                }

                // Spawn publisher task
                let callback = callback.to_owned();
                let conn_clone = connection.clone();
                let server_ext_clone = server_extensions.clone();
                let conn_ext_clone = connection_extensions.clone();

                tokio::spawn(async move {
                    let peer = conn_clone.remote_id();

                    // Build RequestContext for subscription
                    let ctx = RequestContext::new(
                        conn_clone,
                        request.service.clone(),
                        request.resource.clone(),
                        server_ext_clone,
                        conn_ext_clone,
                    );

                    if let Err(e) = (callback)(ctx, request, sub_tx).await {
                        warn!("subscription task for peer {} failed {e}", peer);
                    }
                });
            }
        }
    }
    Ok(())
}
