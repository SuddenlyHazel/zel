use anyhow::Context;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use iroh::{endpoint::Connection, protocol::ProtocolHandler};
use log::warn;
use tokio_util::codec::{Framed, FramedParts, FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::protocol::{
    Request, ResourceError, ResourceResponse, Response, ServiceMap, SubscriptionMsg,
};

// Lowest level will establish a bidi connection and dispatch all requests
// to child services
impl ProtocolHandler for super::RpcServer<'static> {
    fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> impl Future<Output = Result<(), iroh::protocol::AcceptError>> + Send {
        let remote_id = connection.remote_id();

        let services = self.services.clone();
        async move {
            tokio::spawn(async move {
                if let Err(e) = connection_handler(services, connection).await {
                    log::error!("{e}");
                }
            });
            Ok(())
        }
    }
}

async fn connection_handler<'a>(
    service_map: ServiceMap<'a>,
    connection: Connection,
) -> anyhow::Result<()> {
    let (tx, rx) = connection.accept_bi().await?;

    let mut tx = FramedWrite::new(tx, LengthDelimitedCodec::new());
    let mut rx = FramedRead::new(rx, LengthDelimitedCodec::new());

    while let Some(req) = rx.next().await {
        let req_maybe = match req {
            Ok(req) => serde_json::from_slice::<Request>(&req),
            Err(err) => {
                log::error!(
                    "Failed to read frame from peer {} error: {err}",
                    connection.remote_id()
                );
                break;
            }
        };

        let Ok(request) = req_maybe else {
            log::error!("Peer {} sent a malformed request", connection.remote_id());
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
                let response: ResourceResponse = (callback)(connection.clone(), request).await;
                let response = match response {
                    Ok(r) => Ok(r),
                    Err(e) => Err(ResourceError::CallbackError(e.to_string())),
                };

                let response_bytes = match serde_json::to_vec(&response) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        let error = ResourceError::SerializationError(e.to_string());
                        serde_json::to_vec(&Err::<Response, _>(error))
                            .unwrap_or_else(|_| b"{}".to_vec())
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
                let ack = serde_json::to_vec(&ack).unwrap_or_else(|_| b"{}".to_vec());
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
                let response_bytes =
                    serde_json::to_vec(&response).unwrap_or_else(|_| b"{}".to_vec());
                if let Err(e) = tx.send(response_bytes.into()).await {
                    log::error!(
                        "failed to send subscription response to peer {}: {e}",
                        connection.remote_id()
                    );
                    break;
                }

                // Spawn publisher task
                let callback = callback.to_owned();
                let connection = connection.clone();

                tokio::spawn(async move {
                    let peer = connection.remote_id();
                    if let Err(e) = (callback)(connection, request, sub_tx).await {
                        warn!("subscription task for peer {} failed {e}", peer);
                    }
                });
            }
        }
    }
    Ok(())
}
