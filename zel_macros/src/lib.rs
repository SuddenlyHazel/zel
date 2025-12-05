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
///
/// #[zel_service(name = "math")]
/// trait Math {
///     /// Simple request/response
///     #[method(name = "add")]
///     async fn add(&self, a: i32, b: i32) -> Result<i32, String>;
///
///     /// Server-to-client streaming
///     #[subscription(name = "counter", item = "u64")]
///     async fn counter(&self, interval_ms: u64) -> Result<(), String>;
///
///     /// Client-to-server streaming
///     #[notification(name = "log", item = "String")]
///     async fn log_messages(&self) -> Result<(), String>;
///
///     /// Bidirectional raw stream
///     #[stream(name = "transfer")]
///     async fn file_transfer(&self, filename: String) -> Result<(), String>;
/// }
///
/// // Implement the generated MathServer trait
/// #[derive(Clone)]
/// struct MathImpl;
///
/// #[async_trait]
/// impl MathServer for MathImpl {
///     async fn add(&self, ctx: RequestContext, a: i32, b: i32) -> Result<i32, String> {
///         Ok(a + b)
///     }
///     // ... other methods
/// }
/// ```
///
/// # Generated Code
///
/// For each service, the macro generates:
/// - `{Service}Server` trait - Implement this with your business logic
/// - `{Service}Client` struct - Type-safe client for making calls
/// - Helper types for subscriptions and notifications
///
/// See [`zel_core::protocol`](https://docs.rs/zel_core/latest/zel_core/protocol/) for usage in context.
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
