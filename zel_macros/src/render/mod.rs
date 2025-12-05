pub mod client;
pub mod codegen;
pub mod server;
pub mod types;

use crate::service::ServiceDescription;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

/// Main entry point for rendering a service
/// This orchestrates the generation of server traits, client structs, and typed wrappers
pub fn render_service(service: &ServiceDescription) -> TokenStream2 {
    let server_trait = server::render_server_trait(service);
    let typed_sinks = types::render_typed_sinks(&service.subscriptions, &service.trait_ident);
    let typed_receivers =
        types::render_typed_receivers(&service.notifications, &service.trait_ident);
    let client_struct = client::render_client_struct(service);

    quote! {
        #typed_sinks
        #typed_receivers
        #server_trait
        #client_struct
    }
}
