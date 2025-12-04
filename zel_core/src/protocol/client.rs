use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::{SinkExt, Stream, StreamExt};
use iroh::endpoint::{Connection, RecvStream};
use tokio::sync::Mutex;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::protocol::{Body, Request, ResourceError, Response, SubscriptionMsg};

/// Client for making RPC calls and subscriptions over Iroh
#[derive(Clone)]
pub struct RpcClient {
    connection: Connection,
    tx: Arc<Mutex<FramedWrite<iroh::endpoint::SendStream, LengthDelimitedCodec>>>,
    rx: Arc<Mutex<FramedRead<iroh::endpoint::RecvStream, LengthDelimitedCodec>>>,
}

/// Errors that can occur during client operations
#[derive(thiserror::Error, Debug)]
pub enum ClientError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Resource error: {0}")]
    Resource(#[from] ResourceError),

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Protocol error: {0}")]
    Protocol(String),
}

impl RpcClient {
    /// Create a new RPC client from an Iroh connection
    ///
    /// This opens a bidirectional stream that will be reused for all RPC calls
    /// and subscription requests.
    pub async fn new(connection: Connection) -> Result<Self, ClientError> {
        // Open the persistent bidi stream
        let (tx, rx) = connection
            .open_bi()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        let tx = Arc::new(Mutex::new(FramedWrite::new(
            tx,
            LengthDelimitedCodec::new(),
        )));
        let rx = Arc::new(Mutex::new(FramedRead::new(rx, LengthDelimitedCodec::new())));

        Ok(Self { connection, tx, rx })
    }

    /// Call an RPC resource
    ///
    /// # Arguments
    /// * `service` - The service name
    /// * `resource` - The resource name
    /// * `body` - The request body data
    ///
    /// # Returns
    /// The response from the server
    pub async fn call(
        &self,
        service: impl Into<String>,
        resource: impl Into<String>,
        body: Bytes,
    ) -> Result<Response, ClientError> {
        // Send request on shared bidi stream
        let request = Request {
            service: service.into(),
            resource: resource.into(),
            body: Body::Rpc(body),
        };

        let request_bytes = serde_json::to_vec(&request)?;

        {
            let mut tx = self.tx.lock().await;
            tx.send(request_bytes.into())
                .await
                .map_err(|e| ClientError::Connection(e.to_string()))?;
        }

        // Receive response on shared bidi stream
        let response_bytes = {
            let mut rx = self.rx.lock().await;
            rx.next()
                .await
                .ok_or_else(|| ClientError::Protocol("No response received".into()))?
                .map_err(|e| ClientError::Connection(e.to_string()))?
        };

        let response: Result<Response, ResourceError> = serde_json::from_slice(&response_bytes)?;

        match response {
            Ok(resp) => Ok(resp),
            Err(e) => Err(ClientError::Resource(e)),
        }
    }

    /// Subscribe to a resource
    ///
    /// # Arguments
    /// * `service` - The service name
    /// * `resource` - The resource name
    /// * `body` - Optional request body data (parameters for the subscription)
    ///
    /// # Returns
    /// A `SubscriptionStream` that yields subscription messages
    pub async fn subscribe(
        &self,
        service: impl Into<String>,
        resource: impl Into<String>,
        body: Option<Bytes>,
    ) -> Result<SubscriptionStream, ClientError> {
        let service = service.into();
        let resource = resource.into();

        // Send subscribe request on shared bidi stream
        let request = Request {
            service: service.clone(),
            resource: resource.clone(),
            body: match body {
                Some(data) => Body::Rpc(data),
                None => Body::Subscribe,
            },
        };

        let request_bytes = serde_json::to_vec(&request)?;

        {
            let mut tx = self.tx.lock().await;
            tx.send(request_bytes.into())
                .await
                .map_err(|e| ClientError::Connection(e.to_string()))?;
        }

        // Wait for Response (OK) on shared bidi stream
        let response_bytes = {
            let mut rx = self.rx.lock().await;
            rx.next()
                .await
                .ok_or_else(|| ClientError::Protocol("No response received".into()))?
                .map_err(|e| ClientError::Connection(e.to_string()))?
        };

        let response: Result<Response, ResourceError> = serde_json::from_slice(&response_bytes)?;
        response.map_err(ClientError::Resource)?;

        // Accept uni stream for subscription data from server
        let sub_rx = self
            .connection
            .accept_uni()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        let mut sub_rx = FramedRead::new(sub_rx, LengthDelimitedCodec::new());

        // Receive SubscriptionMsg::Established
        let established_bytes = sub_rx
            .next()
            .await
            .ok_or_else(|| ClientError::Protocol("No established message received".into()))?
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        let established: SubscriptionMsg = serde_json::from_slice(&established_bytes)?;

        match established {
            SubscriptionMsg::Established {
                service: s,
                resource: r,
            } => {
                if s != service || r != resource {
                    return Err(ClientError::Protocol(format!(
                        "Established message mismatch: expected {}/{}, got {}/{}",
                        service, resource, s, r
                    )));
                }
            }
            _ => {
                return Err(ClientError::Protocol(
                    "Expected Established message, got something else".into(),
                ));
            }
        }

        // Return SubscriptionStream
        Ok(SubscriptionStream { inner: sub_rx })
    }

