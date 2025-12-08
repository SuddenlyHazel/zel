//! Wire protocol types for the Zel RPC framework.
//!
//! This module defines the fundamental message types used in the Zel RPC protocol,
//! including requests, responses, and specialized messages for subscriptions and notifications.
//!
//! These are pure wire protocol definitions intended to be stable across versions.
//! Implementation details live in `zel_core`.
//!
//! # Protocol Messages
//!
//! - [`Request`] - RPC request containing service, resource, and body
//! - [`Response`] - RPC response with serialized data
//! - [`Body`] - Request body type indicating endpoint type (RPC/Subscribe/Stream/Notify)
//! - [`SubscriptionMsg`] - Server-to-client streaming messages
//! - [`NotificationMsg`] - Client-to-server streaming messages

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Request body type indicating the endpoint type and optional parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Body {
    /// Subscription request (server-to-client streaming)
    Subscribe,
    /// RPC method call with serialized parameters
    Rpc(Bytes),
    /// Raw bidirectional stream request with optional serialized parameters
    Stream(Bytes),
    /// Notification stream request (client-to-server streaming) with optional parameters
    Notify(Bytes),
}

/// RPC request containing service name, resource name, and body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// The service name to route to
    pub service: String,
    /// The resource (method/subscription/etc) name within the service
    pub resource: String,
    /// The request body indicating endpoint type and parameters
    pub body: Body,
}

/// RPC response containing serialized result data.
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    /// Serialized response data
    pub data: Bytes,
}

/// Messages for subscription streams (server-to-client streaming)
#[derive(Debug, Serialize, Deserialize)]
pub enum SubscriptionMsg {
    Established { service: String, resource: String },
    Data(Bytes),
    Stopped,
    ServerShutdown, // Server is shutting down gracefully
}

/// Messages for notification streams (client-to-server streaming)
#[derive(Debug, Serialize, Deserialize)]
pub enum NotificationMsg {
    Established { service: String, resource: String },
    Data(Bytes),    // Client sends data
    Ack,            // Server acknowledges receipt
    Error(String),  // Server reports error
    Completed,      // Client signals completion
    ServerShutdown, // Server is shutting down gracefully
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_serialization() {
        // Test RPC request
        let request = Request {
            service: "test_service".to_string(),
            resource: "test_resource".to_string(),
            body: Body::Rpc(Bytes::from("test data")),
        };
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.service, "test_service");
        assert_eq!(deserialized.resource, "test_resource");
        assert!(matches!(deserialized.body, Body::Rpc(_)));

        // Test Subscribe request
        let request = Request {
            service: "test_service".to_string(),
            resource: "test_resource".to_string(),
            body: Body::Subscribe,
        };
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized.body, Body::Subscribe));
    }

    #[test]
    fn test_response_serialization() {
        let response = Response {
            data: Bytes::from("test response"),
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.data, Bytes::from("test response"));
    }

    #[test]
    fn test_subscription_msg_serialization() {
        // Test Established
        let msg = SubscriptionMsg::Established {
            service: "test_service".to_string(),
            resource: "test_resource".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: SubscriptionMsg = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(deserialized, SubscriptionMsg::Established { service, resource }
            if service == "test_service" && resource == "test_resource")
        );

        // Test Data
        let msg = SubscriptionMsg::Data(Bytes::from("test data"));
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: SubscriptionMsg = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, SubscriptionMsg::Data(_)));

        // Test Stopped
        let msg = SubscriptionMsg::Stopped;
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: SubscriptionMsg = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, SubscriptionMsg::Stopped));
    }
}
