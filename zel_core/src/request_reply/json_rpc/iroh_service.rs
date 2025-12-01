//! Iroh-specific RpcService implementation with tower middleware support
//!
//! This module provides an implementation of jsonrpsee's `RpcServiceT` trait
//! that includes Iroh-specific context (peer ID, ALPN, connection stats) which
//! can be accessed by middleware layers.

use std::sync::Arc;

use iroh::PublicKey;
use iroh::endpoint::Connection;
use jsonrpsee::core::id_providers::RandomIntegerIdProvider;
use jsonrpsee::core::middleware::{Batch, Notification, ResponseFuture, RpcServiceT};
use jsonrpsee::core::server::{
    BatchResponseBuilder, BoundedSubscriptions, MethodResponse, MethodSink, Methods,
    SubscriptionState,
};
use jsonrpsee::core::traits::IdProvider;
use jsonrpsee::types::{ErrorCode, ErrorObject, Params, Request};
use jsonrpsee::{ConnectionId, Extensions};
use log::info;
use serde_json::value::RawValue;
use tokio::sync::mpsc;

/// Iroh-aware RPC service that implements RpcServiceT
///
/// This service wraps jsonrpsee's `Methods` and adds Iroh-specific context
/// that can be accessed by middleware layers. Unlike jsonrpsee's internal
/// `RpcService`, this implementation:
/// - Uses only public APIs
/// - Exposes Iroh connection context (peer_id, alpn, connection stats)
/// - Manages subscriptions directly without private `RpcServiceCfg`
#[derive(Clone)]
pub struct IrohRpcService {
    // Core RPC components
    methods: Methods,
    max_response_size: usize,

    // Subscription support (public alternatives to private RpcServiceCfg)
    bounded_subscriptions: BoundedSubscriptions,
    method_sink: MethodSink,
    id_provider: Arc<dyn IdProvider>,
    #[allow(dead_code)] // Used for connection lifecycle tracking
    pending_calls: mpsc::Sender<()>,

    // Iroh-specific context (unique to our implementation!)
    peer_id: PublicKey,
    alpn: Vec<u8>,
    connection: Connection,
}

impl IrohRpcService {
    /// Create a new Iroh-aware RPC service
    ///
    /// Returns the service and a receiver for subscription notifications.
    /// The receiver must be forwarded to the client via the Iroh transport.
    ///
    /// # Arguments
    ///
    /// * `methods` - The RPC methods to serve
    /// * `max_response_size` - Maximum size of responses in bytes
    /// * `max_subscriptions` - Maximum number of concurrent subscriptions
    /// * `connection` - The Iroh connection (provides peer_id, stats, etc.)
    /// * `alpn` - The ALPN protocol identifier
    ///
    /// # Returns
    ///
    /// A tuple of (service, subscription_receiver)
    pub fn new(
        methods: Methods,
        max_response_size: usize,
        max_subscriptions: usize,
        connection: Connection,
        alpn: &[u8],
    ) -> (Self, mpsc::Receiver<Box<RawValue>>) {
        // Create subscription channel for notifications
        let (tx, rx) = mpsc::channel(1024);
        let method_sink = MethodSink::new(tx);

        // Create subscription manager
        let bounded_subscriptions = BoundedSubscriptions::new(max_subscriptions as u32);

        // Create ID provider for subscriptions
        let id_provider = Arc::new(RandomIntegerIdProvider) as Arc<dyn IdProvider>;

        // Create pending calls tracker for graceful shutdown
        let (pending_calls, _pending_calls_rx) = mpsc::channel(1);

        let service = Self {
            methods,
            max_response_size,
            bounded_subscriptions,
            method_sink,
            id_provider,
            pending_calls,
            peer_id: connection.remote_id(),
            alpn: alpn.to_vec(),
            connection,
        };

        (service, rx)
    }

    /// Get the peer ID for this connection
    ///
    /// This can be accessed by middleware to implement peer-based authentication
    /// or authorization.
    pub fn peer_id(&self) -> PublicKey {
        self.peer_id
    }

    /// Get the ALPN protocol identifier
    ///
    /// This can be accessed by middleware to apply different policies based
    /// on the protocol version or type.
    pub fn alpn(&self) -> &[u8] {
        &self.alpn
    }

