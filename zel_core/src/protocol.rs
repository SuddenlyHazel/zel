use std::{collections::HashMap, fmt::Debug, sync::Arc};

use bytes::Bytes;
use futures::future::BoxFuture;
use iroh::{
    Endpoint,
    endpoint::{Connection, SendStream},
};
use serde::{Deserialize, Serialize};
use tokio_util::codec::{FramedWrite, LengthDelimitedCodec};

pub mod client;
pub mod context;
pub mod extensions;
pub mod server;

// Re-export the procedural macro
pub use zel_macros::zel_service;

// Re-export client types for generated code
pub use client::{ClientError, RpcClient, SubscriptionStream};

// Re-export new context types
pub use context::RequestContext;
pub use extensions::Extensions;

#[derive(Debug, Serialize, Deserialize)]
pub enum Body {
    Subscribe,
    Rpc(Bytes),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub service: String,
    pub resource: String,
    pub body: Body,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub data: Bytes,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum SubscriptionMsg {
    Established { service: String, resource: String },
    Data(Bytes),
    Stopped,
}

// Lowest level
#[derive(Debug)]
pub struct RpcServer<'a> {
    // APLN for all the child service
    alpn: &'a [u8],
    endpoint: Endpoint,
    services: ServiceMap<'a>,
    server_extensions: Extensions,
}

type ServiceMap<'a> = Arc<HashMap<&'a str, RpcService<'a>>>;

#[derive(Debug)]
pub struct RpcService<'a> {
    service: &'a str,
    resources: HashMap<&'a str, ResourceCallback>,
}

pub enum ResourceCallback {
    Rpc(Rpc),
    SubscriptionProducer(Subscription),
}

#[derive(thiserror::Error, Debug, Serialize, Deserialize)]
pub enum ResourceError {
    #[error("Service '{service}' not found")]
    ServiceNotFound { service: String },

    #[error("Resource '{resource}' not found in service '{service}'")]
    ResourceNotFound { service: String, resource: String },

    #[error("Callback execution failed: {0}")]
    CallbackError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),
}

impl Debug for ResourceCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rpc(_) => f.debug_tuple("Rpc").finish(),
            Self::SubscriptionProducer(_) => f.debug_tuple("SubscriptionProducer").finish(),
        }
    }
}

pub type ResourceResponse = Result<Response, ResourceError>;

pub type Rpc =
    Arc<dyn Send + Sync + Fn(RequestContext, Request) -> BoxFuture<'static, ResourceResponse>>;

pub type Subscription = Arc<
    dyn Send
        + Sync
        + Fn(
            RequestContext,
            Request,
            FramedWrite<SendStream, LengthDelimitedCodec>,
        ) -> BoxFuture<'static, ResourceResponse>,
>;

// ============================================================================
// Subscription Sink Wrapper
// ============================================================================

use futures::SinkExt;

/// Wrapper around FramedWrite for easy subscription message sending
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
}

/// Errors that can occur when sending subscription messages
#[derive(thiserror::Error, Debug)]
pub enum SubscriptionError {
    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Send error: {0}")]
    Send(String),
}

pub struct ServiceRequest {
    req: Request,
    connection: Connection,
}

pub trait RpcServiceT {
    /// Processes a request to a Service
    fn call<'a>(
        &self,
        request: ServiceRequest,
    ) -> impl Future<Output = ResourceResponse> + Send + 'a;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_serialization() {
        // Test ServiceNotFound
        let error = ResourceError::ServiceNotFound {
            service: "test_service".to_string(),
        };
        let json = serde_json::to_string(&error).unwrap();
        let deserialized: ResourceError = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(deserialized, ResourceError::ServiceNotFound { service } if service == "test_service")
        );

        // Test ResourceNotFound
        let error = ResourceError::ResourceNotFound {
            service: "test_service".to_string(),
            resource: "test_resource".to_string(),
        };
        let json = serde_json::to_string(&error).unwrap();
        let deserialized: ResourceError = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(deserialized, ResourceError::ResourceNotFound { service, resource }
            if service == "test_service" && resource == "test_resource")
        );

        // Test CallbackError
        let error = ResourceError::CallbackError("test error".to_string());
        let json = serde_json::to_string(&error).unwrap();
        let deserialized: ResourceError = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, ResourceError::CallbackError(msg) if msg == "test error"));

        // Test SerializationError
        let error = ResourceError::SerializationError("test error".to_string());
        let json = serde_json::to_string(&error).unwrap();
        let deserialized: ResourceError = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(deserialized, ResourceError::SerializationError(msg) if msg == "test error")
        );
    }

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

    #[test]
    fn test_resource_response_serialization() {
        // Test success response
        let response: ResourceResponse = Ok(Response {
            data: Bytes::from("success"),
        });
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: ResourceResponse = serde_json::from_str(&json).unwrap();
        assert!(deserialized.is_ok());

        // Test error response
        let response: ResourceResponse = Err(ResourceError::CallbackError("test".to_string()));
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: ResourceResponse = serde_json::from_str(&json).unwrap();
        assert!(deserialized.is_err());
    }
}