    /// Start a notification stream to server (client-to-server streaming)
    ///
    /// This allows clients to push events to the server over time with
    /// acknowledgment support. Each message sent receives an acknowledgment.
    ///
    /// # Arguments
    /// * `service` - The service name
    /// * `resource` - The resource name
    /// * `params` - Optional serialized parameters
    ///
    /// # Returns
    /// A `NotificationSender` for sending events to the server
    pub async fn notify(
        &self,
        service: impl Into<String>,
        resource: impl Into<String>,
        params: Option<Bytes>,
    ) -> Result<NotificationSender, ClientError> {
        let service = service.into();
        let resource = resource.into();

        // Send notification request on shared bidi stream
        let request = Request {
            service: service.clone(),
            resource: resource.clone(),
            body: Body::Notify(params.unwrap_or_default()),
        };

        let request_bytes = serde_json::to_vec(&request)?;

        {
            let mut tx = self.tx.lock().await;
            tx.send(request_bytes.into())
                .await
                .map_err(|e| ClientError::Connection(e.to_string()))?;
        }

        // Wait for Response (OK) on shared bidi stream
        let response_bytes = {
            let mut rx = self.rx.lock().await;
            rx.next()
                .await
                .ok_or_else(|| ClientError::Protocol("No response received".into()))?
                .map_err(|e| ClientError::Connection(e.to_string()))?
        };

        let response: Result<Response, ResourceError> = serde_json::from_slice(&response_bytes)?;
        response.map_err(ClientError::Resource)?;

        // Accept bidirectional stream from server
        let (notif_tx, notif_rx) = self
            .connection
            .accept_bi()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        let mut notif_rx = FramedRead::new(notif_rx, LengthDelimitedCodec::new());
        let notif_tx = FramedWrite::new(notif_tx, LengthDelimitedCodec::new());

        // Receive Established message
        let established_bytes = notif_rx
            .next()
            .await
            .ok_or_else(|| ClientError::Protocol("No established message received".into()))?
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        let established: crate::protocol::NotificationMsg =
            serde_json::from_slice(&established_bytes)?;

        match established {
            crate::protocol::NotificationMsg::Established {
                service: s,
                resource: r,
            } => {
                if s != service || r != resource {
                    return Err(ClientError::Protocol(format!(
                        "Established message mismatch: expected {}/{}, got {}/{}",
                        service, resource, s, r
                    )));
                }
            }
            _ => {
                return Err(ClientError::Protocol(
                    "Expected Established message, got something else".into(),
                ));
            }
        }

        // Return NotificationSender
        Ok(NotificationSender {
            tx: notif_tx,
            rx: notif_rx,
        })
    }

