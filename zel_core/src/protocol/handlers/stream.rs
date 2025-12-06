use anyhow::{Context, Result};
use bytes::Bytes;
use futures::SinkExt;
use iroh::endpoint::{Connection, Endpoint, RecvStream, SendStream};
use log::{error, warn};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::codec::{FramedWrite, LengthDelimitedCodec};

use crate::protocol::{
    error_classification::ErrorSeverity as ErrorSeverityType,
    Extensions, Request, RequestMiddleware, ResourceError, Response, StreamHandler,
};
use super::helpers::{apply_middleware, build_request_context, send_error_response};

/// Handle raw stream handler request
pub async fn handle_stream_handler(
    callback: StreamHandler,
    request: Request,
    tx: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
    connection: &Connection,
    endpoint: Endpoint,
    server_extensions: Extensions,
    connection_extensions: Extensions,
    request_middleware: Vec<RequestMiddleware>,
    shutdown_signal: Arc<Notify>,
) -> Result<()> {
    // Open new bidirectional stream for raw stream handler
    let Ok((mut stream_tx, stream_rx)) = connection.open_bi().await else {
        send_error_response(
            tx,
            ResourceError::CallbackError {
                message: "Failed to open bidi stream".into(),
                severity: ErrorSeverityType::Application,
                context: None,
            },
        )
        .await;
        return Ok(());
    };

    // CRITICAL: Must write to stream BEFORE client accept_bi() will succeed
    // Send a simple ACK byte to establish the stream
    if let Err(e) = stream_tx.write_all(b"OK").await {
        error!(
            "failed to send stream ACK to peer {}: {e}",
            connection.remote_id()
        );
        send_error_response(
            tx,
            ResourceError::CallbackError {
                message: format!("Failed to send stream ACK: {e}"),
                severity: ErrorSeverityType::Application,
                context: None,
            },
        )
        .await;
        return Ok(());
    };

    // Send success response on request stream
    let response: Result<Response, ResourceError> = Ok(Response { data: Bytes::new() });
    let response_bytes = serde_json::to_vec(&response)
        .context("failed to serialize response something really bad is happening")?;
    if let Err(e) = tx.send(response_bytes.into()).await {
        error!(
            "failed to send stream response to peer {}: {e}",
            connection.remote_id()
        );
        return Ok(());
    }

    // Spawn stream handler task with RAW streams (no codec wrapping)
    spawn_stream_handler_task(
        callback,
        request,
        stream_tx,
        stream_rx,
        connection.clone(),
        endpoint,
        server_extensions,
        connection_extensions,
        request_middleware,
        shutdown_signal,
    );

    Ok(())
}

pub fn spawn_stream_handler_task(
    callback: StreamHandler,
    request: Request,
    stream_tx: SendStream,
    stream_rx: RecvStream,
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

        // Call handler with raw streams - no framing/codec
        if let Err(e) = (callback)(ctx, request, stream_tx, stream_rx).await {
            warn!("stream handler task for peer {} failed {e}", peer);
        }
    });
}