// ============================================================================
// Builder Pattern
// ============================================================================

/// Builder for constructing an RpcServer with a fluent API
pub struct RpcServerBuilder<'a> {
    alpn: &'a [u8],
    endpoint: Endpoint,
    services: HashMap<&'a str, RpcService<'a>>,
    server_extensions: Extensions,
}

impl<'a> RpcServerBuilder<'a> {
    /// Create a new RPC server builder
    ///
    /// # Arguments
    /// * `alpn` - The ALPN protocol identifier
    /// * `endpoint` - The Iroh endpoint
    /// * `router` - The Iroh protocol router
    pub fn new(alpn: &'a [u8], endpoint: Endpoint) -> Self {
        Self {
            alpn,
            endpoint,
            services: HashMap::new(),
            server_extensions: Extensions::new(),
        }
    }

    /// Set server-level extensions (shared across all connections)
    pub fn with_extensions(mut self, extensions: Extensions) -> Self {
        self.server_extensions = extensions;
        self
    }

    /// Begin defining a service
    ///
    /// # Arguments
    /// * `name` - The service name
    ///
    /// # Returns
    /// A ServiceBuilder for adding resources to this service
    pub fn service(self, name: &'a str) -> ServiceBuilder<'a> {
        ServiceBuilder::new(name, self)
    }

    /// Build the RpcServer
    pub fn build(self) -> RpcServer<'a> {
        RpcServer {
            alpn: self.alpn,
            endpoint: self.endpoint,
            services: Arc::new(self.services),
            server_extensions: self.server_extensions,
        }
    }
}

/// Builder for constructing an RpcService with resources
pub struct ServiceBuilder<'a> {
    name: &'a str,
    resources: HashMap<&'a str, ResourceCallback>,
    server_builder: RpcServerBuilder<'a>,
}

impl<'a> ServiceBuilder<'a> {
    fn new(name: &'a str, server_builder: RpcServerBuilder<'a>) -> Self {
        Self {
            name,
            resources: HashMap::new(),
            server_builder,
        }
    }

    /// Add an RPC resource to this service
    ///
    /// # Arguments
    /// * `name` - The resource name
    /// * `callback` - The RPC callback function
    pub fn rpc_resource<F>(mut self, name: &'a str, callback: F) -> Self
    where
        F: Fn(RequestContext, Request) -> BoxFuture<'static, ResourceResponse>
            + Send
            + Sync
            + 'static,
    {
        self.resources
            .insert(name, ResourceCallback::Rpc(Arc::new(callback)));
        self
    }

    /// Add a subscription resource to this service
    ///
    /// # Arguments
    /// * `name` - The resource name
    /// * `callback` - The subscription callback function
    pub fn subscription_resource<F>(mut self, name: &'a str, callback: F) -> Self
    where
        F: Fn(
                RequestContext,
                Request,
                FramedWrite<SendStream, LengthDelimitedCodec>,
            ) -> BoxFuture<'static, ResourceResponse>
            + Send
            + Sync
            + 'static,
    {
        self.resources.insert(
            name,
            ResourceCallback::SubscriptionProducer(Arc::new(callback)),
        );
        self
    }

    /// Finish building this service and return to the server builder
    pub fn build(mut self) -> RpcServerBuilder<'a> {
        let service = RpcService {
            service: self.name,
            resources: self.resources,
        };

        self.server_builder.services.insert(self.name, service);
        self.server_builder
    }
}
