//! Iroh transport implementation for JSON-RPC
//!
//! This module provides COBS-framed transport for JSON-RPC messages over Iroh streams.

use std::io;
use std::sync::Arc;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, PublicKey};
use jsonrpsee::core::client::{ReceivedMessage, TransportReceiverT, TransportSenderT};
use log::error;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::request_reply::json_rpc::errors::{BuildError, IrohTransportError};

// ============================================================================
// Transport Implementation
// ============================================================================

/// JSON-RPC sender over Iroh streams using COBS framing
///
/// Implements [`TransportSenderT`] to send JSON-RPC messages over Iroh.
/// Uses COBS (Consistent Overhead Byte Stuffing) for message framing.
///
/// This type is cloneable to support subscription forwarding where multiple
/// tasks need to send messages on the same underlying stream.
pub struct IrohSender {
    send: Arc<Mutex<SendStream>>,
    max_request_size: usize,
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl IrohSender {
    pub(crate) fn new(send: SendStream, max_request_size: usize) -> Self {
        Self {
            send: Arc::new(Mutex::new(send)),
            max_request_size,
            buffer: Arc::new(Mutex::new(Vec::with_capacity(max_request_size))),
        }
    }
}

impl Clone for IrohSender {
    fn clone(&self) -> Self {
        Self {
            send: Arc::clone(&self.send),
            max_request_size: self.max_request_size,
            // Each clone gets its own buffer to avoid contention
            buffer: Arc::new(Mutex::new(Vec::with_capacity(self.max_request_size))),
        }
    }
}

impl TransportSenderT for IrohSender {
    type Error = IrohTransportError;

    async fn send(&mut self, msg: String) -> Result<(), Self::Error> {
        let len = msg.len();
        if len > self.max_request_size {
            error!(
                "Length {len} larger than max_request_size {}",
                self.max_request_size
            );
            return Err(IrohTransportError::MessageTooLarge {
                size: len,
                max: self.max_request_size,
            });
        }

        // Acquire buffer lock
        let mut buffer = self.buffer.lock().await;

        // COBS encode with 0x00 delimiter
        buffer.clear();
        buffer.resize(msg.len() + (msg.len() / 254) + 2, 0);
        match cobs::try_encode(msg.as_bytes(), &mut buffer) {
            Ok(encoded_len) => {
                buffer.truncate(encoded_len);
            }
            Err(e) => {
                error!("dest buffer too small {e}");
                return Err(IrohTransportError::CobsEncode(e.to_string()));
            }
        }
        buffer.push(0x00); // Frame delimiter

        // Acquire stream lock and write
        let mut send = self.send.lock().await;
        send.write_all(&buffer)
            .await
            .map_err(|e| IrohTransportError::Io(io::Error::new(io::ErrorKind::Other, e)))?;
        send.flush()
            .await
            .map_err(|e| IrohTransportError::Io(io::Error::new(io::ErrorKind::Other, e)))?;

        Ok(())
    }

    async fn send_ping(&mut self) -> Result<(), Self::Error> {
        // Iroh handles connection keep-alive internally
        Ok(())
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        let mut send = self.send.lock().await;
        send.finish().map_err(|e| {
            IrohTransportError::Io(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to finish stream: {}", e),
            ))
        })?;
        Ok(())
    }
}

/// JSON-RPC receiver over Iroh streams using COBS framing
///
/// Implements [`TransportReceiverT`] to receive JSON-RPC messages from Iroh.
/// Uses COBS (Consistent Overhead Byte Stuffing) for message framing.
pub struct IrohReceiver {
    recv: RecvStream,
    max_response_size: usize,
    buffer: Vec<u8>,
}

impl IrohReceiver {
    pub(crate) fn new(recv: RecvStream, max_response_size: usize) -> Self {
        Self {
            recv,
            max_response_size,
            buffer: Vec::with_capacity(max_response_size),
        }
    }
}

impl TransportReceiverT for IrohReceiver {
    type Error = IrohTransportError;

    async fn receive(&mut self) -> Result<ReceivedMessage, Self::Error> {
        // Read until 0x00 delimiter (COBS frame boundary)
        self.buffer.clear();
        let mut byte = [0u8; 1];

        loop {
            self.recv
                .read_exact(&mut byte)
                .await
                .map_err(|e| IrohTransportError::Io(io::Error::new(io::ErrorKind::Other, e)))?;

            if byte[0] == 0x00 {
                break; // Frame complete
            }

            self.buffer.push(byte[0]);

            if self.buffer.len() > self.max_response_size {
                return Err(IrohTransportError::MessageTooLarge {
                    size: self.buffer.len(),
                    max: self.max_response_size,
                });
            }
        }

        // COBS decode
        let mut decoded = vec![0u8; self.buffer.len()];
        let len =
            cobs::decode(&self.buffer, &mut decoded).map_err(|_| IrohTransportError::CobsDecode)?;
        decoded.truncate(len);

        // UTF-8 decode
        let msg = String::from_utf8(decoded)?;

        Ok(ReceivedMessage::Text(msg))
    }
}

// ============================================================================
// Transport Builder
// ============================================================================

/// Default maximum request size (10 MB)
pub const DEFAULT_MAX_REQUEST_SIZE: usize = 10 * 1024 * 1024;

/// Default maximum response size (10 MB)
pub const DEFAULT_MAX_RESPONSE_SIZE: usize = 10 * 1024 * 1024;

/// Builder for creating Iroh JSON-RPC transport
///
/// # Example
///
/// ```no_run
/// use zel_core::request_reply::json_rpc::IrohTransportBuilder;
///
/// # async fn example(endpoint: iroh::Endpoint, peer: iroh::PublicKey) -> Result<(), Box<dyn std::error::Error>> {
/// let (sender, receiver) = IrohTransportBuilder::new()
///     .max_request_size(5 * 1024 * 1024)  // 5MB
///     .build(&endpoint, peer, b"my-rpc")
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct IrohTransportBuilder {
    max_request_size: usize,
    max_response_size: usize,
}

