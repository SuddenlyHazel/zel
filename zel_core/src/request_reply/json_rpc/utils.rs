//! Utility functions for JSON-RPC over Iroh
//!
//! This module provides helper functions for handling JSON-RPC requests,
//! adapted from jsonrpsee's internal implementation to work with our
//! tower middleware stack over Iroh transport.

use jsonrpsee::Extensions;
use jsonrpsee::core::middleware::{Batch, BatchEntry, BatchEntryErr, RpcServiceT};
use jsonrpsee::core::server::MethodResponse;
use jsonrpsee::core::server::helpers::prepare_error;
use jsonrpsee::types::{ErrorCode, ErrorObject, Id, InvalidRequest};
use serde_json::value::RawValue;

/// Batch request configuration
#[derive(Debug, Clone, Copy)]
pub enum BatchRequestConfig {
    /// Batch requests are disabled
    Disabled,
    /// Batch requests are limited to a maximum number
    Limit(u32),
    /// Batch requests are unlimited
    Unlimited,
}

/// Handle a JSON-RPC call over Iroh transport
///
/// This is our version of jsonrpsee's internal `handle_rpc_call`,
/// adapted for use with tower middleware over Iroh.
///
/// # Arguments
///
/// * `body` - Raw bytes of the JSON-RPC request
/// * `is_single` - true for single request `{...}`, false for batch `[...]`
/// * `batch_config` - Batch request configuration
/// * `rpc_service` - The RPC service wrapped with middleware
/// * `extensions` - Extensions to pass through (contains Iroh connection context)
///
/// # Returns
///
/// A `MethodResponse` containing the result or error
pub async fn handle_rpc_call<S>(
    body: &[u8],
    is_single: bool,
    batch_config: BatchRequestConfig,
    rpc_service: &S,
    extensions: Extensions,
) -> MethodResponse
where
    S: RpcServiceT<
            MethodResponse = MethodResponse,
            BatchResponse = MethodResponse,
            NotificationResponse = MethodResponse,
        > + Send,
{
    // Handle single request or notification
    if is_single {
        // Try to parse as Request
        if let Ok(req) = deserialize_with_ext::call::from_slice(body, &extensions) {
            return rpc_service.call(req).await;
        }

        // Try to parse as Notification
        // Use () as the generic type since we don't need the notification params
        if let Ok(notif) = deserialize_with_ext::notif::from_slice(body, &extensions) {
            return rpc_service.notification(notif).await;
        }

        // Failed to parse - return error
        let (id, code) = prepare_error(body);
        return MethodResponse::error(id, ErrorObject::from(code));
    }

    // Handle batch request
    let max_len = match batch_config {
        BatchRequestConfig::Disabled => {
            return MethodResponse::error(
                Id::Null,
                ErrorObject::borrowed(-32600, "Batch requests not supported", None),
            );
        }
        BatchRequestConfig::Limit(limit) => limit as usize,
        BatchRequestConfig::Unlimited => usize::MAX,
    };

    // Parse as array of raw JSON values
    let unchecked_batch: Vec<&RawValue> = match serde_json::from_slice(body) {
        Ok(batch) => batch,
        Err(_) => {
            return MethodResponse::error(Id::Null, ErrorObject::from(ErrorCode::ParseError));
        }
    };

    // Check batch size limit
    if unchecked_batch.len() > max_len {
        let msg = format!("Batch too large: max {}", max_len);
        return MethodResponse::error(Id::Null, ErrorObject::owned(-32600, msg, None::<()>));
    }

    // Parse each batch item
    let mut batch = Vec::with_capacity(unchecked_batch.len());
    for call in unchecked_batch {
        if let Ok(req) = deserialize_with_ext::call::from_str(call.get(), &extensions) {
            batch.push(Ok(BatchEntry::Call(req)));
        } else if let Ok(notif) =
            deserialize_with_ext::notif::from_str(call.get(), &extensions)
        {
            batch.push(Ok(BatchEntry::Notification(notif)));
        } else {
            let id = serde_json::from_str::<InvalidRequest>(call.get())
                .map(|err| err.id)
                .unwrap_or(Id::Null);
            batch.push(Err(BatchEntryErr::new(
                id,
                ErrorCode::InvalidRequest.into(),
            )));
        }
    }

    rpc_service.batch(Batch::from(batch)).await
}


/// Deserialize calls, notifications and responses with HTTP extensions.
pub mod deserialize_with_ext {
	/// Method call.
	pub mod call {
  use jsonrpsee::{Extensions, types::Request};

		/// Wrapper over `serde_json::from_slice` that sets the extensions.
		pub fn from_slice<'a>(
			data: &'a [u8],
			extensions: &'a Extensions,
		) -> Result<Request<'a>, serde_json::Error> {
			let mut req: Request = serde_json::from_slice(data)?;
			*req.extensions_mut() = extensions.clone();
			Ok(req)
		}

		/// Wrapper over `serde_json::from_str` that sets the extensions.
		pub fn from_str<'a>(data: &'a str, extensions: &'a Extensions) -> Result<Request<'a>, serde_json::Error> {
			let mut req: Request = serde_json::from_str(data)?;
			*req.extensions_mut() = extensions.clone();
			Ok(req)
		}
	}

	/// Notification.
	pub mod notif {
    use jsonrpsee::{Extensions, types::Notification};


		/// Wrapper over `serde_json::from_slice` that sets the extensions.
		pub fn from_slice<'a, T>(
			data: &'a [u8],
			extensions: &'a Extensions,
		) -> Result<Notification<'a, T>, serde_json::Error>
		where
			T: serde::Deserialize<'a>,
		{
			let mut notif: Notification<T> = serde_json::from_slice(data)?;
			*notif.extensions_mut() = extensions.clone();
			Ok(notif)
		}

		/// Wrapper over `serde_json::from_str` that sets the extensions.
		pub fn from_str<'a, T>(
			data: &'a str,
			extensions: &Extensions,
		) -> Result<Notification<'a, T>, serde_json::Error>
		where
			T: serde::Deserialize<'a>,
		{
			let mut notif: Notification<T> = serde_json::from_str(data)?;
			*notif.extensions_mut() = extensions.clone();
			Ok(notif)
		}
	}
}

