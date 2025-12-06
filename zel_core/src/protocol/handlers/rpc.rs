use anyhow::Result;
use iroh::endpoint::{Connection, Endpoint, SendStream};
use iroh::PublicKey;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::codec::{FramedWrite, LengthDelimitedCodec};

use crate::protocol::{
    error_classification::{ErrorClassificationExt, ErrorSeverity as ErrorSeverityType},
    circuit_breaker::PeerCircuitBreakers,
    Extensions, Request, RequestContext, RequestMiddleware, ResourceError, ResourceResponse, Rpc,
};
use super::helpers::{apply_middleware, build_request_context, send_response};

/// Handle RPC request
pub async fn handle_rpc(
    callback: Rpc,
    request: Request,
    tx: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
    connection: &Connection,
    endpoint: Endpoint,
    server_extensions: Extensions,
    connection_extensions: Extensions,
    request_middleware: Vec<RequestMiddleware>,
    shutdown_signal: Arc<Notify>,
    circuit_breakers: Option<Arc<PeerCircuitBreakers>>,
) -> Result<()> {
    let ctx = build_request_context(
        connection.clone(),
        endpoint,
        &request,
        server_extensions,
        connection_extensions,
        shutdown_signal,
    );
    let ctx = apply_middleware(ctx, request_middleware).await;

    let response = execute_rpc_with_circuit_breaker(
        callback,
        ctx,
        request,
        circuit_breakers,
        connection.remote_id(),
    )
    .await;

    send_response(tx, response).await?;
    Ok(())
}

/// Execute RPC callback with optional circuit breaker protection
pub async fn execute_rpc_with_circuit_breaker(
    callback: Rpc,
    ctx: RequestContext,
    request: Request,
    circuit_breakers: Option<Arc<PeerCircuitBreakers>>,
    peer_id: PublicKey,
) -> ResourceResponse {
    if let Some(cb) = circuit_breakers {
        let ctx_clone = ctx.clone();
        let req_clone = request.clone();
        let callback_clone = callback.clone();

        match cb
            .call_async(&peer_id, || async move {
                (callback_clone)(ctx_clone, req_clone).await.map_err(|e| {
                    // Preserve severity when converting ResourceError to anyhow::Error
                    let severity = e.severity();
                    let err = anyhow::anyhow!("{}", e);
                    match severity {
                        ErrorSeverityType::Application => err.as_application(),
                        ErrorSeverityType::Infrastructure => err.as_infrastructure(),
                        ErrorSeverityType::SystemFailure => err.as_system_failure(),
                    }
                })
            })
            .await
        {
            Ok(resp) => Ok(resp),
            Err(circuitbreaker_rs::BreakerError::Open) => Err(ResourceError::CircuitBreakerOpen {
                peer_id: peer_id.to_string(),
            }),
            Err(circuitbreaker_rs::BreakerError::Operation(e)) => {
                Err(ResourceError::CallbackError {
                    message: e.to_string(),
                    severity: e.severity(),
                    context: None,
                })
            }
            Err(e) => Err(ResourceError::CallbackError {
                message: e.to_string(),
                severity: ErrorSeverityType::Application,
                context: None,
            }),
        }
    } else {
        (callback)(ctx, request).await
    }
}
