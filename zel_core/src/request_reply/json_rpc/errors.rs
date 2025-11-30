// ============================================================================
// Error Types
// ============================================================================

use std::io;

use iroh::endpoint::{ConnectError, ConnectionError};
use thiserror::Error;

// ============================================================================
// Build Error
// ============================================================================

/// Errors that can occur while building the transport
#[derive(Debug, Error)]
pub enum BuildError {
    /// Failed to connect to peer
    #[error("Failed to connect to peer: {0}")]
    Connect(#[from] ConnectError),

    /// Connection error
    #[error("Connection error: {0}")]
    Connection(#[from] ConnectionError),
}

// ============================================================================
// Transport Error
// ============================================================================

/// Errors that can occur during Iroh JSON-RPC transport operations
#[derive(Debug, Error)]
pub enum IrohTransportError {
    /// IO error occurred during stream operations
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// COBS encoding failed
    #[error("COBS encoding error: {0}")]
    CobsEncode(String),

    /// COBS decoding failed  
    #[error("COBS decoding error")]
    CobsDecode,

    /// Message exceeds size limit
    #[error("Message too large: {size} bytes exceeds maximum {max} bytes")]
    MessageTooLarge {
        /// Actual message size
        size: usize,
        /// Maximum allowed size
        max: usize,
    },

    /// UTF-8 decoding failed
    #[error("UTF-8 decode error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}
