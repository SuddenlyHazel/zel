use proc_macro::TokenStream;
use syn::{parse_macro_input, ItemTrait};

mod attributes;
mod helpers;
mod render;
mod service;

/// Generates type-safe server and client code for RPC services.
///
/// This macro transforms a trait definition into a complete RPC service with:
/// - Server trait with `RequestContext` parameter
/// - Typed client wrapper for type-safe calls
/// - Automatic serialization/deserialization
/// - Support for methods, subscriptions, notifications, and raw streams
///
/// # Service Definition
///
/// ```rust,ignore
/// use zel_macros::zel_service;
/// use async_trait::async_trait;
/// use zel_core::protocol::RequestContext;
/// use zel_types::ResourceError;
///
/// #[zel_service(name = "math")]
/// trait Math {
///     /// Simple request/response
///     #[method(name = "add")]
///     async fn add(&self, a: i32, b: i32) -> Result<i32, ResourceError>;
///
///     /// Server-to-client streaming
///     #[subscription(name = "counter", item = "u64")]
///     async fn counter(&self, interval_ms: u64) -> Result<(), ResourceError>;
///
///     /// Client-to-server streaming
///     #[notification(name = "log", item = "String")]
///     async fn log_messages(&self) -> Result<(), ResourceError>;
///
///     /// Bidirectional raw stream
///     #[stream(name = "transfer")]
///     async fn file_transfer(&self, filename: String) -> Result<(), ResourceError>;
/// }
///
/// // Implement the generated MathServer trait
/// #[derive(Clone)]
/// struct MathImpl;
///
/// #[async_trait]
/// impl MathServer for MathImpl {
///     async fn add(&self, ctx: RequestContext, a: i32, b: i32) -> Result<i32, ResourceError> {
///         Ok(a + b)
///     }
///     // ... other methods
/// }
/// ```
/// # Generated Code Preview
///
/// ```rust
/// // Generated MathServer trait (implement this)
/// #[async_trait]
/// pub trait MathServer {
///     async fn add(&self, ctx: RequestContext, a: i32, b: i32) -> Result<i32, ResourceError>;
///
///     // Subscription with typed sink
///     async fn counter(
///         &self,
///         ctx: RequestContext,
///         sink: MathCounterSink,
///         interval_ms: u64
///     ) -> Result<(), ResourceError>;
/// }
///
/// // Generated MathClient (type-safe calls)
/// pub struct MathClient<RpcClient> {
///     client: RpcClient
/// }
///
/// impl<R> MathClient<R> {
///     pub async fn add(&self, a: i32, b: i32) -> Result<i32, ClientError>;
///     pub async fn counter(&self, interval_ms: u64) -> Result<SubscriptionStream, ClientError>;
/// }
/// ```
///
/// See [`zel_core::protocol`](https://docs.rs/zel_core/latest/zel_core/protocol/) for full usage.
/// For complete examples, see the
/// [examples directory](https://github.com/SuddenlyHazel/zel/tree/main/zel_core/examples).
#[proc_macro_attribute]
pub fn zel_service(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = parse_macro_input!(attr as attributes::ServiceAttr);
    let trait_def = parse_macro_input!(item as ItemTrait);

    match service::ServiceDescription::from_item(attr, trait_def) {
        Ok(service) => service.render().into(),
        Err(e) => e.to_compile_error().into(),
    }
}
