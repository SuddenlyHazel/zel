use proc_macro::TokenStream;
use syn::{parse_macro_input, ItemTrait};

mod attributes;
mod helpers;
mod render;
mod service;

/// Procedural macro for generating Zel service implementations
///
/// # Example
///
/// ```rust,ignore
/// use zel_macros::zel_service;
///
/// #[zel_service(name = "math")]
/// trait Math {
///     #[method(name = "add")]
///     async fn add(&self, a: i32, b: i32) -> Result<i32, String>;
///
///     #[subscription(name = "counter")]
///     async fn counter(&self, interval_ms: u64) -> Result<(), String>;
/// }
/// ```
#[proc_macro_attribute]
pub fn zel_service(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = parse_macro_input!(attr as attributes::ServiceAttr);
    let trait_def = parse_macro_input!(item as ItemTrait);

    match service::ServiceDescription::from_item(attr, trait_def) {
        Ok(service) => service.render().into(),
        Err(e) => e.to_compile_error().into(),
    }
}
