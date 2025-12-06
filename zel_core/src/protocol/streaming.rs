//! Streaming utilities for subscriptions and notifications.
//!
//! This module provides wrapper types for server-to-client streaming (subscriptions)
//! and client-to-server streaming (notifications).

use bytes::Bytes;
use futures::SinkExt;
use iroh::endpoint::SendStream;
use serde::Serialize;
use tokio_util::codec::{FramedWrite, LengthDelimitedCodec};

use super::types::{NotificationMsg, SubscriptionMsg};

// ============================================================================
// Subscription Sink Wrapper
// ============================================================================

/// Wrapper for sending subscription data from server to client.
///
/// This is used in **server-to-client streaming** endpoints (subscriptions).
/// The server sends data over time to the client using this sink.
///
/// # Directionality
///
/// ```text
/// Server ──[SubscriptionSink]──> Client
/// ```
///
/// The server handler receives a `SubscriptionSink` and uses it to push data to the client.
/// Contrast this with [`NotificationSink`] which goes the opposite direction.
///
/// # Example
///
/// ```rust,ignore
/// async fn counter(
///     ctx: RequestContext,
///     mut sink: SubscriptionSink,
///     interval_ms: u64,
/// ) -> Result<(), zel_types::ResourceError> {
///     let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
///
///     for i in 0..10 {
///         interval.tick().await;
///         sink.send(&i).await?; // Server sends to client
///     }
///
///     sink.close().await?;
///     Ok(())
/// }
/// ```
pub struct SubscriptionSink {
    inner: FramedWrite<SendStream, LengthDelimitedCodec>,
}

impl SubscriptionSink {
    /// Create a new SubscriptionSink from a FramedWrite
    pub fn new(inner: FramedWrite<SendStream, LengthDelimitedCodec>) -> Self {
        Self { inner }
    }

    /// Send data to the subscription
    ///
    /// Automatically wraps the data in SubscriptionMsg::Data and serializes it.
    pub async fn send<T: Serialize>(&mut self, data: T) -> Result<(), SubscriptionError> {
        let data = serde_json::to_vec(&data)
            .map_err(|e| SubscriptionError::Serialization(e.to_string()))?;

        let msg = SubscriptionMsg::Data(Bytes::from(data));
        let msg_bytes = serde_json::to_vec(&msg)
            .map_err(|e| SubscriptionError::Serialization(e.to_string()))?;

        self.inner
            .send(msg_bytes.into())
            .await
            .map_err(|e| SubscriptionError::Send(e.to_string()))?;

        Ok(())
    }

    /// Send the stopped message and close the subscription
    pub async fn close(mut self) -> Result<(), SubscriptionError> {
        let msg = SubscriptionMsg::Stopped;
        let msg_bytes = serde_json::to_vec(&msg)
            .map_err(|e| SubscriptionError::Serialization(e.to_string()))?;

        self.inner
            .send(msg_bytes.into())
            .await
            .map_err(|e| SubscriptionError::Send(e.to_string()))?;

        Ok(())
    }

    /// Get a reference to the underlying SendStream
    ///
    /// This allows direct access to all SendStream methods like:
    /// - `set_priority(i32)` - Set stream priority for multiplexing
    /// - `priority()` - Get current priority
    /// - `id()` - Get the stream ID
    /// - `reset(VarInt)` - Abort the stream with an error code
    /// - `stopped()` - Wait for peer acknowledgment
    ///
    /// # Example
    /// ```no_run
    /// # use zel_core::protocol::SubscriptionSink;
    /// # async fn example(mut sink: SubscriptionSink) -> Result<(), Box<dyn std::error::Error>> {
    /// // Set high priority for this subscription
    /// sink.stream().set_priority(10)?;
    ///
    /// // Send data
    /// sink.send(&"important data").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn stream(&self) -> &SendStream {
        self.inner.get_ref()
    }
}

/// Errors that can occur when sending subscription messages
#[derive(thiserror::Error, Debug)]
pub enum SubscriptionError {
    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Send error: {0}")]
    Send(String),
}

// ============================================================================
// Notification Sink Wrapper (Server-side)
// ============================================================================

/// Wrapper for sending acknowledgments in client-to-server streaming.
///
/// This is used in **client-to-server streaming** endpoints (notifications).
/// The server receives data from the client and uses this sink to send acknowledgments back.
///
/// # Directionality
///
/// ```text
/// Client ──[NotificationMsg::Data]──> Server
/// Client <──[NotificationMsg::Ack]──── Server (via NotificationSink)
/// ```
///
/// The client pushes data to the server, and the server uses `NotificationSink` to acknowledge
/// receipt. Contrast this with [`SubscriptionSink`] which sends data from server to client.
///
/// # Example
///
/// ```rust,ignore
/// async fn log_receiver(
///     ctx: RequestContext,
///     mut recv: FramedRead<RecvStream, LengthDelimitedCodec>,
///     mut ack_sink: NotificationSink,
/// ) -> Result<(), zel_types::ResourceError> {
///     while let Some(msg_bytes) = recv.next().await {
///         let msg: NotificationMsg = serde_json::from_slice(&msg_bytes?)?;
///
///         match msg {
///             NotificationMsg::Data(data) => {
///                 // Process client data
///                 process_log_entry(&data);
///                 ack_sink.ack().await?; // Acknowledge receipt
///             }
///             NotificationMsg::Completed => break,
///             _ => {}
///         }
///     }
///     Ok(())
/// }
/// ```
pub struct NotificationSink {
    inner: FramedWrite<SendStream, LengthDelimitedCodec>,
}

impl NotificationSink {
    /// Create a new NotificationSink from a FramedWrite
    pub fn new(inner: FramedWrite<SendStream, LengthDelimitedCodec>) -> Self {
        Self { inner }
    }

    /// Send an acknowledgment for the received notification
    pub async fn ack(&mut self) -> Result<(), NotificationError> {
        let msg = NotificationMsg::Ack;
        let msg_bytes = serde_json::to_vec(&msg)
            .map_err(|e| NotificationError::Serialization(e.to_string()))?;

        self.inner
            .send(msg_bytes.into())
            .await
            .map_err(|e| NotificationError::Send(e.to_string()))?;

        Ok(())
    }

    /// Send an error message to the client
    pub async fn error(&mut self, error_msg: impl Into<String>) -> Result<(), NotificationError> {
        let msg = NotificationMsg::Error(error_msg.into());
        let msg_bytes = serde_json::to_vec(&msg)
            .map_err(|e| NotificationError::Serialization(e.to_string()))?;

        self.inner
            .send(msg_bytes.into())
            .await
            .map_err(|e| NotificationError::Send(e.to_string()))?;

        Ok(())
    }

    /// Get a reference to the underlying SendStream
    ///
    /// This allows direct access to all SendStream methods for the acknowledgment stream.
    pub fn stream(&self) -> &SendStream {
        self.inner.get_ref()
    }
}

/// Errors that can occur when handling notifications
#[derive(thiserror::Error, Debug)]
pub enum NotificationError {
    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Deserialization error: {0}")]
    Deserialization(String),

    #[error("Send error: {0}")]
    Send(String),

    #[error("Receive error: {0}")]
    Receive(String),

    #[error("Protocol error: {0}")]
    Protocol(String),
}
