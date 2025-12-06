use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use iroh::endpoint::{Connection, Endpoint, RecvStream, SendStream};
use log::{error, warn};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::protocol::{Extensions, Request, RequestContext, RequestMiddleware, ResourceError, ResourceResponse, Response};

/// Read and deserialize a request from the stream
pub async fn read_and_deserialize_request(
    rx: &mut FramedRead<RecvStream, LengthDelimitedCodec>,
    connection: &Connection,
) -> Result<Option<Request>> {
    let req_bytes = match rx.next().await {
        Some(Ok(bytes)) => bytes,
        Some(Err(e)) => {
            error!(
                "Failed to read request from peer {}: {e}",
                connection.remote_id()
            );
            return Err(e.into());
        }
        None => {
            warn!(
                "Stream closed by peer {} before sending request",
                connection.remote_id()
            );
            return Ok(None);
        }
    };

    let request: Request = match serde_json::from_slice(&req_bytes) {
        Ok(req) => req,
        Err(e) => {
            error!(
                "Peer {} sent malformed request: {e}",
                connection.remote_id()
            );
            return Err(e.into());
        }
    };

    Ok(Some(request))
}

/// Send an error response on the stream
pub async fn send_error_response(
    tx: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
    error: ResourceError,
) {
    let response: Result<Response, ResourceError> = Err(error);
    if let Ok(response_bytes) = serde_json::to_vec(&response) {
        let _ = tx.send(response_bytes.into()).await;
    }
}

/// Send a successful response on the stream
pub async fn send_response(
    tx: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
    response: ResourceResponse,
) -> Result<()> {
    let response_bytes = match serde_json::to_vec(&response) {
        Ok(bytes) => bytes,
        Err(e) => {
            let error = ResourceError::SerializationError(e.to_string());
            serde_json::to_vec(&Err::<Response, _>(error))
                .context("failed to serialize error response something really bad is happening")?
        }
    };

    if let Err(e) = tx.send(response_bytes.into()).await {
        error!("failed to send response: {e}");
    }

    Ok(())
}

/// Build RequestContext and apply middleware
pub fn build_request_context(
    connection: Connection,
    endpoint: Endpoint,
    request: &Request,
    server_extensions: Extensions,
    connection_extensions: Extensions,
    shutdown_signal: Arc<Notify>,
) -> RequestContext {
    RequestContext::new(
        connection,
        endpoint,
        request.service.clone(),
        request.resource.clone(),
        server_extensions,
        connection_extensions,
        shutdown_signal,
    )
}

/// Apply middleware chain to a RequestContext
pub async fn apply_middleware(
    mut ctx: RequestContext,
    request_middleware: Vec<RequestMiddleware>,
) -> RequestContext {
    for middleware in &request_middleware {
        ctx = middleware(ctx).await;
    }
    ctx
}