    /// Get the underlying Iroh connection
    ///
    /// This provides access to connection statistics like RTT, packet counts,
    /// which middleware can use for quality-of-service decisions.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Convert peer ID to connection ID
    ///
    /// Uses the first 8 bytes of the peer ID to create a stable connection ID.
    fn peer_id_to_conn_id(&self) -> ConnectionId {
        let bytes = self.peer_id.as_bytes();
        let id = u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]) as usize;
        ConnectionId(id)
    }
}

// ============================================================================
// RpcServiceT Implementation
// ============================================================================

use jsonrpsee::core::middleware::BatchEntry;
use jsonrpsee::core::server::MethodCallback;
use jsonrpsee::types::error::reject_too_many_subscriptions;

impl RpcServiceT for IrohRpcService {
    type MethodResponse = MethodResponse;
    type BatchResponse = MethodResponse;
    type NotificationResponse = MethodResponse;

    fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
        // Clone what we need from self (since &self is short-lived)
        info!("req {req:?}");
        let methods = self.methods.clone();
        let max_response_size = self.max_response_size;
        let bounded_subscriptions = self.bounded_subscriptions.clone();
        let method_sink = self.method_sink.clone();
        let id_provider = self.id_provider.clone();
        let conn_id = self.peer_id_to_conn_id();

        // Destructure request
        let Request {
            id,
            method,
            params,
            extensions,
            ..
        } = req;
        let params = Params::new(params.as_ref().map(|p| p.get()));
        // Look up and dispatch - ALL INLINE
        match methods.method_with_name(&method) {
            None => {
                let rp = MethodResponse::error(id, ErrorCode::MethodNotFound)
                    .with_extensions(extensions);
                ResponseFuture::ready(rp)
            }
            Some((_name, method_callback)) => match method_callback {
                MethodCallback::Sync(callback) => {
                    // INLINE: Execute sync callback
                    let rp = (callback)(id, params, max_response_size, extensions);
                    ResponseFuture::ready(rp)
                }
                MethodCallback::Async(callback) => {
                    // INLINE: Execute async callback
                    let params = params.into_owned();
                    let id = id.into_owned();
                    let fut = (callback)(id, params, conn_id, max_response_size, extensions);
                    ResponseFuture::future(fut)
                }
                MethodCallback::Subscription(callback) => {
                    // INLINE: ALL subscription logic here
                    if let Some(permit) = bounded_subscriptions.acquire() {
                        let state = SubscriptionState {
                            conn_id,
                            id_provider: &*id_provider.clone(),
                            subscription_permit: permit,
                        };
                        info!("Received Sub for {conn_id:?} {id}");
                        let fut = (callback)(id.clone(), params, method_sink, state, extensions);
                        ResponseFuture::future(fut)
                    } else {
                        let max = bounded_subscriptions.max();
                        let rp = MethodResponse::error(id, reject_too_many_subscriptions(max))
                            .with_extensions(extensions);
                        ResponseFuture::ready(rp)
                    }
                }
                MethodCallback::Unsubscription(callback) => {
                    // INLINE: Execute unsubscribe callback
                    let rp = (callback)(id, params, conn_id, max_response_size, extensions);
                    ResponseFuture::ready(rp)
                }
            },
        }
    }

    fn batch<'a>(&self, batch: Batch<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
        let mut batch_rp = BatchResponseBuilder::new_with_limit(self.max_response_size);
        let service = self.clone();

        async move {
            let mut got_notification = false;

            for batch_entry in batch.into_iter() {
                match batch_entry {
                    Ok(BatchEntry::Call(req)) => {
                        let rp = service.call(req).await;
                        if let Err(err) = batch_rp.append(rp) {
                            return err;
                        }
                    }
                    Ok(BatchEntry::Notification(n)) => {
                        got_notification = true;
                        service.notification(n).await;
                    }
                    Err(err) => {
                        let (err, id) = err.into_parts();
                        let rp = MethodResponse::error(id, err);
                        if let Err(err) = batch_rp.append(rp) {
                            return err;
                        }
                    }
                }
            }

            if batch_rp.is_empty() && got_notification {
                MethodResponse::notification()
            } else {
                MethodResponse::from_batch(batch_rp.finish())
            }
        }
    }

    fn notification<'a>(
        &self,
        n: Notification<'a>,
    ) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
        async move { MethodResponse::notification().with_extensions(n.extensions) }
    }
}