impl Default for IrohTransportBuilder {
    fn default() -> Self {
        Self {
            max_request_size: DEFAULT_MAX_REQUEST_SIZE,
            max_response_size: DEFAULT_MAX_RESPONSE_SIZE,
        }
    }
}

impl IrohTransportBuilder {
    /// Create a new transport builder with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum request size in bytes
    ///
    /// Default: 10 MB
    pub fn max_request_size(mut self, size: usize) -> Self {
        self.max_request_size = size;
        self
    }

    /// Set the maximum response size in bytes
    ///
    /// Default: 10 MB
    pub fn max_response_size(mut self, size: usize) -> Self {
        self.max_response_size = size;
        self
    }

    /// Build the transport by connecting to a peer
    ///
    /// This method:
    /// 1. Connects to the specified peer using the ALPN
    /// 2. Opens a bidirectional stream
    /// 3. Creates sender and receiver with COBS framing
    ///
    /// # Arguments
    ///
    /// * `endpoint` - The Iroh endpoint to use for connecting
    /// * `peer` - The public key of the peer to connect to
    /// * `alpn` - The ALPN (Application-Layer Protocol Negotiation) identifier
    pub async fn build(
        self,
        endpoint: &Endpoint,
        peer: PublicKey,
        alpn: &[u8],
    ) -> Result<(IrohSender, IrohReceiver), BuildError> {
        // Connect to peer
        let conn = endpoint.connect(peer, alpn).await?;

        // Open bidirectional stream
        let (send, recv) = conn.open_bi().await?;

        let sender = IrohSender::new(send, self.max_request_size);
        let receiver = IrohReceiver::new(recv, self.max_response_size);

        Ok((sender, receiver))
    }
}

// ============================================================================
// Server Support
// ============================================================================

/// Read a single COBS-framed message from an Iroh stream
///
/// Reads bytes until the 0x00 delimiter, then COBS-decodes the frame.
/// Returns the raw decoded bytes without UTF-8 conversion.
///
/// # Arguments
///
/// * `recv` - The receive stream to read from
/// * `max_size` - Maximum allowed message size in bytes
///
/// # Returns
///
/// The decoded message bytes, or an error if the message is too large or decoding fails
pub async fn read_cobs_frame(
    recv: &mut RecvStream,
    max_size: usize,
) -> Result<Vec<u8>, IrohTransportError> {
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];

    // Read until 0x00 delimiter
    loop {
        recv.read_exact(&mut byte)
            .await
            .map_err(|e| IrohTransportError::Io(io::Error::new(io::ErrorKind::Other, e)))?;

        if byte[0] == 0x00 {
            break; // Frame complete
        }

        buffer.push(byte[0]);

        if buffer.len() > max_size {
            return Err(IrohTransportError::MessageTooLarge {
                size: buffer.len(),
                max: max_size,
            });
        }
    }

    // COBS decode
    let mut decoded = vec![0u8; buffer.len()];
    let len = cobs::decode(&buffer, &mut decoded).map_err(|_| IrohTransportError::CobsDecode)?;
    decoded.truncate(len);

    Ok(decoded)
}

/// Write a COBS-framed message to an Iroh stream
///
/// Encodes the data using COBS and writes it with a 0x00 delimiter.
///
/// # Arguments
///
/// * `send` - The send stream to write to
/// * `data` - The data to encode and send
///
/// # Returns
///
/// Ok(()) on success, or an error if encoding or writing fails
pub async fn write_cobs_frame(
    send: &mut SendStream,
    data: &[u8],
) -> Result<(), IrohTransportError> {
    // Allocate buffer for COBS encoding
    let mut buffer = vec![0u8; data.len() + (data.len() / 254) + 2];

    let encoded_len = cobs::try_encode(data, &mut buffer)
        .map_err(|e| IrohTransportError::CobsEncode(e.to_string()))?;

    buffer.truncate(encoded_len);
    buffer.push(0x00); // Frame delimiter

    send.write_all(&buffer)
        .await
        .map_err(|e| IrohTransportError::Io(io::Error::new(io::ErrorKind::Other, e)))?;

    send.flush()
        .await
        .map_err(|e| IrohTransportError::Io(io::Error::new(io::ErrorKind::Other, e)))?;

    Ok(())
}

/// Accept an incoming connection and create transports for server-side handling
///
/// This is the server-side equivalent of [`IrohTransportBuilder::build`].
/// It accepts a bidirectional stream from the client and creates sender/receiver.
///
/// **Note**: This function is primarily for CLIENT compatibility. Server code should
/// use `read_cobs_frame()` and `write_cobs_frame()` directly for better control.
///
/// # Arguments
///
/// * `connection` - The incoming Iroh connection
/// * `max_request_size` - Maximum size for incoming requests
/// * `max_response_size` - Maximum size for outgoing responses
pub async fn accept_connection(
    connection: &Connection,
    max_request_size: usize,
    max_response_size: usize,
) -> Result<(IrohSender, IrohReceiver), IrohTransportError> {
    // Server accepts the bidirectional stream that client opened
    let (send, recv) = connection.accept_bi().await.map_err(|e| {
        IrohTransportError::Io(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to accept bi stream: {}", e),
        ))
    })?;

    // Create sender and receiver (note: server sends responses, receives requests)
    let sender = IrohSender::new(send, max_response_size);
    let receiver = IrohReceiver::new(recv, max_request_size);

    Ok((sender, receiver))
}
