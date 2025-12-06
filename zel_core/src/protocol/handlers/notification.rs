use anyhow::{Context, Result};
use bytes::Bytes;
use futures::SinkExt;
use iroh::endpoint::{Connection, Endpoint, RecvStream, SendStream};
use log::{error, warn};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::protocol::{
    error_classification::ErrorSeverity as ErrorSeverityType,
    Extensions, NotificationHandler, NotificationMsg, Request, RequestMiddleware, ResourceError, Response,
};
use super::helpers::{apply_middleware, build_request_context, send_error_response};

/// Handle notification request
pub async fn handle_notification(
    callback: NotificationHandler,
    request: Request,
    tx: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
    connection: &Connection,
    endpoint: Endpoint,
    server_extensions: Extensions,
    connection_extensions: Extensions,
    request_middleware: Vec<RequestMiddleware>,
    shutdown_signal: Arc<Notify>,
) -> Result<()> {
    // Open bidirectional stream for notification
    let Ok((notif_tx, notif_rx)) = connection.open_bi().await else {
        send_error_response(
            tx,
            ResourceError::CallbackError {
                message: "Failed to establish notification stream".into(),
                severity: ErrorSeverityType::Application,
                context: None,
            },
        )
        .await;
        return Ok(());
    };

    let ldc = LengthDelimitedCodec::new();
    let mut notif_tx = FramedWrite::new(notif_tx, ldc);
    let notif_rx = FramedRead::new(notif_rx, LengthDelimitedCodec::new());

    // Send established ack on notification stream
    if let Err(e) = send_notification_ack(&mut notif_tx, &request, connection).await {
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
    let response_bytes =
        serde_json::to_vec(&response).context("failed to serialize notification response")?;
    if let Err(e) = tx.send(response_bytes.into()).await {
        error!(
            "failed to send notification response to peer {}: {e}",
            connection.remote_id()
        );
        return Ok(());
    }

    // Spawn notification consumer task
    spawn_notification_task(
        callback,
        request,
        notif_rx,
        notif_tx,
        connection.clone(),
        endpoint,
        server_extensions,
        connection_extensions,
        request_middleware,
        shutdown_signal,
    );

    Ok(())
}

pub async fn send_notification_ack(
    notif_tx: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
    request: &Request,
    connection: &Connection,
) -> Result<()> {
    let ack = NotificationMsg::Established {
        service: request.service.clone(),
        resource: request.resource.clone(),
    };
    let ack = serde_json::to_vec(&ack).context("failed to serialize notification ack message")?;
    if let Err(e) = notif_tx.send(ack.into()).await {
        error!(
            "attempted to send notification ack to peer {} but failed {e}",
            connection.remote_id()
        );
        return Err(e.into());
    }
    Ok(())
}

pub fn spawn_notification_task(
    callback: NotificationHandler,
    request: Request,
    notif_rx: FramedRead<RecvStream, LengthDelimitedCodec>,
    notif_tx: FramedWrite<SendStream, LengthDelimitedCodec>,
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

        if let Err(e) = (callback)(ctx, request, notif_rx, notif_tx).await {
            warn!("notification consumer task for peer {} failed {e}", peer);
        }
    });
}
