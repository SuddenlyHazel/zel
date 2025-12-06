use anyhow::{Context, Result};
use bytes::Bytes;
use futures::SinkExt;
use iroh::endpoint::{Connection, Endpoint, SendStream};
use log::{error, warn};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::codec::{FramedWrite, LengthDelimitedCodec};

use crate::protocol::{
    error_classification::ErrorSeverity as ErrorSeverityType,
    Extensions, Request, RequestMiddleware, ResourceError, Response, Subscription, SubscriptionMsg,
};
use super::helpers::{apply_middleware, build_request_context, send_error_response};

/// Handle subscription request
pub async fn handle_subscription(
    callback: Subscription,
    request: Request,
    tx: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
    connection: &Connection,
    endpoint: Endpoint,
    server_extensions: Extensions,
    connection_extensions: Extensions,
    request_middleware: Vec<RequestMiddleware>,
    shutdown_signal: Arc<Notify>,
) -> Result<()> {
    // Open uni stream for subscription
    let Ok(sub_tx) = connection.open_uni().await else {
        send_error_response(
            tx,
            ResourceError::CallbackError {
                message: "Failed to establish stream".into(),
                severity: ErrorSeverityType::Application,
                context: None,
            },
        )
        .await;
        return Ok(());
    };

    let ldc = LengthDelimitedCodec::new();
    let mut sub_tx = FramedWrite::new(sub_tx, ldc);

    // Send established ack on uni stream
    if let Err(e) = send_subscription_ack(&mut sub_tx, &request, connection).await {
        send_error_response(
            tx,
            ResourceError::CallbackError {
                message: format!("Failed to send ack: {e}"),
                severity: ErrorSeverityType::Application,
                context: None,
            },
        )
        .await;
        return Ok(());
    }

    // Send success response on request stream
    let response: Result<Response, ResourceError> = Ok(Response { data: Bytes::new() });
    let response_bytes = serde_json::to_vec(&response)
        .context("failed to serialize response something really bad is happening")?;
    if let Err(e) = tx.send(response_bytes.into()).await {
        error!(
            "failed to send subscription response to peer {}: {e}",
            connection.remote_id()
        );
        return Ok(());
    }

    // Spawn publisher task
    spawn_subscription_task(
        callback,
        request,
        sub_tx,
        connection.clone(),
        endpoint,
        server_extensions,
        connection_extensions,
        request_middleware,
        shutdown_signal,
    );

    Ok(())
}

pub async fn send_subscription_ack(
    sub_tx: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
    request: &Request,
    connection: &Connection,
) -> Result<()> {
    let ack = SubscriptionMsg::Established {
        service: request.service.clone(),
        resource: request.resource.clone(),
    };
    let ack = serde_json::to_vec(&ack)
        .context("failed to serialize ack message something really bad is happening")?;
    if let Err(e) = sub_tx.send(ack.into()).await {
        error!(
            "attempted to send subscription ack to peer {} but failed {e}",
            connection.remote_id()
        );
        return Err(e.into());
    }
    Ok(())
}

pub fn spawn_subscription_task(
    callback: Subscription,
    request: Request,
    sub_tx: FramedWrite<SendStream, LengthDelimitedCodec>,
    connection: Connection,
    endpoint: Endpoint,
    server_extensions: Extensions,
    connection_extensions: Extensions,
    request_middleware: Vec<RequestMiddleware>,
    shutdown_signal: Arc<Notify>,
) {
    tokio::spawn(async move {
        let peer = connection.remote_id();

        let ctx = build_request_context(
            connection,
            endpoint,
            &request,
            server_extensions,
            connection_extensions,
            shutdown_signal,
        );
        let ctx = apply_middleware(ctx, request_middleware).await;

        if let Err(e) = (callback)(ctx, request, sub_tx).await {
            warn!("subscription task for peer {} failed {e}", peer);
        }
    });
}
