use crate::service::{
    MethodDescription, NotificationDescription, ServiceDescription, StreamDescription,
    SubscriptionDescription,
};
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use super::codegen::{
    generate_param_serialization, generate_param_serialization_optional, generate_typed_name,
};
use super::types::{render_notification_sender, render_subscription_stream};

/// Render the client struct with all methods, subscriptions, notifications, and streams
pub fn render_client_struct(service: &ServiceDescription) -> TokenStream2 {
    let trait_name = &service.trait_ident;
    let client_name = quote::format_ident!("{}Client", trait_name);
    let service_name = &service.service_name;

    let client_methods: Vec<_> = service
        .methods
        .iter()
        .map(|m| render_client_method(m, service_name))
        .collect();

    let client_subscriptions: Vec<_> = service
        .subscriptions
        .iter()
        .map(|s| render_client_subscription(s, service_name, trait_name))
        .collect();

    let client_notifications: Vec<_> = service
        .notifications
        .iter()
        .map(|n| render_client_notification(n, service_name, trait_name))
        .collect();

    let client_streams: Vec<_> = service
        .streams
        .iter()
        .map(|s| render_client_stream(s, service_name))
        .collect();

    let subscription_streams: Vec<_> = service
        .subscriptions
        .iter()
        .map(|s| render_subscription_stream(s, trait_name))
        .collect();

    let notification_senders: Vec<_> = service
        .notifications
        .iter()
        .map(|n| render_notification_sender(n, trait_name))
        .collect();

    quote! {
        #[derive(Clone)]
        pub struct #client_name {
            client: zel_core::protocol::RpcClient,
            service_name: String,
        }

        impl #client_name {
            pub fn new(client: zel_core::protocol::RpcClient) -> Self {
                Self {
                    client,
                    service_name: #service_name.to_string(),
                }
            }

            #(#client_methods)*
            #(#client_subscriptions)*
            #(#client_notifications)*
            #(#client_streams)*
        }

        #(#subscription_streams)*
        #(#notification_senders)*
    }
}

/// Render a client-side method
fn render_client_method(method: &MethodDescription, _service_name: &str) -> TokenStream2 {
    let method_name = &method.signature.sig.ident;
    let rpc_name = &method.rpc_name;
    let params = &method.params;
    let param_idents: Vec<_> = params.iter().map(|p| &p.ident).collect();
    let param_types: Vec<_> = params.iter().map(|p| &p.ty).collect();

    let return_type = method
        .return_type
        .as_ref()
        .map(|ty| quote! { #ty })
        .unwrap_or_else(|| quote! { () });

    let serialize_params = generate_param_serialization(params);

    quote! {
        pub async fn #method_name(&self, #(#param_idents: #param_types),*)
            -> Result<#return_type, zel_core::protocol::ClientError>
        {
            #serialize_params

            let response = self.client.call(&self.service_name, #rpc_name, body).await?;
            let result: #return_type = serde_json::from_slice(&response.data)
                .map_err(|e| zel_core::protocol::ClientError::Serialization(e))?;
            Ok(result)
        }
    }
}

/// Render a client-side subscription method
fn render_client_subscription(
    sub: &SubscriptionDescription,
    _service_name: &str,
    trait_name: &syn::Ident,
) -> TokenStream2 {
    let method_name = &sub.signature.sig.ident;
    let rpc_name = &sub.rpc_name;
    let stream_name = generate_typed_name(trait_name, method_name, "Stream");

    let params = &sub.params;
    let param_idents: Vec<_> = params.iter().map(|p| &p.ident).collect();
    let param_types: Vec<_> = params.iter().map(|p| &p.ty).collect();

    let serialize_params = generate_param_serialization_optional(params);

    quote! {
        pub async fn #method_name(&self, #(#param_idents: #param_types),*)
            -> Result<#stream_name, zel_core::protocol::ClientError>
        {
            #serialize_params

            let stream = self.client.subscribe(&self.service_name, #rpc_name, body).await?;
            Ok(#stream_name { inner: stream })
        }
    }
}

/// Render a client-side notification method
fn render_client_notification(
    notif: &NotificationDescription,
    _service_name: &str,
    trait_name: &syn::Ident,
) -> TokenStream2 {
    let method_name = &notif.signature.sig.ident;
    let rpc_name = &notif.rpc_name;
    let sender_name = generate_typed_name(trait_name, method_name, "Sender");

    let params = &notif.params;
    let param_idents: Vec<_> = params.iter().map(|p| &p.ident).collect();
    let param_types: Vec<_> = params.iter().map(|p| &p.ty).collect();

    let serialize_params = generate_param_serialization_optional(params);

    quote! {
        pub async fn #method_name(&self, #(#param_idents: #param_types),*)
            -> Result<#sender_name, zel_core::protocol::ClientError>
        {
            #serialize_params

            let sender = self.client.notify(&self.service_name, #rpc_name, body).await?;
            Ok(#sender_name { inner: sender })
        }
    }
}

/// Render a client-side stream method
fn render_client_stream(stream: &StreamDescription, _service_name: &str) -> TokenStream2 {
    let method_name = &stream.signature.sig.ident;
    let rpc_name = &stream.rpc_name;
    let params = &stream.params;
    let param_idents: Vec<_> = params.iter().map(|p| &p.ident).collect();
    let param_types: Vec<_> = params.iter().map(|p| &p.ty).collect();

    let serialize_params = generate_param_serialization_optional(params);

    quote! {
        pub async fn #method_name(&self, #(#param_idents: #param_types),*)
            -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream), zel_core::protocol::ClientError>
        {
            #serialize_params

            self.client.open_stream(&self.service_name, #rpc_name, body).await
        }
    }
}