    /// Open a raw bidirectional stream for custom protocols
    ///
    /// This allows implementing custom protocols like video/audio streaming,
    /// file transfers, etc. Returns raw Iroh streams with no framing/codec.
    ///
    /// # Arguments
    /// * `service` - The service name
    /// * `resource` - The resource name
    /// * `params` - Optional serialized parameters
    ///
    /// # Returns
    /// A tuple of (SendStream, RecvStream) for implementing custom protocols
    pub async fn open_stream(
        &self,
        service: impl Into<String>,
        resource: impl Into<String>,
        params: Option<Bytes>,
    ) -> Result<(iroh::endpoint::SendStream, RecvStream), ClientError> {
        // Send stream request on shared bidi stream
        let request = Request {
            service: service.into(),
            resource: resource.into(),
            body: Body::Stream(params.unwrap_or_default()),
        };

        let request_bytes = serde_json::to_vec(&request)?;

        {
            let mut tx = self.tx.lock().await;
            tx.send(request_bytes.into())
                .await
                .map_err(|e| ClientError::Connection(e.to_string()))?;
        }

        // Wait for Response (OK) on shared bidi stream
        let response_bytes = {
            let mut rx = self.rx.lock().await;
            rx.next()
                .await
                .ok_or_else(|| ClientError::Protocol("No response received".into()))?
                .map_err(|e| ClientError::Connection(e.to_string()))?
        };

        let response: Result<Response, ResourceError> = serde_json::from_slice(&response_bytes)?;
        response.map_err(ClientError::Resource)?;

        // Accept the new bidi stream opened by server
        let (stream_tx, mut stream_rx) = self
            .connection
            .accept_bi()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        // Read the ACK byte that was sent to establish the stream
        let mut ack = [0u8; 2];
        stream_rx
            .read_exact(&mut ack)
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        if &ack != b"OK" {
            return Err(ClientError::Protocol("Invalid stream ACK".into()));
        }

        // Return RAW streams (no codec wrapping)
        Ok((stream_tx, stream_rx))
    }
}

/// A stream of subscription messages
pub struct SubscriptionStream {
    inner: FramedRead<RecvStream, LengthDelimitedCodec>,
}

impl Stream for SubscriptionStream {
    type Item = Result<SubscriptionMsg, ClientError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                match serde_json::from_slice::<SubscriptionMsg>(&bytes) {
                    Ok(msg) => Poll::Ready(Some(Ok(msg))),
                    Err(e) => Poll::Ready(Some(Err(ClientError::Serialization(e)))),
                }
            }
            Poll::Ready(Some(Err(e))) => {
                Poll::Ready(Some(Err(ClientError::Connection(e.to_string()))))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for SubscriptionStream {
    fn drop(&mut self) {
        // Iroh will close the stream automatically when dropped
    }
}

/// Client-side notification sender (client-to-server streaming)
pub struct NotificationSender {
    tx: FramedWrite<iroh::endpoint::SendStream, LengthDelimitedCodec>,
    rx: FramedRead<RecvStream, LengthDelimitedCodec>,
}

impl NotificationSender {
    /// Send a notification message to the server and wait for acknowledgment
    ///
    /// This method will serialize the data, send it to the server, and wait
    /// for an acknowledgment before returning.
    pub async fn send<T: serde::Serialize>(&mut self, data: T) -> Result<(), ClientError> {
        // Serialize the data
        let data = serde_json::to_vec(&data)?;
        let msg = crate::protocol::NotificationMsg::Data(Bytes::from(data));
        let msg_bytes = serde_json::to_vec(&msg)?;

        // Send the notification
        self.tx
            .send(msg_bytes.into())
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        // Wait for acknowledgment
        let ack_bytes = self
            .rx
            .next()
            .await
            .ok_or_else(|| ClientError::Protocol("No acknowledgment received".into()))?
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        let ack: crate::protocol::NotificationMsg = serde_json::from_slice(&ack_bytes)?;

        match ack {
            crate::protocol::NotificationMsg::Ack => Ok(()),
            crate::protocol::NotificationMsg::Error(e) => {
                Err(ClientError::Protocol(format!("Server error: {}", e)))
            }
            _ => Err(ClientError::Protocol("Unexpected message type".into())),
        }
    }

    /// Signal completion of the notification stream
    ///
    /// This sends a Completed message to the server and consumes the sender.
    pub async fn complete(mut self) -> Result<(), ClientError> {
        let msg = crate::protocol::NotificationMsg::Completed;
        let msg_bytes = serde_json::to_vec(&msg)?;

        self.tx
            .send(msg_bytes.into())
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        Ok(())
    }
}
